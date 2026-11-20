//! MongoDB administrative client, replacing the Go driver for the one
//! operation instance initialization needs: creating the first root user.
//!
//! The MongoDB server package ships `mongod`/`mongos` only — no shell — so the
//! port speaks the wire protocol directly: one OP_MSG carrying a BSON
//! `createUser` command over IPv4 loopback. This is deliberately tiny (no
//! pooling, no sessions, no SCRAM) because it runs against a freshly
//! initialized, empty `mongod` that allows the localhost exception.
//!
//! [`MongoAdmin`] is the injected seam; tests use a fake and never open a
//! socket. The wire client is exercised against a loopback test server that
//! validates the exact BSON command and returns a fixed reply.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

use super::DatabaseError;
use super::inventory::InstanceCatalog;

mod mongo_data;

/// The administrative action initialization needs.
pub trait MongoAdmin: Send + Sync {
    /// Create `user` with the `root` role in `admin`.
    fn create_root_user(&self, port: i64, user: &str, password: &str) -> Result<(), DatabaseError>;

    /// List the database names and login users of a running instance.
    fn catalog(&self, port: i64, password: &str) -> Result<InstanceCatalog, DatabaseError>;

    /// Create `name` (materialized as an empty collection, like Go).
    fn create_database(&self, port: i64, password: &str, name: &str) -> Result<(), DatabaseError>;

    /// Drop `name` and all of its collections.
    fn drop_database(&self, port: i64, password: &str, name: &str) -> Result<(), DatabaseError>;

    /// Create a login with the `readWrite` role on `database`.
    fn create_user(
        &self,
        port: i64,
        password: &str,
        user: &str,
        user_password: &str,
        database: &str,
    ) -> Result<(), DatabaseError>;

    /// Drop `user` from `database`.
    fn drop_user(
        &self,
        port: i64,
        password: &str,
        user: &str,
        database: &str,
    ) -> Result<(), DatabaseError>;

    /// Write a logical dump of `database` to `path`.
    fn backup(
        &self,
        port: i64,
        password: &str,
        database: &str,
        path: &Path,
    ) -> Result<(), DatabaseError>;

    /// Restore `path` into the absent `database`.
    fn restore(
        &self,
        port: i64,
        password: &str,
        database: &str,
        path: &Path,
    ) -> Result<(), DatabaseError>;
}

/// The production client: a single OP_MSG command over loopback.
#[derive(Debug, Clone, Copy, Default)]
pub struct WireMongoAdmin;

impl MongoAdmin for WireMongoAdmin {
    fn create_root_user(&self, port: i64, user: &str, password: &str) -> Result<(), DatabaseError> {
        let port = u16::try_from(port)
            .map_err(|_| DatabaseError::invalid("invalid database connection port"))?;
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        let mut stream =
            TcpStream::connect_timeout(&address, Duration::from_secs(3)).map_err(|error| {
                DatabaseError::invalid(format!("MongoDB connection failed: {error}"))
            })?;
        let timeout = Some(Duration::from_secs(10));
        let _ = stream.set_read_timeout(timeout);
        let _ = stream.set_write_timeout(timeout);

        // Drivers handshake first; the server is then willing to process the
        // unauthenticated localhost-exception `createUser`.
        exchange(&mut stream, 1, &hello_command())?;
        let reply = exchange(&mut stream, 2, &create_user_command(user, password))?;
        match reply.ok {
            Some(ok) if ok != 0.0 => Ok(()),
            _ => Err(DatabaseError::invalid(format!(
                "MongoDB user creation failed: {}",
                reply
                    .error_message
                    .unwrap_or_else(|| "the server rejected the command".to_string())
            ))),
        }
    }

    fn catalog(&self, port: i64, password: &str) -> Result<InstanceCatalog, DatabaseError> {
        mongo_data::catalog(port, password)
    }

    fn create_database(&self, port: i64, password: &str, name: &str) -> Result<(), DatabaseError> {
        mongo_data::create_database(port, password, name)
    }

    fn drop_database(&self, port: i64, password: &str, name: &str) -> Result<(), DatabaseError> {
        mongo_data::drop_database(port, password, name)
    }

    fn create_user(
        &self,
        port: i64,
        password: &str,
        user: &str,
        user_password: &str,
        database: &str,
    ) -> Result<(), DatabaseError> {
        mongo_data::create_user(port, password, user, user_password, database)
    }

    fn drop_user(
        &self,
        port: i64,
        password: &str,
        user: &str,
        database: &str,
    ) -> Result<(), DatabaseError> {
        mongo_data::drop_user(port, password, user, database)
    }

    fn backup(
        &self,
        port: i64,
        password: &str,
        database: &str,
        path: &Path,
    ) -> Result<(), DatabaseError> {
        mongo_data::backup(port, password, database, path)
    }

    fn restore(
        &self,
        port: i64,
        password: &str,
        database: &str,
        path: &Path,
    ) -> Result<(), DatabaseError> {
        mongo_data::restore(port, password, database, path)
    }
}

fn exchange(
    stream: &mut TcpStream,
    request_id: i32,
    command: &[u8],
) -> Result<Reply, DatabaseError> {
    let message = op_msg(request_id, command);
    stream
        .write_all(&message)
        .and_then(|()| stream.flush())
        .map_err(|error| DatabaseError::invalid(format!("MongoDB command failed: {error}")))?;

    let mut header = [0u8; 16];
    stream
        .read_exact(&mut header)
        .map_err(|error| DatabaseError::invalid(format!("MongoDB command failed: {error}")))?;
    let length = i32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    if !(16..=16 * 1024 * 1024).contains(&length) {
        return Err(DatabaseError::invalid(
            "MongoDB reply is not a valid message",
        ));
    }
    let op_code = i32::from_le_bytes([header[12], header[13], header[14], header[15]]);
    if op_code != 2013 {
        return Err(DatabaseError::invalid("MongoDB reply is not an OP_MSG"));
    }
    let mut payload = vec![0u8; length as usize - 16];
    stream
        .read_exact(&mut payload)
        .map_err(|error| DatabaseError::invalid(format!("MongoDB command failed: {error}")))?;
    parse_top_level(op_msg_body(&payload)?)
}

fn hello_command() -> Vec<u8> {
    document(vec![
        element_i32("hello", 1),
        element_string("$db", "admin"),
    ])
}

fn create_user_command(user: &str, password: &str) -> Vec<u8> {
    let roles_document = document(vec![
        element_string("role", "root"),
        element_string("db", "admin"),
    ]);
    let roles = document(vec![element_document("0", &roles_document)]);
    document(vec![
        element_string("createUser", user),
        element_string("pwd", password),
        element_array("roles", &roles),
        element_string("$db", "admin"),
    ])
}

fn op_msg(request_id: i32, command: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(4 + 1 + command.len());
    body.extend_from_slice(&0i32.to_le_bytes()); // flagBits
    body.push(0); // section kind 0: a body document
    body.extend_from_slice(command);
    let length = 16 + body.len() as i32;
    let mut message = Vec::with_capacity(length as usize);
    message.extend_from_slice(&length.to_le_bytes());
    message.extend_from_slice(&request_id.to_le_bytes());
    message.extend_from_slice(&0i32.to_le_bytes()); // responseTo
    message.extend_from_slice(&2013i32.to_le_bytes()); // OP_MSG
    message.extend_from_slice(&body);
    message
}

fn op_msg_body(payload: &[u8]) -> Result<&[u8], DatabaseError> {
    if payload.len() < 5 {
        return Err(DatabaseError::invalid("MongoDB reply is not an OP_MSG"));
    }
    if payload[4] != 0 {
        return Err(DatabaseError::invalid(
            "MongoDB reply has an unsupported section",
        ));
    }
    let body = &payload[5..];
    if body.len() < 5 {
        return Err(DatabaseError::invalid("MongoDB reply is not an OP_MSG"));
    }
    let length = i32::from_le_bytes([body[0], body[1], body[2], body[3]]);
    if length < 5 || length as usize > body.len() {
        return Err(DatabaseError::invalid("MongoDB reply document is invalid"));
    }
    Ok(&body[..length as usize])
}

#[derive(Default)]
struct Reply {
    ok: Option<f64>,
    error_message: Option<String>,
}

fn parse_top_level(document: &[u8]) -> Result<Reply, DatabaseError> {
    let end = documented_end(document)?;
    let mut reply = Reply::default();
    let mut index = 4;
    while index < end {
        let tag = document[index];
        index += 1;
        let key_end = find_nul(document, index)?;
        let key = std::str::from_utf8(&document[index..key_end])
            .map_err(|_| DatabaseError::invalid("MongoDB reply document is invalid"))?;
        index = key_end + 1;
        match tag {
            0x01 => {
                let value = read_f64(document, index)?;
                if key == "ok" {
                    reply.ok = Some(value);
                }
                index += 8;
            }
            0x02 => {
                let (value, next) = read_string(document, index)?;
                if key == "errmsg" {
                    reply.error_message = Some(value);
                }
                index = next;
            }
            0x10 => {
                let value = read_i32(document, index)?;
                if key == "ok" {
                    reply.ok = Some(f64::from(value));
                }
                index += 4;
            }
            _ => {
                index = skip_value(document, tag, index)?;
            }
        }
    }
    Ok(reply)
}

fn documented_end(document: &[u8]) -> Result<usize, DatabaseError> {
    if document.len() < 5 {
        return Err(DatabaseError::invalid("MongoDB document is invalid"));
    }
    let length = read_i32(document, 0)?;
    if length < 5 || length as usize > document.len() {
        return Err(DatabaseError::invalid("MongoDB document is invalid"));
    }
    Ok(length as usize - 1)
}

fn find_nul(document: &[u8], from: usize) -> Result<usize, DatabaseError> {
    document[from..]
        .iter()
        .position(|byte| *byte == 0)
        .map(|offset| from + offset)
        .ok_or_else(|| DatabaseError::invalid("MongoDB document is invalid"))
}

fn read_i32(document: &[u8], index: usize) -> Result<i32, DatabaseError> {
    document
        .get(index..index + 4)
        .map(|bytes| i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        .ok_or_else(|| DatabaseError::invalid("MongoDB document is invalid"))
}

fn read_f64(document: &[u8], index: usize) -> Result<f64, DatabaseError> {
    document
        .get(index..index + 8)
        .map(|bytes| {
            f64::from_le_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ])
        })
        .ok_or_else(|| DatabaseError::invalid("MongoDB document is invalid"))
}

fn read_string(document: &[u8], index: usize) -> Result<(String, usize), DatabaseError> {
    let length = read_i32(document, index)?;
    if length < 1 {
        return Err(DatabaseError::invalid("MongoDB document is invalid"));
    }
    let start = index + 4;
    let end = start + length as usize;
    let bytes = document
        .get(start..end)
        .ok_or_else(|| DatabaseError::invalid("MongoDB document is invalid"))?;
    let value = std::str::from_utf8(&bytes[..bytes.len() - 1])
        .map_err(|_| DatabaseError::invalid("MongoDB document is invalid"))?
        .to_string();
    Ok((value, end))
}

fn skip_value(document: &[u8], tag: u8, index: usize) -> Result<usize, DatabaseError> {
    let invalid = || DatabaseError::invalid("MongoDB document is invalid");
    match tag {
        0x06 | 0x0A | 0x7F | 0xFF => Ok(index),
        0x08 => Ok(index + 1),
        0x09 | 0x11 | 0x12 => Ok(index + 8),
        0x03 | 0x04 => {
            let length = read_i32(document, index)?;
            if length < 5 {
                return Err(invalid());
            }
            Ok(index + length as usize)
        }
        0x05 => {
            let length = read_i32(document, index)?;
            if length < 0 {
                return Err(invalid());
            }
            Ok(index + 4 + 1 + length as usize)
        }
        0x07 => Ok(index + 12),
        0x13 => Ok(index + 16),
        0x0B => {
            let first = find_nul(document, index)? + 1;
            Ok(find_nul(document, first)? + 1)
        }
        _ => Err(invalid()),
    }
}

fn document(elements: Vec<Vec<u8>>) -> Vec<u8> {
    let mut body = Vec::new();
    for element in elements {
        body.extend_from_slice(&element);
    }
    let length = 4 + body.len() + 1;
    let mut out = Vec::with_capacity(length);
    out.extend_from_slice(&(length as i32).to_le_bytes());
    out.extend_from_slice(&body);
    out.push(0);
    out
}

fn element_string(key: &str, value: &str) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(0x02);
    out.extend_from_slice(key.as_bytes());
    out.push(0);
    let bytes = value.as_bytes();
    out.extend_from_slice(&((bytes.len() + 1) as i32).to_le_bytes());
    out.extend_from_slice(bytes);
    out.push(0);
    out
}

fn element_document(key: &str, value: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(0x03);
    out.extend_from_slice(key.as_bytes());
    out.push(0);
    out.extend_from_slice(value);
    out
}

fn element_i32(key: &str, value: i32) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(0x10);
    out.extend_from_slice(key.as_bytes());
    out.push(0);
    out.extend_from_slice(&value.to_le_bytes());
    out
}

fn element_array(key: &str, value: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(0x04);
    out.extend_from_slice(key.as_bytes());
    out.push(0);
    out.extend_from_slice(value);
    out
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    fn element_double(key: &str, value: f64) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(0x01);
        out.extend_from_slice(key.as_bytes());
        out.push(0);
        out.extend_from_slice(&value.to_le_bytes());
        out
    }

    fn read_payload(stream: &mut TcpStream) -> Vec<u8> {
        let mut header = [0u8; 16];
        stream.read_exact(&mut header).unwrap();
        let length = i32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let mut payload = vec![0u8; length as usize - 16];
        stream.read_exact(&mut payload).unwrap();
        payload
    }

    fn write_reply(stream: &mut TcpStream, document: &[u8]) {
        stream.write_all(&op_msg(99, document)).unwrap();
        stream.flush().unwrap();
    }

    #[test]
    fn wire_client_sends_a_handshake_then_create_user() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port() as i64;
        let (tx, rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let hello = read_payload(&mut stream);
            write_reply(&mut stream, &document(vec![element_double("ok", 1.0)]));
            let create = read_payload(&mut stream);
            write_reply(&mut stream, &document(vec![element_double("ok", 1.0)]));
            tx.send((hello, create)).unwrap();
        });

        WireMongoAdmin
            .create_root_user(port, "devmanager", "hidden-secret")
            .unwrap();
        server.join().unwrap();
        let (hello, create) = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(String::from_utf8_lossy(op_msg_body(&hello).unwrap()).contains("hello"));
        let text = String::from_utf8_lossy(op_msg_body(&create).unwrap());
        assert!(text.contains("createUser"));
        assert!(text.contains("devmanager"));
        assert!(text.contains("hidden-secret"));
        assert!(text.contains("$db"));
        assert!(text.contains("admin"));
    }

    #[test]
    fn wire_client_reports_the_server_error_message() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port() as i64;
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_payload(&mut stream);
            write_reply(&mut stream, &document(vec![element_double("ok", 1.0)]));
            let _ = read_payload(&mut stream);
            let reply = document(vec![
                element_double("ok", 0.0),
                element_string("errmsg", "not authorized"),
            ]);
            write_reply(&mut stream, &reply);
        });
        let error = WireMongoAdmin
            .create_root_user(port, "devmanager", "hidden-secret")
            .unwrap_err();
        assert!(error.to_string().contains("not authorized"));
    }
}

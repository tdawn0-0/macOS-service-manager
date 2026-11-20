//! Redis (Valkey) logical backup and restore, ported from `redis.go`.
//!
//! The engine ships no dump tool, so the Go code speaks RESP in-process. That
//! work lives behind [`RedisAdmin`] exactly like the MongoDB driver does behind
//! [`super::MongoAdmin`]; tests use a fake and never open a socket.
//!
//! Backups are a single JSON document with `DUMP` binary values base64-ed by
//! JSON (serde encodes `Vec<u8>` as an array, matching Go's `[]byte` array
//! encoding). A restore refuses a non-empty target, replays `RESTORE` in
//! 500-entry batches with relative TTLs, and `FLUSHDB`s on any failure.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::DatabaseError;

/// The logical Redis operations the database slice needs.
pub trait RedisAdmin: Send + Sync {
    /// The non-empty numbered databases in the server's `INFO keyspace`.
    fn keyspace_databases(&self, port: i64, password: &str) -> Result<Vec<String>, DatabaseError>;

    /// Write the logical dump of `db` to `path`.
    fn backup(&self, port: i64, password: &str, db: i64, path: &Path) -> Result<(), DatabaseError>;

    /// Restore `path` into the empty database `db`.
    fn restore(&self, port: i64, password: &str, db: i64, path: &Path)
    -> Result<(), DatabaseError>;
}

/// The production client: a single RESP connection over loopback.
#[derive(Debug, Clone, Copy, Default)]
pub struct WireRedisAdmin;

impl RedisAdmin for WireRedisAdmin {
    fn keyspace_databases(&self, port: i64, password: &str) -> Result<Vec<String>, DatabaseError> {
        let mut session = Session::open(port, password, 0)?;
        let raw = session.call(&[b"INFO", b"keyspace"])?;
        let text = match raw {
            Value::Bulk(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Value::Simple(text) => text,
            _ => return Err(DatabaseError::invalid("invalid Redis response")),
        };
        Ok(parse_redis_keyspace_databases(&text))
    }

    fn backup(&self, port: i64, password: &str, db: i64, path: &Path) -> Result<(), DatabaseError> {
        let mut session = Session::open(port, password, db)?;
        let created = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_millis() as i64)
            .unwrap_or(0);
        let mut entries: Vec<RedisEntry> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut cursor = b"0".to_vec();
        loop {
            let response = session.call(&[b"SCAN", &cursor, b"COUNT", b"500"])?;
            let Value::Array(parts) = response else {
                return Err(DatabaseError::invalid("invalid SCAN response"));
            };
            if parts.len() != 2 {
                return Err(DatabaseError::invalid("invalid SCAN response"));
            }
            cursor = match &parts[0] {
                Value::Bulk(bytes) => bytes.clone(),
                _ => return Err(DatabaseError::invalid("invalid SCAN cursor")),
            };
            let Value::Array(raw_keys) = &parts[1] else {
                return Err(DatabaseError::invalid("invalid SCAN keys array"));
            };
            let mut batch = Vec::new();
            for key in raw_keys {
                let Value::Bulk(bytes) = key else {
                    continue;
                };
                if seen.insert(bytes.clone()) {
                    batch.push(bytes.clone());
                }
            }
            if !batch.is_empty() {
                let count = batch.len().to_string();
                let mut args: Vec<Vec<u8>> = vec![
                    b"EVAL".to_vec(),
                    redis_dump_batch_script().to_vec(),
                    count.into_bytes(),
                ];
                args.extend(batch);
                let response = session.call_owned(&args)?;
                let Value::Array(triples) = response else {
                    return Err(DatabaseError::invalid("invalid DUMP batch response"));
                };
                if triples.len() % 3 != 0 {
                    return Err(DatabaseError::invalid("invalid DUMP batch response"));
                }
                for triple in triples.chunks(3) {
                    let (Value::Bulk(key), Value::Bulk(value), Value::Integer(ttl)) =
                        (&triple[0], &triple[1], &triple[2])
                    else {
                        return Err(DatabaseError::invalid("invalid DUMP batch entry"));
                    };
                    entries.push(RedisEntry {
                        key: key.clone(),
                        value: value.clone(),
                        ttl: *ttl,
                    });
                }
            }
            if cursor == b"0" {
                break;
            }
        }
        let document = RedisBackup {
            schema: 1,
            created,
            entries,
        };
        let mut bytes = serde_json::to_vec(&document)?;
        bytes.push(b'\n');
        crate::fs::atomic_write(path, &bytes, crate::fs::PRIVATE_FILE)?;
        Ok(())
    }

    fn restore(
        &self,
        port: i64,
        password: &str,
        db: i64,
        path: &Path,
    ) -> Result<(), DatabaseError> {
        let bytes = crate::fs::read_regular_file(
            path.parent().unwrap_or_else(|| Path::new(".")),
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default(),
            512 << 20,
        )?;
        let backup: RedisBackup = serde_json::from_slice(&bytes)?;
        if backup.schema != 1 || backup.created <= 0 {
            return Err(DatabaseError::invalid("unsupported Redis backup"));
        }
        let mut session = Session::open(port, password, db)?;
        let Value::Integer(size) = session.call(&[b"DBSIZE"])? else {
            return Err(DatabaseError::invalid("invalid Redis response"));
        };
        if size != 0 {
            return Err(DatabaseError::invalid("restore target must be empty"));
        }
        let elapsed = (SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_millis() as i64)
            .unwrap_or(0)
            - backup.created)
            .max(0);
        let mut batch: Vec<Vec<u8>> = Vec::new();
        let restore = (|| -> Result<(), DatabaseError> {
            for entry in &backup.entries {
                let ttl = if entry.ttl >= 0 {
                    let ttl = entry.ttl - elapsed;
                    if ttl <= 0 {
                        continue;
                    }
                    ttl
                } else {
                    0
                };
                batch.push(entry.key.clone());
                batch.push(ttl.to_string().into_bytes());
                batch.push(entry.value.clone());
                if batch.len() >= 500 * 3 {
                    flush_restore_batch(&mut session, &mut batch)?;
                }
            }
            flush_restore_batch(&mut session, &mut batch)
        })();
        if restore.is_err() {
            let _ = session.call(&[b"FLUSHDB"]);
        }
        restore
    }
}

fn flush_restore_batch(
    session: &mut Session,
    batch: &mut Vec<Vec<u8>>,
) -> Result<(), DatabaseError> {
    if batch.is_empty() {
        return Ok(());
    }
    let mut args: Vec<Vec<u8>> = vec![
        b"EVAL".to_vec(),
        redis_restore_batch_script().to_vec(),
        b"0".to_vec(),
    ];
    args.append(batch);
    session.call_owned(&args)?;
    Ok(())
}

fn redis_dump_batch_script() -> &'static [u8] {
    b"local out={}\nfor i=1,#KEYS do\n  local value=redis.call('DUMP',KEYS[i])\n  if value then\n    table.insert(out,KEYS[i])\n    table.insert(out,value)\n    table.insert(out,redis.call('PTTL',KEYS[i]))\n  end\nend\nreturn out"
}

fn redis_restore_batch_script() -> &'static [u8] {
    b"local restored={}\nfor i=1,#ARGV,3 do\n  local result=redis.pcall('RESTORE',ARGV[i],ARGV[i+1],ARGV[i+2])\n  if type(result)=='table' and result.err then\n    return redis.error_reply(result.err)\n  end\n  table.insert(restored,ARGV[i])\nend\nreturn #restored"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RedisEntry {
    #[serde(with = "base64_bytes")]
    key: Vec<u8>,
    #[serde(with = "base64_bytes")]
    value: Vec<u8>,
    ttl: i64,
}

/// Go's `encoding/json` encodes `[]byte` as base64; keep backups readable by
/// both implementations during the phase-in.
mod base64_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let value = (u32::from(chunk[0]) << 16)
                | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
                | u32::from(*chunk.get(2).unwrap_or(&0));
            out.push(ALPHABET[((value >> 18) & 63) as usize] as char);
            out.push(ALPHABET[((value >> 12) & 63) as usize] as char);
            out.push(if chunk.len() > 1 {
                ALPHABET[((value >> 6) & 63) as usize] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                ALPHABET[(value & 63) as usize] as char
            } else {
                '='
            });
        }
        serializer.serialize_str(&out)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        let mut out = Vec::new();
        let mut buffer = 0u32;
        let mut bits = 0u32;
        for character in text.bytes() {
            if character == b'=' {
                break;
            }
            let value = match character {
                b'A'..=b'Z' => character - b'A',
                b'a'..=b'z' => character - b'a' + 26,
                b'0'..=b'9' => character - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                _ => {
                    return Err(serde::de::Error::custom("invalid base64"));
                }
            };
            buffer = (buffer << 6) | u32::from(value);
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push(((buffer >> bits) & 0xff) as u8);
            }
        }
        Ok(out)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RedisBackup {
    schema: i64,
    created: i64,
    entries: Vec<RedisEntry>,
}

/// `db<n>:` lines from `INFO keyspace`, sorted; `["0"]` when empty.
pub fn parse_redis_keyspace_databases(info: &str) -> Vec<String> {
    let mut numbers: Vec<i64> = Vec::new();
    for line in info.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("db") else {
            continue;
        };
        let Some((prefix, _)) = rest.split_once(':') else {
            continue;
        };
        let Ok(number) = prefix.parse::<i64>() else {
            continue;
        };
        if !(0..=15).contains(&number) || numbers.contains(&number) {
            continue;
        }
        numbers.push(number);
    }
    if numbers.is_empty() {
        return vec!["0".to_string()];
    }
    numbers.sort_unstable();
    numbers.into_iter().map(|value| value.to_string()).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Value {
    Simple(String),
    Integer(i64),
    Bulk(Vec<u8>),
    Array(Vec<Value>),
    Nil,
}

struct Session {
    stream: TcpStream,
    buffer: Vec<u8>,
    position: usize,
}

impl Session {
    fn open(port: i64, password: &str, db: i64) -> Result<Self, DatabaseError> {
        let port = u16::try_from(port)
            .map_err(|_| DatabaseError::invalid("invalid database connection port"))?;
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        let stream = TcpStream::connect_timeout(&address, Duration::from_secs(3))?;
        let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
        let mut session = Session {
            stream,
            buffer: Vec::new(),
            position: 0,
        };
        session
            .call(&[b"AUTH", password.as_bytes()])
            .map_err(|_| DatabaseError::invalid("Redis authentication failed"))?;
        session.call(&[b"SELECT", db.to_string().as_bytes()])?;
        Ok(session)
    }

    fn call(&mut self, args: &[&[u8]]) -> Result<Value, DatabaseError> {
        let owned: Vec<Vec<u8>> = args.iter().map(|arg| arg.to_vec()).collect();
        self.call_owned(&owned)
    }

    fn call_owned(&mut self, args: &[Vec<u8>]) -> Result<Value, DatabaseError> {
        write_command(&mut self.stream, args)?;
        self.stream.flush()?;
        self.read_value(0)
    }

    fn read_value(&mut self, depth: usize) -> Result<Value, DatabaseError> {
        if depth > 8 {
            return Err(DatabaseError::invalid("Redis response nesting limit"));
        }
        let line = self.read_line()?;
        let (kind, text) = line.split_at(1);
        match kind {
            "+" => Ok(Value::Simple(text.to_string())),
            "-" => Err(DatabaseError::invalid(text.to_string())),
            ":" => text
                .parse::<i64>()
                .map(Value::Integer)
                .map_err(|_| DatabaseError::invalid("invalid Redis response")),
            "$" => {
                let length: i64 = text
                    .parse()
                    .map_err(|_| DatabaseError::invalid("invalid Redis bulk length"))?;
                if !(-1..=256 << 20).contains(&length) {
                    return Err(DatabaseError::invalid("invalid Redis bulk length"));
                }
                if length == -1 {
                    return Ok(Value::Nil);
                }
                let mut bytes = vec![0u8; length as usize + 2];
                self.read_exact(&mut bytes)?;
                if &bytes[length as usize..] != b"\r\n" {
                    return Err(DatabaseError::invalid("invalid Redis bulk terminator"));
                }
                bytes.truncate(length as usize);
                Ok(Value::Bulk(bytes))
            }
            "*" => {
                let count: i64 = text
                    .parse()
                    .map_err(|_| DatabaseError::invalid("invalid Redis array length"))?;
                if !(-1..=300_000).contains(&count) {
                    return Err(DatabaseError::invalid("invalid Redis array length"));
                }
                if count == -1 {
                    return Ok(Value::Nil);
                }
                let mut values = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    values.push(self.read_value(depth + 1)?);
                }
                Ok(Value::Array(values))
            }
            _ => Err(DatabaseError::invalid("unknown Redis response")),
        }
    }

    fn read_line(&mut self) -> Result<String, DatabaseError> {
        loop {
            if let Some(index) = self.buffer[self.position..]
                .iter()
                .position(|byte| *byte == b'\n')
            {
                let end = self.position + index;
                let line = &self.buffer[self.position..end];
                self.position = end + 1;
                if line.len() < 2 || !line.ends_with(b"\r") {
                    return Err(DatabaseError::invalid("invalid Redis response"));
                }
                return String::from_utf8(line[..line.len() - 1].to_vec())
                    .map_err(|_| DatabaseError::invalid("invalid Redis response"));
            }
            self.fill()?;
        }
    }

    fn read_exact(&mut self, out: &mut [u8]) -> Result<(), DatabaseError> {
        while out.len() > self.buffer.len() - self.position {
            self.fill()?;
        }
        out.copy_from_slice(&self.buffer[self.position..self.position + out.len()]);
        self.position += out.len();
        Ok(())
    }

    fn fill(&mut self) -> Result<(), DatabaseError> {
        if self.position > 0 {
            self.buffer.drain(..self.position);
            self.position = 0;
        }
        let mut chunk = [0u8; 8192];
        let read = self.stream.read(&mut chunk)?;
        if read == 0 {
            return Err(DatabaseError::invalid("Redis connection closed"));
        }
        self.buffer.extend_from_slice(&chunk[..read]);
        Ok(())
    }
}

fn write_command(stream: &mut TcpStream, args: &[Vec<u8>]) -> Result<(), DatabaseError> {
    write!(stream, "*{}\r\n", args.len())?;
    for arg in args {
        write!(stream, "${}\r\n", arg.len())?;
        stream.write_all(arg)?;
        stream.write_all(b"\r\n")?;
    }
    Ok(())
}

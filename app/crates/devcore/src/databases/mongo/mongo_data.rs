//! Authenticated MongoDB wire operations for inventory, drops, and logical
//! backup/restore. `mongod` ships no shell, so the port speaks OP_MSG directly
//! and authenticates with SCRAM-SHA-256 (the mechanism the Go driver picks).
//!
//! Tests never reach this module: they inject a [`super::MongoAdmin`] fake.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{document, element_array, element_document, element_i32, element_string};
use crate::databases::DatabaseError;
use crate::databases::inventory::InstanceCatalog;

const MESSAGE_LIMIT: usize = 16 * 1024 * 1024;
const DOCUMENT_LIMIT: usize = 256 << 20;
const FILE_LIMIT: u64 = 512 << 20;

pub(super) struct Session {
    stream: TcpStream,
    request_id: i32,
}

impl Session {
    pub(super) fn open(port: i64, password: &str) -> Result<Self, DatabaseError> {
        let port = u16::try_from(port)
            .map_err(|_| DatabaseError::invalid("invalid database connection port"))?;
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        let stream =
            TcpStream::connect_timeout(&address, Duration::from_secs(3)).map_err(|error| {
                DatabaseError::invalid(format!("MongoDB connection failed: {error}"))
            })?;
        let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
        let mut session = Session {
            stream,
            request_id: 1,
        };
        session.request("admin", vec![element_i32("hello", 1)])?;
        session.authenticate("devmanager", password)?;
        Ok(session)
    }

    fn request(
        &mut self,
        database: &str,
        mut elements: Vec<Vec<u8>>,
    ) -> Result<Vec<u8>, DatabaseError> {
        elements.push(element_string("$db", database));
        let command = document(elements);
        self.request_id += 1;
        let reply = exchange_raw(&mut self.stream, self.request_id, &command)?;
        let ok = double_value(&reply, "ok")?.unwrap_or(0.0);
        if ok != 1.0 {
            let message = element_string_value(&reply, "errmsg")?
                .unwrap_or_else(|| "the server rejected the command".to_string());
            return Err(DatabaseError::invalid(format!(
                "MongoDB command failed: {message}"
            )));
        }
        Ok(reply)
    }

    fn authenticate(&mut self, user: &str, password: &str) -> Result<(), DatabaseError> {
        let nonce = format!(
            "{}{}",
            crate::software::random_id(),
            crate::software::random_id()
        );
        let escaped_user = user.replace('=', "=3D").replace(',', "=2C");
        let client_first_bare = format!("n={escaped_user},r={nonce}");
        let client_first = format!("n,,{client_first_bare}");
        let reply = self.request(
            "admin",
            vec![
                element_i32("saslStart", 1),
                element_string("mechanism", "SCRAM-SHA-256"),
                element_binary("payload", client_first.as_bytes()),
                element_i32("autoAuthorize", 1),
            ],
        )?;
        let conversation = element_i32_value(&reply, "conversationId")?
            .ok_or_else(|| DatabaseError::invalid("MongoDB authentication reply is invalid"))?;
        let server_first =
            String::from_utf8(element_binary_value(&reply, "payload")?.ok_or_else(|| {
                DatabaseError::invalid("MongoDB authentication reply is invalid")
            })?)
            .map_err(|_| DatabaseError::invalid("MongoDB authentication reply is invalid"))?;
        let values: BTreeMap<&str, &str> = server_first
            .split(',')
            .filter_map(|part| part.split_once('='))
            .collect();
        let server_nonce = *values
            .get("r")
            .ok_or_else(|| DatabaseError::invalid("MongoDB authentication reply is invalid"))?;
        let salt =
            decode_base64(values.get("s").ok_or_else(|| {
                DatabaseError::invalid("MongoDB authentication reply is invalid")
            })?)?;
        let iterations: u32 = values
            .get("i")
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| DatabaseError::invalid("MongoDB authentication reply is invalid"))?;
        if !server_nonce.starts_with(&nonce) {
            return Err(DatabaseError::invalid(
                "MongoDB authentication nonce does not match",
            ));
        }
        let salted = pbkdf2_sha256(password.as_bytes(), &salt, iterations, 32);
        let client_key = hmac_sha256(&salted, b"Client Key");
        let stored_key = Sha256::digest(&client_key);
        let final_without_proof = format!("c=biws,r={server_nonce}");
        let auth_message = format!("{client_first_bare},{server_first},{final_without_proof}");
        let signature = hmac_sha256(&stored_key, auth_message.as_bytes());
        let mut proof = client_key.clone();
        for (slot, byte) in proof.iter_mut().zip(signature.iter()) {
            *slot ^= byte;
        }
        let final_message = format!("{final_without_proof},p={}", encode_base64(&proof));
        let reply = self.request(
            "admin",
            vec![
                element_i32("saslContinue", 1),
                element_i32("conversationId", conversation),
                element_binary("payload", final_message.as_bytes()),
            ],
        )?;
        if let Some(payload) = element_binary_value(&reply, "payload")?
            && !payload.is_empty()
        {
            let server_final = String::from_utf8(payload)
                .map_err(|_| DatabaseError::invalid("MongoDB authentication reply is invalid"))?;
            let server_key = hmac_sha256(&salted, b"Server Key");
            let expected = encode_base64(&hmac_sha256(&server_key, auth_message.as_bytes()));
            let server_signature = server_final
                .split(',')
                .find_map(|part| part.strip_prefix("v="))
                .ok_or_else(|| DatabaseError::invalid("MongoDB authentication reply is invalid"))?;
            if server_signature != expected {
                return Err(DatabaseError::invalid(
                    "MongoDB server signature does not match",
                ));
            }
        }
        if element_bool_value(&reply, "done")?.unwrap_or(false) {
            return Ok(());
        }
        self.request(
            "admin",
            vec![
                element_i32("saslContinue", 1),
                element_i32("conversationId", conversation),
                element_binary("payload", b""),
            ],
        )?;
        Ok(())
    }
}

fn exchange_raw(
    stream: &mut TcpStream,
    request_id: i32,
    command: &[u8],
) -> Result<Vec<u8>, DatabaseError> {
    let mut body = Vec::with_capacity(4 + 1 + command.len());
    body.extend_from_slice(&0i32.to_le_bytes());
    body.push(0);
    body.extend_from_slice(command);
    let length = 16 + body.len() as i32;
    let mut message = Vec::with_capacity(length as usize);
    message.extend_from_slice(&length.to_le_bytes());
    message.extend_from_slice(&request_id.to_le_bytes());
    message.extend_from_slice(&0i32.to_le_bytes());
    message.extend_from_slice(&2013i32.to_le_bytes());
    message.extend_from_slice(&body);
    stream
        .write_all(&message)
        .and_then(|()| stream.flush())
        .map_err(|error| DatabaseError::invalid(format!("MongoDB command failed: {error}")))?;
    let mut header = [0u8; 16];
    stream
        .read_exact(&mut header)
        .map_err(|error| DatabaseError::invalid(format!("MongoDB command failed: {error}")))?;
    let reply_length = i32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    if !(16..=MESSAGE_LIMIT as i32).contains(&reply_length) {
        return Err(DatabaseError::invalid(
            "MongoDB reply is not a valid message",
        ));
    }
    let mut payload = vec![0u8; reply_length as usize - 16];
    stream
        .read_exact(&mut payload)
        .map_err(|error| DatabaseError::invalid(format!("MongoDB command failed: {error}")))?;
    if payload.len() < 5 || payload[4] != 0 {
        return Err(DatabaseError::invalid("MongoDB reply is not an OP_MSG"));
    }
    let body = &payload[5..];
    let length = i32::from_le_bytes([body[0], body[1], body[2], body[3]]);
    if length < 5 || length as usize > body.len() {
        return Err(DatabaseError::invalid("MongoDB reply document is invalid"));
    }
    Ok(body[..length as usize].to_vec())
}

pub(super) fn catalog(port: i64, password: &str) -> Result<InstanceCatalog, DatabaseError> {
    let mut session = Session::open(port, password)?;
    let reply = session.request(
        "admin",
        vec![element_i32("listDatabases", 1), element_i32("nameOnly", 1)],
    )?;
    let mut databases = Vec::new();
    if let Some(array) = element_document_value(&reply, "databases")? {
        for value in array_values(&array)? {
            if let Some(name) = element_string_value(&value, "name")?
                && name != "local"
                && name != "config"
            {
                databases.push(name);
            }
        }
    }
    databases.sort();
    let reply = session.request(
        "admin",
        vec![element_document(
            "usersInfo",
            &document(vec![element_i32("forAllDBs", 1)]),
        )],
    )?;
    let mut users = Vec::new();
    let mut seen = BTreeMap::new();
    if let Some(array) = element_document_value(&reply, "users")? {
        for value in array_values(&array)? {
            let Some(user) = element_string_value(&value, "user")? else {
                continue;
            };
            if user.is_empty() {
                continue;
            }
            let database = element_string_value(&value, "db")?.unwrap_or_default();
            let label = if !database.is_empty() && database != "admin" {
                format!("{user}@{database}")
            } else {
                user
            };
            seen.insert(label.clone(), ());
            users.push(label);
        }
    }
    let _ = seen;
    users.sort();
    Ok(InstanceCatalog { databases, users })
}

pub(super) fn create_database(port: i64, password: &str, name: &str) -> Result<(), DatabaseError> {
    let mut session = Session::open(port, password)?;
    session.request(name, vec![element_string("create", "__devmanager")])?;
    Ok(())
}

pub(super) fn drop_database(port: i64, password: &str, name: &str) -> Result<(), DatabaseError> {
    let mut session = Session::open(port, password)?;
    session.request(name, vec![element_i32("dropDatabase", 1)])?;
    Ok(())
}

pub(super) fn create_user(
    port: i64,
    password: &str,
    user: &str,
    user_password: &str,
    database: &str,
) -> Result<(), DatabaseError> {
    let mut session = Session::open(port, password)?;
    let roles = document(vec![element_document(
        "0",
        &document(vec![
            element_string("role", "readWrite"),
            element_string("db", database),
        ]),
    )]);
    session.request(
        database,
        vec![
            element_string("createUser", user),
            element_string("pwd", user_password),
            element_array("roles", &roles),
        ],
    )?;
    Ok(())
}

pub(super) fn drop_user(
    port: i64,
    password: &str,
    user: &str,
    database: &str,
) -> Result<(), DatabaseError> {
    let mut session = Session::open(port, password)?;
    session.request(database, vec![element_string("dropUser", user)])?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Backup {
    schema: i64,
    collections: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    metadata: BTreeMap<String, Metadata>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Metadata {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    options: String,
    #[serde(default)]
    indexes: Vec<String>,
}

pub(super) fn backup(
    port: i64,
    password: &str,
    database: &str,
    path: &Path,
) -> Result<(), DatabaseError> {
    let mut session = Session::open(port, password)?;
    let reply = session.request(
        database,
        vec![
            element_i32("listCollections", 1),
            element_document("filter", &document(Vec::new())),
        ],
    )?;
    let first = element_document_value(&reply, "cursor")?
        .and_then(|cursor| element_document_value(&cursor, "firstBatch").ok().flatten())
        .ok_or_else(|| DatabaseError::invalid("MongoDB collection listing is invalid"))?;
    let mut backup = Backup {
        schema: 2,
        collections: BTreeMap::new(),
        metadata: BTreeMap::new(),
    };
    let mut total = 0usize;
    for collection in array_values(&first)? {
        let Some(name) = element_string_value(&collection, "name")? else {
            continue;
        };
        if name.starts_with("system.") {
            continue;
        }
        let kind = element_string_value(&collection, "type")?.unwrap_or_default();
        let options = element_document_value(&collection, "options")?
            .map(|value| crate::fs::encode_hex(&value))
            .unwrap_or_default();
        let mut metadata = Metadata {
            kind: kind.clone(),
            options,
            indexes: Vec::new(),
        };
        let mut documents = Vec::new();
        if kind != "view" {
            let indexes = session.request(database, vec![element_string("listIndexes", &name)])?;
            if let Some(cursor) = element_document_value(&indexes, "cursor")?
                && let Some(batch) = element_document_value(&cursor, "firstBatch")?
            {
                for index in array_values(&batch)? {
                    total += index.len();
                    metadata.indexes.push(crate::fs::encode_hex(&index));
                }
            }
            let mut reply = session.request(
                database,
                vec![
                    element_string("find", &name),
                    element_document("filter", &document(Vec::new())),
                ],
            )?;
            loop {
                let cursor = element_document_value(&reply, "cursor")?
                    .ok_or_else(|| DatabaseError::invalid("MongoDB find reply is invalid"))?;
                if let Some(batch) = element_document_value(&cursor, "firstBatch")?
                    && !batch.is_empty()
                {
                    for value in array_values(&batch)? {
                        total += value.len();
                        if total > DOCUMENT_LIMIT {
                            return Err(DatabaseError::invalid(
                                "MongoDB logical backup exceeds the 256 MiB document limit",
                            ));
                        }
                        documents.push(crate::fs::encode_hex(&value));
                    }
                }
                let cursor_id = element_i64_value(&cursor, "id")?.unwrap_or(0);
                if cursor_id == 0 {
                    break;
                }
                reply = session.request(
                    database,
                    vec![
                        element_i64("getMore", cursor_id),
                        element_string("collection", &name),
                    ],
                )?;
            }
        }
        backup.collections.insert(name.clone(), documents);
        backup.metadata.insert(name, metadata);
    }
    let mut bytes = serde_json::to_vec(&backup)?;
    bytes.push(b'\n');
    crate::fs::atomic_write(path, &bytes, crate::fs::PRIVATE_FILE)?;
    Ok(())
}

pub(super) fn restore(
    port: i64,
    password: &str,
    database: &str,
    path: &Path,
) -> Result<(), DatabaseError> {
    let info = std::fs::metadata(path)?;
    if info.len() > FILE_LIMIT {
        return Err(DatabaseError::invalid(
            "MongoDB backup file exceeds 512 MiB",
        ));
    }
    let bytes = std::fs::read(path)?;
    let backup: Backup = serde_json::from_slice(&bytes)?;
    if !(backup.schema == 1 || backup.schema == 2) {
        return Err(DatabaseError::invalid("unsupported MongoDB backup schema"));
    }
    let mut session = Session::open(port, password)?;
    let names = session.request(
        "admin",
        vec![
            element_i32("listDatabases", 1),
            element_i32("nameOnly", 1),
            element_document("filter", &document(vec![element_string("name", database)])),
        ],
    )?;
    if let Some(array) = element_document_value(&names, "databases")?
        && !array_values(&array)?.is_empty()
    {
        return Err(DatabaseError::invalid("restore target already exists"));
    }
    let mut collections: Vec<&String> = backup.collections.keys().collect();
    collections.sort_by(|left, right| {
        let left_view = backup
            .metadata
            .get(*left)
            .map(|meta| meta.kind == "view")
            .unwrap_or(false);
        let right_view = backup
            .metadata
            .get(*right)
            .map(|meta| meta.kind == "view")
            .unwrap_or(false);
        left_view.cmp(&right_view).then_with(|| left.cmp(right))
    });
    for name in collections {
        let metadata = backup.metadata.get(name).cloned().unwrap_or_default();
        let mut command = vec![element_string("create", name)];
        if !metadata.options.is_empty()
            && let Some(options) = crate::fs::decode_hex(&metadata.options)
        {
            for element in filter_elements(&options, &[])? {
                command.push(element);
            }
        }
        session.request(database, command)?;
        let documents: Vec<Vec<u8>> = backup.collections[name]
            .iter()
            .filter_map(|hex| crate::fs::decode_hex(hex))
            .collect();
        for batch in documents.chunks(500) {
            let array = document(
                batch
                    .iter()
                    .enumerate()
                    .map(|(index, value)| element_document(&index.to_string(), value))
                    .collect(),
            );
            session.request(
                database,
                vec![
                    element_string("insert", name),
                    element_array("documents", &array),
                ],
            )?;
        }
        let mut indexes = Vec::new();
        for index in &metadata.indexes {
            let Some(raw) = crate::fs::decode_hex(index) else {
                continue;
            };
            if element_string_value(&raw, "name")?.as_deref() == Some("_id_") {
                continue;
            }
            let filtered = filter_elements(&raw, &["ns", "v"])?;
            indexes.extend(filtered);
        }
        if !indexes.is_empty() {
            let array = document(indexes);
            session.request(
                database,
                vec![
                    element_string("createIndexes", name),
                    element_array("indexes", &array),
                ],
            )?;
        }
    }
    Ok(())
}

fn element_i64(key: &str, value: i64) -> Vec<u8> {
    let mut out = vec![0x12];
    out.extend_from_slice(key.as_bytes());
    out.push(0);
    out.extend_from_slice(&value.to_le_bytes());
    out
}

fn element_binary(key: &str, value: &[u8]) -> Vec<u8> {
    let mut out = vec![0x05];
    out.extend_from_slice(key.as_bytes());
    out.push(0);
    out.extend_from_slice(&(value.len() as i32).to_le_bytes());
    out.push(0);
    out.extend_from_slice(value);
    out
}

#[derive(Debug, Clone)]
struct RawElement {
    key: String,
    tag: u8,
    start: usize,
    end: usize,
}

fn raw_elements(document: &[u8]) -> Result<Vec<RawElement>, DatabaseError> {
    let end = document_end(document)?;
    let mut out = Vec::new();
    let mut index = 4;
    while index < end {
        let tag = document[index];
        index += 1;
        let key_end = document[index..]
            .iter()
            .position(|byte| *byte == 0)
            .map(|offset| index + offset)
            .ok_or_else(|| DatabaseError::invalid("MongoDB document is invalid"))?;
        let key = String::from_utf8(document[index..key_end].to_vec())
            .map_err(|_| DatabaseError::invalid("MongoDB document is invalid"))?;
        index = key_end + 1;
        let start = index;
        index = skip(document, tag, index)?;
        out.push(RawElement {
            key,
            tag,
            start,
            end: index,
        });
    }
    Ok(out)
}

fn filter_elements(document: &[u8], skipped: &[&str]) -> Result<Vec<Vec<u8>>, DatabaseError> {
    let mut out = Vec::new();
    for element in raw_elements(document)? {
        if skipped.contains(&element.key.as_str()) {
            continue;
        }
        let mut bytes = vec![element.tag];
        bytes.extend_from_slice(element.key.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&document[element.start..element.end]);
        out.push(bytes);
    }
    Ok(out)
}

fn document_end(document: &[u8]) -> Result<usize, DatabaseError> {
    if document.len() < 5 {
        return Err(DatabaseError::invalid("MongoDB document is invalid"));
    }
    let length = i32::from_le_bytes([document[0], document[1], document[2], document[3]]);
    if length < 5 || length as usize > document.len() {
        return Err(DatabaseError::invalid("MongoDB document is invalid"));
    }
    Ok(length as usize - 1)
}

fn skip(document: &[u8], tag: u8, index: usize) -> Result<usize, DatabaseError> {
    let invalid = || DatabaseError::invalid("MongoDB document is invalid");
    match tag {
        0x06 | 0x0A | 0x7F | 0xFF => Ok(index),
        0x08 => Ok(index + 1),
        0x09 | 0x11 | 0x12 => Ok(index + 8),
        0x01 => Ok(index + 8),
        0x10 => Ok(index + 4),
        0x03 | 0x04 => {
            let length = i32::from_le_bytes([
                document[index],
                document[index + 1],
                document[index + 2],
                document[index + 3],
            ]);
            if length < 5 {
                return Err(invalid());
            }
            Ok(index + length as usize)
        }
        0x05 => {
            let length = i32::from_le_bytes([
                document[index],
                document[index + 1],
                document[index + 2],
                document[index + 3],
            ]);
            if length < 0 {
                return Err(invalid());
            }
            Ok(index + 4 + 1 + length as usize)
        }
        0x02 => {
            let length = i32::from_le_bytes([
                document[index],
                document[index + 1],
                document[index + 2],
                document[index + 3],
            ]);
            if length < 1 {
                return Err(invalid());
            }
            Ok(index + 4 + length as usize)
        }
        0x07 => Ok(index + 12),
        0x13 => Ok(index + 16),
        _ => Err(invalid()),
    }
}

fn find_element(document: &[u8], key: &str) -> Result<Option<RawElement>, DatabaseError> {
    Ok(raw_elements(document)?
        .into_iter()
        .find(|element| element.key == key))
}

fn element_string_value(document: &[u8], key: &str) -> Result<Option<String>, DatabaseError> {
    let Some(element) = find_element(document, key)? else {
        return Ok(None);
    };
    if element.tag != 0x02 {
        return Ok(None);
    }
    let length = i32::from_le_bytes([
        document[element.start],
        document[element.start + 1],
        document[element.start + 2],
        document[element.start + 3],
    ]);
    let start = element.start + 4;
    let end = start + length as usize;
    if length < 1 || end > document.len() {
        return Err(DatabaseError::invalid("MongoDB document is invalid"));
    }
    String::from_utf8(document[start..end - 1].to_vec())
        .map(Some)
        .map_err(|_| DatabaseError::invalid("MongoDB document is invalid"))
}

fn element_i32_value(document: &[u8], key: &str) -> Result<Option<i32>, DatabaseError> {
    let Some(element) = find_element(document, key)? else {
        return Ok(None);
    };
    if element.tag != 0x10 {
        return Ok(None);
    }
    Ok(Some(i32::from_le_bytes([
        document[element.start],
        document[element.start + 1],
        document[element.start + 2],
        document[element.start + 3],
    ])))
}

fn element_i64_value(document: &[u8], key: &str) -> Result<Option<i64>, DatabaseError> {
    let Some(element) = find_element(document, key)? else {
        return Ok(None);
    };
    if element.tag != 0x12 {
        return Ok(None);
    }
    let bytes = document
        .get(element.start..element.start + 8)
        .ok_or_else(|| DatabaseError::invalid("MongoDB document is invalid"))?;
    Ok(Some(i64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])))
}

fn double_value(document: &[u8], key: &str) -> Result<Option<f64>, DatabaseError> {
    let Some(element) = find_element(document, key)? else {
        return Ok(None);
    };
    if element.tag != 0x01 && element.tag != 0x10 {
        return Ok(None);
    }
    if element.tag == 0x10 {
        return Ok(element_i32_value(document, key)?.map(f64::from));
    }
    let bytes = document
        .get(element.start..element.start + 8)
        .ok_or_else(|| DatabaseError::invalid("MongoDB document is invalid"))?;
    Ok(Some(f64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])))
}

fn element_bool_value(document: &[u8], key: &str) -> Result<Option<bool>, DatabaseError> {
    let Some(element) = find_element(document, key)? else {
        return Ok(None);
    };
    Ok(Some(document[element.start] != 0))
}

fn element_binary_value(document: &[u8], key: &str) -> Result<Option<Vec<u8>>, DatabaseError> {
    let Some(element) = find_element(document, key)? else {
        return Ok(None);
    };
    if element.tag != 0x05 {
        return Ok(None);
    }
    let length = i32::from_le_bytes([
        document[element.start],
        document[element.start + 1],
        document[element.start + 2],
        document[element.start + 3],
    ]);
    let start = element.start + 5;
    if length < 0 || start + length as usize > document.len() {
        return Err(DatabaseError::invalid("MongoDB document is invalid"));
    }
    Ok(Some(document[start..start + length as usize].to_vec()))
}

fn element_document_value(document: &[u8], key: &str) -> Result<Option<Vec<u8>>, DatabaseError> {
    let Some(element) = find_element(document, key)? else {
        return Ok(None);
    };
    if element.tag != 0x03 && element.tag != 0x04 {
        return Ok(None);
    }
    Ok(Some(document[element.start..element.end].to_vec()))
}

fn array_values(array: &[u8]) -> Result<Vec<Vec<u8>>, DatabaseError> {
    let mut out = Vec::new();
    for element in raw_elements(array)? {
        let mut value = Vec::new();
        value.extend_from_slice(&array[element.start..element.end]);
        out.push(value);
    }
    Ok(out)
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> Vec<u8> {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Vec::with_capacity(64 + message.len());
    let mut outer = Vec::with_capacity(64 + 32);
    for byte in block {
        inner.push(byte ^ 0x36);
        outer.push(byte ^ 0x5c);
    }
    inner.extend_from_slice(message);
    let inner = Sha256::digest(inner);
    outer.extend_from_slice(&inner);
    Sha256::digest(outer).to_vec()
}

fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32, length: usize) -> Vec<u8> {
    let mut output = Vec::with_capacity(length);
    let mut block = 1u32;
    while output.len() < length {
        let mut message = salt.to_vec();
        message.extend_from_slice(&block.to_be_bytes());
        let mut current = hmac_sha256(password, &message);
        let mut value = current.clone();
        for _ in 1..iterations {
            current = hmac_sha256(password, &current);
            for (slot, byte) in value.iter_mut().zip(current.iter()) {
                *slot ^= byte;
            }
        }
        output.extend_from_slice(&value);
        block += 1;
    }
    output.truncate(length);
    output
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn encode_base64(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(BASE64[((value >> 18) & 63) as usize] as char);
        out.push(BASE64[((value >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            BASE64[((value >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            BASE64[(value & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

fn decode_base64(text: &str) -> Result<Vec<u8>, DatabaseError> {
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
            _ => return Err(DatabaseError::invalid("invalid base64")),
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

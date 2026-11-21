//! The helper's JSON types, ported from Go's `protocol.go`.
//!
//! Go encodes `[]byte` as a base64 string and omits zero-value fields marked
//! `omitempty`. The serde attributes below preserve both, and field order
//! follows the Go declarations because `encoding/json` marshals structs in
//! declaration order.

use serde::{Deserialize, Serialize};

/// The request and response body types travel on one HTTP request per
/// operation, so they use only the standard alphabet with padding.
pub const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encodes bytes the way Go's `encoding/base64.StdEncoding` does.
pub fn base64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).copied().map(u32::from);
        let b2 = chunk.get(2).copied().map(u32::from);
        out.push(BASE64_ALPHABET[(b0 >> 2) as usize] as char);
        out.push(BASE64_ALPHABET[(((b0 & 0x03) << 4) | b1.unwrap_or(0) >> 4) as usize] as char);
        match (b1, b2) {
            (Some(b1), Some(b2)) => {
                out.push(BASE64_ALPHABET[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
                out.push(BASE64_ALPHABET[(b2 & 0x3f) as usize] as char);
            }
            (Some(b1), None) => {
                out.push(BASE64_ALPHABET[((b1 & 0x0f) << 2) as usize] as char);
                out.push('=');
            }
            (None, _) => {
                out.push('=');
                out.push('=');
            }
        }
    }
    out
}

/// Decodes standard padded base64, reporting Go's `CorruptInputError` shape.
pub fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    let bytes = input.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return Err(format!(
            "illegal base64 data at input byte {}",
            bytes.len() / 4 * 4
        ));
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let mut index = 0;
    while index < bytes.len() {
        let chunk = &bytes[index..index + 4];
        let last = index + 4 == bytes.len();
        let pad = chunk.iter().filter(|b| **b == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return Err(format!("illegal base64 data at input byte {index}"));
        }
        for (offset, byte) in chunk.iter().enumerate() {
            if *byte == b'=' && offset < 4 - pad {
                return Err(format!("illegal base64 data at input byte {index}"));
            }
        }
        let value = |byte: u8, position: usize| -> Result<u32, String> {
            if byte == b'=' {
                return Ok(0);
            }
            BASE64_ALPHABET
                .iter()
                .position(|candidate| *candidate == byte)
                .map(|value| value as u32)
                .ok_or_else(|| format!("illegal base64 data at input byte {}", index + position))
        };
        let v0 = value(chunk[0], 0)?;
        let v1 = value(chunk[1], 1)?;
        let v2 = value(chunk[2], 2)?;
        let v3 = value(chunk[3], 3)?;
        out.push(((v0 << 2) | (v1 >> 4)) as u8);
        if pad < 2 {
            out.push((((v1 & 0x0f) << 4) | (v2 >> 2)) as u8);
        }
        if pad == 0 {
            out.push((((v2 & 0x03) << 6) | v3) as u8);
        }
        index += 4;
    }
    Ok(out)
}

/// A serde adapter for Go's `[]byte` fields: base64 strings, not arrays.
pub mod base64_bytes {
    use super::{base64_decode, base64_encode};
    use serde::{Deserialize, Deserializer, Serializer, de::Error as _};

    pub fn serialize<S: Serializer>(value: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&base64_encode(value))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        base64_decode(&text).map_err(D::Error::custom)
    }
}

/// Returned by `GET /v1/status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusResponse {
    #[serde(default)]
    pub protocol_version: i64,
    #[serde(default)]
    pub owner_uid: i64,
}

/// Sent to `POST /v1/proxy-socket/install`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxySocketInstallRequest {
    #[serde(with = "base64_bytes", default)]
    pub plist: Vec<u8>,
    #[serde(default)]
    pub job_path: String,
    #[serde(default)]
    pub target: String,
}

/// Sent to `POST /v1/proxy-socket/stop`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxySocketStopRequest {
    #[serde(default)]
    pub job_path: String,
    #[serde(default)]
    pub target: String,
}

/// Sent to `POST /v1/proxy-socket/kickstart`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxySocketKickstartRequest {
    #[serde(default)]
    pub target: String,
}

/// Sent to `POST /v1/ports/inspect`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortInspectRequest {
    #[serde(default)]
    pub port: i64,
}

/// Returned by `POST /v1/ports/inspect`; `output` is omitted when empty.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortInspectResponse {
    #[serde(with = "base64_bytes", default, skip_serializing_if = "Vec::is_empty")]
    pub output: Vec<u8>,
}

/// Sent to `POST /v1/launchd/control`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchdControlRequest {
    #[serde(default)]
    pub commands: Vec<Vec<String>>,
}

/// Sent to `POST /v1/launchd/save`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchdSaveRequest {
    #[serde(with = "base64_bytes", default)]
    pub plist: Vec<u8>,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub target: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub existing_path: String,
    #[serde(default)]
    pub load: bool,
}

/// Returned by `POST /v1/launchd/save`; HTTP 200 even when loading failed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchdSaveResponse {
    #[serde(default)]
    pub saved: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
}

/// Sent to `POST /v1/launchd/remove`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchdRemoveRequest {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub trash_dir: String,
}

/// Sent to `POST /v1/systemd/control` on Linux.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemdControlRequest {
    #[serde(default)]
    pub commands: Vec<Vec<String>>,
}

/// Sent to `POST /v1/systemd/save` on Linux.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemdSaveRequest {
    #[serde(with = "base64_bytes", default)]
    pub unit_content: Vec<u8>,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub unit_name: String,
    #[serde(default)]
    pub reload: bool,
    #[serde(default)]
    pub enable: bool,
    #[serde(default)]
    pub start: bool,
}

/// Sent to `POST /v1/systemd/remove` on Linux.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemdRemoveRequest {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub unit_name: String,
    #[serde(default)]
    pub trash_dir: String,
}

/// Sent to `POST /v1/ca/install` on Linux.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaInstallRequest {
    #[serde(with = "base64_bytes", default)]
    pub cert_pem: Vec<u8>,
    #[serde(default)]
    pub name: String,
}

/// Sent to `POST /v1/ca/remove` on Linux.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaRemoveRequest {
    #[serde(default)]
    pub name: String,
}

/// Sent to `POST /v1/systemd/proxy-socket/install` on Linux.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinuxProxySocketInstallRequest {
    #[serde(default)]
    pub socket_unit_name: String,
    #[serde(default)]
    pub service_unit_name: String,
    #[serde(with = "base64_bytes", default)]
    pub socket_content: Vec<u8>,
    #[serde(with = "base64_bytes", default)]
    pub service_content: Vec<u8>,
}

/// Sent to `POST /v1/systemd/proxy-socket/stop` on Linux.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinuxProxySocketStopRequest {
    #[serde(default)]
    pub socket_unit_name: String,
    #[serde(default)]
    pub service_unit_name: String,
}

/// The `{"ok":true}` success body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OkResponse {
    pub ok: bool,
}

impl OkResponse {
    pub const fn success() -> Self {
        OkResponse { ok: true }
    }
}

/// The `{"error":"..."}` failure body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
}

impl ErrorResponse {
    pub fn new(message: impl Into<String>) -> Self {
        ErrorResponse {
            error: message.into(),
        }
    }
}

/// Serializes a response the way the Go helper's `json.Marshal` does: compact,
/// no trailing newline, and HTML-escaped (`<`, `>`, `&`, U+2028, U+2029).
pub fn encode_server_json<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    let data = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    Ok(escape_html(&data))
}

/// Serializes a request the way Go's `json.Marshal` does.
pub fn encode_request_json<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    encode_server_json(value)
}

fn escape_html(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 8);
    let mut index = 0;
    while index < data.len() {
        match data[index] {
            b'<' => {
                out.extend_from_slice(b"\\u003c");
                index += 1;
            }
            b'>' => {
                out.extend_from_slice(b"\\u003e");
                index += 1;
            }
            b'&' => {
                out.extend_from_slice(b"\\u0026");
                index += 1;
            }
            0xE2 if data[index..].starts_with(&[0xE2, 0x80, 0xA8]) => {
                out.extend_from_slice(b"\\u2028");
                index += 3;
            }
            0xE2 if data[index..].starts_with(&[0xE2, 0x80, 0xA9]) => {
                out.extend_from_slice(b"\\u2029");
                index += 3;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    out
}

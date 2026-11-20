//! The tunnel wire protocol: 9-byte binary frames, JSON payloads, and the
//! HMAC-SHA256 handshake.
//!
//! Ported 1:1 from `tunnel_proto.go`. One WebSocket carries a multiplexed
//! stream of HTTP and WebSocket traffic; frame layout and payload names are
//! frozen by the vendored Worker.

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;

pub const PROTO_VERSION: u8 = 1;
pub const HEADER_SIZE: usize = 9;
pub const MAX_CHUNK: usize = 256 * 1024;

pub const FLAG_FIN: u8 = 0x01;
pub const FLAG_TEXT: u8 = 0x02;

pub const FRAME_REGISTER: u8 = 0x01;
pub const FRAME_REGISTER_ACK: u8 = 0x02;

pub const FRAME_HTTP_REQ_HEAD: u8 = 0x10;
pub const FRAME_HTTP_REQ_DATA: u8 = 0x11;
pub const FRAME_HTTP_RES_HEAD: u8 = 0x12;
pub const FRAME_HTTP_RES_DATA: u8 = 0x13;
pub const FRAME_STREAM_RESET: u8 = 0x14;

pub const FRAME_WS_OPEN: u8 = 0x20;
pub const FRAME_WS_OPEN_ACK: u8 = 0x21;
pub const FRAME_WS_DATA: u8 = 0x22;
pub const FRAME_WS_CLOSE: u8 = 0x23;

/// A decoded tunnel frame. Payload borrows from the input on decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame<'a> {
    pub frame_type: u8,
    pub flags: u8,
    pub stream_id: u32,
    pub code: u16,
    pub payload: &'a [u8],
}

/// Encode a frame header plus payload.
pub fn encode_frame(
    frame_type: u8,
    flags: u8,
    stream_id: u32,
    code: u16,
    payload: &[u8],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_SIZE + payload.len());
    out.push(PROTO_VERSION);
    out.push(frame_type);
    out.push(flags);
    out.extend_from_slice(&stream_id.to_be_bytes());
    out.extend_from_slice(&code.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// Decode a frame, rejecting truncated input and unsupported versions.
pub fn decode_frame(raw: &[u8]) -> Result<Frame<'_>, TunnelProtoError> {
    if raw.len() < HEADER_SIZE {
        return Err(TunnelProtoError::Truncated);
    }
    if raw[0] != PROTO_VERSION {
        return Err(TunnelProtoError::UnsupportedVersion(raw[0]));
    }
    Ok(Frame {
        frame_type: raw[1],
        flags: raw[2],
        stream_id: u32::from_be_bytes([raw[3], raw[4], raw[5], raw[6]]),
        code: u16::from_be_bytes([raw[7], raw[8]]),
        payload: &raw[HEADER_SIZE..],
    })
}

/// Frame encoding errors.
#[derive(Debug, thiserror::Error)]
pub enum TunnelProtoError {
    #[error("truncated tunnel frame")]
    Truncated,
    #[error("unsupported tunnel frame version {0}")]
    UnsupportedVersion(u8),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// The registration payload sent after the WebSocket opens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegisterPayload {
    pub client_id: String,
}

/// The gateway's answer to registration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegisterAckPayload {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub conflict_owner: String,
}

/// An inbound HTTP request head.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpRequestHead {
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub headers: Vec<[String; 2]>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub remote_addr: String,
}

/// An HTTP response head.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpResponseHead {
    pub status: u16,
    #[serde(default)]
    pub headers: Vec<[String; 2]>,
}

/// A request to open a local WebSocket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebSocketOpenHead {
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub protocols: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<[String; 2]>,
}

/// The answer to a WebSocket open: either a negotiated protocol or an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebSocketOpenAck {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub protocol: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
}

/// Encode a JSON payload as a frame.
pub fn encode_json_frame<T: Serialize>(
    frame_type: u8,
    flags: u8,
    stream_id: u32,
    code: u16,
    payload: &T,
) -> Result<Vec<u8>, TunnelProtoError> {
    let json = serde_json::to_vec(payload)?;
    Ok(encode_frame(frame_type, flags, stream_id, code, &json))
}

/// Sign `"v1\n<clientID>\n<timestamp>\n<nonce>"` with the Worker secret.
pub fn sign_handshake(secret: &str, client_id: &str, timestamp: &str, nonce: &str) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts keys of any length");
    mac.update(format!("v1\n{client_id}\n{timestamp}\n{nonce}").as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Verify a handshake signature in constant time. Malformed hex is a reject.
pub fn verify_handshake(
    secret: &str,
    client_id: &str,
    timestamp: &str,
    nonce: &str,
    signature: &str,
) -> bool {
    let expected = sign_handshake(secret, client_id, timestamp, nonce);
    let (Ok(expected), Ok(provided)) = (hex::decode(&expected), hex::decode(signature)) else {
        return false;
    };
    expected.ct_eq(&provided).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_header_is_nine_network_order_bytes() {
        let encoded = encode_frame(FRAME_HTTP_REQ_HEAD, FLAG_FIN, 0x01020304, 0x0a0b, b"{}");
        assert_eq!(&encoded[..9], &[1, 0x10, 0x01, 1, 2, 3, 4, 0x0a, 0x0b]);
        assert_eq!(&encoded[9..], b"{}");
        let decoded = decode_frame(&encoded).unwrap();
        assert_eq!(decoded.frame_type, FRAME_HTTP_REQ_HEAD);
        assert_eq!(decoded.flags, FLAG_FIN);
        assert_eq!(decoded.stream_id, 0x01020304);
        assert_eq!(decoded.code, 0x0a0b);
        assert_eq!(decoded.payload, b"{}");
    }

    #[test]
    fn all_declared_frame_types_round_trip() {
        for frame_type in [
            FRAME_REGISTER,
            FRAME_REGISTER_ACK,
            FRAME_HTTP_REQ_HEAD,
            FRAME_HTTP_REQ_DATA,
            FRAME_HTTP_RES_HEAD,
            FRAME_HTTP_RES_DATA,
            FRAME_STREAM_RESET,
            FRAME_WS_OPEN,
            FRAME_WS_OPEN_ACK,
            FRAME_WS_DATA,
            FRAME_WS_CLOSE,
        ] {
            let raw = encode_frame(frame_type, FLAG_TEXT, 7, 1000, b"payload");
            let frame = decode_frame(&raw).unwrap();
            assert_eq!(frame.frame_type, frame_type);
            assert_eq!(frame.payload, b"payload");
        }
    }

    #[test]
    fn decode_rejects_truncated_and_future_versions() {
        assert!(matches!(
            decode_frame(&[1, 2]).unwrap_err(),
            TunnelProtoError::Truncated
        ));
        let mut raw = encode_frame(FRAME_REGISTER, 0, 0, 0, b"");
        raw[0] = 9;
        assert!(matches!(
            decode_frame(&raw).unwrap_err(),
            TunnelProtoError::UnsupportedVersion(9)
        ));
    }

    #[test]
    fn json_payloads_use_the_frozen_names() {
        let head = HttpRequestHead {
            method: "GET".into(),
            host: "app.example.com".into(),
            path: "/a?b=1".into(),
            headers: vec![["Accept".into(), "*/*".into()]],
            remote_addr: "203.0.113.4".into(),
        };
        let raw = encode_json_frame(FRAME_HTTP_REQ_HEAD, FLAG_FIN, 3, 0, &head).unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(decode_frame(&raw).unwrap().payload).unwrap();
        assert_eq!(value["method"], "GET");
        assert_eq!(value["host"], "app.example.com");
        assert_eq!(value["path"], "/a?b=1");
        assert_eq!(value["headers"][0][0], "Accept");
        assert_eq!(value["remoteAddr"], "203.0.113.4");

        let ack = RegisterAckPayload {
            ok: false,
            conflict_owner: "other".into(),
        };
        let raw = encode_json_frame(FRAME_REGISTER_ACK, 0, 0, 0, &ack).unwrap();
        let decoded: RegisterAckPayload =
            serde_json::from_slice(decode_frame(&raw).unwrap().payload).unwrap();
        assert_eq!(decoded, ack);
        let value: serde_json::Value =
            serde_json::from_slice(decode_frame(&raw).unwrap().payload).unwrap();
        assert_eq!(value["conflictOwner"], "other");

        let ack = WebSocketOpenAck::default();
        let json = serde_json::to_value(&ack).unwrap();
        assert_eq!(json, serde_json::json!({}));
    }

    #[test]
    fn register_payload_rejects_unknown_fields() {
        assert!(serde_json::from_str::<RegisterPayload>(r#"{"clientId":"a","extra":1}"#).is_err());
    }

    #[test]
    fn handshake_matches_the_worker_mac() {
        assert_eq!(
            sign_handshake(
                "0123456789abcdef",
                "mac-12345678",
                "1700000000",
                "abcdef0123456789"
            ),
            "ceb8b2c00b3aea2bfe8119a0c8f4183d6641c8bc24d49ac361ad2f944b51e09c"
        );
    }

    #[test]
    fn handshake_accepts_only_the_exact_signature() {
        let secret = "0123456789abcdef";
        let signature = sign_handshake(secret, "mac-1", "1", "n");
        assert!(verify_handshake(secret, "mac-1", "1", "n", &signature));
        assert!(!verify_handshake(secret, "mac-2", "1", "n", &signature));
        assert!(!verify_handshake(secret, "mac-1", "2", "n", &signature));
        assert!(!verify_handshake(secret, "mac-1", "1", "n", "zz"));
        assert!(!verify_handshake("other", "mac-1", "1", "n", &signature));
    }
}

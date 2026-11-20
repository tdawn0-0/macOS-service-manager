//! A small RFC 6455 WebSocket client with the exact framing used by the Go
//! tunnel client (`ws.go`).
//!
//! The client is deliberately hand-written: manual HTTP/1.1 upgrade, masking,
//! 4 MiB message cap, fragment reassembly, automatic Pong, and close echo. It
//! is used for the outbound Worker connection and for local upstream
//! WebSockets inside the tunnel session.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use rustls::pki_types::ServerName;
use sha1::{Digest, Sha1};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf, split};
use tokio::net::TcpStream;
use tokio::sync::Mutex;

pub const OP_CONTINUATION: u8 = 0x0;
pub const OP_TEXT: u8 = 0x1;
pub const OP_BINARY: u8 = 0x2;
pub const OP_CLOSE: u8 = 0x8;
pub const OP_PING: u8 = 0x9;
pub const OP_PONG: u8 = 0xA;

const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const MAX_MESSAGE_SIZE: usize = 4 * 1024 * 1024;
const HANDSHAKE_LIMIT: usize = 64 * 1024;
const DIAL_TIMEOUT: Duration = Duration::from_secs(15);

/// Errors from a WebSocket connection.
#[derive(Debug, thiserror::Error)]
pub enum WsError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Tls(#[from] rustls::Error),
    #[error(transparent)]
    Handshake(#[from] WsHandshakeError),
    #[error(transparent)]
    Close(#[from] WsCloseError),
    #[error("{0}")]
    Protocol(String),
    #[error("{0}")]
    InvalidUrl(String),
}

/// A rejected upgrade, carrying the status and up to 256 body bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WsHandshakeError {
    pub status_code: u16,
    pub status: String,
    pub body: String,
}

impl std::fmt::Display for WsHandshakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.body.is_empty() {
            write!(
                f,
                "websocket handshake failed: HTTP {} {}",
                self.status_code, self.status
            )
        } else {
            write!(
                f,
                "websocket handshake failed: HTTP {} {}: {}",
                self.status_code, self.status, self.body
            )
        }
    }
}

impl std::error::Error for WsHandshakeError {}

/// A close frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WsCloseError {
    pub code: u16,
    pub reason: String,
}

impl std::fmt::Display for WsCloseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.reason.is_empty() {
            write!(f, "websocket closed ({})", self.code)
        } else {
            write!(f, "websocket closed ({}): {}", self.code, self.reason)
        }
    }
}

impl std::error::Error for WsCloseError {}

/// The response head of a successful upgrade.
#[derive(Debug, Clone, Default)]
pub struct HandshakeResponse {
    pub status_code: u16,
    pub status: String,
    pub headers: Vec<(String, String)>,
}

impl HandshakeResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// The readable/writable stream a WebSocket runs over.
pub trait IoStream: AsyncRead + AsyncWrite + Unpin + Send + Sync {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + Sync> IoStream for T {}
pub type BoxStream = Box<dyn IoStream>;

/// A WebSocket client connection. Reads and writes use separate halves so a
/// keepalive task can ping while the read loop waits.
pub struct WsConn<S> {
    reader: ReadHalf<S>,
    writer: Arc<Mutex<WriteHalf<S>>>,
    is_client: bool,
}

impl<S: AsyncRead + AsyncWrite + Unpin> WsConn<S> {
    /// A sender clone for control frames.
    pub fn sender(&self) -> WsSender<S> {
        WsSender {
            writer: Arc::clone(&self.writer),
            is_client: self.is_client,
        }
    }

    /// Read one complete text or binary message, answering Ping and Close.
    pub async fn read_message(&mut self) -> Result<(u8, Vec<u8>), WsError> {
        let mut message_opcode = 0;
        let mut buffer = Vec::new();
        let mut in_fragment = false;
        loop {
            let (fin, opcode, payload) = read_frame(&mut self.reader, MAX_MESSAGE_SIZE).await?;
            match opcode {
                OP_PING => {
                    self.sender().write_frame(OP_PONG, &payload).await?;
                }
                OP_PONG => {}
                OP_CLOSE => {
                    let (code, reason) = parse_close(&payload);
                    let _ = self.sender().write_close(code, "").await;
                    return Err(WsError::Close(WsCloseError { code, reason }));
                }
                OP_TEXT | OP_BINARY => {
                    if in_fragment {
                        return Err(WsError::Protocol(
                            "unexpected new websocket frame inside fragmented message".into(),
                        ));
                    }
                    if fin {
                        return Ok((opcode, payload));
                    }
                    message_opcode = opcode;
                    in_fragment = true;
                    buffer = payload;
                }
                OP_CONTINUATION => {
                    if !in_fragment {
                        return Err(WsError::Protocol(
                            "unexpected websocket continuation frame".into(),
                        ));
                    }
                    if buffer.len() + payload.len() > MAX_MESSAGE_SIZE {
                        return Err(WsError::Protocol(
                            "fragmented websocket message exceeds maximum size".into(),
                        ));
                    }
                    buffer.extend_from_slice(&payload);
                    if fin {
                        return Ok((message_opcode, buffer));
                    }
                }
                other => {
                    return Err(WsError::Protocol(format!(
                        "unsupported websocket opcode {other}"
                    )));
                }
            }
        }
    }

    pub async fn write_message(&self, opcode: u8, data: &[u8]) -> Result<(), WsError> {
        self.sender().write_message(opcode, data).await
    }

    pub async fn write_binary(&self, data: &[u8]) -> Result<(), WsError> {
        self.write_message(OP_BINARY, data).await
    }

    pub async fn write_ping(&self, data: &[u8]) -> Result<(), WsError> {
        self.sender().write_ping(data).await
    }

    pub async fn write_close(&self, code: u16, reason: &str) -> Result<(), WsError> {
        self.sender().write_close(code, reason).await
    }
}

/// The write half, safe to clone into a ping task.
#[derive(Clone)]
pub struct WsSender<S> {
    writer: Arc<Mutex<WriteHalf<S>>>,
    is_client: bool,
}

impl<S: AsyncWrite + Unpin> WsSender<S> {
    pub async fn write_message(&self, opcode: u8, data: &[u8]) -> Result<(), WsError> {
        self.write_frame(opcode, data).await
    }

    pub async fn write_binary(&self, data: &[u8]) -> Result<(), WsError> {
        self.write_message(OP_BINARY, data).await
    }

    pub async fn write_ping(&self, data: &[u8]) -> Result<(), WsError> {
        let truncated = &data[..data.len().min(125)];
        self.write_frame(OP_PING, truncated).await
    }

    pub async fn write_close(&self, code: u16, reason: &str) -> Result<(), WsError> {
        if code == 0 {
            return self.write_frame(OP_CLOSE, &[]).await;
        }
        let mut payload = Vec::with_capacity(2 + reason.len());
        payload.extend_from_slice(&code.to_be_bytes());
        let reason = &reason.as_bytes()[..reason.len().min(123)];
        payload.extend_from_slice(reason);
        self.write_frame(OP_CLOSE, &payload).await
    }

    async fn write_frame(&self, opcode: u8, data: &[u8]) -> Result<(), WsError> {
        let mut writer = self.writer.lock().await;
        let mut header = [0u8; 14];
        header[0] = 0x80 | (opcode & 0x0F);
        let length = data.len();
        let mask_bit = if self.is_client { 0x80 } else { 0 };
        let header_len = match length {
            0..=125 => {
                header[1] = mask_bit | length as u8;
                2
            }
            126..=65535 => {
                header[1] = mask_bit | 126;
                header[2..4].copy_from_slice(&(length as u16).to_be_bytes());
                4
            }
            _ => {
                header[1] = mask_bit | 127;
                header[2..10].copy_from_slice(&(length as u64).to_be_bytes());
                10
            }
        };
        let mut frame;
        if self.is_client {
            let mask: [u8; 4] = rand::random();
            header[header_len..header_len + 4].copy_from_slice(&mask);
            let total = header_len + 4;
            frame = Vec::with_capacity(total + length);
            frame.extend_from_slice(&header[..total]);
            frame.extend(
                data.iter()
                    .enumerate()
                    .map(|(index, byte)| byte ^ mask[index % 4]),
            );
        } else {
            frame = Vec::with_capacity(header_len + length);
            frame.extend_from_slice(&header[..header_len]);
            frame.extend_from_slice(data);
        }
        writer.write_all(&frame).await?;
        writer.flush().await?;
        Ok(())
    }
}

/// Compute the expected `Sec-WebSocket-Accept` value.
pub fn accept_key(key: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(WS_GUID.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(hasher.finalize())
}

/// Connect over TCP/TLS and perform the upgrade.
pub async fn dial(
    raw_url: &str,
    headers: &[(String, String)],
    protocols: &[String],
    tls_config: Option<Arc<rustls::ClientConfig>>,
) -> Result<(WsConn<BoxStream>, HandshakeResponse), WsError> {
    crate::ensure_crypto_provider();
    let url = url::Url::parse(raw_url).map_err(|error| WsError::InvalidUrl(error.to_string()))?;
    let scheme = url.scheme().to_lowercase();
    let host = url
        .host_str()
        .ok_or_else(|| WsError::InvalidUrl("websocket URL has no host".into()))?
        .to_string();
    let port = url
        .port_or_known_default()
        .ok_or_else(|| WsError::InvalidUrl("websocket URL has no port".into()))?;
    let address = format!("{host}:{port}");
    let stream: BoxStream = match scheme.as_str() {
        "ws" => Box::new(connect_tcp(&address).await?),
        "wss" => {
            let config = match tls_config {
                Some(config) => config,
                None => {
                    let mut roots = rustls::RootCertStore::empty();
                    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
                    let mut config = rustls::ClientConfig::builder()
                        .with_root_certificates(roots)
                        .with_no_client_auth();
                    config.alpn_protocols = vec![b"http/1.1".to_vec()];
                    Arc::new(config)
                }
            };
            let connector = tokio_rustls::TlsConnector::from(config);
            let server_name = ServerName::try_from(host.clone())
                .map_err(|_| WsError::InvalidUrl("invalid websocket server name".into()))?;
            let tcp = connect_tcp(&address).await?;
            Box::new(connector.connect(server_name, tcp).await?)
        }
        other => {
            return Err(WsError::InvalidUrl(format!(
                "unsupported websocket scheme {other:?}"
            )));
        }
    };
    client_handshake(stream, &url, &host, port, headers, protocols).await
}

/// Upgrade an already-connected stream. Used for local `ws://` upstreams.
pub async fn client_handshake<S: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static>(
    stream: S,
    url: &url::Url,
    host: &str,
    port: u16,
    headers: &[(String, String)],
    protocols: &[String],
) -> Result<(WsConn<BoxStream>, HandshakeResponse), WsError> {
    let key_bytes: [u8; 16] = rand::random();
    let key = base64::engine::general_purpose::STANDARD.encode(key_bytes);
    let default_port =
        (url.scheme() == "wss" && port == 443) || (url.scheme() == "ws" && port == 80);
    let host_header = if host.contains(':') || default_port {
        host.to_string()
    } else {
        format!("{host}:{port}")
    };
    let mut path = url.path().to_string();
    if path.is_empty() {
        path = "/".into();
    }
    if let Some(query) = url.query() {
        path.push('?');
        path.push_str(query);
    }
    let override_host = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("host"))
        .map(|(_, value)| value.clone());
    let mut request = String::new();
    request.push_str(&format!("GET {path} HTTP/1.1\r\n"));
    request.push_str(&format!(
        "Host: {}\r\n",
        override_host.as_deref().unwrap_or(&host_header)
    ));
    request.push_str("Upgrade: websocket\r\nConnection: Upgrade\r\n");
    request.push_str(&format!("Sec-WebSocket-Key: {key}\r\n"));
    request.push_str("Sec-WebSocket-Version: 13\r\n");
    if !protocols.is_empty() {
        request.push_str(&format!(
            "Sec-WebSocket-Protocol: {}\r\n",
            protocols.join(", ")
        ));
    }
    for (name, value) in headers {
        let lower = name.to_lowercase();
        if matches!(
            lower.as_str(),
            "host"
                | "upgrade"
                | "connection"
                | "sec-websocket-key"
                | "sec-websocket-version"
                | "sec-websocket-accept"
                | "sec-websocket-protocol"
        ) {
            continue;
        }
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    if !headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("user-agent"))
    {
        request.push_str("User-Agent: Devbase-Tunnel/1.0\r\n");
    }
    request.push_str("\r\n");

    let mut stream = stream;
    let handshake = async {
        stream.write_all(request.as_bytes()).await?;
        stream.flush().await?;
        read_response(&mut stream).await
    };
    let (response, accepted) = tokio::time::timeout(Duration::from_secs(15), handshake)
        .await
        .map_err(|_| WsError::Protocol("websocket handshake timed out".into()))??;

    if response.status_code != 101 {
        let body = response_body(&response.raw_rest);
        return Err(WsError::Handshake(WsHandshakeError {
            status_code: response.status_code,
            status: response.status.clone(),
            body,
        }));
    }
    let upgrade = response.header("Upgrade").unwrap_or_default();
    let connection = response.header("Connection").unwrap_or_default();
    if !upgrade.eq_ignore_ascii_case("websocket") || !header_has_token(connection, "upgrade") {
        return Err(WsError::Protocol(
            "websocket server did not complete upgrade".into(),
        ));
    }
    let expected_accept = accept_key(&key);
    if accepted.as_deref() != Some(expected_accept.as_str()) {
        return Err(WsError::Protocol(
            "invalid Sec-WebSocket-Accept header".into(),
        ));
    }
    let boxed: BoxStream = Box::new(stream);
    let (reader, writer) = split(boxed);
    Ok((
        WsConn {
            reader,
            writer: Arc::new(Mutex::new(writer)),
            is_client: true,
        },
        response.into_public(),
    ))
}

async fn connect_tcp(address: &str) -> Result<TcpStream, WsError> {
    match tokio::time::timeout(DIAL_TIMEOUT, TcpStream::connect(address)).await {
        Ok(Ok(stream)) => {
            let _ = stream.set_nodelay(true);
            Ok(stream)
        }
        Ok(Err(error)) => Err(error.into()),
        Err(_) => Err(WsError::Protocol("websocket dial timed out".into())),
    }
}

struct ResponseHead {
    status_code: u16,
    status: String,
    headers: Vec<(String, String)>,
    raw_rest: Vec<u8>,
}

impl ResponseHead {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    fn into_public(self) -> HandshakeResponse {
        HandshakeResponse {
            status_code: self.status_code,
            status: self.status,
            headers: self.headers,
        }
    }
}

async fn read_response<S: AsyncRead + Unpin>(
    stream: &mut S,
) -> Result<(ResponseHead, Option<String>), WsError> {
    // Read one byte at a time so a frame the server pipelined right after the
    // 101 stays in the socket for `read_message`.
    let mut buffer = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let read = stream.read(&mut byte).await?;
        if read == 0 {
            return Err(WsError::Protocol(
                "websocket handshake ended before response headers".into(),
            ));
        }
        buffer.push(byte[0]);
        if buffer.len() > HANDSHAKE_LIMIT {
            return Err(WsError::Protocol(
                "websocket handshake response is too large".into(),
            ));
        }
        if buffer.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buffer).to_string();
    let mut rest = Vec::new();
    if !head.starts_with("HTTP/1.1 101") && !head.starts_with("HTTP/1.0 101") {
        let mut chunk = [0u8; 256];
        if let Ok(Ok(read)) =
            tokio::time::timeout(Duration::from_millis(100), stream.read(&mut chunk)).await
        {
            rest.extend_from_slice(&chunk[..read]);
        }
    }
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let mut parts = status_line.splitn(3, ' ');
    let _version = parts.next();
    let status_code = parts
        .next()
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| WsError::Protocol("invalid websocket status line".into()))?;
    let status = parts.next().unwrap_or_default().to_string();
    let mut headers = BTreeMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_lowercase(), value.trim().to_string());
        }
    }
    let accepted = headers.get("sec-websocket-accept").cloned();
    let headers = headers.into_iter().collect();
    Ok((
        ResponseHead {
            status_code,
            status,
            headers,
            raw_rest: rest,
        },
        accepted,
    ))
}

fn response_body(rest: &[u8]) -> String {
    let capped = &rest[..rest.len().min(256)];
    String::from_utf8_lossy(capped).trim().to_string()
}

fn header_has_token(header: &str, token: &str) -> bool {
    header
        .split(',')
        .any(|part| part.trim().eq_ignore_ascii_case(token))
}

fn parse_close(payload: &[u8]) -> (u16, String) {
    if payload.len() >= 2 {
        (
            u16::from_be_bytes([payload[0], payload[1]]),
            String::from_utf8_lossy(&payload[2..]).into_owned(),
        )
    } else {
        (1005, String::new())
    }
}

async fn read_frame<S>(
    reader: &mut ReadHalf<S>,
    max_size: usize,
) -> Result<(bool, u8, Vec<u8>), WsError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut header = [0u8; 2];
    reader.read_exact(&mut header).await?;
    let fin = header[0] & 0x80 != 0;
    let opcode = header[0] & 0x0F;
    let masked = header[1] & 0x80 != 0;
    let mut length = u64::from(header[1] & 0x7F);
    match length {
        126 => {
            let mut extended = [0u8; 2];
            reader.read_exact(&mut extended).await?;
            length = u64::from(u16::from_be_bytes(extended));
        }
        127 => {
            let mut extended = [0u8; 8];
            reader.read_exact(&mut extended).await?;
            length = u64::from_be_bytes(extended);
        }
        _ => {}
    }
    if length > max_size as u64 {
        return Err(WsError::Protocol(
            "websocket frame exceeds maximum size".into(),
        ));
    }
    let mut mask = [0u8; 4];
    if masked {
        reader.read_exact(&mut mask).await?;
    }
    let mut payload = vec![0u8; length as usize];
    if !payload.is_empty() {
        reader.read_exact(&mut payload).await?;
        if masked {
            for (index, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[index % 4];
            }
        }
    }
    Ok((fin, opcode, payload))
}

#[cfg(test)]
mod tests {
    use tokio::io::DuplexStream;

    use super::*;

    async fn handshake_server(stream: &mut DuplexStream) -> String {
        let mut request = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            stream.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
            if request.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        let text = String::from_utf8(request).unwrap();
        let key = text
            .lines()
            .find_map(|line| line.strip_prefix("Sec-WebSocket-Key: "))
            .unwrap()
            .trim()
            .to_string();
        let accept = accept_key(&key);
        stream
            .write_all(
                format!(
                    "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        text
    }

    async fn write_server_frame(stream: &mut DuplexStream, opcode: u8, fin: bool, data: &[u8]) {
        let mut frame = Vec::new();
        frame.push(if fin { 0x80 | opcode } else { opcode });
        assert!(data.len() <= 125);
        frame.push(data.len() as u8);
        frame.extend_from_slice(data);
        stream.write_all(&frame).await.unwrap();
    }

    async fn read_client_frame(stream: &mut DuplexStream) -> (u8, Vec<u8>) {
        let mut header = [0u8; 2];
        stream.read_exact(&mut header).await.unwrap();
        let opcode = header[0] & 0x0F;
        assert_ne!(header[1] & 0x80, 0, "client frames are masked");
        let length = (header[1] & 0x7F) as usize;
        let mut mask = [0u8; 4];
        stream.read_exact(&mut mask).await.unwrap();
        let mut payload = vec![0u8; length];
        stream.read_exact(&mut payload).await.unwrap();
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[index % 4];
        }
        (opcode, payload)
    }

    #[tokio::test]
    async fn handshake_messages_ping_fragments_and_close() {
        let (client, mut server) = tokio::io::duplex(64 * 1024);
        let server_task = tokio::spawn(async move {
            let request = handshake_server(&mut server).await;
            write_server_frame(&mut server, OP_TEXT, true, b"hello").await;
            let binary = read_client_frame(&mut server).await;
            write_server_frame(&mut server, OP_PING, true, b"p").await;
            let pong = read_client_frame(&mut server).await;
            write_server_frame(&mut server, OP_BINARY, false, b"frag").await;
            write_server_frame(&mut server, OP_CONTINUATION, true, b"ment").await;
            let mut close = vec![0x03, 0xe8];
            close.extend_from_slice(b"bye");
            write_server_frame(&mut server, OP_CLOSE, true, &close).await;
            (request, binary, pong)
        });

        let url = url::Url::parse("ws://example.test/socket?x=1").unwrap();
        let (mut ws, response) = client_handshake(
            client,
            &url,
            "example.test",
            80,
            &[("X-Test".into(), "1".into())],
            &["chat".into()],
        )
        .await
        .unwrap();
        assert_eq!(response.status_code, 101);
        assert_eq!(response.header("upgrade"), Some("websocket"));

        let (opcode, message) = ws.read_message().await.unwrap();
        assert_eq!((opcode, message.as_slice()), (OP_TEXT, b"hello".as_slice()));
        ws.write_binary(b"world").await.unwrap();
        let (opcode, message) = ws.read_message().await.unwrap();
        assert_eq!(
            (opcode, message.as_slice()),
            (OP_BINARY, b"fragment".as_slice())
        );
        let error = ws.read_message().await.unwrap_err();
        match error {
            WsError::Close(close) => {
                assert_eq!(close.code, 1000);
                assert_eq!(close.reason, "bye");
            }
            other => panic!("expected close, got {other:?}"),
        }

        let (request, binary, pong) = server_task.await.unwrap();
        assert!(request.contains("GET /socket?x=1 HTTP/1.1"));
        assert!(request.contains("Host: example.test"));
        assert!(request.contains("Sec-WebSocket-Protocol: chat"));
        assert!(request.contains("User-Agent: Devbase-Tunnel/1.0"));
        assert!(request.contains("X-Test: 1"));
        assert_eq!(binary, (OP_BINARY, b"world".to_vec()));
        assert_eq!(pong, (OP_PONG, b"p".to_vec()));
    }

    #[tokio::test]
    async fn handshake_rejection_carries_status_and_body() {
        let (client, mut server) = tokio::io::duplex(8 * 1024);
        let task = tokio::spawn(async move {
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            loop {
                server.read_exact(&mut byte).await.unwrap();
                request.push(byte[0]);
                if request.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            server
                .write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 3\r\n\r\nno!\r\nnothing")
                .await
                .unwrap();
        });
        let url = url::Url::parse("ws://example.test/").unwrap();
        let error = match client_handshake(client, &url, "example.test", 80, &[], &[]).await {
            Ok(_) => panic!("handshake unexpectedly succeeded"),
            Err(error) => error,
        };
        match error {
            WsError::Handshake(handshake) => {
                assert_eq!(handshake.status_code, 401);
                assert!(handshake.body.starts_with("no!"));
                assert!(handshake.to_string().contains("HTTP 401 Unauthorized"));
            }
            other => panic!("expected handshake error, got {other:?}"),
        }
        task.await.unwrap();
    }

    #[tokio::test]
    async fn invalid_accept_key_is_rejected() {
        let (client, mut server) = tokio::io::duplex(8 * 1024);
        let task = tokio::spawn(async move {
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            loop {
                server.read_exact(&mut byte).await.unwrap();
                request.push(byte[0]);
                if request.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            server
                .write_all(
                    b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: wrong\r\n\r\n",
                )
                .await
                .unwrap();
        });
        let url = url::Url::parse("ws://example.test/").unwrap();
        let error = match client_handshake(client, &url, "example.test", 80, &[], &[]).await {
            Ok(_) => panic!("handshake unexpectedly succeeded"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("Sec-WebSocket-Accept"));
        task.await.unwrap();
    }

    #[test]
    fn accept_key_matches_rfc_example() {
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }
}

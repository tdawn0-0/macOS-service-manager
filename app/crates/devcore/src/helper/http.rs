//! The minimal HTTP/1.1 wire layer shared by the helper server and client.
//!
//! The endpoints are narrow and always JSON over a Unix socket, so this is a
//! hand-written parser/writer rather than a server framework. It reproduces
//! the parts of Go's `net/http` the helper contract depends on: exact
//! method/path routing, `Content-Length` bodies, a 4 MiB body cap, and plain
//! `404`/`405` responses.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

/// The header section may not exceed this before the request is rejected.
pub const MAX_HEADER_BYTES: usize = 64 * 1024;

/// A parsed request. The path excludes the query string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn new(method: &str, path: &str) -> Self {
        Request {
            method: method.to_string(),
            path: path.to_string(),
            headers: HashMap::new(),
            body: Vec::new(),
        }
    }

    pub fn with_body(method: &str, path: &str, body: Vec<u8>) -> Self {
        let mut request = Request::new(method, path);
        request
            .headers
            .insert("content-type".to_string(), "application/json".to_string());
        request.body = body;
        request
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }
}

/// A response to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(status: u16, content_type: &str, body: Vec<u8>) -> Self {
        Response {
            status,
            headers: vec![("Content-Type".to_string(), content_type.to_string())],
            body,
        }
    }

    pub fn json(status: u16, body: Vec<u8>) -> Self {
        Response::new(status, "application/json", body)
    }

    pub fn text(status: u16, body: &str) -> Self {
        let mut response = Response::new(
            status,
            "text/plain; charset=utf-8",
            body.as_bytes().to_vec(),
        );
        response
            .headers
            .push(("X-Content-Type-Options".to_string(), "nosniff".to_string()));
        response
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }
}

/// Why reading a request stopped.
#[derive(Debug)]
pub enum ReadError {
    /// The peer closed or timed out; no response should be attempted.
    Io(std::io::Error),
    /// A malformed request line or headers.
    BadRequest(String),
    /// The body exceeded [`MAX_REQUEST_BYTES`].
    TooLarge,
}

impl From<std::io::Error> for ReadError {
    fn from(error: std::io::Error) -> Self {
        ReadError::Io(error)
    }
}

/// The HTTP status reason phrases the helper uses.
pub fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        _ => "Unknown",
    }
}

/// Reads one request. `limit` caps the body in bytes.
pub fn read_request(stream: &mut UnixStream, limit: usize) -> Result<Request, ReadError> {
    let mut buffer = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    while !buffer.ends_with(b"\r\n\r\n") {
        if buffer.len() >= MAX_HEADER_BYTES {
            return Err(ReadError::BadRequest(
                "header section too large".to_string(),
            ));
        }
        match stream.read(&mut byte) {
            Ok(0) => {
                return Err(ReadError::Io(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "connection closed before headers",
                )));
            }
            Ok(_) => buffer.push(byte[0]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(ReadError::Io(error)),
        }
    }
    let header_text = String::from_utf8_lossy(&buffer).into_owned();
    let mut lines = header_text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| ReadError::BadRequest("missing request line".to_string()))?;
    let mut parts = request_line.split(' ');
    let method = parts
        .next()
        .filter(|method| !method.is_empty())
        .ok_or_else(|| ReadError::BadRequest("missing method".to_string()))?;
    let target = parts
        .next()
        .filter(|target| !target.is_empty())
        .ok_or_else(|| ReadError::BadRequest("missing request target".to_string()))?;
    let version = parts
        .next()
        .ok_or_else(|| ReadError::BadRequest("missing HTTP version".to_string()))?;
    if parts.next().is_some() || !version.starts_with("HTTP/") {
        return Err(ReadError::BadRequest("malformed request line".to_string()));
    }
    let path = target.split('?').next().unwrap_or(target).to_string();
    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(ReadError::BadRequest("malformed header".to_string()));
        };
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }
    if headers
        .get("transfer-encoding")
        .is_some_and(|value| !value.eq_ignore_ascii_case("identity"))
    {
        return Err(ReadError::BadRequest(
            "unsupported transfer encoding".to_string(),
        ));
    }
    let length = match headers.get("content-length") {
        None => 0,
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| ReadError::BadRequest("invalid Content-Length".to_string()))?,
    };
    if length > limit {
        return Err(ReadError::TooLarge);
    }
    let mut body = vec![0u8; length];
    if length > 0 {
        stream.read_exact(&mut body)?;
    }
    Ok(Request {
        method: method.to_string(),
        path,
        headers,
        body,
    })
}

/// Writes one response and closes the connection.
pub fn write_response(stream: &mut UnixStream, response: &Response) -> std::io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        response.status,
        reason_phrase(response.status),
        response.body.len()
    );
    for (name, value) in &response.headers {
        head.push_str(name);
        head.push_str(": ");
        head.push_str(value);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())?;
    stream.write_all(&response.body)?;
    stream.flush()
}

/// Reads a whole response from a Unix socket, capped at `limit` bytes.
pub fn read_response(stream: &mut UnixStream, limit: usize) -> std::io::Result<Vec<u8>> {
    let mut data = Vec::with_capacity(256);
    let mut chunk = [0u8; 8192];
    loop {
        if data.len() > limit {
            data.truncate(limit);
            break;
        }
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => data.extend_from_slice(&chunk[..read]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(data)
}

/// Sets both socket directions to the same deadline duration.
pub fn set_timeouts(stream: &UnixStream, timeout: Duration) -> std::io::Result<()> {
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))
}

/// A parsed HTTP response: status, headers and body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

/// Splits raw HTTP response bytes into status, headers and body.
pub fn parse_response(data: &[u8]) -> Result<ParsedResponse, String> {
    let split = data
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| "response has no header terminator".to_string())?;
    let head = String::from_utf8_lossy(&data[..split]).into_owned();
    let mut lines = head.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| "response has no status line".to_string())?;
    let mut parts = status_line.split(' ');
    let version = parts.next().unwrap_or_default();
    if !version.starts_with("HTTP/") {
        return Err("malformed status line".to_string());
    }
    let status = parts
        .next()
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| "malformed status code".to_string())?;
    let mut headers = HashMap::new();
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }
    let mut body = data[split + 4..].to_vec();
    if headers
        .get("transfer-encoding")
        .is_some_and(|value| value.eq_ignore_ascii_case("chunked"))
    {
        body = decode_chunked(&body)?;
    } else if let Some(length) = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        && body.len() > length
    {
        body.truncate(length);
    }
    Ok(ParsedResponse {
        status,
        headers,
        body,
    })
}

fn decode_chunked(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut index = 0;
    loop {
        let line_end = data[index..]
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| "truncated chunk header".to_string())?;
        let line = std::str::from_utf8(&data[index..index + line_end])
            .map_err(|_| "invalid chunk header".to_string())?;
        let size_text = line.split(';').next().unwrap_or(line).trim();
        let size =
            usize::from_str_radix(size_text, 16).map_err(|_| "invalid chunk size".to_string())?;
        index += line_end + 2;
        if size == 0 {
            break;
        }
        if index + size > data.len() {
            return Err("truncated chunk".to_string());
        }
        out.extend_from_slice(&data[index..index + size]);
        index += size;
        if data.get(index..index + 2) == Some(b"\r\n") {
            index += 2;
        }
    }
    Ok(out)
}

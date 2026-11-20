//! The multiplexed tunnel session: HTTP forwarding, WebSocket pumps, stream
//! resets, and the forwarded-header policy.
//!
//! Ported from `tunnel_session.go`. One session owns up to 256 concurrent
//! streams over one Worker WebSocket. Outgoing frames go through a single
//! channel; the caller owns the socket and its read loop.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode, Uri};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty};
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tokio::sync::{Semaphore, mpsc, watch};

use super::config::TunnelRoute;
use super::proto::{
    FLAG_FIN, FLAG_TEXT, FRAME_HTTP_RES_DATA, FRAME_HTTP_RES_HEAD, FRAME_STREAM_RESET,
    HttpRequestHead, HttpResponseHead, MAX_CHUNK, WebSocketOpenAck, WebSocketOpenHead,
    encode_frame, encode_json_frame,
};
use super::ws::{OP_BINARY, OP_TEXT, WsError};

pub const MAX_STREAMS: usize = 256;
pub const MAX_STREAM_BUFFER: usize = 16 * 1024 * 1024;
pub const MAX_WS_MESSAGE: usize = 4 * 1024 * 1024;
const LOCAL_WS_QUEUE: usize = 64;
const UPSTREAM_RESPONSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

type Outgoing = mpsc::UnboundedSender<Vec<u8>>;
type BoxBodyIo = BoxBody<Bytes, io::Error>;

/// One running tunnel session.
pub struct TunnelSession {
    route: TunnelRoute,
    outgoing: Outgoing,
    streams: Arc<Semaphore>,
    shutdown: watch::Receiver<bool>,
    http_streams: Mutex<BTreeMap<u32, HttpStream>>,
    ws_streams: Mutex<BTreeMap<u32, WsStream>>,
}

struct HttpStream {
    body: Arc<StreamBodyBuffer>,
    cancel: watch::Sender<bool>,
}

struct WsStream {
    outbound: mpsc::Sender<WsOutbound>,
    cancel: watch::Sender<bool>,
    fragments: Arc<Mutex<Vec<u8>>>,
    /// Held for the stream's lifetime so concurrency stays bounded.
    _permit: Option<tokio::sync::OwnedSemaphorePermit>,
}

#[derive(Debug)]
enum WsOutbound {
    Message(u8, Vec<u8>),
    Close(u16, String),
}

impl TunnelSession {
    pub fn new(
        route: TunnelRoute,
        outgoing: Outgoing,
        shutdown: watch::Receiver<bool>,
    ) -> Arc<Self> {
        Arc::new(TunnelSession {
            route,
            outgoing,
            streams: Arc::new(Semaphore::new(MAX_STREAMS)),
            shutdown,
            http_streams: Mutex::new(BTreeMap::new()),
            ws_streams: Mutex::new(BTreeMap::new()),
        })
    }

    /// The registration frame payload client ID is sent by the caller.
    pub fn route(&self) -> &TunnelRoute {
        &self.route
    }

    fn send(&self, frame: Vec<u8>) {
        let _ = self.outgoing.send(frame);
    }

    fn send_reset(&self, stream_id: u32) {
        self.send(encode_frame(
            FRAME_STREAM_RESET,
            FLAG_FIN,
            stream_id,
            0,
            &[],
        ));
    }

    /// Dispatch one binary tunnel frame.
    pub fn dispatch(self: &Arc<Self>, raw: &[u8]) {
        let Ok(frame) = super::proto::decode_frame(raw) else {
            return;
        };
        match frame.frame_type {
            super::proto::FRAME_HTTP_REQ_HEAD => self.handle_http_head(&frame),
            super::proto::FRAME_HTTP_REQ_DATA => self.handle_http_data(&frame),
            super::proto::FRAME_STREAM_RESET => self.handle_reset(frame.stream_id),
            super::proto::FRAME_WS_OPEN => self.handle_ws_open(&frame),
            super::proto::FRAME_WS_DATA => self.handle_ws_data(&frame),
            super::proto::FRAME_WS_CLOSE => self.handle_ws_close(&frame),
            _ => {}
        }
    }

    fn handle_http_head(self: &Arc<Self>, frame: &super::proto::Frame<'_>) {
        let Ok(head) = serde_json::from_slice::<HttpRequestHead>(frame.payload) else {
            self.send_reset(frame.stream_id);
            return;
        };
        let Ok(permit) = Arc::clone(&self.streams).try_acquire_owned() else {
            self.send_simple_http(
                frame.stream_id,
                503,
                "text/plain; charset=utf-8",
                b"Tunnel concurrency limit reached",
            );
            return;
        };
        let body = if frame.flags & FLAG_FIN != 0 {
            None
        } else {
            Some(Arc::new(StreamBodyBuffer::new(MAX_STREAM_BUFFER)))
        };
        let cancel = watch::channel(false).0;
        self.http_streams.lock().unwrap().insert(
            frame.stream_id,
            HttpStream {
                body: body
                    .clone()
                    .unwrap_or_else(|| Arc::new(StreamBodyBuffer::empty())),
                cancel: cancel.clone(),
            },
        );
        let session = Arc::clone(self);
        let stream_id = frame.stream_id;
        tokio::spawn(async move {
            Arc::clone(&session)
                .forward_http(stream_id, head, body, cancel)
                .await;
            session.http_streams.lock().unwrap().remove(&stream_id);
            drop(permit);
        });
    }

    fn handle_http_data(&self, frame: &super::proto::Frame<'_>) {
        let stream = self
            .http_streams
            .lock()
            .unwrap()
            .get(&frame.stream_id)
            .map(|stream| stream.body.clone());
        let Some(body) = stream else {
            return;
        };
        if !frame.payload.is_empty() && !body.push(frame.payload) {
            self.reset_http(frame.stream_id);
            return;
        }
        if frame.flags & FLAG_FIN != 0 {
            body.finish();
        }
    }

    fn handle_reset(&self, stream_id: u32) {
        if let Some(stream) = self.http_streams.lock().unwrap().remove(&stream_id) {
            let _ = stream.cancel.send(true);
            stream.body.close();
        }
        if let Some(stream) = self.ws_streams.lock().unwrap().remove(&stream_id) {
            let _ = stream.cancel.send(true);
        }
    }

    fn reset_http(&self, stream_id: u32) {
        if let Some(stream) = self.http_streams.lock().unwrap().remove(&stream_id) {
            let _ = stream.cancel.send(true);
            stream.body.close();
        }
        self.send_reset(stream_id);
    }

    async fn forward_http(
        self: Arc<Self>,
        stream_id: u32,
        head: HttpRequestHead,
        body: Option<Arc<StreamBodyBuffer>>,
        cancel: watch::Sender<bool>,
    ) {
        let mut cancelled = cancel.subscribe();
        let path = normalize_path(&head.path);
        let method = Method::from_bytes(head.method.as_bytes()).unwrap_or(Method::GET);
        let mut request = Request::builder().method(method.clone()).uri(
            path.parse::<Uri>()
                .unwrap_or_else(|_| Uri::from_static("/")),
        );
        let local_host = self.route.local_host_header();
        let headers = apply_forwarded_headers(
            &head.headers,
            &self.route.public_host(),
            &local_host,
            &head.remote_addr,
        );
        {
            let builder_headers = request.headers_mut().expect("request builder");
            for (name, value) in &headers {
                builder_headers.append(name, value.clone());
            }
            if let Ok(value) = HeaderValue::from_str(&local_host) {
                builder_headers.insert(http::header::HOST, value);
            }
        }
        let request_body: BoxBodyIo = match &body {
            None => Empty::<Bytes>::new()
                .map_err(|never| match never {})
                .boxed(),
            Some(buffer) => buffer.body(),
        };
        let request = match request.body(request_body) {
            Ok(request) => request,
            Err(_) => {
                self.send_simple_http(
                    stream_id,
                    400,
                    "text/plain; charset=utf-8",
                    b"Invalid tunneled request",
                );
                return;
            }
        };

        let send = async {
            let mut sender = connect_upstream(&self.route).await?;
            sender.send_request(request).await.map_err(io::Error::other)
        };
        let response = tokio::select! {
            changed = cancelled.changed() => {
                let _ = changed;
                return;
            }
            result = tokio::time::timeout(UPSTREAM_RESPONSE_TIMEOUT, send) => {
                match result {
                    Ok(Ok(response)) => response,
                    Ok(Err(error)) => {
                        let (status, page) = failure_page(&self.route, &error);
                        self.send_simple_http(stream_id, status, "text/html; charset=utf-8", &page);
                        return;
                    }
                    Err(_) => {
                        let (status, page) = failure_page(
                            &self.route,
                            &io::Error::new(io::ErrorKind::TimedOut, "upstream timeout"),
                        );
                        self.send_simple_http(stream_id, status, "text/html; charset=utf-8", &page);
                        return;
                    }
                }
            }
        };

        let status = response.status();
        let response_headers = filter_response_headers(response.headers());
        let content_length = response
            .headers()
            .get(http::header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok());
        let bodyless = is_bodyless(&method, status, content_length);
        let head_frame = match encode_json_frame(
            FRAME_HTTP_RES_HEAD,
            if bodyless { FLAG_FIN } else { 0 },
            stream_id,
            status.as_u16(),
            &HttpResponseHead {
                status: status.as_u16(),
                headers: response_headers,
            },
        ) {
            Ok(frame) => frame,
            Err(_) => {
                self.send_reset(stream_id);
                return;
            }
        };
        self.send(head_frame);
        if bodyless {
            return;
        }

        let mut incoming = response.into_body();
        loop {
            let frame = tokio::select! {
                changed = cancelled.changed() => {
                    let _ = changed;
                    return;
                }
                frame = incoming.frame() => frame,
            };
            let Some(frame) = frame else {
                self.send(encode_frame(
                    FRAME_HTTP_RES_DATA,
                    FLAG_FIN,
                    stream_id,
                    0,
                    &[],
                ));
                return;
            };
            let Ok(frame) = frame else {
                self.send_reset(stream_id);
                return;
            };
            let Some(data) = frame.data_ref() else {
                continue;
            };
            let mut offset = 0;
            while offset < data.len() {
                let end = (offset + MAX_CHUNK).min(data.len());
                // FIN is only known on the next loop iteration; send a final
                // empty FIN frame when the body ends, as the Go client does.
                self.send(encode_frame(
                    FRAME_HTTP_RES_DATA,
                    0,
                    stream_id,
                    0,
                    &data[offset..end],
                ));
                offset = end;
            }
        }
    }

    fn send_simple_http(&self, stream_id: u32, status: u16, content_type: &str, body: &[u8]) {
        let headers = vec![
            ["Content-Type".into(), content_type.into()],
            ["Cache-Control".into(), "no-store".into()],
            ["Content-Length".into(), body.len().to_string()],
        ];
        let flags = if body.is_empty() { FLAG_FIN } else { 0 };
        let Ok(head) = encode_json_frame(
            FRAME_HTTP_RES_HEAD,
            flags,
            stream_id,
            status,
            &HttpResponseHead { status, headers },
        ) else {
            return;
        };
        self.send(head);
        if body.is_empty() {
            return;
        }
        let mut offset = 0;
        while offset < body.len() {
            let end = (offset + MAX_CHUNK).min(body.len());
            let flags = if end == body.len() { FLAG_FIN } else { 0 };
            self.send(encode_frame(
                FRAME_HTTP_RES_DATA,
                flags,
                stream_id,
                0,
                &body[offset..end],
            ));
            offset = end;
        }
    }

    fn handle_ws_open(self: &Arc<Self>, frame: &super::proto::Frame<'_>) {
        let Ok(head) = serde_json::from_slice::<WebSocketOpenHead>(frame.payload) else {
            self.send_ws_error(frame.stream_id, 400, "invalid websocket open payload");
            return;
        };
        let Ok(permit) = Arc::clone(&self.streams).try_acquire_owned() else {
            self.send_ws_error(frame.stream_id, 503, "tunnel concurrency limit reached");
            return;
        };
        let session = Arc::clone(self);
        let stream_id = frame.stream_id;
        tokio::spawn(async move {
            session.open_ws(stream_id, head, permit).await;
        });
    }

    async fn open_ws(
        self: &Arc<Self>,
        stream_id: u32,
        head: WebSocketOpenHead,
        permit: tokio::sync::OwnedSemaphorePermit,
    ) {
        let path = normalize_path(&head.path);
        let local_host = self.route.local_host_header();
        let headers =
            apply_forwarded_headers(&head.headers, &self.route.public_host(), &local_host, "");
        let header_list: Vec<(String, String)> = headers
            .iter()
            .filter_map(|(name, value)| {
                Some((name.as_str().to_string(), value.to_str().ok()?.to_string()))
            })
            .collect();
        let origin = self.route.upstream_origin();
        let raw_url = format!("{origin}{path}");
        let Ok(url) = url::Url::parse(&raw_url) else {
            self.send_ws_error(stream_id, 502, "local websocket upstream unavailable");
            return;
        };
        let host = url.host_str().unwrap_or("127.0.0.1").to_string();
        let port = url.port_or_known_default().unwrap_or(80);
        let address = format!("{host}:{port}");
        let connect = async {
            let stream = TcpStream::connect(address).await?;
            super::ws::client_handshake(stream, &url, &host, port, &header_list, &head.protocols)
                .await
        };
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), connect).await;
        let (ws, response) = match result {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => {
                let status = match &error {
                    WsError::Handshake(handshake) => handshake.status_code,
                    _ => 502,
                };
                self.send_ws_error(stream_id, status, "local websocket upstream unavailable");
                return;
            }
            Err(_) => {
                self.send_ws_error(stream_id, 504, "local websocket upstream unavailable");
                return;
            }
        };

        let negotiated = response
            .header("sec-websocket-protocol")
            .unwrap_or_default()
            .to_string();
        let Ok(ack) = encode_json_frame(
            super::proto::FRAME_WS_OPEN_ACK,
            FLAG_FIN,
            stream_id,
            101,
            &WebSocketOpenAck {
                protocol: negotiated,
                error: String::new(),
            },
        ) else {
            return;
        };
        self.send(ack);

        let (outbound_tx, mut outbound_rx) = mpsc::channel::<WsOutbound>(LOCAL_WS_QUEUE);
        let cancel = watch::channel(false).0;
        self.ws_streams.lock().unwrap().insert(
            stream_id,
            WsStream {
                outbound: outbound_tx,
                cancel: cancel.clone(),
                fragments: Arc::new(Mutex::new(Vec::new())),
                _permit: Some(permit),
            },
        );

        let sender = ws.sender();
        let mut reader = ws;
        let mut cancelled = cancel.subscribe();
        let session = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    changed = cancelled.changed() => {
                        let _ = changed;
                        break;
                    }
                    message = outbound_rx.recv() => {
                        let Some(message) = message else { break; };
                        match message {
                            WsOutbound::Message(opcode, data) => {
                                if sender.write_message(opcode, &data).await.is_err() {
                                    session.send_reset(stream_id);
                                    break;
                                }
                            }
                            WsOutbound::Close(code, reason) => {
                                let _ = sender.write_close(code, &reason).await;
                                break;
                            }
                        }
                    }
                    message = reader.read_message() => {
                        match message {
                            Ok((opcode, data)) => {
                                let text = if opcode == OP_TEXT { FLAG_TEXT } else { 0 };
                                if data.is_empty() {
                                    session.send(encode_frame(
                                        super::proto::FRAME_WS_DATA,
                                        text | FLAG_FIN,
                                        stream_id,
                                        0,
                                        &[],
                                    ));
                                    continue;
                                }
                                let mut offset = 0;
                                while offset < data.len() {
                                    let end = (offset + MAX_CHUNK).min(data.len());
                                    let mut flags = text;
                                    if end == data.len() { flags |= FLAG_FIN; }
                                    session.send(encode_frame(
                                        super::proto::FRAME_WS_DATA,
                                        flags,
                                        stream_id,
                                        0,
                                        &data[offset..end],
                                    ));
                                    offset = end;
                                }
                            }
                            Err(WsError::Close(close)) => {
                                session.send(encode_frame(
                                    super::proto::FRAME_WS_CLOSE,
                                    FLAG_FIN,
                                    stream_id,
                                    close.code,
                                    close.reason.as_bytes(),
                                ));
                                break;
                            }
                            Err(_) => {
                                session.send_reset(stream_id);
                                break;
                            }
                        }
                    }
                }
            }
            session.ws_streams.lock().unwrap().remove(&stream_id);
        });
    }

    fn send_ws_error(&self, stream_id: u32, status: u16, message: &str) {
        if let Ok(frame) = encode_json_frame(
            super::proto::FRAME_WS_OPEN_ACK,
            FLAG_FIN,
            stream_id,
            status,
            &WebSocketOpenAck {
                protocol: String::new(),
                error: message.to_string(),
            },
        ) {
            self.send(frame);
        }
    }

    fn handle_ws_data(&self, frame: &super::proto::Frame<'_>) {
        let Some((outbound, cancel, fragments)) = self
            .ws_streams
            .lock()
            .unwrap()
            .get(&frame.stream_id)
            .map(|stream| {
                (
                    stream.outbound.clone(),
                    stream.cancel.clone(),
                    Arc::clone(&stream.fragments),
                )
            })
        else {
            return;
        };
        let mut buffer = fragments.lock().unwrap();
        if buffer.len() + frame.payload.len() > MAX_WS_MESSAGE {
            drop(buffer);
            let _ = cancel.send(true);
            self.send(encode_frame(
                super::proto::FRAME_WS_CLOSE,
                FLAG_FIN,
                frame.stream_id,
                1009,
                b"message too big",
            ));
            self.send_reset(frame.stream_id);
            return;
        }
        buffer.extend_from_slice(frame.payload);
        if frame.flags & FLAG_FIN == 0 {
            return;
        }
        let data = std::mem::take(&mut *buffer);
        drop(buffer);
        let opcode = if frame.flags & FLAG_TEXT != 0 {
            OP_TEXT
        } else {
            OP_BINARY
        };
        if outbound
            .try_send(WsOutbound::Message(opcode, data))
            .is_err()
        {
            let _ = cancel.send(true);
            self.send(encode_frame(
                super::proto::FRAME_WS_CLOSE,
                FLAG_FIN,
                frame.stream_id,
                1001,
                b"local websocket backpressure",
            ));
            self.send_reset(frame.stream_id);
        }
    }

    fn handle_ws_close(&self, frame: &super::proto::Frame<'_>) {
        let stream = self.ws_streams.lock().unwrap().remove(&frame.stream_id);
        if let Some(stream) = stream {
            let code = if frame.code == 0 { 1000 } else { frame.code };
            let _ = stream.outbound.try_send(WsOutbound::Close(
                code,
                String::from_utf8_lossy(frame.payload).into(),
            ));
        }
    }

    /// Cancel every stream; called when the Worker socket closes.
    pub fn close_all(&self) {
        for (_, stream) in std::mem::take(&mut *self.http_streams.lock().unwrap()) {
            let _ = stream.cancel.send(true);
            stream.body.close();
        }
        for (_, stream) in std::mem::take(&mut *self.ws_streams.lock().unwrap()) {
            let _ = stream.cancel.send(true);
        }
    }

    /// Whether the session's owner has begun shutdown.
    pub fn is_shutdown(&self) -> bool {
        *self.shutdown.borrow()
    }
}

/// A bounded, asynchronous request-body buffer. `push` returns false when the
/// 16 MiB cap would be exceeded.
pub struct StreamBodyBuffer {
    sender: Mutex<Option<mpsc::UnboundedSender<Result<Bytes, io::Error>>>>,
    receiver: Mutex<Option<mpsc::UnboundedReceiver<Result<Bytes, io::Error>>>>,
    queued: Arc<AtomicUsize>,
    limit: usize,
}

impl StreamBodyBuffer {
    pub fn new(limit: usize) -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();
        StreamBodyBuffer {
            sender: Mutex::new(Some(sender)),
            receiver: Mutex::new(Some(receiver)),
            queued: Arc::new(AtomicUsize::new(0)),
            limit,
        }
    }

    fn empty() -> Self {
        StreamBodyBuffer::new(0)
    }

    pub fn push(&self, data: &[u8]) -> bool {
        if data.is_empty() {
            return true;
        }
        let queued = self.queued.load(Ordering::SeqCst);
        if queued + data.len() > self.limit {
            return false;
        }
        let sender = self
            .sender
            .lock()
            .unwrap()
            .as_ref()
            .map(mpsc::UnboundedSender::clone);
        let Some(sender) = sender else {
            return false;
        };
        self.queued.fetch_add(data.len(), Ordering::SeqCst);
        sender.send(Ok(Bytes::copy_from_slice(data))).is_ok()
    }

    pub fn finish(&self) {
        self.sender.lock().unwrap().take();
    }

    pub fn close(&self) {
        let sender = self.sender.lock().unwrap().take();
        if let Some(sender) = sender {
            let _ = sender.send(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "stream reset by peer",
            )));
        }
    }

    /// Convert to a streaming hyper body. The channel exists from creation so
    /// data frames arriving immediately after the head are never lost.
    pub fn body(&self) -> BoxBodyIo {
        let receiver = self
            .receiver
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| mpsc::unbounded_channel().1);
        ChannelBody {
            receiver: tokio::sync::Mutex::new(receiver),
            queued: Arc::clone(&self.queued),
        }
        .boxed()
    }
}

struct ChannelBody {
    receiver: tokio::sync::Mutex<mpsc::UnboundedReceiver<Result<Bytes, io::Error>>>,
    queued: Arc<AtomicUsize>,
}

impl hyper::body::Body for ChannelBody {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Bytes>, io::Error>>> {
        let this = self.get_mut();
        let mut receiver = match this.receiver.try_lock() {
            Ok(receiver) => receiver,
            Err(_) => {
                context.waker().wake_by_ref();
                return std::task::Poll::Pending;
            }
        };
        match receiver.poll_recv(context) {
            std::task::Poll::Ready(Some(Ok(bytes))) => {
                this.queued.fetch_sub(bytes.len(), Ordering::SeqCst);
                std::task::Poll::Ready(Some(Ok(hyper::body::Frame::data(bytes))))
            }
            std::task::Poll::Ready(Some(Err(error))) => std::task::Poll::Ready(Some(Err(error))),
            std::task::Poll::Ready(None) => std::task::Poll::Ready(None),
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

async fn connect_upstream(
    route: &TunnelRoute,
) -> Result<hyper::client::conn::http1::SendRequest<BoxBodyIo>, io::Error> {
    let host = if route.upstream_host.is_empty() {
        "127.0.0.1"
    } else {
        &route.upstream_host
    };
    let port = route.local_port as u16;
    let stream = if host == "127.0.0.1" {
        match TcpStream::connect(("127.0.0.1", port)).await {
            Ok(stream) => stream,
            Err(first) => TcpStream::connect(("::1", port)).await.map_err(|_| first)?,
        }
    } else {
        TcpStream::connect((host, port)).await?
    };
    let (sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(io::Error::other)?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    Ok(sender)
}

fn normalize_path(path: &str) -> String {
    if path.is_empty() || !path.starts_with('/') {
        format!("/{}", path.trim_start_matches('/'))
    } else {
        path.to_string()
    }
}

/// Apply the forwarded-header policy: strip hop-by-hop and spoofable headers,
/// rewrite Origin, then set X-Forwarded-Host/Proto/For and X-Real-IP.
pub fn apply_forwarded_headers(
    incoming: &[[String; 2]],
    public_host: &str,
    local_host: &str,
    remote_addr: &str,
) -> HeaderMap {
    let mut hop_by_hop: BTreeSet<String> = [
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
        "host",
        "forwarded",
        "x-forwarded-for",
        "x-forwarded-host",
        "x-forwarded-proto",
        "x-real-ip",
        "sec-websocket-key",
        "sec-websocket-version",
        "sec-websocket-extensions",
        "sec-websocket-protocol",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    for pair in incoming {
        if pair[0].eq_ignore_ascii_case("connection") {
            for token in pair[1].split(',') {
                let token = token.trim().to_lowercase();
                if !token.is_empty() {
                    hop_by_hop.insert(token);
                }
            }
        }
    }
    let mut headers = HeaderMap::new();
    for pair in incoming {
        let lower = pair[0].to_lowercase();
        if hop_by_hop.contains(&lower) {
            continue;
        }
        if lower == "origin" && !public_host.is_empty() {
            let origin = pair[1].trim();
            let secure = format!("https://{public_host}");
            let secure_port = format!("https://{public_host}:443");
            if origin.eq_ignore_ascii_case(&secure) || origin.eq_ignore_ascii_case(&secure_port) {
                if let Ok(value) = HeaderValue::from_str(origin) {
                    headers.insert("x-forwarded-origin", value);
                }
                if let Ok(value) = HeaderValue::from_str(&format!("http://{local_host}")) {
                    headers.insert(http::header::ORIGIN, value);
                }
                continue;
            }
        }
        let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(pair[0].as_bytes()),
            HeaderValue::from_str(&pair[1]),
        ) else {
            continue;
        };
        headers.append(name, value);
    }
    if !public_host.is_empty()
        && let Ok(value) = HeaderValue::from_str(public_host)
    {
        headers.insert("x-forwarded-host", value);
    }
    headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
    if !remote_addr.is_empty()
        && let Ok(value) = HeaderValue::from_str(remote_addr)
    {
        headers.insert("x-forwarded-for", value.clone());
        headers.insert("x-real-ip", value);
    }
    headers
}

/// Strip hop-by-hop response headers and return sorted `[name, value]` pairs.
pub fn filter_response_headers(headers: &HeaderMap) -> Vec<[String; 2]> {
    let mut hop_by_hop: BTreeSet<String> = [
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    for value in headers.get_all(http::header::CONNECTION) {
        if let Ok(value) = value.to_str() {
            for token in value.split(',') {
                let token = token.trim().to_lowercase();
                if !token.is_empty() {
                    hop_by_hop.insert(token);
                }
            }
        }
    }
    let mut keys: Vec<&HeaderName> = headers
        .keys()
        .filter(|name| !hop_by_hop.contains(name.as_str()))
        .collect();
    keys.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    let mut out = Vec::new();
    for name in keys {
        for value in headers.get_all(name) {
            if let Ok(value) = value.to_str() {
                out.push([canonical_header_name(name.as_str()), value.to_string()]);
            }
        }
    }
    out
}

/// Canonicalize a header name the way Go's `http.Header` does.
pub fn canonical_header_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut upper = true;
    for character in name.chars() {
        if upper && character.is_ascii_lowercase() {
            out.push(character.to_ascii_uppercase());
        } else if !upper && character.is_ascii_uppercase() {
            out.push(character.to_ascii_lowercase());
        } else {
            out.push(character);
        }
        upper = character == '-';
    }
    out
}

/// Whether a response cannot carry a body.
pub fn is_bodyless(method: &Method, status: StatusCode, content_length: Option<u64>) -> bool {
    method == Method::HEAD
        || status == StatusCode::NO_CONTENT
        || status == StatusCode::NOT_MODIFIED
        || status == StatusCode::SWITCHING_PROTOCOLS
        || content_length == Some(0)
}

/// The fixed failure page for a tunneled upstream error.
pub fn failure_page(route: &TunnelRoute, error: &io::Error) -> (u16, Vec<u8>) {
    let (status, title, message) = if error.kind() == io::ErrorKind::TimedOut {
        (
            504,
            "Local service timed out",
            "The local server did not respond within the tunnel timeout.",
        )
    } else {
        (
            502,
            "Local service is unavailable",
            "The local server did not accept this tunneled request. Start the local application or check its port.",
        )
    };
    let upstream = format!(
        "{}:{}",
        if route.upstream_host.is_empty() {
            "127.0.0.1"
        } else {
            &route.upstream_host
        },
        route.local_port
    );
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>{title}</title><style>body{{font:16px/1.6 system-ui;margin:12vh auto;padding:24px;max-width:640px;color:#202124;background:#f5f6f8}}main{{background:white;padding:32px;border-radius:16px}}h1{{line-height:1.2;font-size:28px}}code{{word-break:break-all}}p{{color:#555}}</style></head><body><main><h1>{title}</h1><p>{message}</p><p><strong>{host}</strong> → <code>http://{upstream}</code></p><p>Open Devbase to check the local service and tunnel status.</p></main></body></html>",
        host = html_escape(&route.public_host()),
        upstream = html_escape(&upstream),
    );
    (status, html.into_bytes())
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwarded_headers_strip_spoofing_and_set_the_hop_values() {
        let incoming = vec![
            ["Connection".into(), "keep-alive, X-Hop".into()],
            ["X-Hop".into(), "1".into()],
            ["Host".into(), "app.example.com".into()],
            ["Forwarded".into(), "for=1.2.3.4".into()],
            ["X-Forwarded-For".into(), "6.6.6.6".into()],
            ["X-Forwarded-Host".into(), "evil".into()],
            ["X-Real-IP".into(), "6.6.6.6".into()],
            ["Origin".into(), "https://app.example.com".into()],
            ["Cf-Connecting-Ip".into(), "203.0.113.9".into()],
            ["Accept".into(), "text/html".into()],
        ];
        let headers = apply_forwarded_headers(
            &incoming,
            "app.example.com",
            "localhost:3000",
            "203.0.113.9",
        );
        assert!(headers.get("x-hop").is_none());
        assert!(headers.get("host").is_none());
        assert!(headers.get("forwarded").is_none());
        assert_eq!(headers["x-forwarded-for"], "203.0.113.9");
        assert_eq!(headers["x-forwarded-host"], "app.example.com");
        assert_eq!(headers["x-forwarded-proto"], "https");
        assert_eq!(headers["x-real-ip"], "203.0.113.9");
        assert_eq!(headers["origin"], "http://localhost:3000");
        assert_eq!(headers["x-forwarded-origin"], "https://app.example.com");
        assert_eq!(headers["cf-connecting-ip"], "203.0.113.9");
        assert_eq!(headers["accept"], "text/html");
    }

    #[test]
    fn origin_is_only_rewritten_for_the_public_host() {
        let incoming = vec![
            ["Origin".into(), "https://other.example.com".into()],
            ["Origin".into(), "https://app.example.com:443".into()],
        ];
        let headers = apply_forwarded_headers(&incoming, "app.example.com", "localhost:3000", "");
        assert_eq!(
            headers["origin"], "http://localhost:3000",
            "the matching origin is rewritten by Set semantics"
        );
        assert_eq!(headers["x-forwarded-origin"], "https://app.example.com:443");

        let only_other = apply_forwarded_headers(
            &[["Origin".into(), "https://other.example.com".into()]],
            "app.example.com",
            "localhost:3000",
            "",
        );
        assert_eq!(only_other["origin"], "https://other.example.com");
        assert!(only_other.get("x-forwarded-origin").is_none());
    }

    #[test]
    fn response_headers_are_filtered_and_sorted() {
        let mut headers = HeaderMap::new();
        headers.insert("Content-Type", HeaderValue::from_static("text/plain"));
        headers.insert("Connection", HeaderValue::from_static("X-Private"));
        headers.append("X-Private", HeaderValue::from_static("1"));
        headers.insert("Transfer-Encoding", HeaderValue::from_static("chunked"));
        headers.insert("A", HeaderValue::from_static("2"));
        let filtered = filter_response_headers(&headers);
        assert_eq!(
            filtered,
            vec![
                ["A".to_string(), "2".to_string()],
                ["Content-Type".to_string(), "text/plain".to_string()],
            ]
        );
    }

    #[test]
    fn bodyless_rules_match_go() {
        assert!(is_bodyless(&Method::HEAD, StatusCode::OK, Some(10)));
        assert!(is_bodyless(&Method::GET, StatusCode::NO_CONTENT, None));
        assert!(is_bodyless(&Method::GET, StatusCode::NOT_MODIFIED, None));
        assert!(is_bodyless(
            &Method::GET,
            StatusCode::SWITCHING_PROTOCOLS,
            None
        ));
        assert!(is_bodyless(&Method::GET, StatusCode::OK, Some(0)));
        assert!(!is_bodyless(&Method::GET, StatusCode::OK, Some(10)));
        assert!(!is_bodyless(&Method::GET, StatusCode::OK, None));
    }

    #[test]
    fn stream_body_buffer_enforces_its_limit() {
        let buffer = StreamBodyBuffer::new(4);
        let _body = buffer.body();
        assert!(buffer.push(b"abcd"));
        assert!(!buffer.push(b"e"));
        buffer.finish();
        let closed = StreamBodyBuffer::new(4);
        closed.close();
        assert!(!closed.push(b"a"));
    }

    #[test]
    fn paths_are_forced_to_origin_form() {
        assert_eq!(normalize_path(""), "/");
        assert_eq!(normalize_path("a/b"), "/a/b");
        assert_eq!(normalize_path("/a?b=1"), "/a?b=1");
    }
}

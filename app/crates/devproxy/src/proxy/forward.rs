//! The forwarder behind the local HTTPS proxy.
//!
//! Go's `httputil.ReverseProxy` has no Rust drop-in, so this module implements
//! the parts the proxy contract pins: immediate streaming, `101` upgrade
//! tunneling, `Rewrite`-style Host and `X-Forwarded-*` handling, and the fixed
//! failure page. Connections are opened per request to the validated local
//! upstream; IPv4 loopback is tried before IPv6.

use std::io;
use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, Request, Response, StatusCode, Uri};
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty};
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;

use super::rules::ProxyRoute;

/// The upstream response-header timeout.
pub const RESPONSE_HEADER_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

pub type ProxyBody = BoxBody<Bytes, io::Error>;

/// A classified failure, with the exact copy shown by the Go page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProxyFailure {
    pub status: StatusCode,
    pub title: &'static str,
    pub message: &'static str,
}

/// Classify a forwarding error. Timeouts become 504; everything else 502.
pub fn classify_failure(error: &io::Error) -> ProxyFailure {
    if has_certificate_error(error) {
        return ProxyFailure {
            status: StatusCode::BAD_GATEWAY,
            title: "Upstream certificate is not trusted",
            message: "The HTTPS upstream certificate could not be verified. Use a trusted certificate with the correct hostname.",
        };
    }
    if error.kind() == io::ErrorKind::TimedOut {
        ProxyFailure {
            status: StatusCode::GATEWAY_TIMEOUT,
            title: "Local service timed out",
            message: "The local server did not respond within the proxy timeout. Check its logs or adjust the proxy timeout.",
        }
    } else {
        ProxyFailure {
            status: StatusCode::BAD_GATEWAY,
            title: "Local service is unavailable",
            message: "The local server did not accept this request. Start the local application or check its port.",
        }
    }
}

fn has_certificate_error(error: &io::Error) -> bool {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(current) = source {
        let text = current.to_string().to_lowercase();
        if text.contains("certificate")
            || text.contains("unknownissuer")
            || text.contains("notvalidforname")
        {
            return true;
        }
        source = current.source();
    }
    false
}

/// The fixed HTML failure page; only the domain and upstream vary.
pub fn failure_page(domain: &str, upstream: &str, failure: ProxyFailure) -> Vec<u8> {
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>{title}</title><style>body{{font:16px/1.6 system-ui;margin:12vh auto;padding:24px;max-width:640px;color:#202124;background:#f5f6f8}}main{{background:white;padding:32px;border-radius:16px}}h1{{line-height:1.2;font-size:28px}}code{{word-break:break-all}}p{{color:#555}}</style></head><body><main><h1>{title}</h1><p>{message}</p><p><strong>{domain}</strong> → <code>{upstream}</code></p><p>Open Devbase to check the local port and application logs.</p></main></body></html>",
        title = failure.title,
        message = failure.message,
        domain = escape(domain),
        upstream = escape(upstream),
    );
    html.into_bytes()
}

/// Build an HTTP response for a failure page.
pub fn failure_response(
    domain: &str,
    upstream: &str,
    failure: ProxyFailure,
) -> Response<ProxyBody> {
    let body = failure_page(domain, upstream, failure);
    let mut response = Response::new(full_body(body));
    *response.status_mut() = failure.status;
    response.headers_mut().insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    response.headers_mut().insert(
        http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    response
}

/// Rewrite inbound headers for the local upstream.
pub fn upstream_headers(
    inbound: &HeaderMap,
    inbound_host: &str,
    remote: Option<SocketAddr>,
) -> HeaderMap {
    let upgrading = inbound
        .get(http::header::CONNECTION)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
        })
        .unwrap_or(false)
        && inbound.contains_key(http::header::UPGRADE);
    let mut hop_by_hop: Vec<String> = [
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
        "proxy-connection",
        "host",
        "forwarded",
        "x-forwarded-for",
        "x-forwarded-host",
        "x-forwarded-proto",
        "x-real-ip",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    if !upgrading {
        hop_by_hop.push("upgrade".into());
    }
    for value in inbound.get_all(http::header::CONNECTION) {
        if let Ok(value) = value.to_str() {
            for token in value.split(',') {
                let token = token.trim().to_lowercase();
                if !token.is_empty() {
                    hop_by_hop.push(token);
                }
            }
        }
    }
    let mut headers = HeaderMap::new();
    for (name, value) in inbound {
        let lower = name.as_str();
        if hop_by_hop.iter().any(|item| item == lower) {
            continue;
        }
        headers.append(name.clone(), value.clone());
    }
    if upgrading && let Some(upgrade) = inbound.get(http::header::UPGRADE) {
        headers.insert(http::header::UPGRADE, upgrade.clone());
        headers.insert(
            http::header::CONNECTION,
            HeaderValue::from_static("Upgrade"),
        );
    }
    if let Ok(value) = HeaderValue::from_str(inbound_host) {
        headers.insert(http::header::HOST, value);
    }
    if let Ok(value) = HeaderValue::from_str(inbound_host) {
        headers.insert("x-forwarded-host", value);
    }
    headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
    if let Some(remote) = remote
        && let Ok(value) = HeaderValue::from_str(&remote.ip().to_string())
    {
        headers.insert("x-forwarded-for", value);
    }
    headers
}

/// Strip hop-by-hop response headers, except on a `101` upgrade.
pub fn response_headers(inbound: &HeaderMap, upgrading: bool) -> HeaderMap {
    if upgrading {
        return inbound.clone();
    }
    let mut hop_by_hop: Vec<String> = [
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
        "proxy-connection",
        "upgrade",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    for value in inbound.get_all(http::header::CONNECTION) {
        if let Ok(value) = value.to_str() {
            for token in value.split(',') {
                let token = token.trim().to_lowercase();
                if !token.is_empty() {
                    hop_by_hop.push(token);
                }
            }
        }
    }
    let mut headers = HeaderMap::new();
    for (name, value) in inbound {
        if hop_by_hop.iter().any(|item| item == name.as_str()) {
            continue;
        }
        headers.append(name.clone(), value.clone());
    }
    headers
}

/// Forward one request and stream the response back.
pub async fn forward(
    site: &ProxyRoute,
    mut request: Request<Incoming>,
    remote: Option<SocketAddr>,
) -> Response<ProxyBody> {
    let host = inbound_host(request.headers());
    let upgrading = request
        .headers()
        .get(http::header::CONNECTION)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
        })
        .unwrap_or(false)
        && request.headers().contains_key(http::header::UPGRADE);
    let inbound_upgrade = if upgrading {
        Some(hyper::upgrade::on(&mut request))
    } else {
        None
    };
    let method = request.method().clone();
    let path = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str().to_string())
        .unwrap_or_else(|| "/".into());
    let headers = upstream_headers(request.headers(), &host, remote);
    let body: ProxyBody = request.into_body().map_err(io::Error::other).boxed();
    let uri = path
        .parse::<Uri>()
        .unwrap_or_else(|_| Uri::from_static("/"));
    let mut outbound = Request::builder()
        .method(method)
        .uri(uri)
        .body(body)
        .expect("request parts are valid");
    *outbound.headers_mut() = headers;

    let attempt = async {
        let mut sender = connect_upstream(site).await?;
        sender
            .send_request(outbound)
            .await
            .map_err(io::Error::other)
    };
    let response = match tokio::time::timeout(RESPONSE_HEADER_TIMEOUT, attempt).await {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            return failure_response(
                &site.domain,
                &site.upstream_host_port(),
                classify_failure(&error),
            );
        }
        Err(_) => {
            let timeout = io::Error::new(io::ErrorKind::TimedOut, "upstream timeout");
            return failure_response(
                &site.domain,
                &site.upstream_host_port(),
                classify_failure(&timeout),
            );
        }
    };

    let status = response.status();
    let is_upgrade = status == StatusCode::SWITCHING_PROTOCOLS;
    let response_headers = response_headers(response.headers(), is_upgrade);
    if is_upgrade {
        let mut upstream_upgrade = response;
        let upgrade = hyper::upgrade::on(&mut upstream_upgrade);
        if let Some(inbound_upgrade) = inbound_upgrade {
            tokio::spawn(async move {
                let (inbound, outbound) = match (inbound_upgrade.await, upgrade.await) {
                    (Ok(inbound), Ok(outbound)) => (TokioIo::new(inbound), TokioIo::new(outbound)),
                    _ => return,
                };
                let mut inbound = inbound;
                let mut outbound = outbound;
                let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
            });
        }
        let mut response = Response::new(empty_body());
        *response.status_mut() = status;
        *response.headers_mut() = response_headers;
        return response;
    }

    let body: ProxyBody = response.into_body().map_err(io::Error::other).boxed();
    let mut result = Response::new(body);
    *result.status_mut() = status;
    *result.headers_mut() = response_headers;
    result
}

/// The lowercased host from a `Host` header, with the port removed.
pub fn inbound_host(headers: &HeaderMap) -> String {
    let raw = headers
        .get(http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .trim();
    let host = if let Some(rest) = raw.strip_prefix('[') {
        rest.split(']').next().unwrap_or(rest)
    } else {
        raw.rsplit_once(':')
            .filter(|(_, port)| port.chars().all(|character| character.is_ascii_digit()))
            .map(|(host, _)| host)
            .unwrap_or(raw)
    };
    host.to_lowercase()
}

fn full_body(bytes: Vec<u8>) -> ProxyBody {
    http_body_util::Full::new(Bytes::from(bytes))
        .map_err(|never| match never {})
        .boxed()
}

fn empty_body() -> ProxyBody {
    Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed()
}

async fn connect_upstream(
    site: &ProxyRoute,
) -> Result<hyper::client::conn::http1::SendRequest<ProxyBody>, io::Error> {
    let port = site.local_port as u16;
    let connect = async {
        if site.upstream_host.is_empty() {
            match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(("127.0.0.1", port)))
                .await
            {
                Ok(Ok(stream)) => Ok(stream),
                Ok(Err(first)) => {
                    match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(("::1", port)))
                        .await
                    {
                        Ok(Ok(stream)) => Ok(stream),
                        _ => Err(first),
                    }
                }
                Err(_) => Err(io::Error::new(io::ErrorKind::TimedOut, "connect timeout")),
            }
        } else {
            match tokio::time::timeout(
                CONNECT_TIMEOUT,
                TcpStream::connect((site.upstream_host.as_str(), port)),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => Err(io::Error::new(io::ErrorKind::TimedOut, "connect timeout")),
            }
        }
    };
    let stream: TcpStream = connect.await?;
    let _ = stream.set_nodelay(true);
    let (sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .map_err(io::Error::other)?;
    tokio::spawn(async move {
        let _ = connection.with_upgrades().await;
    });
    Ok(sender)
}

fn escape(value: &str) -> String {
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
    fn forwarded_headers_strip_spoofing_and_keep_upgrades() {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::HOST,
            HeaderValue::from_static("app.devbase.localhost"),
        );
        headers.insert(
            http::header::FORWARDED,
            HeaderValue::from_static("for=1.2.3.4"),
        );
        headers.insert("x-forwarded-for", HeaderValue::from_static("6.6.6.6"));
        headers.insert("x-real-ip", HeaderValue::from_static("6.6.6.6"));
        headers.insert(
            http::header::CONNECTION,
            HeaderValue::from_static("Upgrade"),
        );
        headers.insert(http::header::UPGRADE, HeaderValue::from_static("websocket"));
        headers.insert("sec-websocket-key", HeaderValue::from_static("abc"));
        let remote: SocketAddr = "203.0.113.9:5000".parse().unwrap();
        let headers = upstream_headers(&headers, "app.devbase.localhost", Some(remote));
        assert_eq!(headers["host"], "app.devbase.localhost");
        assert!(headers.get("forwarded").is_none());
        assert_eq!(headers["x-forwarded-for"], "203.0.113.9");
        assert!(headers.get("x-real-ip").is_none());
        assert_eq!(headers["x-forwarded-host"], "app.devbase.localhost");
        assert_eq!(headers["x-forwarded-proto"], "https");
        assert_eq!(headers["upgrade"], "websocket");
        assert_eq!(headers["connection"], "Upgrade");
        assert_eq!(headers["sec-websocket-key"], "abc");
    }

    #[test]
    fn non_upgrade_requests_drop_upgrade_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(http::header::UPGRADE, HeaderValue::from_static("h2c"));
        headers.insert(
            http::header::CONNECTION,
            HeaderValue::from_static("keep-alive"),
        );
        let headers = upstream_headers(&headers, "app.devbase.localhost", None);
        assert!(headers.get("upgrade").is_none());
        assert!(headers.get("connection").is_none());
        assert_eq!(headers["x-forwarded-proto"], "https");
        assert!(headers.get("x-forwarded-for").is_none());
    }

    #[test]
    fn host_parsing_handles_ports_and_ipv6() {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::HOST,
            HeaderValue::from_static("App.Devbase.Localhost:443"),
        );
        assert_eq!(inbound_host(&headers), "app.devbase.localhost");
        headers.insert(http::header::HOST, HeaderValue::from_static("[::1]:443"));
        assert_eq!(inbound_host(&headers), "::1");
        headers.remove(http::header::HOST);
        assert_eq!(inbound_host(&headers), "");
    }

    #[test]
    fn failure_pages_match_the_go_copy() {
        let timeout = classify_failure(&io::Error::new(io::ErrorKind::TimedOut, "x"));
        assert_eq!(timeout.status, StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(timeout.title, "Local service timed out");
        let refused = classify_failure(&io::Error::new(io::ErrorKind::ConnectionRefused, "x"));
        assert_eq!(refused.status, StatusCode::BAD_GATEWAY);
        assert_eq!(refused.title, "Local service is unavailable");
        let certificate =
            classify_failure(&io::Error::other("invalid peer certificate: UnknownIssuer"));
        assert_eq!(certificate.title, "Upstream certificate is not trusted");
        let page = String::from_utf8(failure_page(
            "app.devbase.localhost",
            "127.0.0.1:3000",
            refused,
        ))
        .unwrap();
        assert!(page.contains("app.devbase.localhost"));
        assert!(page.contains("127.0.0.1:3000"));
        assert!(page.contains("Open Devbase to check the local port and application logs."));
    }

    #[test]
    fn response_headers_strip_hop_by_hop_except_on_upgrade() {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::CONNECTION,
            HeaderValue::from_static("X-Private"),
        );
        headers.insert("x-private", HeaderValue::from_static("1"));
        headers.insert(http::header::UPGRADE, HeaderValue::from_static("websocket"));
        headers.insert(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain"),
        );
        let normal = response_headers(&headers, false);
        assert!(normal.get("x-private").is_none());
        assert!(normal.get("upgrade").is_none());
        assert_eq!(normal["content-type"], "text/plain");
        let upgrade = response_headers(&headers, true);
        assert_eq!(upgrade["upgrade"], "websocket");
    }
}

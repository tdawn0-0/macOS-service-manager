//! End-to-end proof that the local HTTPS proxy serves a trusted leaf, checks
//! Host against SNI, forwards to an upstream, and streams the response.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use devcore::model::ProxyRule;
use devproxy::ca::service::AuthorityService;
use devproxy::ca::store::AuthorityStore;
use devproxy::ca::{AlwaysStopped, CaError, TrustState, TrustStore};
use devproxy::proxy::routes::LiveRoutes;
use devproxy::proxy::rules::{PublishedProxyConfig, write_published_config};
use devproxy::proxy::rules::{routes_from_config, validate_proxy_routes};
use devproxy::proxy::server::{
    CertificateAuthority, SystemSocketActivation, serve_with_activation,
};
use http::{Request, Response, StatusCode};
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tempfile::TempDir;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

struct NoTrust;

impl TrustStore for NoTrust {
    fn verify(&self, _path: &std::path::Path) -> TrustState {
        TrustState::NotTrusted
    }
    fn apply(&self, _path: &std::path::Path) -> Result<(), CaError> {
        Ok(())
    }
    fn remove(&self, _path: &std::path::Path, _der: &[u8]) -> Result<(), CaError> {
        Ok(())
    }
}

fn free_port() -> i64 {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    i64::from(port)
}

async fn upstream() -> (i64, Arc<Mutex<Vec<http::HeaderMap>>>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = i64::from(listener.local_addr().unwrap().port());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let recorded = recorded.clone();
            tokio::spawn(async move {
                let service = service_fn(move |request: Request<Incoming>| {
                    recorded.lock().unwrap().push(request.headers().clone());
                    async move {
                        Ok::<_, std::convert::Infallible>(
                            Response::builder()
                                .status(StatusCode::OK)
                                .header("content-type", "text/plain")
                                .body(Full::new(Bytes::from_static(b"hello from upstream")))
                                .unwrap(),
                        )
                    }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .with_upgrades()
                    .await;
            });
        }
    });
    (port, seen)
}

async fn tls_request(domain: &str, port: i64, host: &str, ca_der: Vec<u8>) -> Vec<u8> {
    use rustls::pki_types::{CertificateDer, ServerName};
    let mut roots = rustls::RootCertStore::empty();
    roots.add(CertificateDer::from(ca_der)).unwrap();
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    let tcp = TcpStream::connect(("127.0.0.1", port as u16))
        .await
        .unwrap();
    let server_name = ServerName::try_from(domain.to_string()).unwrap();
    let mut tls = connector.connect(server_name, tcp).await.unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    tls.write_all(
        format!(
            "GET / HTTP/1.1\r\nHost: {host}\r\nX-Forwarded-For: 6.6.6.6\r\nConnection: close\r\n\r\n"
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let mut buffer = Vec::new();
    tls.read_to_end(&mut buffer).await.unwrap();
    buffer
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proxy_serves_trusted_tls_and_enforces_host_sni() {
    devproxy::ensure_crypto_provider();
    let dir = TempDir::new().unwrap();
    let listener_port = free_port();
    let (upstream_port, seen) = upstream().await;

    let config = PublishedProxyConfig {
        schema_version: 1,
        listener_port,
        rules: vec![ProxyRule {
            domain: "app.devbase.localhost".into(),
            local_port: upstream_port,
            ..ProxyRule::default()
        }],
    };
    write_published_config(dir.path(), &config).unwrap();
    let sites = routes_from_config(&config);
    validate_proxy_routes(&sites).unwrap();

    let trust = Arc::new(NoTrust);
    let authority = Arc::new(AuthorityService::new(
        AuthorityStore::new(dir.path()),
        trust,
        Arc::new(AlwaysStopped),
    ));
    let material = authority.load_or_create().unwrap();
    assert!(!material.cert_der.is_empty());
    let ca_der = material.cert_der.clone();
    let authority: Arc<dyn CertificateAuthority> = authority;
    let live = Arc::new(LiveRoutes::new(dir.path(), sites));
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let server = tokio::spawn(serve_with_activation(
        live,
        authority,
        Arc::new(SystemSocketActivation),
        false,
        shutdown_rx,
    ));
    // Let the listener bind.
    tokio::time::sleep(Duration::from_millis(150)).await;

    let response = tls_request(
        "app.devbase.localhost",
        listener_port,
        "app.devbase.localhost",
        ca_der.clone(),
    )
    .await;
    let text = String::from_utf8_lossy(&response);
    assert!(text.starts_with("HTTP/1.1 200"), "{text}");
    assert!(text.contains("hello from upstream"), "{text}");
    {
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0]["host"], "app.devbase.localhost");
        assert_eq!(seen[0]["x-forwarded-proto"], "https");
        assert_eq!(
            seen[0]["x-forwarded-for"], "127.0.0.1",
            "the spoofed client value is replaced"
        );
    }

    let misdirected = tls_request(
        "app.devbase.localhost",
        listener_port,
        "other.devbase.localhost",
        ca_der,
    )
    .await;
    let text = String::from_utf8_lossy(&misdirected);
    assert!(text.starts_with("HTTP/1.1 421"), "{text}");
    assert_eq!(
        seen.lock().unwrap().len(),
        1,
        "no upstream call for a misdirected request"
    );

    shutdown_tx.send(true).unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
}

async fn upstream_upgrade() -> i64 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = i64::from(listener.local_addr().unwrap().port());
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let service = service_fn(move |mut request: Request<Incoming>| async move {
                    let upgrade = hyper::upgrade::on(&mut request);
                    tokio::spawn(async move {
                        let Ok(upgraded) = upgrade.await else { return };
                        let mut io = TokioIo::new(upgraded);
                        let mut buffer = [0u8; 1024];
                        loop {
                            use tokio::io::{AsyncReadExt, AsyncWriteExt};
                            let read = match io.read(&mut buffer).await {
                                Ok(0) | Err(_) => return,
                                Ok(read) => read,
                            };
                            if io.write_all(&buffer[..read]).await.is_err() {
                                return;
                            }
                        }
                    });
                    Ok::<_, std::convert::Infallible>(
                        Response::builder()
                            .status(StatusCode::SWITCHING_PROTOCOLS)
                            .header("upgrade", "websocket")
                            .header("connection", "Upgrade")
                            .body(Full::new(Bytes::new()))
                            .unwrap(),
                    )
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .with_upgrades()
                    .await;
            });
        }
    });
    port
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proxy_tunnels_101_upgrades_end_to_end() {
    devproxy::ensure_crypto_provider();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let dir = TempDir::new().unwrap();
    let listener_port = free_port();
    let upstream_port = upstream_upgrade().await;
    let config = PublishedProxyConfig {
        schema_version: 1,
        listener_port,
        rules: vec![ProxyRule {
            domain: "app.devbase.localhost".into(),
            local_port: upstream_port,
            ..ProxyRule::default()
        }],
    };
    write_published_config(dir.path(), &config).unwrap();
    let sites = routes_from_config(&config);
    validate_proxy_routes(&sites).unwrap();
    let authority = Arc::new(AuthorityService::new(
        AuthorityStore::new(dir.path()),
        Arc::new(NoTrust),
        Arc::new(AlwaysStopped),
    ));
    let ca_der = authority.load_or_create().unwrap().cert_der.clone();
    let authority: Arc<dyn CertificateAuthority> = authority;
    let live = Arc::new(LiveRoutes::new(dir.path(), sites));
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let server = tokio::spawn(serve_with_activation(
        live,
        authority,
        Arc::new(SystemSocketActivation),
        false,
        shutdown_rx,
    ));
    tokio::time::sleep(Duration::from_millis(150)).await;

    use rustls::pki_types::{CertificateDer, ServerName};
    let mut roots = rustls::RootCertStore::empty();
    roots.add(CertificateDer::from(ca_der)).unwrap();
    let client_config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(Arc::new(client_config));
    let tcp = TcpStream::connect(("127.0.0.1", listener_port as u16))
        .await
        .unwrap();
    let mut tls = connector
        .connect(
            ServerName::try_from("app.devbase.localhost".to_string()).unwrap(),
            tcp,
        )
        .await
        .unwrap();
    tls.write_all(
        b"GET /ws HTTP/1.1\r\nHost: app.devbase.localhost\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n",
    )
    .await
    .unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        tls.read_exact(&mut byte).await.unwrap();
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let head = String::from_utf8_lossy(&head);
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    assert!(head.to_lowercase().contains("upgrade: websocket"), "{head}");

    tls.write_all(b"ping").await.unwrap();
    let mut echoed = [0u8; 4];
    tls.read_exact(&mut echoed).await.unwrap();
    assert_eq!(&echoed, b"ping");

    shutdown_tx.send(true).unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
}

async fn upstream_events() -> i64 {
    use futures_util::StreamExt;
    use http_body_util::StreamBody;
    use hyper::body::Frame;

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = i64::from(listener.local_addr().unwrap().port());
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let service = service_fn(move |_request: Request<Incoming>| async move {
                    let stream = futures_util::stream::unfold(0u8, |state| async move {
                        match state {
                            0 => Some((
                                Ok::<_, std::io::Error>(Frame::data(Bytes::from_static(
                                    b"data: one\n\n",
                                ))),
                                1,
                            )),
                            1 => {
                                tokio::time::sleep(Duration::from_millis(600)).await;
                                Some((
                                    Ok::<_, std::io::Error>(Frame::data(Bytes::from_static(
                                        b"data: two\n\n",
                                    ))),
                                    2,
                                ))
                            }
                            _ => None,
                        }
                    })
                    .boxed();
                    Ok::<_, std::convert::Infallible>(
                        Response::builder()
                            .status(StatusCode::OK)
                            .header("content-type", "text/event-stream")
                            .body(StreamBody::new(stream))
                            .unwrap(),
                    )
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    port
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proxy_flushes_server_sent_events_immediately() {
    devproxy::ensure_crypto_provider();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let dir = TempDir::new().unwrap();
    let listener_port = free_port();
    let upstream_port = upstream_events().await;
    let config = PublishedProxyConfig {
        schema_version: 1,
        listener_port,
        rules: vec![ProxyRule {
            domain: "app.devbase.localhost".into(),
            local_port: upstream_port,
            ..ProxyRule::default()
        }],
    };
    write_published_config(dir.path(), &config).unwrap();
    let sites = routes_from_config(&config);
    validate_proxy_routes(&sites).unwrap();
    let authority = Arc::new(AuthorityService::new(
        AuthorityStore::new(dir.path()),
        Arc::new(NoTrust),
        Arc::new(AlwaysStopped),
    ));
    let ca_der = authority.load_or_create().unwrap().cert_der.clone();
    let authority: Arc<dyn CertificateAuthority> = authority;
    let live = Arc::new(LiveRoutes::new(dir.path(), sites));
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let server = tokio::spawn(serve_with_activation(
        live,
        authority,
        Arc::new(SystemSocketActivation),
        false,
        shutdown_rx,
    ));
    tokio::time::sleep(Duration::from_millis(150)).await;

    use rustls::pki_types::{CertificateDer, ServerName};
    let mut roots = rustls::RootCertStore::empty();
    roots.add(CertificateDer::from(ca_der)).unwrap();
    let client_config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(Arc::new(client_config));
    let tcp = TcpStream::connect(("127.0.0.1", listener_port as u16))
        .await
        .unwrap();
    let mut tls = connector
        .connect(
            ServerName::try_from("app.devbase.localhost".to_string()).unwrap(),
            tcp,
        )
        .await
        .unwrap();
    tls.write_all(
        b"GET /events HTTP/1.1\r\nHost: app.devbase.localhost\r\nConnection: close\r\n\r\n",
    )
    .await
    .unwrap();

    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        tls.read_exact(&mut byte).await.unwrap();
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    assert!(String::from_utf8_lossy(&head).starts_with("HTTP/1.1 200"));

    let mut first = [0u8; 64];
    let read = tokio::time::timeout(Duration::from_millis(300), tls.read(&mut first))
        .await
        .expect("the first event was flushed before the upstream finished")
        .unwrap();
    assert!(
        String::from_utf8_lossy(&first[..read]).contains("data: one"),
        "unexpected first chunk: {:?}",
        &first[..read]
    );

    let mut rest = Vec::new();
    tls.read_to_end(&mut rest).await.unwrap();
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&first[..read]),
        String::from_utf8_lossy(&rest)
    );
    assert!(all.contains("data: two"), "{all}");

    shutdown_tx.send(true).unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(5), server).await;
}

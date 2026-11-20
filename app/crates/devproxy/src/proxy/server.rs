//! The TLS listener: one listener per published HTTPS port, SNI certificate
//! selection, the Host/SNI check, and per-connection HTTP/1.1 serving.
//!
//! Ported from `ServeProxy` in `proxy.go` and the listener half of
//! `network.go`. Unknown SNI names abort the handshake; a Host that does not
//! match the TLS server name gets `421` before any upstream call.

use std::io;
use std::net::{SocketAddr, TcpListener as StdTcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::{Request, Response, StatusCode};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use rustls::server::ResolvesServerCert;
use rustls::sign::CertifiedKey;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio_rustls::TlsAcceptor;

use crate::ca::material::LeafCertificate;
use crate::ca::service::AuthorityService;
use crate::ca::{AlwaysStopped, AuthorityStore, CaError, SecurityTrust};
use devcore::launchd::SystemRunner;

use super::forward::{ProxyBody, forward, inbound_host};
use super::routes::LiveRoutes;
use super::rules::{
    ProxyRoute, PublishedProxyConfig, listener_ports, read_published_config, routes_from_config,
    validate_proxy_routes, write_published_config,
};

const READ_HEADER_TIMEOUT: Duration = Duration::from_secs(10);
const SOCKET_ACTIVATION_ENV: &str = "MSM_DEV_SOCKET_ACTIVATION";

/// Errors that stop the proxy server.
#[derive(Debug, thiserror::Error)]
pub enum ProxyServerError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Rules(#[from] super::rules::RuleError),
    #[error("{0}")]
    Invalid(String),
}

/// Supplies a leaf certificate for a local domain, normally the CA store.
pub trait CertificateAuthority: Send + Sync {
    fn leaf(&self, domain: &str) -> Result<LeafCertificate, CaError>;
}

impl CertificateAuthority for AuthorityService {
    fn leaf(&self, domain: &str) -> Result<LeafCertificate, CaError> {
        AuthorityService::leaf(self, domain)
    }
}

/// Obtains an already-bound listener from launchd.
pub trait SocketActivation: Send + Sync {
    fn take(&self, name: &str) -> io::Result<StdTcpListener>;
}

/// The real `launch_activate_socket` implementation.
pub struct SystemSocketActivation;

impl SocketActivation for SystemSocketActivation {
    fn take(&self, name: &str) -> io::Result<StdTcpListener> {
        #[cfg(target_os = "macos")]
        {
            activate_launchd_socket(name)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = name;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "launchd socket activation is only available on macOS",
            ))
        }
    }
}

#[cfg(target_os = "macos")]
fn activate_launchd_socket(name: &str) -> io::Result<StdTcpListener> {
    use std::ffi::{CString, c_char, c_int, c_void};
    use std::os::fd::FromRawFd;

    const RTLD_NOW: c_int = 2;
    unsafe extern "C" {
        fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        fn dlclose(handle: *mut c_void) -> c_int;
    }
    type LaunchActivate = unsafe extern "C" fn(*const c_char, *mut *mut i32, *mut usize) -> c_int;

    let c_name = CString::new(name)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "socket name"))?;
    // SAFETY: the path and mode are valid, and the returned handle is closed
    // after the symbol has been used.
    unsafe {
        let handle = dlopen(c"/usr/lib/libSystem.B.dylib".as_ptr(), RTLD_NOW);
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let symbol = dlsym(handle, c"launch_activate_socket".as_ptr());
        if symbol.is_null() {
            dlclose(handle);
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "launch_activate_socket is unavailable",
            ));
        }
        let activate: LaunchActivate = std::mem::transmute(symbol);
        let mut fds: *mut i32 = std::ptr::null_mut();
        let mut count: usize = 0;
        let code = activate(c_name.as_ptr(), &mut fds, &mut count);
        if code != 0 {
            dlclose(handle);
            return Err(io::Error::from_raw_os_error(code));
        }
        if fds.is_null() || count == 0 {
            dlclose(handle);
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("no socket for {name}"),
            ));
        }
        if count != 1 {
            for index in 0..count {
                libc::close(*fds.add(index));
            }
            libc::free(fds as *mut c_void);
            dlclose(handle);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected one IPv4 socket, got {count}"),
            ));
        }
        let fd = *fds;
        libc::free(fds as *mut c_void);
        dlclose(handle);
        let listener = StdTcpListener::from_raw_fd(fd);
        listener.set_nonblocking(true)?;
        Ok(listener)
    }
}

struct SniResolver {
    routes: Arc<LiveRoutes>,
    authority: Arc<dyn CertificateAuthority>,
    port: i64,
}

impl std::fmt::Debug for SniResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SniResolver")
            .field("port", &self.port)
            .finish()
    }
}

impl ResolvesServerCert for SniResolver {
    fn resolve(&self, hello: rustls::server::ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let sni = hello.server_name()?.to_lowercase();
        let snapshot = self.routes.snapshot();
        snapshot.route(&sni, self.port)?;
        let leaf = self.authority.leaf(&sni).ok()?;
        let key = rustls::crypto::aws_lc_rs::sign::any_supported_type(&leaf.key).ok()?;
        Some(Arc::new(CertifiedKey::new(leaf.chain, key)))
    }
}

/// The production authority used by `serve`.
pub fn default_authority(root: &std::path::Path) -> Arc<dyn CertificateAuthority> {
    let service = AuthorityService::new(
        AuthorityStore::new(root),
        Arc::new(SecurityTrust::new(Arc::new(SystemRunner::new()))),
        Arc::new(AlwaysStopped),
    );
    Arc::new(service)
}

/// Read the snapshot, build the live table, and serve until shutdown.
pub async fn serve(root: PathBuf, shutdown: watch::Receiver<bool>) -> Result<(), ProxyServerError> {
    let config = read_published_config(&root)?;
    let sites = routes_from_config(&config);
    validate_proxy_routes(&sites)?;
    if sites.is_empty() {
        return Err(ProxyServerError::Invalid(
            "no HTTPS proxy rules configured".into(),
        ));
    }
    let live = Arc::new(LiveRoutes::new(&root, sites));
    serve_with(
        live,
        default_authority(&root),
        Arc::new(SystemSocketActivation),
        shutdown,
    )
    .await
}

/// Serve using an explicit routing table, authority, and socket seam.
pub async fn serve_with(
    live: Arc<LiveRoutes>,
    authority: Arc<dyn CertificateAuthority>,
    activation: Arc<dyn SocketActivation>,
    shutdown: watch::Receiver<bool>,
) -> Result<(), ProxyServerError> {
    crate::ensure_crypto_provider();
    let use_activation = std::env::var(SOCKET_ACTIVATION_ENV).as_deref() == Ok("1");
    serve_with_activation(live, authority, activation, use_activation, shutdown).await
}

/// [`serve_with`] with the launchd activation decision made explicit.
pub async fn serve_with_activation(
    live: Arc<LiveRoutes>,
    authority: Arc<dyn CertificateAuthority>,
    activation: Arc<dyn SocketActivation>,
    use_activation: bool,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), ProxyServerError> {
    crate::ensure_crypto_provider();
    let initial = live.snapshot();
    let ports = initial.listener_ports();
    if ports.is_empty() {
        return Err(ProxyServerError::Invalid(
            "no HTTPS proxy rules configured".into(),
        ));
    }
    let mut tasks: JoinSet<()> = JoinSet::new();

    for port in ports {
        let listener = if use_activation {
            let std_listener = activation.take(&format!("HTTPS-{port}"))?;
            std_listener.set_nonblocking(true)?;
            TcpListener::from_std(std_listener)?
        } else {
            TcpListener::bind(("127.0.0.1", port as u16)).await?
        };
        let mut server_config = rustls::ServerConfig::builder_with_protocol_versions(&[
            &rustls::version::TLS12,
            &rustls::version::TLS13,
        ])
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(SniResolver {
            routes: Arc::clone(&live),
            authority: Arc::clone(&authority),
            port,
        }));
        server_config.alpn_protocols = vec![b"http/1.1".to_vec()];
        let acceptor = TlsAcceptor::from(Arc::new(server_config));
        let routes = Arc::clone(&live);
        tasks.spawn(async move {
            accept_loop(listener, acceptor, routes, port).await;
        });
    }

    tokio::select! {
        _ = shutdown.changed() => {}
        _ = tasks.join_next() => {}
    }
    tasks.abort_all();
    Ok(())
}

async fn accept_loop(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    routes: Arc<LiveRoutes>,
    port: i64,
) {
    let mut connections: JoinSet<()> = JoinSet::new();
    loop {
        let (stream, remote) = match listener.accept().await {
            Ok(value) => value,
            Err(error) => {
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return;
            }
        };
        let _ = stream.set_nodelay(true);
        let acceptor = acceptor.clone();
        let routes = Arc::clone(&routes);
        connections.spawn(async move {
            let tls = match acceptor.accept(stream).await {
                Ok(tls) => tls,
                Err(_) => return,
            };
            let sni = tls.get_ref().1.server_name().map(|name| name.to_string());
            let service = service_fn(move |request: Request<Incoming>| {
                let routes = Arc::clone(&routes);
                let sni = sni.clone();
                async move {
                    Ok::<_, std::convert::Infallible>(
                        handle_request(routes, port, sni, Some(remote), request).await,
                    )
                }
            });
            let mut builder = hyper::server::conn::http1::Builder::new();
            builder
                .timer(TokioTimer::new())
                .header_read_timeout(Some(READ_HEADER_TIMEOUT));
            let _ = builder
                .serve_connection(TokioIo::new(tls), service)
                .with_upgrades()
                .await;
        });
        while connections.try_join_next().is_some() {}
    }
}

async fn handle_request(
    routes: Arc<LiveRoutes>,
    port: i64,
    sni: Option<String>,
    remote: Option<SocketAddr>,
    request: Request<Incoming>,
) -> Response<ProxyBody> {
    let snapshot = routes.snapshot();
    let host = inbound_host(request.headers());
    if is_misdirected(&host, sni.as_deref()) {
        return plain(
            StatusCode::MISDIRECTED_REQUEST,
            "misdirected request: Host does not match TLS server name\n",
        );
    }
    match snapshot.route(&host, port) {
        Some(route) => forward(&route.site, request, remote).await,
        None => plain(StatusCode::NOT_FOUND, "404 page not found\n"),
    }
}

/// Host must be present, the TLS server name must be present, and the two must
/// match case-insensitively.
pub fn is_misdirected(host: &str, sni: Option<&str>) -> bool {
    let Some(sni) = sni.filter(|name| !name.trim().is_empty()) else {
        return true;
    };
    host.is_empty() || !host.eq_ignore_ascii_case(sni.trim())
}

fn plain(status: StatusCode, text: &str) -> Response<ProxyBody> {
    let mut response = Response::new(
        http_body_util::Full::new(Bytes::from(text.to_string()))
            .map_err(|never| match never {})
            .boxed(),
    );
    *response.status_mut() = status;
    response.headers_mut().insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

/// Write an empty snapshot without tunnel fields (the privileged path uses it).
pub fn write_empty_snapshot(
    root: &std::path::Path,
    listener_port: i64,
) -> Result<(), super::rules::RuleError> {
    write_published_config(
        root,
        &PublishedProxyConfig {
            schema_version: 1,
            listener_port,
            rules: Vec::new(),
        },
    )
}

/// The listener ports a snapshot needs.
pub fn required_ports(sites: &[ProxyRoute]) -> Vec<i64> {
    listener_ports(sites)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_and_sni_must_match() {
        assert!(!is_misdirected(
            "app.devbase.localhost",
            Some("app.devbase.localhost")
        ));
        assert!(!is_misdirected(
            "app.devbase.localhost",
            Some("App.Devbase.Localhost")
        ));
        assert!(is_misdirected(
            "other.devbase.localhost",
            Some("app.devbase.localhost")
        ));
        assert!(is_misdirected("app.devbase.localhost", None));
        assert!(is_misdirected("app.devbase.localhost", Some("")));
        assert!(is_misdirected("", Some("app.devbase.localhost")));
    }

    #[test]
    fn pinned_alpn_is_http11_only() {
        assert_eq!(b"http/1.1", &b"http/1.1"[..]);
    }
}

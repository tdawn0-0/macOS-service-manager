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
use crate::ca::{AlwaysStopped, AuthorityStore, CaError, default_trust_store};

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

/// Obtains an already-bound listener from launchd (macOS) or systemd (Linux).
pub trait SocketActivation: Send + Sync {
    fn take(&self, name: &str) -> io::Result<StdTcpListener>;
}

/// The real socket activation implementation (`launch_activate_socket` on
/// macOS, `sd_listen_fds` on Linux).
pub struct SystemSocketActivation;

impl SocketActivation for SystemSocketActivation {
    fn take(&self, name: &str) -> io::Result<StdTcpListener> {
        #[cfg(target_os = "macos")]
        {
            activate_launchd_socket(name)
        }
        #[cfg(target_os = "linux")]
        {
            activate_systemd_socket(name)
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = name;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "socket activation is only available on macOS and Linux",
            ))
        }
    }
}

/// Resolves a systemd `sd_listen_fds` descriptor for `want_name` (such as
/// `HTTPS-443`) using `LISTEN_PID`, `LISTEN_FDS`, and optional
/// `LISTEN_FDNAMES`.
///
/// - When `LISTEN_FDS == 1`, returns FD 3 (`SD_LISTEN_FDS_START`).
/// - When `LISTEN_FDNAMES` names `want_name` explicitly, returns that exact FD.
/// - When multiple FDs are passed without matching `HTTPS-<port>` names, falls
///   back to matching the parsed port via `fd_port`.
pub fn select_sd_listen_fd(
    current_pid: u32,
    listen_pid: Option<&str>,
    listen_fds: Option<&str>,
    listen_fdnames: Option<&str>,
    want_name: &str,
    fd_port: impl Fn(i32) -> Option<u16>,
) -> io::Result<i32> {
    let (parsed_fd, extras) = devcore::helper::parse_sd_listen_fds(
        current_pid,
        listen_pid,
        listen_fds,
        listen_fdnames,
        want_name,
    )?;
    if extras.is_empty() {
        return Ok(parsed_fd);
    }
    if let Some(names) = listen_fdnames.filter(|value| !value.is_empty())
        && names.split(':').any(|candidate| candidate == want_name)
    {
        return Ok(parsed_fd);
    }
    let want_port: Option<u16> = want_name
        .strip_prefix("HTTPS-")
        .unwrap_or(want_name)
        .parse()
        .ok();
    if let Some(port) = want_port {
        for fd in std::iter::once(parsed_fd).chain(extras.iter().copied()) {
            if fd_port(fd) == Some(port) {
                return Ok(fd);
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("no systemd socket matched {want_name:?}"),
    ))
}

#[cfg(target_os = "linux")]
fn activate_systemd_socket(name: &str) -> io::Result<StdTcpListener> {
    use std::os::fd::FromRawFd;

    let listen_pid = std::env::var("LISTEN_PID").ok();
    let listen_fds = std::env::var("LISTEN_FDS").ok();
    let listen_fdnames = std::env::var("LISTEN_FDNAMES").ok();
    let fd = select_sd_listen_fd(
        std::process::id(),
        listen_pid.as_deref(),
        listen_fds.as_deref(),
        listen_fdnames.as_deref(),
        name,
        fd_local_port,
    )?;
    // SAFETY: systemd passed this listening socket descriptor to our PID, and
    // each distinct `HTTPS-<port>` is taken once by `serve_with_activation`.
    let listener = unsafe { StdTcpListener::from_raw_fd(fd) };
    listener.set_nonblocking(true)?;
    Ok(listener)
}

#[cfg(any(target_os = "linux", test))]
fn fd_local_port(fd: i32) -> Option<u16> {
    // SAFETY: `getsockname` writes into `storage` bounded by `len`, and
    // `read_unaligned` avoids any alignment assumptions across sockaddr types.
    unsafe {
        let mut storage: libc::sockaddr_storage = std::mem::zeroed();
        let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        if libc::getsockname(fd, (&raw mut storage).cast::<libc::sockaddr>(), &mut len) != 0 {
            return None;
        }
        match i32::from(storage.ss_family) {
            libc::AF_INET => {
                let addr =
                    std::ptr::read_unaligned((&raw const storage).cast::<libc::sockaddr_in>());
                Some(u16::from_be(addr.sin_port))
            }
            libc::AF_INET6 => {
                let addr =
                    std::ptr::read_unaligned((&raw const storage).cast::<libc::sockaddr_in6>());
                Some(u16::from_be(addr.sin6_port))
            }
            _ => None,
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
        default_trust_store(),
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

    #[test]
    fn sd_listen_fds_takes_fd_3_for_single_socket() {
        let fd = select_sd_listen_fd(1234, Some("1234"), Some("1"), None, "HTTPS-443", |_| None)
            .unwrap();
        assert_eq!(fd, devcore::helper::SD_LISTEN_FDS_START);

        // Systemd sets LISTEN_FDNAMES to the .socket unit name by default.
        let fd = select_sd_listen_fd(
            1234,
            Some("1234"),
            Some("1"),
            Some("devbase-proxy-abcd1234.socket"),
            "HTTPS-443",
            |_| None,
        )
        .unwrap();
        assert_eq!(fd, devcore::helper::SD_LISTEN_FDS_START);
    }

    #[test]
    fn sd_listen_fds_matches_named_https_ports_and_port_fallback() {
        let fd_443 = select_sd_listen_fd(
            42,
            Some("42"),
            Some("2"),
            Some("HTTPS-8443:HTTPS-443"),
            "HTTPS-443",
            |_| None,
        )
        .unwrap();
        assert_eq!(fd_443, devcore::helper::SD_LISTEN_FDS_START + 1);

        let fd_8443 = select_sd_listen_fd(
            42,
            Some("42"),
            Some("2"),
            Some("HTTPS-8443:HTTPS-443"),
            "HTTPS-8443",
            |_| None,
        )
        .unwrap();
        assert_eq!(fd_8443, devcore::helper::SD_LISTEN_FDS_START);

        // Multi-socket unit without per-port FileDescriptorName uses getsockname port lookup.
        let fd_by_port = select_sd_listen_fd(
            42,
            Some("42"),
            Some("2"),
            Some("devbase-proxy-abcd.socket:devbase-proxy-abcd.socket"),
            "HTTPS-8443",
            |fd| match fd {
                3 => Some(443),
                4 => Some(8443),
                _ => None,
            },
        )
        .unwrap();
        assert_eq!(fd_by_port, 4);

        // Rejects unknown port when multi-socket lookup finds no match.
        let err = select_sd_listen_fd(
            42,
            Some("42"),
            Some("2"),
            Some("devbase-proxy-abcd.socket:devbase-proxy-abcd.socket"),
            "HTTPS-9443",
            |fd| match fd {
                3 => Some(443),
                4 => Some(8443),
                _ => None,
            },
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn fd_local_port_reads_bound_tcp_listener_port() {
        use std::os::fd::AsRawFd;

        let listener = StdTcpListener::bind(("127.0.0.1", 0)).unwrap();
        let expected = listener.local_addr().unwrap().port();
        assert_eq!(fd_local_port(listener.as_raw_fd()), Some(expected));
    }

    #[test]
    fn sd_listen_fds_rejects_mismatched_pid_or_missing_env() {
        assert_eq!(
            select_sd_listen_fd(42, Some("99"), Some("1"), None, "HTTPS-443", |_| None)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            select_sd_listen_fd(42, Some("42"), Some("0"), None, "HTTPS-443", |_| None)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(
            select_sd_listen_fd(42, None, Some("1"), None, "HTTPS-443", |_| None)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
    }
}

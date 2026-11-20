//! Launchd socket activation and the helper's `--dev helper` entry point.
//!
//! `launch_activate_socket` is resolved with `dlopen`/`dlsym`, following the
//! same pattern as `devproxy`'s socket activation. The returned descriptor is
//! wrapped in a [`UnixListener`]; the socket path is never unlinked, so launchd
//! can start the helper again after it exits on idle.

use std::fmt;
use std::io;
use std::os::unix::net::UnixListener;
use std::sync::Arc;

use super::client::Client;
use super::server::{Server, ServerConfig};
use super::{HelperError, SOCKET_NAME};

/// Obtains an already-bound Unix listener from launchd.
pub trait SocketActivator: Send + Sync + fmt::Debug {
    fn take(&self, name: &str) -> io::Result<UnixListener>;
}

/// The real `launch_activate_socket` implementation.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemSocketActivator;

impl SocketActivator for SystemSocketActivator {
    fn take(&self, name: &str) -> io::Result<UnixListener> {
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
fn activate_launchd_socket(name: &str) -> io::Result<UnixListener> {
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
    // SAFETY: the path and mode are valid, and every descriptor is owned by the
    // listener after it is taken.
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
        let activate = std::mem::transmute::<*mut c_void, LaunchActivate>(symbol);
        let mut fds: *mut i32 = std::ptr::null_mut();
        let mut count: usize = 0;
        let code = activate(c_name.as_ptr(), &mut fds, &mut count);
        if code != 0 {
            dlclose(handle);
            return Err(io::Error::from_raw_os_error(code));
        }
        if fds.is_null() || count == 0 {
            if !fds.is_null() {
                libc::free(fds.cast());
            }
            dlclose(handle);
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("launch_activate_socket({name:?}) returned no file descriptors"),
            ));
        }
        for index in 1..count {
            libc::close(*fds.add(index));
        }
        let fd = *fds;
        libc::free(fds.cast());
        dlclose(handle);
        Ok(UnixListener::from_raw_fd(fd))
    }
}

/// The `--dev helper --owner-uid N` mode: claim the launchd socket and serve
/// until idle.
pub fn run_activated(owner_uid: i64) -> Result<(), HelperError> {
    serve_activated(owner_uid)
}

/// Claims the helper socket from launchd and serves until the idle timeout.
pub fn serve_activated(owner_uid: i64) -> Result<(), HelperError> {
    serve_activated_with(owner_uid, &SystemSocketActivator, ServerConfig::default())
}

/// The injectable form of [`serve_activated`], used by tests.
pub fn serve_activated_with(
    owner_uid: i64,
    activator: &dyn SocketActivator,
    mut config: ServerConfig,
) -> Result<(), HelperError> {
    if owner_uid <= 0 {
        return Err(HelperError::Message(format!(
            "invalid owner uid {owner_uid}"
        )));
    }
    let listener = activator
        .take(SOCKET_NAME)
        .map_err(|error| HelperError::Message(error.to_string()))?;
    config.owner_uid = owner_uid;
    let server = Arc::new(Server::new(config));
    server
        .serve(listener)
        .map_err(|error| HelperError::Message(error.to_string()))
}

/// Parses the `--owner-uid N` pair from the helper's arguments. `--owner-uid`
/// may appear anywhere, matching `runHelper` in `cli.go`.
pub fn parse_owner_uid(args: &[String]) -> Result<i64, HelperError> {
    let mut owner_uid = -1;
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--owner-uid" && index + 1 < args.len() {
            let value = &args[index + 1];
            owner_uid = value.parse::<i64>().map_err(|_| {
                HelperError::Message(format!("invalid --owner-uid {value:?}: invalid syntax"))
            })?;
            index += 1;
        }
        index += 1;
    }
    Ok(owner_uid)
}

/// A convenience for callers that only need the default client.
pub fn require_client() -> Result<Arc<Client>, HelperError> {
    Client::require()
}

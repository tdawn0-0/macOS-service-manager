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

/// The first file descriptor passed by systemd socket activation (`SD_LISTEN_FDS_START`).
pub const SD_LISTEN_FDS_START: i32 = 3;

/// Obtains an already-bound Unix listener from launchd (macOS) or systemd (Linux).
pub trait SocketActivator: Send + Sync + fmt::Debug {
    fn take(&self, name: &str) -> io::Result<UnixListener>;
}

/// Linux systemd `sd_listen_fds` socket activator (`LISTEN_PID` + `LISTEN_FDS`, FD 3).
#[derive(Debug, Clone, Copy, Default)]
pub struct LinuxSocketActivator;

impl SocketActivator for LinuxSocketActivator {
    fn take(&self, name: &str) -> io::Result<UnixListener> {
        activate_systemd_socket(name)
    }
}

/// Parses systemd `LISTEN_PID`, `LISTEN_FDS`, and optional `LISTEN_FDNAMES`
/// variables and returns `(selected_fd, extra_fds_to_close)`.
pub fn parse_sd_listen_fds(
    current_pid: u32,
    listen_pid: Option<&str>,
    listen_fds: Option<&str>,
    listen_fdnames: Option<&str>,
    want_name: &str,
) -> io::Result<(i32, Vec<i32>)> {
    let pid_text = listen_pid
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "LISTEN_PID is not set"))?;
    let expected_pid: u32 = pid_text
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid LISTEN_PID"))?;
    if expected_pid != current_pid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("LISTEN_PID {expected_pid} does not match current PID {current_pid}"),
        ));
    }

    let fds_text = listen_fds
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "LISTEN_FDS is not set"))?;
    let count: i32 = fds_text
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid LISTEN_FDS"))?;
    if count <= 0 {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "LISTEN_FDS reports no file descriptors",
        ));
    }

    let selected_index = if let Some(names) = listen_fdnames.filter(|n| !n.is_empty()) {
        let name_list: Vec<&str> = names.split(':').collect();
        name_list
            .iter()
            .position(|candidate| *candidate == want_name)
            .map(|idx| idx as i32)
            .unwrap_or(0)
    } else {
        0
    };
    if selected_index >= count {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no systemd socket matched {want_name:?}"),
        ));
    }
    let selected_fd = SD_LISTEN_FDS_START + selected_index;
    let extra: Vec<i32> = (0..count)
        .map(|i| SD_LISTEN_FDS_START + i)
        .filter(|fd| *fd != selected_fd)
        .collect();
    Ok((selected_fd, extra))
}

fn activate_systemd_socket(name: &str) -> io::Result<UnixListener> {
    use std::os::fd::FromRawFd;

    let listen_pid = std::env::var("LISTEN_PID").ok();
    let listen_fds = std::env::var("LISTEN_FDS").ok();
    let listen_fdnames = std::env::var("LISTEN_FDNAMES").ok();
    let (fd, extras) = parse_sd_listen_fds(
        std::process::id(),
        listen_pid.as_deref(),
        listen_fds.as_deref(),
        listen_fdnames.as_deref(),
        name,
    )?;
    // SAFETY: systemd passed these descriptors to our PID; extras are closed
    // and `fd` is transferred exclusively to `UnixListener`.
    unsafe {
        for extra in extras {
            libc::close(extra);
        }
        Ok(UnixListener::from_raw_fd(fd))
    }
}

/// The real platform socket activator (`launch_activate_socket` on macOS,
/// `sd_listen_fds` on Linux).
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemSocketActivator;

impl SocketActivator for SystemSocketActivator {
    fn take(&self, name: &str) -> io::Result<UnixListener> {
        #[cfg(target_os = "macos")]
        {
            activate_launchd_socket(name)
        }
        #[cfg(target_os = "linux")]
        {
            LinuxSocketActivator.take(name)
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = name;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "socket activation is not available on this platform",
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

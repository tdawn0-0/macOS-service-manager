//! An isolated in-process helper for tests.
//!
//! Ported from `testenv.go`: a short temporary Unix socket under `/tmp`, a
//! matching staged executable and helper plist, and a real [`Server`] on a
//! background thread. Nothing here touches launchd or asks for a password; the
//! client talks to the temp socket only.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;

use super::LABEL;
use super::client::{BinaryInfo, Client, ClientConfig};
use super::plist_doc::build_helper_plist;
use super::server::{Server, ServerConfig};

/// A temporary directory removed on drop, without a test-only dependency.
#[derive(Debug)]
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new(prefix: &str) -> io::Result<TempDir> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir();
        for _ in 0..100 {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = base.join(format!("{prefix}-{}-{n}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(TempDir { path }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not create a temporary directory",
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A running in-process helper and its client.
pub struct TestHelper {
    client: Arc<Client>,
    owner_uid: u32,
    stop: Arc<AtomicBool>,
    server: Option<JoinHandle<io::Result<()>>>,
    socket_dir: TempDir,
    helper_dir: TempDir,
    plist_dir: TempDir,
}

impl TestHelper {
    /// Starts a helper for `config` and returns a client pointed at it.
    pub fn install(mut config: ServerConfig) -> TestHelper {
        let socket_dir = TempDir::new("devbase-ph").expect("create the socket directory");
        let socket_path = socket_dir.path().join("h.sock");
        let listener =
            std::os::unix::net::UnixListener::bind(&socket_path).expect("bind the helper socket");

        if config.owner_uid == 0 {
            // SAFETY: geteuid has no preconditions.
            config.owner_uid = i64::from(unsafe { libc::geteuid() });
        }
        let owner_uid = u32::try_from(config.owner_uid).unwrap_or(0);
        config.owner_uid = i64::from(owner_uid);

        let helper_dir = TempDir::new("devbase-ph-helper").expect("create the helper directory");
        let staged_exe = helper_dir.path().join("devbase-helper");
        std::fs::write(&staged_exe, b"bin").expect("stage a helper executable");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&staged_exe, std::fs::Permissions::from_mode(0o755))
                .expect("make the staged helper executable");
        }
        let plist_dir = TempDir::new("devbase-ph-plist").expect("create the plist directory");
        let plist_path = plist_dir.path().join(format!("{LABEL}.plist"));
        let plist = build_helper_plist(
            &staged_exe,
            owner_uid,
            socket_path.to_str().expect("socket path is UTF-8"),
        )
        .expect("build the helper plist");
        std::fs::write(&plist_path, plist).expect("write the helper plist");

        let server = Arc::new(Server::new(config));
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let handle = std::thread::spawn(move || server.serve_until(listener, stop_thread));

        let expected = BinaryInfo {
            executable: PathBuf::from("/Applications/Devbase.app/Contents/MacOS/Devbase"),
            bundle_path: None,
            owner_uid,
        };
        let client = Arc::new(Client::new(ClientConfig {
            socket_path: socket_path.clone(),
            plist_path,
            helper_dir: helper_dir.path().to_path_buf(),
            expected,
            ..ClientConfig::default()
        }));
        TestHelper {
            client,
            owner_uid,
            stop,
            server: Some(handle),
            socket_dir,
            helper_dir,
            plist_dir,
        }
    }

    pub fn client(&self) -> Arc<Client> {
        Arc::clone(&self.client)
    }

    pub fn owner_uid(&self) -> u32 {
        self.owner_uid
    }

    pub fn socket_path(&self) -> PathBuf {
        self.socket_dir.path().join("h.sock")
    }

    pub fn helper_dir(&self) -> &Path {
        self.helper_dir.path()
    }

    pub fn plist_path(&self) -> PathBuf {
        self.plist_dir.path().join(format!("{LABEL}.plist"))
    }

    /// Stops the server and waits for its thread, without dropping the rest.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.server.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for TestHelper {
    fn drop(&mut self) {
        self.stop();
    }
}

//! Fakes for the helper tests. Nothing here runs `launchctl`, `lsof`,
//! `osascript` or asks for a password.

use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use super::admin::{AdminError, AdminRunner};
use super::auth::PeerAuth;
use super::server::{Chowner, CommandOutput, CommandRunner, ServerConfig};

pub fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}

pub fn server_config() -> ServerConfig {
    ServerConfig {
        owner_uid: i64::from(euid()),
        runner: FakeRunner::succeeding(),
        peer_auth: FakePeerAuth::allowing(euid()),
        chown: Arc::new(RecordingChown::default()),
        ..ServerConfig::default()
    }
}

type RunnerHandler = Box<dyn Fn(&str, &[&str], usize) -> CommandOutput + Send + Sync>;

/// Records every command and answers through a closure.
pub struct FakeRunner {
    calls: Mutex<Vec<(String, Vec<String>)>>,
    handler: RunnerHandler,
}

impl fmt::Debug for FakeRunner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FakeRunner").finish_non_exhaustive()
    }
}

impl FakeRunner {
    pub fn new<F>(handler: F) -> Arc<Self>
    where
        F: Fn(&str, &[&str], usize) -> CommandOutput + Send + Sync + 'static,
    {
        Arc::new(FakeRunner {
            calls: Mutex::new(Vec::new()),
            handler: Box::new(handler),
        })
    }

    pub fn succeeding() -> Arc<Self> {
        FakeRunner::new(|_, _, _| CommandOutput::success(Vec::new()))
    }

    pub fn calls(&self) -> Vec<(String, Vec<String>)> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Each call as `[program, args…]`, the Go test's shape.
    pub fn argv(&self) -> Vec<Vec<String>> {
        self.calls()
            .into_iter()
            .map(|(program, args)| {
                let mut argv = vec![program];
                argv.extend(args);
                argv
            })
            .collect()
    }
}

impl CommandRunner for FakeRunner {
    fn run(&self, program: &str, args: &[&str]) -> CommandOutput {
        let index = {
            let mut calls = self
                .calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let index = calls.len();
            calls.push((
                program.to_string(),
                args.iter().map(|arg| (*arg).to_string()).collect(),
            ));
            index
        };
        (self.handler)(program, args, index)
    }
}

/// Injected peer identity and code-signature results.
#[derive(Debug)]
pub struct FakePeerAuth {
    pub uid: u32,
    pub uid_error: bool,
    pub code: Result<(), String>,
    pub calls: Mutex<Vec<&'static str>>,
}

impl FakePeerAuth {
    pub fn allowing(uid: u32) -> Arc<Self> {
        Arc::new(FakePeerAuth {
            uid,
            uid_error: false,
            code: Ok(()),
            calls: Mutex::new(Vec::new()),
        })
    }

    pub fn rejecting_code(uid: u32) -> Arc<Self> {
        Arc::new(FakePeerAuth {
            uid,
            uid_error: false,
            code: Err("foreign".to_string()),
            calls: Mutex::new(Vec::new()),
        })
    }

    pub fn uid_error() -> Arc<Self> {
        Arc::new(FakePeerAuth {
            uid: 0,
            uid_error: true,
            code: Ok(()),
            calls: Mutex::new(Vec::new()),
        })
    }

    pub fn calls(&self) -> Vec<&'static str> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl PeerAuth for FakePeerAuth {
    fn peer_uid(&self, _stream: &std::os::unix::net::UnixStream) -> std::io::Result<u32> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push("uid");
        if self.uid_error {
            return Err(std::io::Error::other("getpeereid failed"));
        }
        Ok(self.uid)
    }

    fn peer_authorized(&self, _stream: &std::os::unix::net::UnixStream) -> Result<(), String> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push("code");
        self.code.clone()
    }
}

/// Records ownership changes without performing them.
#[derive(Debug, Default)]
pub struct RecordingChown {
    pub calls: Mutex<Vec<(PathBuf, u32, i32)>>,
}

impl Chowner for RecordingChown {
    fn chown(&self, path: &std::path::Path, uid: u32, gid: i32) -> std::io::Result<()> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((path.to_path_buf(), uid, gid));
        Ok(())
    }
}

type AdminHandler = Box<dyn FnMut(&str, &[&str]) -> (String, Option<AdminError>) + Send>;

/// Records administrator-authorized commands and answers through a closure.
pub struct FakeAdmin {
    calls: Mutex<Vec<(String, Vec<String>)>>,
    handler: Mutex<AdminHandler>,
}

impl fmt::Debug for FakeAdmin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FakeAdmin").finish_non_exhaustive()
    }
}

impl FakeAdmin {
    pub fn with<F>(handler: F) -> Arc<Self>
    where
        F: FnMut(&str, &[&str]) -> (String, Option<AdminError>) + Send + 'static,
    {
        Arc::new(FakeAdmin {
            calls: Mutex::new(Vec::new()),
            handler: Mutex::new(Box::new(handler)),
        })
    }

    pub fn succeeding() -> Arc<Self> {
        FakeAdmin::with(|_, _| (String::new(), None))
    }

    pub fn failing(error: AdminError) -> Arc<Self> {
        FakeAdmin::with(move |_, _| (String::new(), Some(error.clone())))
    }

    pub fn calls(&self) -> Vec<(String, Vec<String>)> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl AdminRunner for FakeAdmin {
    fn run_admin(&self, program: &str, args: &[&str]) -> (String, Option<AdminError>) {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((
                program.to_string(),
                args.iter().map(|arg| (*arg).to_string()).collect(),
            ));
        (self
            .handler
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()))(program, args)
    }
}

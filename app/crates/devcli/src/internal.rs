//! The `--dev` internal dispatch, ported from `internal/devcli/cli.go`.
//!
//! The shipped app answers `--dev` only for what its own launchd jobs, workers
//! and shell setup start. Everything a person does is in the window; every
//! other first token is refused with the Go program's exact message.
//!
//! The subsystem calls are injectable so tests can assert the exact argv
//! forwarding without starting the proxy, the tunnel, launchd, or the helper.

use std::future::Future;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use devcore::diagnostics::Diagnostics;
use devcore::engine::{Engine, SystemCommandRunner};
use devcore::software::Software;
use devcore::state::{Store, default_root};
use tokio::sync::watch;

use crate::error::{CliError, CliResult};

/// The commands `--dev` accepts, in the shipped dispatch.
pub const INTERNAL_COMMANDS: [&str; 6] = [
    "serve",
    "tunnel",
    "maintain",
    "database-worker",
    "node",
    "helper",
];

/// The refusal for anything that is not an internal command.
pub const NO_COMMAND_LINE: &str = "this program has no command line; use the window";

/// Every privileged/background entry point the app itself starts.
///
/// Production is [`SystemInternal`]; tests implement this trait with fakes.
pub trait InternalRunner {
    /// `--dev serve <root>`: the HTTPS proxy listener.
    fn serve(&self, root: &Path) -> CliResult<()>;

    /// `--dev tunnel <root>`: the Cloudflare Worker supervisor.
    fn tunnel(&self, root: &Path) -> CliResult<()>;

    /// `--dev maintain <root>`: the finite log-rotation task.
    fn maintain(&self, root: &Path) -> CliResult<()>;

    /// `--dev database-worker <root> <id>`: one recorded database task.
    fn database_worker(&self, root: &Path, id: &str) -> CliResult<()>;

    /// `--dev node path [dir]`: what goes at the front of `PATH`.
    /// A missing version is a warning on stderr, never a failure.
    fn node_path(&self, dir: &Path, out: &mut dyn Write, err: &mut dyn Write) -> CliResult<()>;

    /// `--dev helper --owner-uid <uid>`: the launchd-activated helper.
    fn helper(&self, owner_uid: i64) -> CliResult<()>;
}

/// The production entry points: devproxy's servers, devcore's maintenance and
/// node selection, and the privileged helper's activated server.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemInternal;

impl InternalRunner for SystemInternal {
    fn serve(&self, root: &Path) -> CliResult<()> {
        if std::env::var("MSM_DEV_SOCKET_ACTIVATION").as_deref() == Ok("1") {
            return serve_socket_activated(root);
        }
        serve_proxy(root)
    }

    fn tunnel(&self, root: &Path) -> CliResult<()> {
        serve_tunnel(root)
    }

    fn maintain(&self, root: &Path) -> CliResult<()> {
        let store = Store::new(root);
        let engine = Engine::new(root, std::sync::Arc::new(SystemCommandRunner::new()));
        let software = Software::new(store.clone());
        Diagnostics::new(store, engine, software)
            .maintain()
            .map_err(|error| CliError::operation(error.to_string()))
    }

    fn database_worker(&self, root: &Path, id: &str) -> CliResult<()> {
        devcore::databases::Databases::new(Store::new(root))
            .run_database_worker(id, &|| false)
            .map_err(|error| CliError::operation(error.to_string()))
    }

    fn node_path(&self, dir: &Path, out: &mut dyn Write, err: &mut dyn Write) -> CliResult<()> {
        let store = Store::new(default_root());
        let software = Software::new(store.clone());
        let shell = devcore::shell::Shell::new(store, software);
        match shell.select_node(dir, None) {
            Ok(selection) => {
                if selection.system {
                    return Ok(());
                }
                writeln!(out, "{}", selection.path())?;
                writeln!(out, "{}", selection.prefix.display())?;
                Ok(())
            }
            Err(devcore::shell::ShellError::Missing(message)) => {
                // A missing version leaves PATH alone; it never fails a prompt.
                writeln!(err, "{message}")?;
                Ok(())
            }
            Err(error) => Err(CliError::operation(error.to_string())),
        }
    }

    fn helper(&self, owner_uid: i64) -> CliResult<()> {
        run_platform_helper(owner_uid)
    }
}

/// Builds the Linux privileged-helper server configuration pinned to
/// [`devcore::helper::LinuxPeerAuth`].
pub fn linux_helper_server_config(owner_uid: i64) -> devcore::helper::ServerConfig {
    devcore::helper::ServerConfig {
        owner_uid,
        peer_auth: std::sync::Arc::new(devcore::helper::LinuxPeerAuth::new()),
        ..devcore::helper::ServerConfig::default()
    }
}

fn run_platform_helper(owner_uid: i64) -> CliResult<()> {
    #[cfg(target_os = "linux")]
    {
        let config = linux_helper_server_config(owner_uid);
        devcore::helper::activate::serve_activated_with(
            owner_uid,
            &devcore::helper::LinuxSocketActivator,
            config,
        )
        .map_err(|error| CliError::operation(error.to_string()))
    }
    #[cfg(not(target_os = "linux"))]
    {
        devcore::helper::run_activated(owner_uid)
            .map_err(|error| CliError::operation(error.to_string()))
    }
}

fn serve_proxy(root: &Path) -> CliResult<()> {
    block_on_serve(root, |root, shutdown| {
        Box::pin(devproxy::proxy::server::serve(root, shutdown))
    })
}

fn serve_socket_activated(root: &Path) -> CliResult<()> {
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(root.join("logs").join("proxy-sockets.log"))?;
    let uid = unsafe { libc::geteuid() };
    writeln!(log, "socket proxy starting as uid {uid}")?;
    let result = serve_proxy(root);
    if let Err(error) = &result {
        let _ = writeln!(log, "socket proxy stopped: {}", error.message());
    }
    result
}

fn serve_tunnel(root: &Path) -> CliResult<()> {
    let event: std::sync::Arc<dyn Fn(&str) + Send + Sync> =
        std::sync::Arc::new(|message: &str| eprintln!("{message}"));
    block_on_serve(root, |root, shutdown| {
        Box::pin(devproxy::tunnel::client::serve(
            root,
            event.clone(),
            shutdown,
        ))
    })
}

/// Run one server on a current-thread runtime until it stops.
///
/// The shutdown channel is held open for the server's whole life; launchd (or
/// the app's process supervision) ends it by terminating the process, the same
/// way the Go build's signal context and launchd lifecycle do.
fn block_on_serve<E>(
    root: &Path,
    start: impl FnOnce(
        std::path::PathBuf,
        watch::Receiver<bool>,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<(), E>>>>,
) -> CliResult<()>
where
    E: std::fmt::Display,
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| CliError::operation(error.to_string()))?;
    let (_shutdown, receiver) = watch::channel(false);
    runtime
        .block_on(start(root.to_path_buf(), receiver))
        .map_err(|error| CliError::operation(error.to_string()))
}

/// `RunInternal`: only the shipped internal commands; every failure exits 1.
pub fn run_internal(args: &[String]) -> CliResult<()> {
    let mut out = std::io::stdout();
    let mut err = std::io::stderr();
    run_internal_with(args, &SystemInternal, &mut out, &mut err)
}

/// [`run_internal`] with injected subsystems and writers.
pub fn run_internal_with(
    args: &[String],
    runner: &dyn InternalRunner,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> CliResult<()> {
    let Some(command) = args.first() else {
        return Err(CliError::operation(NO_COMMAND_LINE));
    };
    if !INTERNAL_COMMANDS.contains(&command.as_str()) {
        return Err(CliError::operation(NO_COMMAND_LINE));
    }
    let rest = &args[1..];
    let need = |count: usize| -> CliResult<()> {
        if args.len() < count {
            return Err(CliError::operation("missing arguments; run --dev help"));
        }
        Ok(())
    };
    match command.as_str() {
        "serve" => {
            need(2)?;
            runner.serve(Path::new(&args[1]))
        }
        "tunnel" => {
            need(2)?;
            runner.tunnel(Path::new(&args[1]))
        }
        "maintain" => {
            need(2)?;
            runner.maintain(Path::new(&args[1]))
        }
        "database-worker" => {
            need(3)?;
            runner.database_worker(Path::new(&args[1]), &args[2])
        }
        "node" => {
            if rest.first().map(String::as_str) != Some("path") {
                return Err(CliError::operation(
                    "node versions are managed in the window: Software, Node.js",
                ));
            }
            let dir = match rest.get(1) {
                Some(dir) => std::path::PathBuf::from(dir),
                None => std::env::current_dir()
                    .map_err(|error| CliError::operation(error.to_string()))?,
            };
            runner.node_path(&dir, out, err)
        }
        "helper" => {
            let owner_uid = parse_owner_uid(rest)?;
            runner.helper(owner_uid)
        }
        _ => unreachable!("INTERNAL_COMMANDS was checked above"),
    }
}

/// Parse `--owner-uid N` anywhere in the helper's arguments, matching Go's
/// `strconv.Atoi` message for malformed values.
pub fn parse_owner_uid(args: &[String]) -> CliResult<i64> {
    let mut owner_uid = -1;
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--owner-uid" && index + 1 < args.len() {
            let value = &args[index + 1];
            owner_uid = value.parse::<i64>().map_err(|_| {
                let reason = if valid_decimal_integer(value) {
                    "value out of range"
                } else {
                    "invalid syntax"
                };
                CliError::operation(format!(
                    "invalid --owner-uid {value:?}: strconv.Atoi: parsing {value:?}: {reason}"
                ))
            })?;
            index += 1;
        }
        index += 1;
    }
    Ok(owner_uid)
}

fn valid_decimal_integer(value: &str) -> bool {
    let digits = value.strip_prefix(['+', '-']).unwrap_or(value);
    !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_helper_config_wires_owner_uid_and_linux_peer_auth() {
        let config = linux_helper_server_config(1000);
        assert_eq!(config.owner_uid, 1000);
        assert!(format!("{:?}", config.peer_auth).contains("LinuxPeerAuth"));

        // Invalid owner UID fails fast before touching systemd descriptors.
        let err = devcore::helper::activate::serve_activated_with(
            0,
            &devcore::helper::LinuxSocketActivator,
            linux_helper_server_config(0),
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid owner uid"));
    }
}

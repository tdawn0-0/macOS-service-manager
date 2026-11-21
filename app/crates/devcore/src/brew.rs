//! Homebrew services provider, ported from the Go `internal/brew` package.
//!
//! Manages Homebrew services via `brew services` for user services and the
//! privileged helper for root services registered under `/Library/LaunchDaemons`.
//! All process execution, privileged helper access, and host paths are injected
//! through [`crate::launchd::Context`], so tests never run `brew` or touch real
//! launchd directories.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::helper::DefaultHelper;
use crate::launchd::{
    CommandError, Context, DomainState, LaunchdError, LaunchdRemoveRequest, PrivilegedHelper,
    SystemHost, SystemRunner, parse_print,
};
use crate::service::{self, Action, Autostart, Detail, Item, LogFile, Row, Source, State};

/// What kind of failure [`Provider`] returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Io,
    Command,
    Parse,
    HelperNotInstalled,
    Helper,
    UnknownAction,
}

/// A typed error from the Homebrew services provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrewError {
    pub code: ErrorCode,
    pub message: String,
}

impl BrewError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        BrewError {
            code,
            message: message.into(),
        }
    }

    pub fn io(message: impl Into<String>) -> Self {
        BrewError::new(ErrorCode::Io, message)
    }

    pub fn command(message: impl Into<String>) -> Self {
        BrewError::new(ErrorCode::Command, message)
    }

    pub fn parse(message: impl Into<String>) -> Self {
        BrewError::new(ErrorCode::Parse, message)
    }

    pub fn helper_not_installed() -> Self {
        BrewError::new(
            ErrorCode::HelperNotInstalled,
            "the privileged helper is not installed",
        )
    }

    pub fn helper(message: impl Into<String>) -> Self {
        BrewError::new(ErrorCode::Helper, message)
    }

    pub fn unknown_action(message: impl Into<String>) -> Self {
        BrewError::new(ErrorCode::UnknownAction, message)
    }

    pub fn is_helper_not_installed(&self) -> bool {
        self.code == ErrorCode::HelperNotInstalled
    }
}

impl fmt::Display for BrewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for BrewError {}

impl From<CommandError> for BrewError {
    fn from(error: CommandError) -> Self {
        BrewError::command(error.message())
    }
}

impl From<LaunchdError> for BrewError {
    fn from(error: LaunchdError) -> Self {
        let code = match error.code {
            crate::launchd::ErrorCode::HelperNotInstalled => ErrorCode::HelperNotInstalled,
            crate::launchd::ErrorCode::Helper => ErrorCode::Helper,
            crate::launchd::ErrorCode::Io => ErrorCode::Io,
            crate::launchd::ErrorCode::Command => ErrorCode::Command,
            crate::launchd::ErrorCode::UnknownAction => ErrorCode::UnknownAction,
            _ => ErrorCode::Helper,
        };
        BrewError::new(code, error.message)
    }
}

/// Directories where Homebrew services place their registration plists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Where a service registered to start at login has its property list
    /// (`~/Library/LaunchAgents` in production).
    pub agent_dir: PathBuf,
    /// Where a service registered as root to start at boot has its property list
    /// (`/Library/LaunchDaemons` in production).
    pub daemon_dir: PathBuf,
}

/// A service in the output of `brew services list --json`.
#[derive(Debug, Deserialize)]
struct Listed {
    #[serde(default)]
    name: String,
    #[serde(default)]
    status: String,
    user: Option<String>,
    #[serde(default)]
    file: String,
    #[serde(default)]
    #[allow(dead_code)]
    exit_code: Option<i32>,
}

/// A service in the output of `brew services info <name> --json`.
#[derive(Debug, Deserialize)]
struct Info {
    #[serde(default)]
    #[allow(dead_code)]
    name: String,
    #[serde(default)]
    service_name: String,
    #[serde(default)]
    #[allow(dead_code)]
    running: bool,
    #[serde(default)]
    #[allow(dead_code)]
    loaded: bool,
    #[serde(default)]
    #[allow(dead_code)]
    schedulable: bool,
    pid: Option<i32>,
    exit_code: Option<i32>,
    user: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    status: String,
    #[allow(dead_code)]
    file: Option<String>,
    command: Option<String>,
    working_dir: Option<String>,
    root_dir: Option<String>,
    log_path: Option<String>,
    error_log_path: Option<String>,
    interval: Option<i64>,
    cron: Option<String>,
}

fn parse_list(data: &str) -> Result<Vec<Listed>, BrewError> {
    serde_json::from_str(data)
        .map_err(|error| BrewError::parse(format!("parse brew services: {error}")))
}

fn parse_info(data: &str) -> Result<Info, BrewError> {
    let mut infos: Vec<Info> = serde_json::from_str(data)
        .map_err(|error| BrewError::parse(format!("parse brew service info: {error}")))?;
    if infos.is_empty() {
        return Err(BrewError::parse("no data"));
    }
    Ok(infos.remove(0))
}

/// Reads a status of `brew services`: `"none"` is not loaded, while `"stopped"`,
/// `"unknown"`, and `"other"` are loaded with no process.
fn state_of(status: &str) -> State {
    match status {
        "started" => State::Running,
        "scheduled" => State::Scheduled,
        "error" => State::Failed,
        "none" | "" => State::Stopped,
        _ => State::Loaded,
    }
}

/// Returns the property list in `dir` that registers `name`, if one exists.
/// Homebrew names it `sh.brew.<name>.plist`, or `homebrew.mxcl.<name>.plist`
/// in older versions.
fn registered(dir: &Path, name: &str) -> Option<PathBuf> {
    for file in [
        format!("sh.brew.{name}.plist"),
        format!("homebrew.mxcl.{name}.plist"),
    ] {
        let path = dir.join(file);
        if std::fs::metadata(&path).is_ok() {
            return Some(path);
        }
    }
    None
}

/// The launchd label of the job a service's property list defines.
fn label_of(path: &Path) -> String {
    let base = path
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    base.strip_suffix(".plist").unwrap_or(&base).to_string()
}

/// The Homebrew services provider.
pub struct Provider {
    config: Config,
    ctx: Arc<Context>,
}

impl Provider {
    /// Builds the machine's Homebrew services provider using the default
    /// system runner, host, and privileged helper client.
    pub fn default_provider() -> Self {
        let ctx = Arc::new(Context::new(
            Arc::new(SystemRunner::new()),
            Arc::new(DefaultHelper) as Arc<dyn PrivilegedHelper>,
            Arc::new(SystemHost::new()),
        ));
        Provider::system(ctx)
    }

    /// Builds a provider pointed at `~/Library/LaunchAgents` and
    /// `/Library/LaunchDaemons` with an injected [`Context`].
    pub fn system(ctx: Arc<Context>) -> Self {
        let home = ctx.host.home_dir().unwrap_or_default();
        Provider::new(
            Config {
                agent_dir: home.join("Library/LaunchAgents"),
                daemon_dir: PathBuf::from("/Library/LaunchDaemons"),
            },
            ctx,
        )
    }

    /// Builds a provider with explicit directories and an injected [`Context`].
    pub fn new(config: Config, ctx: Arc<Context>) -> Self {
        Provider { config, ctx }
    }

    pub fn source(&self) -> Source {
        Source::Brew
    }

    pub fn agent_dir(&self) -> &Path {
        &self.config.agent_dir
    }

    pub fn daemon_dir(&self) -> &Path {
        &self.config.daemon_dir
    }

    pub fn list(&self) -> Result<Vec<Item>, BrewError> {
        let out = self
            .ctx
            .runner
            .run("brew", &["services", "list", "--json"])?;
        let services = parse_list(&out)?;
        Ok(self.items(services))
    }

    /// Maps parsed services to [`Item`]s. Those registered as root are jobs of
    /// the system domain, of which `brew services` run as the user reports
    /// nothing: their state comes from launchd.
    fn items(&self, services: Vec<Listed>) -> Vec<Item> {
        let mut system_state: Option<DomainState> = None;
        let mut items = Vec::with_capacity(services.len());
        for service in services {
            let mut item = Item {
                id: format!("brew:{}", service.name),
                source: Source::Brew,
                name: service.name,
                state: state_of(&service.status),
                pid: 0,
                autostart: Autostart::Off,
                owner: service.user.unwrap_or_default(),
                path: PathBuf::from(service.file),
                kind: None,
                admin: false,
                keep_alive: false,
            };
            if let Some(plist) = registered(&self.config.agent_dir, &item.name) {
                item.autostart = Autostart::On;
                item.path = plist;
            } else if let Some(plist) = registered(&self.config.daemon_dir, &item.name) {
                let label = label_of(&plist);
                item.autostart = Autostart::On;
                item.path = plist;
                item.admin = true;
                item.owner = "root".to_string();
                let state = system_state.get_or_insert_with(|| {
                    match self.ctx.runner.run("launchctl", &["print", "system"]) {
                        Ok(out) => parse_print(&out),
                        Err(_) => DomainState::default(),
                    }
                });
                let (job_state, pid) = state.of(&label);
                item.state = job_state;
                item.pid = pid;
            }
            items.push(item);
        }
        items
    }

    pub fn detail(&self, item: &Item) -> Result<Detail, BrewError> {
        let out = self
            .ctx
            .runner
            .run("brew", &["services", "info", &item.name, "--json"])?;
        let info = parse_info(&out)?;
        Ok(detail_of(&info, item))
    }

    pub fn actions(&self, item: &Item) -> Vec<Action> {
        let mut actions = if item.active() {
            vec![Action::Stop, Action::Restart]
        } else {
            vec![Action::Start]
        };
        if item.autostart == Autostart::On {
            actions.push(Action::Disable);
        } else if !item.admin {
            actions.push(Action::Enable);
        }
        actions
    }

    pub fn confirm(&self, item: &Item, action: Action) -> Option<String> {
        if item.admin && action == Action::Disable {
            let base = item
                .path
                .file_name()
                .map(|name| name.to_string_lossy())
                .unwrap_or_default();
            Some(format!(
                "{} runs as root, at boot. This stops it and deletes {base}.",
                item.name
            ))
        } else {
            None
        }
    }

    /// Runs an action. Starting and stopping leave whether the service starts
    /// at login as it is, and turning that on or off leaves the service running
    /// or stopped: brew's own words mix the two, `start` and `stop` also
    /// registering and unregistering.
    pub fn do_action(&self, item: &Item, action: Action) -> Result<(), BrewError> {
        if item.admin {
            return self.do_as_root(item, action);
        }
        let registered_now = item.autostart == Autostart::On;
        match action {
            Action::Start => {
                if item.state == State::Loaded {
                    // Both `start` and `run` refuse a service that is loaded.
                    self.run_brew_services(&["restart", &item.name])
                } else if registered_now {
                    self.run_brew_services(&["start", &item.name])
                } else {
                    self.run_brew_services(&["run", &item.name])
                }
            }
            Action::Stop => {
                if registered_now {
                    self.run_brew_services(&["stop", "--keep", &item.name])
                } else {
                    self.run_brew_services(&["stop", &item.name])
                }
            }
            Action::Restart => self.run_brew_services(&["restart", &item.name]),
            Action::Enable => self.register(item),
            Action::Disable => self.unregister(item),
            other => Err(BrewError::unknown_action(format!(
                "brew: unknown action \"{other}\""
            ))),
        }
    }

    fn run_brew_services(&self, args: &[&str]) -> Result<(), BrewError> {
        let mut full_args = Vec::with_capacity(args.len() + 1);
        full_args.push("services");
        full_args.extend_from_slice(args);
        self.ctx.runner.run("brew", &full_args)?;
        Ok(())
    }

    /// Makes the service start at login without starting it now: copies the
    /// service's property list into `agent_dir`.
    fn register(&self, item: &Item) -> Result<(), BrewError> {
        let data = std::fs::read(&item.path)
            .map_err(|error| BrewError::io(format!("read the service file: {error}")))?;
        let base = item
            .path
            .file_name()
            .ok_or_else(|| BrewError::io("read the service file: path has no file name"))?;
        std::fs::create_dir_all(&self.config.agent_dir)
            .map_err(|error| BrewError::io(error.to_string()))?;
        let dest = self.config.agent_dir.join(base);
        std::fs::write(&dest, data).map_err(|error| BrewError::io(error.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o644));
        }
        Ok(())
    }

    /// Keeps the service from starting at login without stopping it now.
    fn unregister(&self, item: &Item) -> Result<(), BrewError> {
        let Some(path) = registered(&self.config.agent_dir, &item.name) else {
            return Ok(());
        };
        std::fs::remove_file(path).map_err(|error| BrewError::io(error.to_string()))
    }

    /// Changes a service registered as root via the privileged helper.
    /// Homebrew's commands, run as the user, cannot reach it.
    fn do_as_root(&self, item: &Item, action: Action) -> Result<(), BrewError> {
        let helper = self.ctx.helper.require()?;
        let target = format!("system/{}", label_of(&item.path));
        match action {
            Action::Start => {
                if item.state == State::Stopped {
                    let path = item.path.to_str().ok_or_else(|| {
                        BrewError::io(format!("path {} is not valid UTF-8", item.path.display()))
                    })?;
                    helper.launchd_control(&[vec![
                        "bootstrap".to_string(),
                        "system".to_string(),
                        path.to_string(),
                    ]])?;
                } else {
                    helper.launchd_control(&[vec!["kickstart".to_string(), target]])?;
                }
                Ok(())
            }
            Action::Stop => {
                helper.launchd_control(&[vec!["bootout".to_string(), target]])?;
                Ok(())
            }
            Action::Restart => {
                helper.launchd_control(&[vec![
                    "kickstart".to_string(),
                    "-k".to_string(),
                    target,
                ]])?;
                Ok(())
            }
            Action::Disable => {
                // Not loaded is fine too.
                let _ = helper.launchd_control(&[vec!["bootout".to_string(), target.clone()]]);
                match std::fs::symlink_metadata(&item.path) {
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                    Err(error) => return Err(BrewError::io(error.to_string())),
                }
                let tmp_dir = TempDirGuard::new(&self.ctx.host.temp_dir(), "msm-brew-disabled-")?;
                helper.launchd_remove(LaunchdRemoveRequest {
                    path: item.path.clone(),
                    target,
                    trash_dir: tmp_dir.path().to_path_buf(),
                })?;
                Ok(())
            }
            other => Err(BrewError::unknown_action(format!(
                "brew: {other} does not apply to a service of root"
            ))),
        }
    }
}

fn detail_of(info: &Info, item: &Item) -> Detail {
    let mut detail = Detail::default();
    fn add(rows: &mut Vec<Row>, group: &str, key: &str, value: &str) {
        if !value.is_empty() {
            rows.push(Row {
                group: group.to_string(),
                key: key.to_string(),
                value: value.to_string(),
            });
        }
    }

    const STATUS: &str = "Status";
    const PROGRAM: &str = "Program";
    const FILES: &str = "Files";

    if item.pid > 0 {
        add(
            &mut detail.rows,
            STATUS,
            "Process ID",
            &item.pid.to_string(),
        );
    } else if let Some(pid) = info.pid {
        add(&mut detail.rows, STATUS, "Process ID", &pid.to_string());
    }
    if let Some(exit_code) = info.exit_code {
        add(
            &mut detail.rows,
            STATUS,
            "Last exit code",
            &exit_code.to_string(),
        );
    }
    if item.admin {
        add(&mut detail.rows, STATUS, "Runs as", "root");
    } else if let Some(user) = &info.user {
        add(&mut detail.rows, STATUS, "Runs as", user);
    }
    add(&mut detail.rows, STATUS, "Service name", &info.service_name);
    if let Some(command) = &info.command {
        add(&mut detail.rows, PROGRAM, "Command", command);
    }
    if let Some(working_dir) = &info.working_dir {
        add(&mut detail.rows, PROGRAM, "Working directory", working_dir);
    }
    if let Some(root_dir) = &info.root_dir {
        add(&mut detail.rows, PROGRAM, "Root directory", root_dir);
    }
    if let Some(interval) = info.interval {
        add(
            &mut detail.rows,
            PROGRAM,
            "Runs every",
            &format!("{interval} s"),
        );
    }
    if let Some(cron) = &info.cron {
        add(&mut detail.rows, PROGRAM, "Schedule", cron);
    }
    let plist_path = item.path.to_string_lossy();
    add(&mut detail.rows, FILES, "Property list", &plist_path);
    if let Some(log_path) = &info.log_path {
        add(&mut detail.rows, FILES, "Output log", log_path);
    }
    if let Some(error_log_path) = &info.error_log_path {
        add(&mut detail.rows, FILES, "Error log", error_log_path);
    }

    if let Some(log_path) = &info.log_path
        && !log_path.is_empty()
    {
        detail.logs.push(LogFile {
            label: "Output".to_string(),
            path: PathBuf::from(log_path),
        });
    }
    if let Some(error_log_path) = &info.error_log_path
        && !error_log_path.is_empty()
        && info.log_path.as_deref() != Some(error_log_path.as_str())
    {
        detail.logs.push(LogFile {
            label: "Errors".to_string(),
            path: PathBuf::from(error_log_path),
        });
    }

    detail
}

struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(parent: &Path, prefix: &str) -> Result<Self, BrewError> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        for _ in 0..100 {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!("{prefix}{}-{n}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ =
                            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700));
                    }
                    return Ok(TempDirGuard { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(BrewError::io(error.to_string())),
            }
        }
        Err(BrewError::io("could not create a temporary directory"))
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

impl service::Provider for Provider {
    fn source(&self) -> Source {
        Provider::source(self)
    }

    fn list(&self) -> service::Result<Vec<Item>> {
        Provider::list(self).map_err(|error| Box::new(error) as service::BoxError)
    }

    fn detail(&self, item: &Item) -> service::Result<Detail> {
        Provider::detail(self, item).map_err(|error| Box::new(error) as service::BoxError)
    }

    fn actions(&self, item: &Item) -> Vec<Action> {
        Provider::actions(self, item)
    }

    fn confirm(&self, item: &Item, action: Action) -> Option<String> {
        Provider::confirm(self, item, action)
    }

    fn do_action(&self, item: &Item, action: Action) -> service::Result<()> {
        Provider::do_action(self, item, action)
            .map_err(|error| Box::new(error) as service::BoxError)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use tempfile::TempDir;

    use super::*;
    use crate::helper::server::{
        CommandOutput, CommandRunner as HelperCommandRunner, ServerConfig,
    };
    use crate::helper::testenv::TestHelper;
    use crate::launchd::{Host, ProcessRunner, UnavailableHelper};

    const LIST_JSON: &str = r#"[
  {"name":"postgresql@16","status":"started","user":"jyeu","file":"/a.plist","exit_code":0},
  {"name":"redis","status":"none","user":null,"file":"/opt/homebrew/opt/redis/sh.brew.redis.plist","exit_code":null},
  {"name":"nginx","status":"error","user":"jyeu","file":"/n.plist","exit_code":1},
  {"name":"mysql","status":"stopped","user":"jyeu","file":"/m.plist","exit_code":0},
  {"name":"cron","status":"scheduled","user":"jyeu","file":"/c.plist","exit_code":0},
  {"name":"php","status":"none","user":"root","file":"/Users/x/Library/LaunchAgents/sh.brew.php.plist","exit_code":null}
]"#;

    const SYSTEM_PRINT: &str =
        "system = {\n\tservices = {\n\t\t     407      - \thomebrew.mxcl.php\n\t}\n}";

    struct FakeRunner {
        out: String,
        fail: HashMap<String, String>,
        calls: Mutex<Vec<Vec<String>>>,
    }

    impl FakeRunner {
        fn new(out: impl Into<String>, fail: HashMap<String, String>) -> Arc<Self> {
            Arc::new(FakeRunner {
                out: out.into(),
                fail,
                calls: Mutex::new(Vec::new()),
            })
        }

        fn calls(&self) -> Vec<Vec<String>> {
            self.calls.lock().expect("lock calls").clone()
        }
    }

    impl ProcessRunner for FakeRunner {
        fn run(&self, program: &str, args: &[&str]) -> Result<String, CommandError> {
            let mut call = Vec::with_capacity(args.len() + 1);
            call.push(program.to_string());
            call.extend(args.iter().map(|arg| (*arg).to_string()));
            self.calls.lock().expect("lock calls").push(call.clone());

            if program == "launchctl" {
                return Ok(SYSTEM_PRINT.to_string());
            }
            if let Some(sub) = call.get(2)
                && let Some(message) = self.fail.get(sub)
            {
                return Err(CommandError::new(program, call[1..].to_vec(), message));
            }
            Ok(self.out.clone())
        }
    }

    struct FakeHost {
        home: PathBuf,
        temp: PathBuf,
    }

    impl Host for FakeHost {
        fn uid(&self) -> u32 {
            501
        }

        fn home_dir(&self) -> Result<PathBuf, LaunchdError> {
            Ok(self.home.clone())
        }

        fn temp_dir(&self) -> PathBuf {
            self.temp.clone()
        }

        fn current_exe(&self) -> Result<PathBuf, LaunchdError> {
            Ok(PathBuf::from("/fake/Devbase"))
        }

        fn eval_symlinks(&self, path: &Path) -> Option<PathBuf> {
            Some(path.to_path_buf())
        }

        fn trash(&self, _path: &Path) -> Result<(), LaunchdError> {
            Ok(())
        }
    }

    struct Harness {
        _root: TempDir,
        agent_dir: PathBuf,
        daemon_dir: PathBuf,
        runner: Arc<FakeRunner>,
        provider: Provider,
    }

    impl Harness {
        fn new(
            out: &str,
            fail: HashMap<String, String>,
            helper: Arc<dyn PrivilegedHelper>,
            plists: &[&str],
        ) -> Self {
            let root = TempDir::new().expect("create temp root");
            let home = root.path().join("home");
            let temp = root.path().join("tmp");
            let agent_dir = root.path().join("agents");
            let daemon_dir = root.path().join("daemons");
            for dir in [&home, &temp, &agent_dir, &daemon_dir] {
                std::fs::create_dir_all(dir).expect("create test dir");
            }
            for spec in plists {
                let (dir, name) = match spec.split_once('/') {
                    Some(("daemon", base)) => (&daemon_dir, base),
                    _ => (&agent_dir, *spec),
                };
                std::fs::write(dir.join(name), b"").expect("write test plist");
            }
            let runner = FakeRunner::new(out, fail);
            let ctx = Arc::new(Context::new(
                Arc::clone(&runner) as Arc<dyn ProcessRunner>,
                helper,
                Arc::new(FakeHost { home, temp }),
            ));
            let provider = Provider::new(
                Config {
                    agent_dir: agent_dir.clone(),
                    daemon_dir: daemon_dir.clone(),
                },
                ctx,
            );
            Harness {
                _root: root,
                agent_dir,
                daemon_dir,
                runner,
                provider,
            }
        }
    }

    #[derive(Debug)]
    struct RecordingHelperRunner {
        calls: Arc<Mutex<Vec<Vec<String>>>>,
    }

    impl HelperCommandRunner for RecordingHelperRunner {
        fn run(&self, name: &str, args: &[&str]) -> CommandOutput {
            let mut call = Vec::with_capacity(args.len() + 1);
            call.push(name.to_string());
            call.extend(args.iter().map(|arg| (*arg).to_string()));
            self.calls.lock().expect("lock helper calls").push(call);
            CommandOutput::success(Vec::new())
        }
    }

    #[test]
    fn list_reads_services_and_resolves_autostart_and_root_daemons() {
        let harness = Harness::new(
            LIST_JSON,
            HashMap::new(),
            Arc::new(UnavailableHelper),
            &[
                "sh.brew.postgresql@16.plist",
                "homebrew.mxcl.mysql.plist",
                "sh.brew.other.plist",
                "daemon/homebrew.mxcl.php.plist",
            ],
        );

        let items = harness.provider.list().expect("list brew services");
        let got: Vec<(&str, State, Autostart, &str, bool, i32)> = items
            .iter()
            .map(|item| {
                (
                    item.name.as_str(),
                    item.state,
                    item.autostart,
                    item.owner.as_str(),
                    item.admin,
                    item.pid,
                )
            })
            .collect();
        let want = vec![
            (
                "postgresql@16",
                State::Running,
                Autostart::On,
                "jyeu",
                false,
                0,
            ),
            ("redis", State::Stopped, Autostart::Off, "", false, 0),
            ("nginx", State::Failed, Autostart::Off, "jyeu", false, 0),
            ("mysql", State::Loaded, Autostart::On, "jyeu", false, 0),
            ("cron", State::Scheduled, Autostart::Off, "jyeu", false, 0),
            // `brew services` says none: launchd knows better.
            ("php", State::Running, Autostart::On, "root", true, 407),
        ];
        assert_eq!(got, want);
        assert_eq!(items[0].id, "brew:postgresql@16");
        assert_eq!(items[0].source, Source::Brew);
        assert_eq!(
            items[5].path,
            harness.daemon_dir.join("homebrew.mxcl.php.plist")
        );
        assert_eq!(
            items[0].path,
            harness.agent_dir.join("sh.brew.postgresql@16.plist")
        );
        // `launchctl print system` was queried once for root services.
        let calls = harness.runner.calls();
        assert_eq!(
            calls,
            vec![
                vec!["brew", "services", "list", "--json"],
                vec!["launchctl", "print", "system"],
            ]
        );
    }

    #[test]
    fn list_errors_on_command_failure_and_invalid_json() {
        let mut fail = HashMap::new();
        fail.insert("list".to_string(), "brew: command not found".to_string());
        let harness = Harness::new("", fail, Arc::new(UnavailableHelper), &[]);
        let error = harness.provider.list().expect_err("command failure");
        assert_eq!(error.to_string(), "brew: command not found");
        assert_eq!(error.code, ErrorCode::Command);

        let harness = Harness::new("nope", HashMap::new(), Arc::new(UnavailableHelper), &[]);
        let error = harness.provider.list().expect_err("invalid JSON");
        assert_eq!(error.code, ErrorCode::Parse);
    }

    #[test]
    fn detail_maps_fields_and_logs_and_fails_on_empty_info() {
        let info_json = r#"[{"name":"redis","service_name":"sh.brew.redis","running":true,"loaded":true,
          "schedulable":false,"pid":42,"exit_code":null,"user":"jyeu","status":"started",
          "file":"/r.plist","command":"redis-server","working_dir":null,"root_dir":null,
          "log_path":"/var/log/redis.log","error_log_path":"/var/log/redis.err","interval":null,"cron":null}]"#;
        let harness = Harness::new(info_json, HashMap::new(), Arc::new(UnavailableHelper), &[]);
        let item = Item {
            id: "brew:redis".into(),
            source: Source::Brew,
            name: "redis".into(),
            state: State::Running,
            pid: 0,
            autostart: Autostart::Off,
            owner: String::new(),
            path: PathBuf::from("/r.plist"),
            kind: None,
            admin: false,
            keep_alive: false,
        };
        let detail = harness.provider.detail(&item).expect("detail");
        let rows: HashMap<&str, &str> = detail
            .rows
            .iter()
            .map(|row| (row.key.as_str(), row.value.as_str()))
            .collect();
        assert_eq!(rows.get("Process ID"), Some(&"42"));
        assert_eq!(rows.get("Command"), Some(&"redis-server"));
        assert_eq!(rows.get("Property list"), Some(&"/r.plist"));
        assert!(!rows.contains_key("Working directory"));
        assert_eq!(
            detail.logs,
            vec![
                LogFile {
                    label: "Output".into(),
                    path: PathBuf::from("/var/log/redis.log"),
                },
                LogFile {
                    label: "Errors".into(),
                    path: PathBuf::from("/var/log/redis.err"),
                },
            ]
        );

        // Deduplicates identical output/error log paths and respects root overrides.
        let same_log_json = r#"[{"name":"php","service_name":"homebrew.mxcl.php","running":true,"loaded":true,
          "schedulable":false,"pid":10,"exit_code":0,"user":"jyeu","status":"started",
          "file":"/d.plist","command":"php-fpm","working_dir":"/srv","root_dir":"/",
          "log_path":"/var/log/php.log","error_log_path":"/var/log/php.log","interval":60,"cron":"* * * * *"}]"#;
        let harness = Harness::new(
            same_log_json,
            HashMap::new(),
            Arc::new(UnavailableHelper),
            &[],
        );
        let root_item = Item {
            id: "brew:php".into(),
            source: Source::Brew,
            name: "php".into(),
            state: State::Running,
            pid: 407,
            autostart: Autostart::On,
            owner: "root".into(),
            path: PathBuf::from("/Library/LaunchDaemons/homebrew.mxcl.php.plist"),
            kind: None,
            admin: true,
            keep_alive: false,
        };
        let detail = harness.provider.detail(&root_item).expect("root detail");
        let rows: HashMap<&str, &str> = detail
            .rows
            .iter()
            .map(|row| (row.key.as_str(), row.value.as_str()))
            .collect();
        assert_eq!(rows.get("Process ID"), Some(&"407"));
        assert_eq!(rows.get("Runs as"), Some(&"root"));
        assert_eq!(rows.get("Runs every"), Some(&"60 s"));
        assert_eq!(rows.get("Schedule"), Some(&"* * * * *"));
        assert_eq!(detail.logs.len(), 1);

        let harness = Harness::new("[]", HashMap::new(), Arc::new(UnavailableHelper), &[]);
        let error = harness.provider.detail(&item).expect_err("no data");
        assert_eq!(error.code, ErrorCode::Parse);
    }

    #[test]
    fn actions_and_confirm() {
        let harness = Harness::new("", HashMap::new(), Arc::new(UnavailableHelper), &[]);
        let running = Item {
            id: "brew:redis".into(),
            source: Source::Brew,
            name: "redis".into(),
            state: State::Running,
            pid: 0,
            autostart: Autostart::Off,
            owner: String::new(),
            path: PathBuf::new(),
            kind: None,
            admin: false,
            keep_alive: false,
        };
        let stopped = Item {
            state: State::Stopped,
            autostart: Autostart::On,
            ..running.clone()
        };
        let root = Item {
            id: "brew:php".into(),
            name: "php".into(),
            state: State::Running,
            autostart: Autostart::On,
            admin: true,
            path: PathBuf::from("/x/homebrew.mxcl.php.plist"),
            ..running.clone()
        };
        let root_off = Item {
            autostart: Autostart::Off,
            ..root.clone()
        };

        assert_eq!(
            harness.provider.actions(&running),
            vec![Action::Stop, Action::Restart, Action::Enable]
        );
        assert_eq!(
            harness.provider.actions(&stopped),
            vec![Action::Start, Action::Disable]
        );
        assert_eq!(
            harness.provider.actions(&root),
            vec![Action::Stop, Action::Restart, Action::Disable]
        );
        assert_eq!(
            harness.provider.actions(&root_off),
            vec![Action::Stop, Action::Restart]
        );

        for item in [&running, &stopped] {
            assert_eq!(harness.provider.confirm(item, Action::Enable), None);
            assert_eq!(harness.provider.confirm(item, Action::Disable), None);
        }
        let confirm = harness
            .provider
            .confirm(&root, Action::Disable)
            .expect("unregistering a root service asks first");
        assert!(confirm.contains("php runs as root, at boot"));
        assert!(confirm.contains("homebrew.mxcl.php.plist"));
    }

    #[test]
    fn do_action_preserves_autostart_for_user_services() {
        let base = Item {
            id: "brew:redis".into(),
            source: Source::Brew,
            name: "redis".into(),
            state: State::Stopped,
            pid: 0,
            autostart: Autostart::Off,
            owner: String::new(),
            path: PathBuf::new(),
            kind: None,
            admin: false,
            keep_alive: false,
        };
        let stopped = base.clone();
        let stopped_on = Item {
            autostart: Autostart::On,
            ..base.clone()
        };
        let loaded = Item {
            state: State::Loaded,
            autostart: Autostart::On,
            ..base.clone()
        };
        let running = Item {
            state: State::Running,
            autostart: Autostart::Off,
            ..base.clone()
        };
        let running_on = Item {
            state: State::Running,
            autostart: Autostart::On,
            ..base
        };

        let cases = [
            (
                "start unregistered",
                stopped,
                Action::Start,
                vec!["run", "redis"],
            ),
            (
                "start registered",
                stopped_on,
                Action::Start,
                vec!["start", "redis"],
            ),
            (
                "start loaded",
                loaded,
                Action::Start,
                vec!["restart", "redis"],
            ),
            (
                "stop unregistered",
                running.clone(),
                Action::Stop,
                vec!["stop", "redis"],
            ),
            (
                "stop registered",
                running_on,
                Action::Stop,
                vec!["stop", "--keep", "redis"],
            ),
            (
                "restart",
                running,
                Action::Restart,
                vec!["restart", "redis"],
            ),
        ];

        for (label, item, action, want_args) in cases {
            let harness = Harness::new("", HashMap::new(), Arc::new(UnavailableHelper), &[]);
            harness
                .provider
                .do_action(&item, action)
                .unwrap_or_else(|error| panic!("{label}: {error}"));
            let mut want = vec!["brew".to_string(), "services".to_string()];
            want.extend(want_args.into_iter().map(str::to_string));
            assert_eq!(harness.runner.calls(), vec![want], "{label}");
        }
    }

    #[test]
    fn autostart_moves_the_property_list_only() {
        let harness = Harness::new("", HashMap::new(), Arc::new(UnavailableHelper), &[]);
        let source_dir = TempDir::new().expect("source dir");
        let source = source_dir.path().join("sh.brew.redis.plist");
        std::fs::write(&source, b"the plist").expect("write source plist");
        let dest = harness.agent_dir.join("sh.brew.redis.plist");

        let mut item = Item {
            id: "brew:redis".into(),
            source: Source::Brew,
            name: "redis".into(),
            state: State::Running,
            pid: 0,
            autostart: Autostart::Off,
            owner: String::new(),
            path: source,
            kind: None,
            admin: false,
            keep_alive: false,
        };

        harness
            .provider
            .do_action(&item, Action::Enable)
            .expect("enable");
        assert_eq!(std::fs::read(&dest).expect("read registered"), b"the plist");

        item.autostart = Autostart::On;
        item.path = dest.clone();
        harness
            .provider
            .do_action(&item, Action::Disable)
            .expect("disable");
        assert!(!dest.exists(), "plist should be removed on disable");
        assert!(
            harness.runner.calls().is_empty(),
            "no command runs, so the service is not interrupted"
        );

        // The file an old Homebrew named goes too.
        let old = harness.agent_dir.join("homebrew.mxcl.redis.plist");
        std::fs::write(&old, b"").expect("write old plist");
        harness
            .provider
            .do_action(&item, Action::Disable)
            .expect("disable old name");
        assert!(!old.exists(), "old plist name should be removed");
    }

    #[test]
    fn root_service_changes_through_privileged_helper() {
        let daemon_dir = TempDir::new().expect("daemon dir");
        let plist_path = daemon_dir.path().join("homebrew.mxcl.php.plist");
        let root = Item {
            id: "brew:php".into(),
            source: Source::Brew,
            name: "php".into(),
            state: State::Running,
            pid: 407,
            autostart: Autostart::On,
            owner: "root".into(),
            path: plist_path.clone(),
            kind: None,
            admin: true,
            keep_alive: false,
        };
        let off = Item {
            state: State::Stopped,
            ..root.clone()
        };

        // Without privileged helper, root service changes fail with HelperNotInstalled.
        let harness_no_helper = Harness::new("", HashMap::new(), Arc::new(UnavailableHelper), &[]);
        let error = harness_no_helper
            .provider
            .do_action(&root, Action::Stop)
            .expect_err("expected HelperNotInstalled without helper");
        assert!(error.is_helper_not_installed(), "got {error:?}");

        let helper_calls = Arc::new(Mutex::new(Vec::<Vec<String>>::new()));
        let test_helper = TestHelper::install(ServerConfig {
            managed_launchd_dirs: vec![daemon_dir.path().to_string_lossy().into_owned()],
            runner: Arc::new(RecordingHelperRunner {
                calls: Arc::clone(&helper_calls),
            }),
            ..ServerConfig::default()
        });
        let harness = Harness::new(
            "",
            HashMap::new(),
            test_helper.client() as Arc<dyn PrivilegedHelper>,
            &[],
        );

        let plist_str = plist_path.to_string_lossy().into_owned();
        let cases = [
            (
                root.clone(),
                Action::Stop,
                vec![vec![
                    "/bin/launchctl".to_string(),
                    "bootout".to_string(),
                    "system/homebrew.mxcl.php".to_string(),
                ]],
            ),
            (
                root.clone(),
                Action::Restart,
                vec![vec![
                    "/bin/launchctl".to_string(),
                    "kickstart".to_string(),
                    "-k".to_string(),
                    "system/homebrew.mxcl.php".to_string(),
                ]],
            ),
            (
                off,
                Action::Start,
                vec![vec![
                    "/bin/launchctl".to_string(),
                    "bootstrap".to_string(),
                    "system".to_string(),
                    plist_str,
                ]],
            ),
        ];

        for (item, action, want) in cases {
            helper_calls.lock().expect("lock").clear();
            harness
                .provider
                .do_action(&item, action)
                .unwrap_or_else(|error| panic!("{action}: {error}"));
            assert_eq!(*helper_calls.lock().expect("lock"), want, "{action}");
        }

        // Disable boots out and removes the plist via helper.
        std::fs::write(
            &plist_path,
            b"<plist version=\"1.0\"><dict><key>Label</key><string>homebrew.mxcl.php</string></dict></plist>",
        )
        .expect("write root plist");
        helper_calls.lock().expect("lock").clear();
        harness
            .provider
            .do_action(&root, Action::Disable)
            .expect("disable root service");
        assert!(
            !plist_path.exists(),
            "expected {} removed on disable",
            plist_path.display()
        );
        let want_bootout = vec![
            "/bin/launchctl".to_string(),
            "bootout".to_string(),
            "system/homebrew.mxcl.php".to_string(),
        ];
        assert!(
            helper_calls
                .lock()
                .expect("lock")
                .iter()
                .any(|call| call == &want_bootout),
            "Disable did not bootout via helper"
        );

        let error = harness
            .provider
            .do_action(&root, Action::Enable)
            .expect_err("a root service cannot be re-registered");
        assert_eq!(error.code, ErrorCode::UnknownAction);
    }

    #[test]
    fn provider_trait_impl_works_as_trait_object() {
        let harness = Harness::new(LIST_JSON, HashMap::new(), Arc::new(UnavailableHelper), &[]);
        let provider: &dyn service::Provider = &harness.provider;
        assert_eq!(provider.source(), Source::Brew);
        assert!(provider.as_restricted().is_none());
        let items = provider.list().expect("trait list");
        assert_eq!(items.len(), 6);
    }
}

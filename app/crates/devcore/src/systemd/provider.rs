//! Systemd user & system service providers (`service::Provider` + `service::Batcher`).

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::fs::atomic_write;
use crate::launchd::{Host, ProcessRunner};
use crate::service::{self, Action, Autostart, Detail, Item, LogFile, Row, Source, State};

use super::parse::{
    UnitListEntry, map_autostart, map_unit_state, parse_list_unit_files, parse_list_units,
    parse_show,
};
use super::unit::{SystemdError, UnitSpec, normalize_service_unit_name, parse_unit};

const UNIT_FILE_MODE: u32 = 0o644;

/// A save request forwarded to the privileged helper for system-level units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemdUnitSave {
    pub unit_name: String,
    pub path: PathBuf,
    pub content: Vec<u8>,
    pub reload: bool,
    pub enable: bool,
    pub start: bool,
}

/// A remove request forwarded to the privileged helper for system-level units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemdUnitRemove {
    pub unit_name: String,
    pub path: PathBuf,
    pub trash_dir: PathBuf,
}

/// Privileged helper seam for `SystemdSystem` mutations.
pub trait SystemdHelper: Send + Sync {
    fn systemd_control(&self, commands: &[Vec<String>]) -> Result<(), SystemdError>;
    fn systemd_save(&self, request: SystemdUnitSave) -> Result<(), SystemdError>;
    fn systemd_remove(&self, request: SystemdUnitRemove) -> Result<(), SystemdError>;
}

/// Helper stub used when no privileged helper is configured.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnavailableSystemdHelper;

impl SystemdHelper for UnavailableSystemdHelper {
    fn systemd_control(&self, _: &[Vec<String>]) -> Result<(), SystemdError> {
        Err(SystemdError::HelperNotInstalled)
    }

    fn systemd_save(&self, _: SystemdUnitSave) -> Result<(), SystemdError> {
        Err(SystemdError::HelperNotInstalled)
    }

    fn systemd_remove(&self, _: SystemdUnitRemove) -> Result<(), SystemdError> {
        Err(SystemdError::HelperNotInstalled)
    }
}

/// Production [`SystemdHelper`] backed by [`crate::helper::Client::require`].
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultSystemdHelper;

impl SystemdHelper for DefaultSystemdHelper {
    fn systemd_control(&self, commands: &[Vec<String>]) -> Result<(), SystemdError> {
        let client = crate::helper::Client::require().map_err(map_helper_error)?;
        client.systemd_control(commands).map_err(map_helper_error)
    }

    fn systemd_save(&self, request: SystemdUnitSave) -> Result<(), SystemdError> {
        let client = crate::helper::Client::require().map_err(map_helper_error)?;
        client
            .systemd_save(&crate::helper::protocol::SystemdSaveRequest {
                unit_content: request.content,
                path: request.path.to_string_lossy().into_owned(),
                unit_name: request.unit_name,
                reload: request.reload,
                enable: request.enable,
                start: request.start,
            })
            .map_err(map_helper_error)
    }

    fn systemd_remove(&self, request: SystemdUnitRemove) -> Result<(), SystemdError> {
        let client = crate::helper::Client::require().map_err(map_helper_error)?;
        client
            .systemd_remove(&crate::helper::protocol::SystemdRemoveRequest {
                path: request.path.to_string_lossy().into_owned(),
                unit_name: request.unit_name,
                trash_dir: request.trash_dir.to_string_lossy().into_owned(),
            })
            .map_err(map_helper_error)
    }
}

fn map_helper_error(error: crate::helper::HelperError) -> SystemdError {
    match error {
        crate::helper::HelperError::NotInstalled => SystemdError::HelperNotInstalled,
        other => SystemdError::helper(other.to_string()),
    }
}

/// Configuration for one [`SystemdProvider`] (user or system).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemdConfig {
    pub source: Source,
    /// Directories scanned in precedence order (earlier overrides later).
    pub scan_dirs: Vec<PathBuf>,
    /// Writable directory where user/admin unit files are saved and may be removed.
    pub write_dir: PathBuf,
    /// `false` for `systemctl --user`, `true` for system `systemctl`.
    pub system: bool,
    /// Whether mutations require root privileges via [`SystemdHelper`].
    pub elevated: bool,
}

/// Manages systemd `.service` units for either the user bus or system bus.
pub struct SystemdProvider {
    config: SystemdConfig,
    runner: Arc<dyn ProcessRunner>,
    host: Arc<dyn Host>,
    helper: Arc<dyn SystemdHelper>,
}

impl SystemdProvider {
    /// Creates a provider for user-level systemd services (`systemctl --user`).
    pub fn user(runner: Arc<dyn ProcessRunner>, host: Arc<dyn Host>) -> Self {
        let home = host.home_dir().unwrap_or_default();
        let user_dir = crate::platform::linux::paths::systemd_user_unit_dir(&home);
        SystemdProvider::new(
            SystemdConfig {
                source: Source::SystemdUser,
                scan_dirs: vec![
                    user_dir.clone(),
                    PathBuf::from("/etc/systemd/user"),
                    PathBuf::from("/usr/lib/systemd/user"),
                ],
                write_dir: user_dir,
                system: false,
                elevated: false,
            },
            runner,
            host,
            Arc::new(UnavailableSystemdHelper),
        )
    }

    /// Creates a provider for system-level systemd services (`systemctl` + privileged helper).
    pub fn system(
        runner: Arc<dyn ProcessRunner>,
        host: Arc<dyn Host>,
        helper: Arc<dyn SystemdHelper>,
    ) -> Self {
        SystemdProvider::new(
            SystemdConfig {
                source: Source::SystemdSystem,
                scan_dirs: vec![
                    PathBuf::from("/etc/systemd/system"),
                    PathBuf::from("/usr/lib/systemd/system"),
                ],
                write_dir: PathBuf::from("/etc/systemd/system"),
                system: true,
                elevated: true,
            },
            runner,
            host,
            helper,
        )
    }

    pub fn new(
        config: SystemdConfig,
        runner: Arc<dyn ProcessRunner>,
        host: Arc<dyn Host>,
        helper: Arc<dyn SystemdHelper>,
    ) -> Self {
        SystemdProvider {
            config,
            runner,
            host,
            helper,
        }
    }

    pub fn config(&self) -> &SystemdConfig {
        &self.config
    }

    /// Loads and parses a `.service` unit file from disk.
    pub fn load_unit(&self, path: &Path) -> Result<UnitSpec, SystemdError> {
        let name = path
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or_else(|| SystemdError::invalid("unit path has no filename"))?;
        let raw = std::fs::read_to_string(path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                SystemdError::not_found(format!("unit file not found: {}", path.display()))
            } else {
                SystemdError::from(error)
            }
        })?;
        parse_unit(name, &raw)
    }

    /// Validates, renders, and saves a `.service` (and optional `.timer`) unit.
    pub fn save_unit(
        &self,
        spec: &UnitSpec,
        enable_and_start: bool,
    ) -> Result<PathBuf, SystemdError> {
        spec.validate()?;
        let filename = spec.unit_filename()?;
        let path = self.config.write_dir.join(&filename);
        let rendered = spec.render_unit();

        if self.config.elevated {
            self.helper.systemd_save(SystemdUnitSave {
                unit_name: filename.clone(),
                path: path.clone(),
                content: rendered.into_bytes(),
                reload: true,
                enable: enable_and_start && !spec.has_timer(),
                start: enable_and_start && !spec.has_timer(),
            })?;
            if let Some(timer_content) = spec.render_timer() {
                let timer_name = spec.timer_filename()?;
                let timer_path = self.config.write_dir.join(&timer_name);
                self.helper.systemd_save(SystemdUnitSave {
                    unit_name: timer_name,
                    path: timer_path,
                    content: timer_content.into_bytes(),
                    reload: true,
                    enable: enable_and_start,
                    start: enable_and_start,
                })?;
            }
            return Ok(path);
        }

        std::fs::create_dir_all(&self.config.write_dir)?;
        atomic_write(&path, rendered.as_bytes(), UNIT_FILE_MODE)?;
        let timer_name = if let Some(timer_content) = spec.render_timer() {
            let timer_name = spec.timer_filename()?;
            let timer_path = self.config.write_dir.join(&timer_name);
            atomic_write(&timer_path, timer_content.as_bytes(), UNIT_FILE_MODE)?;
            Some(timer_name)
        } else {
            None
        };

        self.run_systemctl(&["daemon-reload"])?;
        if enable_and_start {
            let target = timer_name.as_deref().unwrap_or(&filename);
            self.run_systemctl(&["enable", "--now", target])?;
        }
        Ok(path)
    }

    /// Lists all discovered `.service` units merged with live `systemctl` state.
    pub fn list_units(&self) -> Result<Vec<Item>, SystemdError> {
        let unit_files = self.read_unit_files();
        let live_units = self.read_live_units();

        // Scan unit directories in precedence order (first seen wins).
        let mut discovered: BTreeMap<String, PathBuf> = BTreeMap::new();
        let mut timers: BTreeMap<String, PathBuf> = BTreeMap::new();
        for dir in &self.config.scan_dirs {
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(file_name) = path.file_name().and_then(OsStr::to_str) else {
                    continue;
                };
                if file_name.ends_with(".service") && !file_name.contains('@') {
                    discovered.entry(file_name.to_string()).or_insert(path);
                } else if let Some(stem) = file_name.strip_suffix(".timer") {
                    timers.entry(format!("{stem}.service")).or_insert(path);
                }
            }
        }

        let owner = if self.config.system { "system" } else { "user" }.to_string();
        let mut items = Vec::with_capacity(discovered.len());
        for (unit_name, path) in discovered {
            let has_timer = timers.contains_key(&unit_name);
            let parsed = std::fs::read_to_string(&path)
                .ok()
                .and_then(|raw| parse_unit(&unit_name, &raw).ok());
            let live = live_units.get(&unit_name);
            let file_state = unit_files.get(&unit_name).map(String::as_str).unwrap_or("");

            let (state, keep_alive, kind) = match (&parsed, live) {
                (None, _) => (State::Invalid, false, Some("Invalid unit file".to_string())),
                (Some(spec), Some(entry)) => {
                    let keep = matches!(
                        spec.restart.as_str(),
                        "always" | "on-failure" | "on-abnormal" | "on-abort"
                    );
                    let st = map_unit_state(&entry.load, &entry.active, &entry.sub, has_timer);
                    let kind = (!spec.description.is_empty()).then(|| spec.description.clone());
                    (st, keep, kind)
                }
                (Some(spec), None) => {
                    let keep = matches!(
                        spec.restart.as_str(),
                        "always" | "on-failure" | "on-abnormal" | "on-abort"
                    );
                    let st = if has_timer && file_state == "enabled" {
                        State::Scheduled
                    } else {
                        State::Stopped
                    };
                    let kind = (!spec.description.is_empty()).then(|| spec.description.clone());
                    (st, keep, kind)
                }
            };

            let autostart = if state == State::Invalid {
                Autostart::Unknown
            } else {
                map_autostart(file_state)
            };

            items.push(Item {
                id: format!("{}:{}", self.config.source.as_str(), path.display()),
                source: self.config.source,
                name: unit_name,
                state,
                pid: 0,
                autostart,
                owner: owner.clone(),
                path,
                kind,
                admin: self.config.elevated,
                keep_alive,
            });
        }
        items.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(items)
    }

    /// Reads detailed properties and log sources for one unit.
    pub fn detail_unit(&self, item: &Item) -> Result<Detail, SystemdError> {
        let unit_name = normalize_service_unit_name(&item.name)?;
        let raw = std::fs::read_to_string(&item.path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                SystemdError::not_found(format!("unit file not found: {}", item.path.display()))
            } else {
                SystemdError::from(error)
            }
        })?;
        let spec = parse_unit(&unit_name, &raw);
        let show = self
            .run_systemctl(&[
                "show",
                "--no-pager",
                "-p",
                "Id,LoadState,ActiveState,SubState,MainPID,UnitFileState,FragmentPath,Description,Restart,User,Group,NRestarts,Result",
                &unit_name,
            ])
            .ok()
            .map(|out| parse_show(&out))
            .unwrap_or_default();

        let mut rows = Vec::new();
        if let Err(error) = &spec {
            rows.push(Row {
                group: "Status".into(),
                key: "Problem".into(),
                value: error.to_string(),
            });
        } else {
            if !show.active_state.is_empty() {
                let status = if show.sub_state.is_empty() {
                    show.active_state.clone()
                } else {
                    format!("{} ({})", show.active_state, show.sub_state)
                };
                rows.push(Row {
                    group: "Status".into(),
                    key: "Active".into(),
                    value: status,
                });
            }
            if !show.load_state.is_empty() {
                rows.push(Row {
                    group: "Status".into(),
                    key: "Loaded".into(),
                    value: show.load_state.clone(),
                });
            }
            if show.main_pid > 0 {
                rows.push(Row {
                    group: "Status".into(),
                    key: "Main PID".into(),
                    value: show.main_pid.to_string(),
                });
            }
            if !show.unit_file_state.is_empty() {
                rows.push(Row {
                    group: "Status".into(),
                    key: "Autostart".into(),
                    value: show.unit_file_state.clone(),
                });
            }
        }

        let mut logs = Vec::new();
        if let Ok(spec) = spec {
            if !spec.description.is_empty() {
                rows.push(Row {
                    group: "Details".into(),
                    key: "Description".into(),
                    value: spec.description,
                });
            }
            if !spec.exec_start.is_empty() {
                rows.push(Row {
                    group: "Command".into(),
                    key: "ExecStart".into(),
                    value: spec.exec_start.join(" "),
                });
            }
            if !spec.working_directory.is_empty() {
                rows.push(Row {
                    group: "Command".into(),
                    key: "WorkingDirectory".into(),
                    value: spec.working_directory,
                });
            }
            if !spec.restart.is_empty() {
                rows.push(Row {
                    group: "Policy".into(),
                    key: "Restart".into(),
                    value: spec.restart,
                });
            }
            if !spec.user.is_empty() {
                rows.push(Row {
                    group: "Policy".into(),
                    key: "User".into(),
                    value: spec.user,
                });
            }
            if !spec.stdout_path.is_empty() {
                logs.push(LogFile::file("Standard output", &spec.stdout_path));
            }
            if !spec.stderr_path.is_empty() && spec.stderr_path != spec.stdout_path {
                logs.push(LogFile::file("Standard error", &spec.stderr_path));
            }
        }

        rows.push(Row {
            group: "Files".into(),
            key: "Unit file".into(),
            value: item.path.display().to_string(),
        });
        logs.push(LogFile::journal(
            "Systemd journal",
            &unit_name,
            self.config.system,
        ));

        Ok(Detail { rows, logs })
    }

    fn is_writable_unit(&self, path: &Path) -> bool {
        path.parent() == Some(self.config.write_dir.as_path())
    }

    fn run_systemctl(&self, args: &[&str]) -> Result<String, SystemdError> {
        if self.config.system {
            self.runner
                .run("systemctl", args)
                .map_err(SystemdError::from)
        } else {
            let mut full = Vec::with_capacity(args.len() + 1);
            full.push("--user");
            full.extend_from_slice(args);
            self.runner
                .run("systemctl", &full)
                .map_err(SystemdError::from)
        }
    }

    fn read_unit_files(&self) -> BTreeMap<String, String> {
        self.run_systemctl(&[
            "list-unit-files",
            "--type=service",
            "--no-legend",
            "--no-pager",
            "--plain",
        ])
        .map(|out| parse_list_unit_files(&out))
        .unwrap_or_default()
    }

    fn read_live_units(&self) -> BTreeMap<String, UnitListEntry> {
        self.run_systemctl(&[
            "list-units",
            "--type=service",
            "--all",
            "--no-legend",
            "--no-pager",
            "--plain",
        ])
        .map(|out| parse_list_units(&out))
        .unwrap_or_default()
    }

    fn control_commands_for(
        action: Action,
        unit_name: &str,
    ) -> Result<Vec<Vec<String>>, SystemdError> {
        let sub = match action {
            Action::Start | Action::Load => "start",
            Action::Stop | Action::Unload => "stop",
            Action::Restart => "restart",
            Action::Enable => "enable",
            Action::Disable => "disable",
            Action::Remove => {
                return Err(SystemdError::invalid("remove uses dedicated remove path"));
            }
        };
        Ok(vec![vec![sub.to_string(), unit_name.to_string()]])
    }

    fn execute_action(&self, item: &Item, action: Action) -> Result<(), SystemdError> {
        let unit_name = normalize_service_unit_name(&item.name)?;
        if action == Action::Remove {
            return self.remove_unit(item, &unit_name);
        }
        let commands = Self::control_commands_for(action, &unit_name)?;
        if self.config.elevated {
            return self.helper.systemd_control(&commands);
        }
        for cmd in commands {
            let refs: Vec<&str> = cmd.iter().map(String::as_str).collect();
            self.run_systemctl(&refs)?;
        }
        Ok(())
    }

    fn remove_unit(&self, item: &Item, unit_name: &str) -> Result<(), SystemdError> {
        if !self.is_writable_unit(&item.path) {
            return Err(SystemdError::invalid(format!(
                "unit {} is vendor-managed and cannot be removed",
                item.path.display()
            )));
        }
        if self.config.elevated {
            let trash_staging = self
                .host
                .temp_dir()
                .join(format!("devbase-systemd-remove-{}", std::process::id()));
            std::fs::create_dir_all(&trash_staging)?;
            let result = self.helper.systemd_remove(SystemdUnitRemove {
                unit_name: unit_name.to_string(),
                path: item.path.clone(),
                trash_dir: trash_staging.clone(),
            });
            if result.is_ok() {
                let staged = trash_staging.join(unit_name);
                if staged.exists() {
                    let _ = self.host.trash(&staged);
                }
            }
            let _ = std::fs::remove_dir_all(&trash_staging);
            return result;
        }
        let _ = self.run_systemctl(&["stop", unit_name]);
        let _ = self.run_systemctl(&["disable", unit_name]);
        self.host
            .trash(&item.path)
            .map_err(|error| SystemdError::io(error.to_string()))?;
        let _ = self.run_systemctl(&["daemon-reload"]);
        Ok(())
    }
}

impl service::Provider for SystemdProvider {
    fn source(&self) -> Source {
        self.config.source
    }

    fn list(&self) -> service::Result<Vec<Item>> {
        self.list_units().map_err(Into::into)
    }

    fn detail(&self, item: &Item) -> service::Result<Detail> {
        self.detail_unit(item).map_err(Into::into)
    }

    fn actions(&self, item: &Item) -> Vec<Action> {
        let mut actions = Vec::new();
        if item.state == State::Invalid {
            if self.is_writable_unit(&item.path) {
                actions.push(Action::Remove);
            }
            return actions;
        }
        match item.state {
            State::Running | State::Scheduled => {
                actions.push(Action::Stop);
                actions.push(Action::Restart);
            }
            State::Failed => {
                actions.push(Action::Restart);
                actions.push(Action::Stop);
            }
            State::Stopped | State::Loaded | State::None => {
                actions.push(Action::Start);
            }
            State::Invalid => {}
        }
        match item.autostart {
            Autostart::On => actions.push(Action::Disable),
            Autostart::Off => actions.push(Action::Enable),
            Autostart::Unknown => {}
        }
        if self.is_writable_unit(&item.path) {
            actions.push(Action::Remove);
        }
        actions
    }

    fn confirm(&self, item: &Item, action: Action) -> Option<String> {
        match action {
            Action::Remove => Some(format!("Stop, disable, and move {} to Trash?", item.name)),
            Action::Stop if self.config.system => Some(format!(
                "Stop system service {}? This may affect other users or system daemons.",
                item.name
            )),
            Action::Disable if self.config.system => Some(format!(
                "Disable system service {} from starting at boot?",
                item.name
            )),
            _ => None,
        }
    }

    fn do_action(&self, item: &Item, action: Action) -> service::Result<()> {
        self.execute_action(item, action).map_err(Into::into)
    }
}

impl service::Batcher for SystemdProvider {
    fn do_all(&self, items: &[Item], action: Action) -> service::Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        if action == Action::Remove {
            for item in items {
                self.execute_action(item, action)?;
            }
            return Ok(());
        }
        let mut commands = Vec::with_capacity(items.len());
        for item in items {
            let unit_name = normalize_service_unit_name(&item.name)?;
            commands.extend(Self::control_commands_for(action, &unit_name)?);
        }
        if self.config.elevated {
            self.helper.systemd_control(&commands)?;
        } else {
            for cmd in commands {
                let refs: Vec<&str> = cmd.iter().map(String::as_str).collect();
                self.run_systemctl(&refs)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tempfile::TempDir;

    use super::*;
    use crate::launchd::{CommandError, LaunchdError};
    use crate::service::{Batcher, LogSource, Provider};

    struct FakeRunner {
        calls: Mutex<Vec<(String, Vec<String>)>>,
        list_unit_files: String,
        list_units: String,
        show: String,
    }

    impl FakeRunner {
        fn new() -> Self {
            FakeRunner {
                calls: Mutex::new(Vec::new()),
                list_unit_files: String::new(),
                list_units: String::new(),
                show: String::new(),
            }
        }
    }

    impl ProcessRunner for FakeRunner {
        fn run(&self, program: &str, args: &[&str]) -> Result<String, CommandError> {
            self.calls.lock().unwrap().push((
                program.to_string(),
                args.iter().map(|a| (*a).to_string()).collect(),
            ));
            if args.contains(&"list-unit-files") {
                return Ok(self.list_unit_files.clone());
            }
            if args.contains(&"list-units") {
                return Ok(self.list_units.clone());
            }
            if args.contains(&"show") {
                return Ok(self.show.clone());
            }
            Ok(String::new())
        }
    }

    #[derive(Default)]
    struct FakeHost {
        home: PathBuf,
        trashed: Mutex<Vec<PathBuf>>,
    }

    impl Host for FakeHost {
        fn uid(&self) -> u32 {
            1000
        }
        fn home_dir(&self) -> Result<PathBuf, LaunchdError> {
            Ok(self.home.clone())
        }
        fn temp_dir(&self) -> PathBuf {
            self.home.join("tmp")
        }
        fn current_exe(&self) -> Result<PathBuf, LaunchdError> {
            Ok(PathBuf::from("/usr/bin/devbase"))
        }
        fn eval_symlinks(&self, path: &Path) -> Option<PathBuf> {
            Some(path.to_path_buf())
        }
        fn trash(&self, path: &Path) -> Result<(), LaunchdError> {
            self.trashed.lock().unwrap().push(path.to_path_buf());
            std::fs::remove_file(path).map_err(|e| LaunchdError::io(e.to_string()))
        }
    }

    #[derive(Default)]
    struct FakeHelper {
        controls: Mutex<Vec<Vec<Vec<String>>>>,
        saves: Mutex<Vec<SystemdUnitSave>>,
        removes: Mutex<Vec<SystemdUnitRemove>>,
    }

    impl SystemdHelper for FakeHelper {
        fn systemd_control(&self, commands: &[Vec<String>]) -> Result<(), SystemdError> {
            self.controls.lock().unwrap().push(commands.to_vec());
            Ok(())
        }
        fn systemd_save(&self, request: SystemdUnitSave) -> Result<(), SystemdError> {
            self.saves.lock().unwrap().push(request);
            Ok(())
        }
        fn systemd_remove(&self, request: SystemdUnitRemove) -> Result<(), SystemdError> {
            self.removes.lock().unwrap().push(request);
            Ok(())
        }
    }

    #[test]
    fn user_provider_lists_details_controls_saves_and_removes() {
        let temp = TempDir::new().unwrap();
        let user_dir = temp.path().join("user");
        let vendor_dir = temp.path().join("vendor");
        std::fs::create_dir_all(&user_dir).unwrap();
        std::fs::create_dir_all(&vendor_dir).unwrap();

        std::fs::write(
            user_dir.join("api.service"),
            "[Unit]\nDescription=API\n[Service]\nExecStart=/usr/bin/api\nRestart=on-failure\nStandardOutput=append:/tmp/api.log\n",
        )
        .unwrap();
        std::fs::write(
            vendor_dir.join("vendor.service"),
            "[Unit]\nDescription=Vendor\n[Service]\nExecStart=/usr/bin/vendor\n",
        )
        .unwrap();

        let mut runner = FakeRunner::new();
        runner.list_unit_files = "api.service enabled enabled\nvendor.service static -\n".into();
        runner.list_units =
            "api.service loaded active running API\nvendor.service loaded inactive dead Vendor\n"
                .into();
        runner.show = "Id=api.service\nLoadState=loaded\nActiveState=active\nSubState=running\nMainPID=777\nUnitFileState=enabled\n".into();
        let runner = Arc::new(runner);
        let host = Arc::new(FakeHost {
            home: temp.path().to_path_buf(),
            trashed: Mutex::new(Vec::new()),
        });

        let provider = SystemdProvider::new(
            SystemdConfig {
                source: Source::SystemdUser,
                scan_dirs: vec![user_dir.clone(), vendor_dir.clone()],
                write_dir: user_dir.clone(),
                system: false,
                elevated: false,
            },
            runner.clone(),
            host.clone(),
            Arc::new(UnavailableSystemdHelper),
        );

        let items = provider.list().unwrap();
        assert_eq!(items.len(), 2);
        let api = items.iter().find(|i| i.name == "api.service").unwrap();
        assert_eq!(api.state, State::Running);
        assert_eq!(api.autostart, Autostart::On);
        assert!(api.keep_alive);
        assert!(provider.actions(api).contains(&Action::Remove));

        let vendor = items.iter().find(|i| i.name == "vendor.service").unwrap();
        assert_eq!(vendor.state, State::Stopped);
        assert!(!provider.actions(vendor).contains(&Action::Remove));

        let detail = provider.detail(api).unwrap();
        assert!(
            detail
                .rows
                .iter()
                .any(|r| r.key == "Main PID" && r.value == "777")
        );
        assert!(
            detail
                .logs
                .iter()
                .any(|l| l.source() == LogSource::File(Path::new("/tmp/api.log")))
        );
        assert!(detail.logs.iter().any(|l| {
            l.source()
                == LogSource::Journal {
                    unit: "api.service",
                    system: false,
                }
        }));

        provider.do_action(api, Action::Restart).unwrap();
        assert!(runner.calls.lock().unwrap().iter().any(|(p, args)| {
            p == "systemctl" && args == &["--user", "restart", "api.service"]
        }));

        // Save a new unit
        let saved = provider
            .save_unit(
                &UnitSpec {
                    name: "worker".into(),
                    description: "Worker".into(),
                    exec_start: vec!["/usr/bin/worker".into()],
                    wanted_by: vec!["default.target".into()],
                    ..UnitSpec::default()
                },
                true,
            )
            .unwrap();
        assert_eq!(saved, user_dir.join("worker.service"));
        assert!(saved.exists());

        // Remove user unit
        provider.do_action(api, Action::Remove).unwrap();
        assert!(!user_dir.join("api.service").exists());
        assert_eq!(
            host.trashed.lock().unwrap().as_slice(),
            &[user_dir.join("api.service")]
        );
    }

    #[test]
    fn system_provider_routes_mutations_and_batches_through_helper() {
        let temp = TempDir::new().unwrap();
        let sys_dir = temp.path().join("system");
        std::fs::create_dir_all(&sys_dir).unwrap();
        let unit_a = sys_dir.join("a.service");
        let unit_b = sys_dir.join("b.service");
        std::fs::write(
            &unit_a,
            "[Unit]\nDescription=A\n[Service]\nExecStart=/bin/a\n",
        )
        .unwrap();
        std::fs::write(
            &unit_b,
            "[Unit]\nDescription=B\n[Service]\nExecStart=/bin/b\n",
        )
        .unwrap();

        let runner = Arc::new(FakeRunner::new());
        let host = Arc::new(FakeHost {
            home: temp.path().to_path_buf(),
            trashed: Mutex::new(Vec::new()),
        });
        let helper = Arc::new(FakeHelper::default());
        let provider = SystemdProvider::new(
            SystemdConfig {
                source: Source::SystemdSystem,
                scan_dirs: vec![sys_dir.clone()],
                write_dir: sys_dir.clone(),
                system: true,
                elevated: true,
            },
            runner,
            host,
            helper.clone(),
        );

        let items = provider.list().unwrap();
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|i| i.admin));

        provider.do_all(&items, Action::Start).unwrap();
        let batches = helper.controls.lock().unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(
            batches[0],
            vec![
                vec!["start".to_string(), "a.service".to_string()],
                vec!["start".to_string(), "b.service".to_string()],
            ]
        );
    }
}

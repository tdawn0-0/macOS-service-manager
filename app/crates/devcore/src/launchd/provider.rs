//! The three launchd providers: user agents, system agents and daemons.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::service::{self, Action, Autostart, Detail, Item, LogFile, Row, Source, State};

use super::error::LaunchdError;
use super::host::{
    Host, LaunchdRemoveRequest, LaunchdSaveRequest, PrivilegedHelper, ProcessRunner,
};
use super::parse::{self, DomainState};
use super::plist_file::{self, FileModel};
use super::spec::{self, Spec};

/// What an invalid property list is.
pub const INVALID_KIND: &str = "Invalid property list";

/// Everything the providers use to touch the machine.
#[derive(Clone)]
pub struct Context {
    pub runner: Arc<dyn ProcessRunner>,
    pub helper: Arc<dyn PrivilegedHelper>,
    pub host: Arc<dyn Host>,
}

impl Context {
    pub fn new(
        runner: Arc<dyn ProcessRunner>,
        helper: Arc<dyn PrivilegedHelper>,
        host: Arc<dyn Host>,
    ) -> Self {
        Context {
            runner,
            helper,
            host,
        }
    }
}

/// Which launch agents or daemons a provider manages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub source: Source,
    /// The directory holding the property lists.
    pub dir: PathBuf,
    /// The launchd domain the jobs load into: `gui/<uid>` or `system`.
    pub domain: String,
    /// Changing jobs takes root, which runs through the privileged helper.
    pub elevated: bool,
}

/// A provider for one directory of launch agents or daemons.
pub struct LaunchdProvider {
    config: Config,
    ctx: Arc<Context>,
}

/// The result of saving a job that only root can write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveOutcome {
    pub path: PathBuf,
    /// `None` means fully saved and loaded (if requested); `Some` means the
    /// file was saved but loading or unloading failed.
    pub load_error: Option<String>,
}

impl LaunchdProvider {
    /// Manages `~/Library/LaunchAgents`.
    pub fn user_agents(ctx: Arc<Context>) -> Self {
        let home = ctx.host.home_dir().unwrap_or_default();
        LaunchdProvider::new(
            Config {
                source: Source::UserAgent,
                dir: home.join("Library/LaunchAgents"),
                domain: gui_domain(ctx.host.uid()),
                elevated: false,
            },
            ctx,
        )
    }

    /// Manages `/Library/LaunchAgents`. Its jobs load into the user's domain,
    /// so the user can run them, but cannot change their files.
    pub fn system_agents(ctx: Arc<Context>) -> Self {
        LaunchdProvider::new(
            Config {
                source: Source::SystemAgent,
                dir: PathBuf::from("/Library/LaunchAgents"),
                domain: gui_domain(ctx.host.uid()),
                elevated: false,
            },
            ctx,
        )
    }

    /// Manages `/Library/LaunchDaemons`, which take root to change.
    pub fn daemons(ctx: Arc<Context>) -> Self {
        LaunchdProvider::new(
            Config {
                source: Source::Daemon,
                dir: PathBuf::from("/Library/LaunchDaemons"),
                domain: "system".to_string(),
                elevated: true,
            },
            ctx,
        )
    }

    pub fn new(config: Config, ctx: Arc<Context>) -> Self {
        LaunchdProvider { config, ctx }
    }

    pub fn source(&self) -> Source {
        self.config.source
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    #[cfg(test)]
    pub(crate) fn ctx(&self) -> Arc<Context> {
        Arc::clone(&self.ctx)
    }

    /// Reads the domain's jobs and disabled overrides from `launchctl print`.
    pub fn state(&self) -> Result<DomainState, LaunchdError> {
        let out = self
            .ctx
            .runner
            .run("launchctl", &["print", &self.config.domain])
            .map_err(LaunchdError::from)?;
        Ok(parse::parse_print(&out))
    }

    pub(crate) fn item(&self, path: &Path, file: &FileModel, state: &DomainState) -> Item {
        let (mut item_state, pid) = state.of(&file.label);
        if item_state == State::Loaded && file.scheduled() {
            item_state = State::Scheduled;
        }
        Item {
            id: format!("{}:{}", self.config.source.as_str(), path.display()),
            source: self.config.source,
            name: file.label.clone(),
            state: item_state,
            pid,
            autostart: if file.disabled_in(state) {
                Autostart::Off
            } else {
                Autostart::On
            },
            owner: self.config.domain.clone(),
            path: path.to_path_buf(),
            kind: None,
            admin: self.config.elevated,
            keep_alive: file.keep_alive(),
        }
    }

    /// The item of a property list launchd cannot load.
    pub(crate) fn invalid(&self, path: &Path) -> Item {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = name.strip_suffix(".plist").unwrap_or(&name).to_string();
        Item {
            id: format!("{}:{}", self.config.source.as_str(), path.display()),
            source: self.config.source,
            name,
            state: State::Invalid,
            pid: 0,
            autostart: Autostart::Unknown,
            owner: self.config.domain.clone(),
            path: path.to_path_buf(),
            kind: Some(INVALID_KIND.to_string()),
            admin: self.config.elevated,
            keep_alive: false,
        }
    }

    /// The items of the source. Always reads `launchctl print` first.
    pub fn list(&self) -> Result<Vec<Item>, LaunchdError> {
        let state = self.state()?;
        Ok(self.scan(&state))
    }

    /// Lists the jobs of the directory, sorted by label. A `.plist` file that
    /// is not a property list with a label is listed as invalid, by its file
    /// name: launchd cannot load it, which is what someone looking here wants
    /// to know.
    fn scan(&self, state: &DomainState) -> Vec<Item> {
        let mut items = Vec::new();
        let entries = match std::fs::read_dir(&self.config.dir) {
            Ok(entries) => entries,
            Err(_) => return items,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension() != Some(std::ffi::OsStr::new("plist")) {
                continue;
            }
            match plist_file::read_file(&path) {
                Ok(file) if !file.label.is_empty() => items.push(self.item(&path, &file, state)),
                _ => items.push(self.invalid(&path)),
            }
        }
        items.sort_by(|a, b| a.name.cmp(&b.name));
        items
    }

    pub fn detail(&self, item: &Item) -> Result<Detail, LaunchdError> {
        if item.state == State::Invalid {
            if std::fs::metadata(&item.path).is_err() {
                return Err(LaunchdError::not_found(format!(
                    "plist file not found: {}",
                    item.path.display()
                )));
            }
            return Ok(Detail {
                rows: vec![
                    Row {
                        group: "Status".to_string(),
                        key: "Problem".to_string(),
                        value: plist_file::problem(&item.path),
                    },
                    Row {
                        group: "Files".to_string(),
                        key: "Property list".to_string(),
                        value: item.path.display().to_string(),
                    },
                ],
                logs: Vec::new(),
            });
        }
        let file = plist_file::read_file(&item.path)?;
        // The state may have changed since the list.
        let state = self.state()?;
        Ok(detail_of(
            &self.item(&item.path, &file, &state),
            &file,
            &state,
        ))
    }

    pub fn actions(&self, item: &Item) -> Vec<Action> {
        if item.state == State::Invalid {
            // Its label is unknown, so nothing can address a job of it. Only a
            // file of the user's own can go, without asking launchd anything.
            if self.config.source == Source::UserAgent {
                return vec![Action::Remove];
            }
            return Vec::new();
        }
        let mut actions = match item.state {
            State::Running if item.keep_alive => {
                // Stopping it unloads it: Unload would do the same.
                vec![Action::Stop, Action::Restart]
            }
            State::Running => vec![Action::Stop, Action::Restart, Action::Unload],
            State::Loaded | State::Scheduled | State::Failed => {
                vec![Action::Start, Action::Unload]
            }
            _ => vec![Action::Load],
        };
        if item.autostart == Autostart::Off {
            actions.push(Action::Enable);
        } else {
            actions.push(Action::Disable);
        }
        actions.push(Action::Remove);
        actions
    }

    pub fn confirm(&self, item: &Item, action: Action) -> Option<String> {
        let base = spec::file_name(&item.path);
        match action {
            Action::Remove if item.state == State::Invalid => Some(format!(
                "Move {base} to the Trash? launchd cannot load it as it is."
            )),
            Action::Remove if self.config.source == Source::UserAgent => Some(format!(
                "Unload {} and move {base} to the Trash?",
                item.name
            )),
            Action::Remove => {
                let folder = item
                    .path
                    .parent()
                    .and_then(Path::file_name)
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                Some(format!(
                    "This unloads the job and moves its file to the Trash, which is not an undo: Put Back cannot return it to the {folder} folder. To get it back, reinstall the software it came from, or install the file again as root. Until then that software may stop working."
                ))
            }
            Action::Unload | Action::Disable if self.config.elevated => Some(format!(
                "{} is a system daemon. Changing it can affect the software that depends on it. Continue?",
                item.name
            )),
            _ => None,
        }
    }

    /// What launchctl runs for an action on an item.
    fn commands(&self, item: &Item, action: Action) -> Result<Vec<Vec<String>>, LaunchdError> {
        if item.state == State::Invalid {
            return Err(LaunchdError::invalid_item(format!(
                "{} is not a job launchd can load",
                spec::file_name(&item.path)
            )));
        }
        let target = format!("{}/{}", self.config.domain, item.name);
        let commands = match action {
            Action::Load => {
                if item.autostart == Autostart::Off {
                    // A disabled job cannot be loaded.
                    vec![
                        vec!["enable".to_string(), target],
                        vec![
                            "bootstrap".to_string(),
                            self.config.domain.clone(),
                            item.path.display().to_string(),
                        ],
                    ]
                } else {
                    vec![vec![
                        "bootstrap".to_string(),
                        self.config.domain.clone(),
                        item.path.display().to_string(),
                    ]]
                }
            }
            Action::Unload => vec![vec!["bootout".to_string(), target]],
            Action::Start => vec![vec!["kickstart".to_string(), target]],
            Action::Restart => vec![vec!["kickstart".to_string(), "-k".to_string(), target]],
            Action::Stop => {
                if item.keep_alive {
                    // launchd would start it again at once.
                    vec![vec!["bootout".to_string(), target]]
                } else {
                    vec![vec!["kill".to_string(), "SIGTERM".to_string(), target]]
                }
            }
            Action::Enable => vec![vec!["enable".to_string(), target]],
            Action::Disable => vec![vec!["disable".to_string(), target]],
            Action::Remove => {
                return Err(LaunchdError::unknown_action(action.as_str()));
            }
        };
        Ok(commands)
    }

    fn launchctl(&self, args: &[&str]) -> Result<(), LaunchdError> {
        let owned: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
        self.launchctls(&[owned])
    }

    /// Runs launchctl once for each list of arguments, stopping at the first
    /// failure. A provider that needs root sends them all to the privileged
    /// helper in one request.
    fn launchctls(&self, commands: &[Vec<String>]) -> Result<(), LaunchdError> {
        if !self.config.elevated {
            for args in commands {
                let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
                self.ctx
                    .runner
                    .run("launchctl", &borrowed)
                    .map_err(LaunchdError::from)?;
            }
            return Ok(());
        }
        let helper = self.ctx.helper.require()?;
        helper.launchd_control(commands)
    }

    pub fn do_action(&self, item: &Item, action: Action) -> Result<(), LaunchdError> {
        if item.state == State::Invalid && !service::has(&self.actions(item), action) {
            return Err(LaunchdError::invalid_item(format!(
                "{} is not a job launchd can load, so {} does not apply",
                spec::file_name(&item.path),
                action.label()
            )));
        }
        if action == Action::Remove {
            if self.config.source != Source::UserAgent {
                return self.remove_as_root(item);
            }
            if item.state != State::Stopped && item.state != State::Invalid {
                // Not loaded is fine too.
                let _ =
                    self.launchctl(&["bootout", &format!("{}/{}", self.config.domain, item.name)]);
            }
            return self.ctx.host.trash(&item.path);
        }
        let commands = self.commands(item, action)?;
        self.launchctls(&commands)
    }

    /// Runs an action on several items, in one request to the privileged helper
    /// where they need root.
    pub fn do_all(&self, items: &[Item], action: Action) -> Result<(), LaunchdError> {
        let mut all = Vec::new();
        for item in items {
            all.extend(self.commands(item, action)?);
        }
        self.launchctls(&all)
    }

    /// Writes a job into the provider's directory, which only root can write
    /// to, owned as launchd demands, and loads it if asked, in one request to
    /// the privileged helper.
    pub fn save_as_root(
        &self,
        spec: &Spec,
        existing: Option<&Path>,
        load: bool,
    ) -> Result<SaveOutcome, LaunchdError> {
        let mut spec = spec.clone();
        let existing = existing.filter(|path| !path.as_os_str().is_empty());
        spec.existing = existing.is_some();
        spec.validate()?;
        let (path, current) = match existing {
            None => {
                if self.config.dir.as_os_str().is_empty() || !self.config.dir.is_absolute() {
                    return Err(LaunchdError::validation(
                        "provider directory is not configured",
                    ));
                }
                let dir = spec::clean_path(&self.config.dir);
                let path = dir.join(format!("{}.plist", spec.label));
                if path.parent() != Some(dir.as_path()) {
                    return Err(LaunchdError::validation(
                        "target path is outside provider directory",
                    ));
                }
                if std::fs::metadata(&path).is_ok() {
                    return Err(LaunchdError::validation(format!(
                        "{} already exists",
                        spec::file_name(&path)
                    )));
                }
                (path, None)
            }
            Some(existing) => {
                let path = self.check_managed_path(existing)?;
                let data =
                    std::fs::read(&path).map_err(|error| LaunchdError::io(error.to_string()))?;
                (path, Some(data))
            }
        };
        // launchd only says "Input/output error" of a user that does not exist.
        if !spec.user_name.is_empty()
            && self.ctx.runner.run("id", &["-u", &spec.user_name]).is_err()
        {
            return Err(LaunchdError::validation(format!(
                "there is no user {:?}",
                spec.user_name
            )));
        }
        if !spec.group_name.is_empty() {
            let group = format!("/Groups/{}", spec.group_name);
            if self
                .ctx
                .runner
                .run("dscl", &[".", "-read", &group, "PrimaryGroupID"])
                .is_err()
            {
                return Err(LaunchdError::validation(format!(
                    "there is no group {:?}",
                    spec.group_name
                )));
            }
        }
        let data = spec.marshal(current.as_deref())?;
        if data.len() > 64 << 10 {
            return Err(LaunchdError::validation(
                "job configuration exceeds authorized command size limit",
            ));
        }
        let tmp = spec::write_checked(&self.ctx.host.temp_dir(), &data, &*self.ctx.runner)?;
        let outcome = self.finish_save(&spec, &path, existing, load, &data);
        let _ = std::fs::remove_file(&tmp);
        outcome
    }

    fn finish_save(
        &self,
        spec: &Spec,
        path: &Path,
        existing: Option<&Path>,
        load: bool,
        data: &[u8],
    ) -> Result<SaveOutcome, LaunchdError> {
        // Agents load into the user's domain, for which the user needs no root.
        let in_script = load && self.config.elevated;
        let target = format!("{}/{}", self.config.domain, spec.label);
        let helper = self.ctx.helper.require()?;
        let response = helper.launchd_save(LaunchdSaveRequest {
            plist: data.to_vec(),
            path: path.to_path_buf(),
            domain: self.config.domain.clone(),
            target: target.clone(),
            existing_path: existing.map(Path::to_path_buf),
            load: in_script,
        })?;
        if let Some(message) = response.error {
            if response.saved {
                return Ok(SaveOutcome {
                    path: path.to_path_buf(),
                    load_error: Some(format!("saved, but it did not load: {message}")),
                });
            }
            return Err(LaunchdError::helper(message));
        }
        if load && !self.config.elevated {
            if existing.is_some() {
                // Not being loaded is fine too.
                let _ = self.launchctl(&["bootout", &target]);
            }
            if let Err(error) = self.launchctl(&[
                "bootstrap",
                &self.config.domain,
                &path.display().to_string(),
            ]) {
                return Ok(SaveOutcome {
                    path: path.to_path_buf(),
                    load_error: Some(format!("saved, but it did not load: {error}")),
                });
            }
        }
        Ok(SaveOutcome {
            path: path.to_path_buf(),
            load_error: None,
        })
    }

    fn check_managed_path(&self, path: &Path) -> Result<PathBuf, LaunchdError> {
        spec::check_managed_path(&self.config.dir, path)
    }

    /// Unloads a job only root can remove via the privileged helper and moves
    /// its file to the user's Trash.
    fn remove_as_root(&self, item: &Item) -> Result<(), LaunchdError> {
        let source_path = self.check_managed_path(&item.path)?;
        let dir = create_temp_dir(&self.ctx.host.temp_dir(), "msm-removed-")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        let moved = dir.join(
            source_path
                .file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("file")),
        );
        let helper = match self.ctx.helper.require() {
            Ok(helper) => helper,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(error);
            }
        };
        let request = LaunchdRemoveRequest {
            path: source_path,
            target: format!("{}/{}", self.config.domain, item.name),
            trash_dir: dir.clone(),
        };
        if let Err(error) = helper.launchd_remove(request) {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(error);
        }
        if let Err(error) = self.ctx.host.trash(&moved) {
            return Err(LaunchdError::io(format!(
                "removed, but it is at {} and not in the Trash: {}",
                moved.display(),
                error
            )));
        }
        let _ = std::fs::remove_dir(&dir);
        Ok(())
    }
}

fn create_temp_dir(parent: &Path, prefix: &str) -> Result<PathBuf, LaunchdError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    for _ in 0..100 {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!("{prefix}{}-{n}", std::process::id()));
        match std::fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(LaunchdError::io(error.to_string())),
        }
    }
    Err(LaunchdError::io("could not create a temporary directory"))
}

fn detail_of(item: &Item, file: &FileModel, state: &DomainState) -> Detail {
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
    if item.pid > 0 {
        add(
            &mut detail.rows,
            "Status",
            "Process ID",
            &item.pid.to_string(),
        );
    }
    if let Some(job) = state.jobs.get(&item.name)
        && job.exit != 0
    {
        add(
            &mut detail.rows,
            "Status",
            "Last exit code",
            &job.exit.to_string(),
        );
    }
    add(&mut detail.rows, "Status", "Runs", &file.runs());
    add(&mut detail.rows, "Status", "Label", &item.name);
    add(&mut detail.rows, "Status", "Domain", &item.owner);
    let command = match &file.program_arguments {
        Some(args) => args.join(" "),
        None => file.program.clone().unwrap_or_default(),
    };
    add(&mut detail.rows, "Program", "Command", &command);
    add(
        &mut detail.rows,
        "Program",
        "Working directory",
        file.working_directory.as_deref().unwrap_or_default(),
    );
    for (key, value) in &file.environment {
        add(&mut detail.rows, "Environment", key, value);
    }
    add(
        &mut detail.rows,
        "Files",
        "Property list",
        &item.path.display().to_string(),
    );
    add(
        &mut detail.rows,
        "Files",
        "Output log",
        file.standard_out_path.as_deref().unwrap_or_default(),
    );
    add(
        &mut detail.rows,
        "Files",
        "Error log",
        file.standard_error_path.as_deref().unwrap_or_default(),
    );
    if let Some(path) = &file.standard_out_path
        && !path.is_empty()
    {
        detail.logs.push(LogFile {
            label: "Output".to_string(),
            path: PathBuf::from(path),
        });
    }
    if let Some(path) = &file.standard_error_path {
        let same_as_output = file.standard_out_path.as_deref() == Some(path.as_str());
        if !path.is_empty() && !same_as_output {
            detail.logs.push(LogFile {
                label: "Errors".to_string(),
                path: PathBuf::from(path),
            });
        }
    }
    detail
}

fn gui_domain(uid: u32) -> String {
    format!("gui/{uid}")
}

impl service::Provider for LaunchdProvider {
    fn source(&self) -> Source {
        LaunchdProvider::source(self)
    }

    fn list(&self) -> service::Result<Vec<Item>> {
        LaunchdProvider::list(self).map_err(|error| Box::new(error) as service::BoxError)
    }

    fn detail(&self, item: &Item) -> service::Result<Detail> {
        LaunchdProvider::detail(self, item).map_err(|error| Box::new(error) as service::BoxError)
    }

    fn actions(&self, item: &Item) -> Vec<Action> {
        LaunchdProvider::actions(self, item)
    }

    fn confirm(&self, item: &Item, action: Action) -> Option<String> {
        LaunchdProvider::confirm(self, item, action)
    }

    fn do_action(&self, item: &Item, action: Action) -> service::Result<()> {
        LaunchdProvider::do_action(self, item, action)
            .map_err(|error| Box::new(error) as service::BoxError)
    }
}

impl service::Batcher for LaunchdProvider {
    fn do_all(&self, items: &[Item], action: Action) -> service::Result<()> {
        LaunchdProvider::do_all(self, items, action)
            .map_err(|error| Box::new(error) as service::BoxError)
    }
}

/// The launchd sources, in the order the interface shows them.
pub struct Launchd {
    providers: BTreeMap<Source, Arc<LaunchdProvider>>,
}

impl Launchd {
    pub fn new(ctx: Arc<Context>) -> Self {
        let mut providers = BTreeMap::new();
        for provider in [
            LaunchdProvider::user_agents(Arc::clone(&ctx)),
            LaunchdProvider::system_agents(Arc::clone(&ctx)),
            LaunchdProvider::daemons(Arc::clone(&ctx)),
        ] {
            providers.insert(provider.source(), Arc::new(provider));
        }
        Launchd { providers }
    }

    pub fn provider(&self, source: Source) -> Option<&Arc<LaunchdProvider>> {
        self.providers.get(&source)
    }

    pub fn providers(&self) -> impl Iterator<Item = (&Source, &Arc<LaunchdProvider>)> {
        self.providers.iter()
    }

    /// One result per launchd source, preserving [`Source::ALL`] order.
    pub fn list_all(&self) -> Vec<(Source, Result<Vec<Item>, LaunchdError>)> {
        Source::ALL
            .iter()
            .filter_map(|source| {
                self.providers
                    .get(source)
                    .map(|provider| (*source, provider.list()))
            })
            .collect()
    }
}

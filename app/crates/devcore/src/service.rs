//! The model every source of startup items shares.
//!
//! This is a direct port of `internal/service/service.go`. The go app's
//! `Provider` interface takes a concrete error type here; callers that need
//! the typed launchd error use [`crate::launchd::LaunchdProvider`]'s inherent
//! methods.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// The error type returned by the object-safe [`Provider`] and [`Batcher`]
/// traits. Concrete providers expose their own typed errors alongside.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The result type of the object-safe provider traits.
pub type Result<T, E = BoxError> = std::result::Result<T, E>;

/// Where an item comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    Brew,
    LoginItem,
    UserAgent,
    SystemAgent,
    Daemon,
    SystemdUser,
    SystemdSystem,
    Autostart,
}

impl Source {
    /// The macOS sources in the order the interface shows them.
    pub const ALL: [Source; 5] = [
        Source::Brew,
        Source::LoginItem,
        Source::UserAgent,
        Source::SystemAgent,
        Source::Daemon,
    ];

    /// The macOS sources.
    pub const MACOS: [Source; 5] = Self::ALL;

    /// The Linux sources in the order the interface shows them.
    pub const LINUX: [Source; 3] = [
        Source::SystemdUser,
        Source::SystemdSystem,
        Source::Autostart,
    ];

    /// The sources supported on the current compiled host platform.
    pub const fn platform_all() -> &'static [Source] {
        #[cfg(target_os = "macos")]
        {
            &Self::MACOS
        }
        #[cfg(not(target_os = "macos"))]
        {
            &Self::LINUX
        }
    }

    /// Parses a source from its stable kebab-case slug.
    pub fn parse(slug: &str) -> Option<Source> {
        match slug {
            "brew" => Some(Source::Brew),
            "login-item" => Some(Source::LoginItem),
            "user-agent" => Some(Source::UserAgent),
            "system-agent" => Some(Source::SystemAgent),
            "daemon" => Some(Source::Daemon),
            "systemd-user" => Some(Source::SystemdUser),
            "systemd-system" => Some(Source::SystemdSystem),
            "autostart" => Some(Source::Autostart),
            _ => None,
        }
    }

    /// The stable string form, as stored in `state.json` and item IDs.
    pub const fn as_str(self) -> &'static str {
        match self {
            Source::Brew => "brew",
            Source::LoginItem => "login-item",
            Source::UserAgent => "user-agent",
            Source::SystemAgent => "system-agent",
            Source::Daemon => "daemon",
            Source::SystemdUser => "systemd-user",
            Source::SystemdSystem => "systemd-system",
            Source::Autostart => "autostart",
        }
    }

    /// Alias for [`Source::as_str`] (kebab-case CLI/UI identifier).
    pub const fn slug(self) -> &'static str {
        self.as_str()
    }

    /// Plural group heading shown in navigation and section titles.
    pub const fn title(self) -> &'static str {
        match self {
            Source::Brew => "Homebrew",
            Source::UserAgent => "User Agents",
            Source::SystemAgent => "System Agents",
            Source::Daemon => "Daemons",
            Source::LoginItem => "Login Items",
            Source::SystemdUser => "User Services",
            Source::SystemdSystem => "System Services",
            Source::Autostart => "Autostart",
        }
    }

    /// Singular human label shown on badges and detail rows.
    pub const fn label(self) -> &'static str {
        match self {
            Source::Brew => "Homebrew",
            Source::LoginItem => "Login Item",
            Source::UserAgent => "User Agent",
            Source::SystemAgent => "System Agent",
            Source::Daemon => "Daemon",
            Source::SystemdUser => "User Service",
            Source::SystemdSystem => "System Service",
            Source::Autostart => "Autostart",
        }
    }

    /// Whether mutating items in this source requires root privileges via the
    /// privileged helper.
    pub const fn is_privileged(self) -> bool {
        matches!(self, Source::Daemon | Source::SystemdSystem)
    }

    /// Whether users can author new service definitions in this source.
    pub const fn is_creatable(self) -> bool {
        matches!(
            self,
            Source::UserAgent | Source::Daemon | Source::SystemdUser | Source::SystemdSystem
        )
    }
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What an item is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    /// The source knows of no process: a login item that is not open.
    None,
    /// Not running, and not loaded where that applies.
    Stopped,
    /// Registered with launchd, with no process now.
    Loaded,
    /// A process runs.
    Running,
    /// Set to run at times.
    Scheduled,
    /// Ran and failed.
    Failed,
    /// Its file cannot be read, so launchd cannot load it.
    Invalid,
}

impl State {
    /// The word the interface shows, empty for states with no display.
    pub const fn label(self) -> &'static str {
        match self {
            State::Stopped => "Stopped",
            State::Loaded => "Idle",
            State::Running => "Running",
            State::Scheduled => "Scheduled",
            State::Failed => "Error",
            State::Invalid => "Invalid",
            State::None => "",
        }
    }
}

impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Whether an item starts by itself, at login or at boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Autostart {
    /// The source cannot tell, or has no such thing.
    Unknown,
    On,
    Off,
}

/// One startup item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    /// Identifies the item among all sources: `<source>:<path>`.
    pub id: String,
    pub source: Source,
    pub name: String,
    pub state: State,
    /// The process ID, 0 for none.
    pub pid: i32,
    pub autostart: Autostart,
    /// The user, developer or domain the item belongs to, if known.
    pub owner: String,
    /// The file that defines the item: a property list or an app.
    pub path: PathBuf,
    /// What the item is, when the source holds more than one kind.
    pub kind: Option<String>,
    /// Changing the item takes root, which runs through the privileged helper.
    pub admin: bool,
    /// launchd starts the item again whenever its process ends, so stopping it
    /// means unloading it.
    pub keep_alive: bool,
}

impl Item {
    /// Whether the item runs, is scheduled to, or failed while running: the
    /// states in which stopping it makes sense.
    pub fn active(&self) -> bool {
        matches!(
            self.state,
            State::Running | State::Scheduled | State::Failed
        )
    }

    /// Whether the item runs, or is set to run at its times.
    pub fn up(&self) -> bool {
        matches!(self.state, State::Running | State::Scheduled)
    }

    /// Whether the item's name, owner or path holds the query, ignoring case.
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return true;
        }
        let path = self.path.to_string_lossy();
        [self.name.as_str(), self.owner.as_str(), path.as_ref()]
            .iter()
            .any(|s| s.to_lowercase().contains(&query))
    }

    /// If this item is backed by systemd, returns `(unit_name, is_system)` for
    /// reading logs via `journalctl`.
    pub fn journal_unit(&self) -> Option<(&str, bool)> {
        match self.source {
            Source::SystemdUser => Some((&self.name, false)),
            Source::SystemdSystem => Some((&self.name, true)),
            _ => None,
        }
    }
}

/// Something done to an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// Start now, leaving the autostart as it is.
    Start,
    /// Stop now, leaving the autostart as it is.
    Stop,
    /// Stop and start again.
    Restart,
    /// Register with launchd.
    Load,
    /// Unregister from launchd.
    Unload,
    /// Let it start by itself.
    Enable,
    /// Keep it from starting by itself.
    Disable,
    /// Delete it.
    Remove,
}

impl Action {
    /// The stable lowercase string form, as the Go type's value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Action::Start => "start",
            Action::Stop => "stop",
            Action::Restart => "restart",
            Action::Load => "load",
            Action::Unload => "unload",
            Action::Enable => "enable",
            Action::Disable => "disable",
            Action::Remove => "remove",
        }
    }

    /// The word on the button of an action.
    pub const fn label(self) -> &'static str {
        match self {
            Action::Start => "Start",
            Action::Stop => "Stop",
            Action::Restart => "Restart",
            Action::Load => "Load",
            Action::Unload => "Unload",
            Action::Enable => "Enable",
            Action::Disable => "Disable",
            Action::Remove => "Remove",
        }
    }

    /// What an action did, for a message.
    pub const fn past(self) -> &'static str {
        match self {
            Action::Start => "started",
            Action::Stop => "stopped",
            Action::Restart => "restarted",
            Action::Load => "loaded",
            Action::Unload => "unloaded",
            Action::Enable => "enabled",
            Action::Disable => "disabled",
            Action::Remove => "removed",
        }
    }
}

impl std::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One labeled value of an item's details, under the heading of its group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub group: String,
    pub key: String,
    pub value: String,
}

/// Distinguishes a file-backed log from a systemd journal unit entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogSource<'a> {
    File(&'a Path),
    Journal { unit: &'a str, system: bool },
}

/// A file (or systemd journal stream) an item writes its output to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogFile {
    pub label: String,
    pub path: PathBuf,
}

const JOURNAL_USER_PREFIX: &str = "journal://user/";
const JOURNAL_SYSTEM_PREFIX: &str = "journal://system/";

impl LogFile {
    /// Creates a file-backed log reference.
    pub fn file(label: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        LogFile {
            label: label.into(),
            path: path.into(),
        }
    }

    /// Creates a systemd journal log reference encoded in `path` without
    /// breaking existing `LogFile` field layouts.
    pub fn journal(label: impl Into<String>, unit: &str, system: bool) -> Self {
        let prefix = if system {
            JOURNAL_SYSTEM_PREFIX
        } else {
            JOURNAL_USER_PREFIX
        };
        LogFile {
            label: label.into(),
            path: PathBuf::from(format!("{prefix}{unit}")),
        }
    }

    /// Returns whether this entry points to a filesystem path or a systemd journal unit.
    pub fn source(&self) -> LogSource<'_> {
        if let Some((unit, system)) = self.journal_unit() {
            LogSource::Journal { unit, system }
        } else {
            LogSource::File(self.path.as_path())
        }
    }

    /// If this log entry refers to a systemd journal unit, returns `(unit, system)`.
    pub fn journal_unit(&self) -> Option<(&str, bool)> {
        let raw = self.path.to_str()?;
        if let Some(unit) = raw.strip_prefix(JOURNAL_SYSTEM_PREFIX)
            && !unit.is_empty()
        {
            return Some((unit, true));
        }
        if let Some(unit) = raw.strip_prefix(JOURNAL_USER_PREFIX)
            && !unit.is_empty()
        {
            return Some((unit, false));
        }
        None
    }
}

/// What an item shows beyond the list: its values and its logs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Detail {
    pub rows: Vec<Row>,
    pub logs: Vec<LogFile>,
}

/// A source of items.
pub trait Provider: Send + Sync {
    fn source(&self) -> Source;

    /// Returns the items of the source. It may return items and an error
    /// together in the Go model; this port returns either, because none of the
    /// launchd paths produce both.
    fn list(&self) -> Result<Vec<Item>>;

    /// Reads an item's values and log files.
    fn detail(&self, item: &Item) -> Result<Detail>;

    /// The actions that apply to the item now, the main one first. Pure: it
    /// reads nothing, so it can run for every frame.
    fn actions(&self, item: &Item) -> Vec<Action>;

    /// What to ask the user before an action with consequences they may not
    /// expect, or `None` if it needs no question. Pure.
    fn confirm(&self, item: &Item, action: Action) -> Option<String>;

    /// Runs an action.
    fn do_action(&self, item: &Item, action: Action) -> Result<()>;

    /// Returns this provider as [`Restricted`] if macOS may keep it from
    /// listing all of its items, or `None` otherwise.
    fn as_restricted(&self) -> Option<&dyn Restricted> {
        None
    }
}

/// What macOS keeps a provider from listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Restriction {
    /// What the list leaves out, and why.
    pub text: String,
    /// Whether System Settings can lift it.
    pub settings: bool,
}

/// A provider that macOS may let read only part of its items.
pub trait Restricted: Send + Sync {
    /// What the last `list` left out, or `None` if nothing.
    fn restriction(&self) -> Option<Restriction>;

    /// Reports whether macOS refuses what the setting allows. It asks the user
    /// nothing, and is quick enough to run every second.
    fn needs_access(&self) -> bool;

    /// Opens the setting that lifts the restriction.
    fn open_settings(&self) -> Result<()>;

    /// Reads what is left out for the next `list`, after macOS asks the user
    /// to authenticate. Only the user's asking may start it.
    fn read_once(&self) -> Result<()>;
}

/// A provider that runs an action on several items at once: one request to the
/// privileged helper, one prompt to install it if it is missing.
pub trait Batcher: Send + Sync {
    fn do_all(&self, items: &[Item], action: Action) -> Result<()>;
}

/// Whether `actions` holds `action`.
pub fn has(actions: &[Action], action: Action) -> bool {
    actions.contains(&action)
}

/// The action that flips an item's autostart, or `None` if it has none.
pub fn toggle(actions: &[Action]) -> Option<Action> {
    actions
        .iter()
        .copied()
        .find(|a| matches!(a, Action::Enable | Action::Disable))
}

/// The main action of an item with the actions, leaving out the autostart
/// toggle and removal, or `None` for none.
pub fn primary(actions: &[Action]) -> Option<Action> {
    actions
        .iter()
        .copied()
        .find(|a| !matches!(a, Action::Enable | Action::Disable | Action::Remove))
}

/// A registry holding all startup-item providers for the host platform.
#[derive(Default)]
pub struct ServiceRegistry {
    providers: Vec<Arc<dyn Provider>>,
    batchers: BTreeMap<Source, Arc<dyn Batcher>>,
}

impl ServiceRegistry {
    pub fn new() -> Self {
        ServiceRegistry::default()
    }

    pub fn register(&mut self, provider: Arc<dyn Provider>) {
        self.providers.push(provider);
    }

    pub fn register_batcher(&mut self, source: Source, batcher: Arc<dyn Batcher>) {
        self.batchers.insert(source, batcher);
    }

    /// Builds the default provider registry for the current host OS using the
    /// default system runner, host, and privileged helper.
    pub fn platform_default() -> Self {
        platform_providers(Arc::new(crate::helper::DefaultHelper))
    }

    /// Returns the registered sources in registration order.
    pub fn sources(&self) -> Vec<Source> {
        self.providers.iter().map(|p| p.source()).collect()
    }

    pub fn provider(&self, source: Source) -> Option<Arc<dyn Provider>> {
        self.providers
            .iter()
            .find(|p| p.source() == source)
            .cloned()
    }

    pub fn batcher(&self, source: Source) -> Option<Arc<dyn Batcher>> {
        self.batchers.get(&source).cloned()
    }

    pub fn list_all(&self) -> Vec<(Source, Result<Vec<Item>>)> {
        self.providers
            .iter()
            .map(|p| (p.source(), p.list()))
            .collect()
    }
}

/// Builds the platform-appropriate [`ServiceRegistry`] wired to `helper`.
pub fn platform_providers(helper: Arc<dyn crate::launchd::PrivilegedHelper>) -> ServiceRegistry {
    let mut registry = ServiceRegistry::new();
    #[cfg(target_os = "macos")]
    {
        let runner: Arc<dyn crate::launchd::ProcessRunner> =
            Arc::new(crate::launchd::SystemRunner::new());
        let host: Arc<dyn crate::launchd::Host> = Arc::new(crate::launchd::SystemHost::new());
        let launchd_ctx = Arc::new(crate::launchd::Context::new(
            runner.clone(),
            helper,
            host.clone(),
        ));
        let brew = Arc::new(crate::brew::Provider::system(launchd_ctx.clone()));
        registry.register(brew);

        let login = Arc::new(crate::loginitem::Provider::system(runner, host));
        registry.register(login);

        let user_agents = Arc::new(crate::launchd::LaunchdProvider::user_agents(
            launchd_ctx.clone(),
        ));
        let system_agents = Arc::new(crate::launchd::LaunchdProvider::system_agents(
            launchd_ctx.clone(),
        ));
        let daemons = Arc::new(crate::launchd::LaunchdProvider::daemons(launchd_ctx));
        registry.register_batcher(Source::UserAgent, user_agents.clone());
        registry.register_batcher(Source::SystemAgent, system_agents.clone());
        registry.register_batcher(Source::Daemon, daemons.clone());
        registry.register(user_agents);
        registry.register(system_agents);
        registry.register(daemons);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = helper;
        let runner: Arc<dyn crate::launchd::ProcessRunner> =
            Arc::new(crate::launchd::SystemRunner::new());
        let host: Arc<dyn crate::launchd::Host> = Arc::new(crate::launchd::SystemHost::new());
        let systemd_user = Arc::new(crate::systemd::SystemdProvider::user(
            runner.clone(),
            host.clone(),
        ));
        let systemd_system = Arc::new(crate::systemd::SystemdProvider::system(
            runner,
            host.clone(),
            Arc::new(crate::systemd::DefaultSystemdHelper),
        ));
        let autostart = Arc::new(crate::autostart::AutostartProvider::system_default(host));
        registry.register_batcher(Source::SystemdUser, systemd_user.clone());
        registry.register_batcher(Source::SystemdSystem, systemd_system.clone());
        registry.register(systemd_user);
        registry.register(systemd_system);
        registry.register(autostart);
    }
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_order_and_titles() {
        assert_eq!(
            Source::ALL.map(Source::as_str),
            ["brew", "login-item", "user-agent", "system-agent", "daemon"]
        );
        assert_eq!(Source::UserAgent.title(), "User Agents");
        assert_eq!(Source::Daemon.title(), "Daemons");
    }

    #[test]
    fn item_activity_and_matching() {
        let mut item = Item {
            id: "user-agent:/x/com.example.job.plist".into(),
            source: Source::UserAgent,
            name: "com.example.job".into(),
            state: State::Running,
            pid: 12,
            autostart: Autostart::On,
            owner: "gui/501".into(),
            path: PathBuf::from("/x/com.example.job.plist"),
            kind: None,
            admin: false,
            keep_alive: false,
        };
        assert!(item.active());
        assert!(item.up());
        assert!(item.matches("  EXAMPLE "));
        assert!(item.matches("gui/501"));
        assert!(item.matches("/X/"));
        assert!(item.matches(""));
        assert!(!item.matches("nope"));
        item.state = State::Failed;
        assert!(item.active());
        assert!(!item.up());
        item.state = State::Loaded;
        assert!(!item.active());
        assert!(!item.up());
    }

    #[test]
    fn action_helpers() {
        let actions = [
            Action::Start,
            Action::Unload,
            Action::Disable,
            Action::Remove,
        ];
        assert!(has(&actions, Action::Unload));
        assert_eq!(toggle(&actions), Some(Action::Disable));
        assert_eq!(primary(&actions), Some(Action::Start));
        assert_eq!(primary(&[Action::Enable, Action::Remove]), None);
        assert_eq!(Action::Restart.label(), "Restart");
        assert_eq!(Action::Restart.past(), "restarted");
    }

    #[test]
    fn linux_sources_and_journal_log_source() {
        assert_eq!(
            Source::LINUX.map(Source::slug),
            ["systemd-user", "systemd-system", "autostart"]
        );
        assert_eq!(Source::parse("systemd-user"), Some(Source::SystemdUser));
        assert_eq!(Source::parse("systemd-system"), Some(Source::SystemdSystem));
        assert_eq!(Source::parse("autostart"), Some(Source::Autostart));
        assert_eq!(Source::parse("unknown"), None);

        assert!(Source::Daemon.is_privileged());
        assert!(Source::SystemdSystem.is_privileged());
        assert!(!Source::SystemdUser.is_privileged());
        assert!(Source::UserAgent.is_creatable());
        assert!(Source::Daemon.is_creatable());
        assert!(Source::SystemdUser.is_creatable());
        assert!(Source::SystemdSystem.is_creatable());
        assert!(!Source::Autostart.is_creatable());

        let file_log = LogFile::file("Standard output", "/tmp/a.log");
        assert_eq!(file_log.source(), LogSource::File(Path::new("/tmp/a.log")));
        assert_eq!(file_log.journal_unit(), None);

        let journal_log = LogFile::journal("Journal", "devbase-api.service", false);
        assert_eq!(
            journal_log.source(),
            LogSource::Journal {
                unit: "devbase-api.service",
                system: false
            }
        );
        let sys_log = LogFile::journal("Journal", "nginx.service", true);
        assert_eq!(sys_log.journal_unit(), Some(("nginx.service", true)));
    }
}

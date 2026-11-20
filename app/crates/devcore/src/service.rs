//! The model every source of startup items shares.
//!
//! This is a direct port of `internal/service/service.go`. The go app's
//! `Provider` interface takes a concrete error type here; callers that need
//! the typed launchd error use [`crate::launchd::LaunchdProvider`]'s inherent
//! methods.

use std::path::PathBuf;

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
}

impl Source {
    /// The sources in the order the interface shows them.
    pub const ALL: [Source; 5] = [
        Source::Brew,
        Source::LoginItem,
        Source::UserAgent,
        Source::SystemAgent,
        Source::Daemon,
    ];

    /// The stable string form, as stored in `state.json` and item IDs.
    pub const fn as_str(self) -> &'static str {
        match self {
            Source::Brew => "brew",
            Source::LoginItem => "login-item",
            Source::UserAgent => "user-agent",
            Source::SystemAgent => "system-agent",
            Source::Daemon => "daemon",
        }
    }

    /// What the interface calls the source.
    pub const fn title(self) -> &'static str {
        match self {
            Source::Brew => "Homebrew",
            Source::UserAgent => "User Agents",
            Source::SystemAgent => "System Agents",
            Source::Daemon => "Daemons",
            Source::LoginItem => "Login Items",
        }
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

/// A file an item writes its output to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogFile {
    pub label: String,
    pub path: PathBuf,
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
}

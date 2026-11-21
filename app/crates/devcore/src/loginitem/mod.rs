//! Login items provider, ported from the Go `internal/loginitem` package.
//!
//! Reads the login items shown in System Settings (apps set to open at login
//! and background helpers registered by apps), and adds or removes apps that
//! open at login.
//!
//! Items come from macOS's Background Task Management database, which macOS
//! lets an app read only with Full Disk Access, and whose format is Apple's
//! own and changes across macOS releases. When the database cannot be read,
//! System Events lists the apps that open at login (requiring Automation
//! permission once), while app-registered helpers are omitted. `sfltool
//! dumpbtm` can list them, but macOS prompts for an administrator each time it
//! runs, so it runs only when the user explicitly requests it via
//! [`Provider::read_once`], and its result is cached.

mod database;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::helper::admin::apple_script_string;
use crate::launchd::{
    CommandError, DomainState, Host, ProcessRunner, SystemHost, SystemRunner, parse_print,
};
use crate::service::{
    self, Action, Autostart, Detail, Item, Restricted, Restriction, Row, Source, State,
};

pub use database::parse_database;

/// Default directory holding macOS's Background Task Management database.
pub const DEFAULT_DATABASE_DIR: &str = "/private/var/db/com.apple.backgroundtaskmanagement";

/// System Settings deep link for Full Disk Access.
pub const FULL_DISK_ACCESS_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_AllFiles";

/// Record type for an app the user set to open at login.
pub const TYPE_APP: &str = "app";
/// Record type for a helper app inside another app's bundle.
pub const TYPE_LOGIN_ITEM: &str = "login item";

/// `Item::kind` value for an app that opens at login.
pub const KIND_APP: &str = "App";
/// `Item::kind` value for a background helper.
pub const KIND_HELPER: &str = "Background helper";

static SECTION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*Records for UID (-?\d+)").expect("valid regex"));
static RECORD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*#\d+:\s*$").expect("valid regex"));
static FIELD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s+([A-Za-z ]+):\s?(.*)$").expect("valid regex"));
static TYPE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([a-z ]+?)\s*\(0x").expect("valid regex"));

/// What kind of failure [`Provider`] returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Io,
    PermissionDenied,
    DatabaseFormat,
    Command,
    AutomationDenied,
    NotFound,
    Validation,
    UnknownAction,
}

/// A typed error from the login items provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginItemError {
    pub code: ErrorCode,
    pub message: String,
}

impl LoginItemError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        LoginItemError {
            code,
            message: message.into(),
        }
    }

    pub fn io(message: impl Into<String>) -> Self {
        LoginItemError::new(ErrorCode::Io, message)
    }

    pub fn permission_denied(message: impl Into<String>) -> Self {
        LoginItemError::new(ErrorCode::PermissionDenied, message)
    }

    pub fn database_format(message: impl Into<String>) -> Self {
        LoginItemError::new(ErrorCode::DatabaseFormat, message)
    }

    pub fn command(message: impl Into<String>) -> Self {
        LoginItemError::new(ErrorCode::Command, message)
    }

    pub fn automation_denied(message: impl Into<String>) -> Self {
        LoginItemError::new(ErrorCode::AutomationDenied, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        LoginItemError::new(ErrorCode::NotFound, message)
    }

    pub fn validation(message: impl Into<String>) -> Self {
        LoginItemError::new(ErrorCode::Validation, message)
    }

    pub fn unknown_action(message: impl Into<String>) -> Self {
        LoginItemError::new(ErrorCode::UnknownAction, message)
    }

    pub fn is_permission_denied(&self) -> bool {
        self.code == ErrorCode::PermissionDenied
    }

    pub fn is_automation_denied(&self) -> bool {
        self.code == ErrorCode::AutomationDenied
    }
}

impl fmt::Display for LoginItemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for LoginItemError {}

impl From<CommandError> for LoginItemError {
    fn from(error: CommandError) -> Self {
        LoginItemError::command(error.message())
    }
}

/// An item of the Background Task Management database that System Settings shows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    #[serde(rename = "UUID", default)]
    pub uuid: String,
    #[serde(rename = "Name", default)]
    pub name: String,
    #[serde(rename = "Developer", default)]
    pub developer: String,
    #[serde(rename = "Team", default)]
    pub team: String,
    #[serde(rename = "Type", default)]
    pub record_type: String,
    #[serde(rename = "Identifier", default)]
    pub identifier: String,
    #[serde(rename = "URL", default)]
    pub url: String,
    #[serde(rename = "Bundle", default)]
    pub bundle: String,
    #[serde(rename = "Parent", default)]
    pub parent: String,
    #[serde(rename = "LastUse", default)]
    pub last_use: String,
    #[serde(rename = "Enabled", default)]
    pub enabled: bool,
}

fn clean(value: &str) -> String {
    if value == "(null)" {
        String::new()
    } else {
        value.to_string()
    }
}

/// Parses the output of `sfltool dumpbtm` and returns the records belonging to
/// the user with `uid`.
pub fn parse_btm(dump: &str, uid: i64) -> Vec<Record> {
    let uid_text = uid.to_string();
    let mut records = Vec::new();
    let mut in_user = false;
    let mut current: Option<Record> = None;

    for line in dump.lines() {
        if let Some(caps) = SECTION_RE.captures(line) {
            if let Some(record) = current.take() {
                records.push(record);
            }
            in_user = caps.get(1).is_some_and(|m| m.as_str() == uid_text);
            continue;
        }
        if !in_user {
            continue;
        }
        if RECORD_RE.is_match(line) {
            if let Some(record) = current.take() {
                records.push(record);
            }
            current = Some(Record::default());
            continue;
        }
        let Some(record) = current.as_mut() else {
            continue;
        };
        let Some(caps) = FIELD_RE.captures(line) else {
            continue;
        };
        let key = caps.get(1).map_or("", |m| m.as_str());
        let val = caps.get(2).map_or("", |m| m.as_str()).trim();
        match key {
            "UUID" => record.uuid = val.to_string(),
            "Name" => record.name = clean(val),
            "Developer Name" => record.developer = clean(val),
            "Team Identifier" => record.team = clean(val),
            "Type" => {
                if let Some(type_caps) = TYPE_RE.captures(val)
                    && let Some(matched) = type_caps.get(1)
                {
                    record.record_type = matched.as_str().to_string();
                }
            }
            "Disposition" => {
                // e.g. "[enabled, allowed, notified] (0xb)"
                let bracket = val.split(']').next().unwrap_or(val);
                let inner = bracket.trim_matches(['[', ' ']);
                for word in inner.split(',') {
                    if word.trim() == "enabled" {
                        record.enabled = true;
                    }
                }
            }
            "Identifier" => record.identifier = clean(val),
            "URL" => record.url = clean(val),
            "Bundle Identifier" => record.bundle = clean(val),
            "Parent Identifier" => record.parent = clean(val),
            "Last Use" => record.last_use = clean(val),
            _ => {}
        }
    }
    if let Some(record) = current {
        records.push(record);
    }
    records
}

/// Reports whether the record is an app set to open at login, as System
/// Settings lists under "Open at Login". The database also holds every app it
/// has seen, disabled; those are not in the list.
fn is_open_at_login(record: &Record) -> bool {
    record.record_type == TYPE_APP && record.enabled
}

/// Returns the launchd label of a helper: its bundle identifier, or its
/// identifier without the leading type number prefix.
fn label_of(record: &Record) -> &str {
    if !record.bundle.is_empty() {
        return &record.bundle;
    }
    if let Some((_, rest)) = record.identifier.split_once('.') {
        return rest;
    }
    &record.identifier
}

/// Finds a record's process PID among the jobs of the user's launchd domain: a
/// helper is a job of its own, and launchd holds a running app as
/// `application.<bundle identifier>.<numbers>`.
fn process_of(record: &Record, state: &DomainState) -> i32 {
    if record.record_type == TYPE_LOGIN_ITEM {
        return state.jobs.get(label_of(record)).map_or(0, |job| job.pid);
    }
    if record.bundle.is_empty() {
        return 0;
    }
    let prefix = format!("application.{}.", record.bundle);
    for (label, job) in &state.jobs {
        if job.pid > 0 && label.starts_with(&prefix) {
            return job.pid;
        }
    }
    0
}

/// Converts raw BTM records into the [`Item`]s System Settings lists: apps
/// enabled to open at login, and background helpers registered by apps
/// (enabled or not).
pub fn items_of(records: &[Record], state: &DomainState) -> Vec<Item> {
    let mut parents: HashMap<&str, &Record> = HashMap::with_capacity(records.len());
    for record in records {
        if !record.identifier.is_empty() {
            parents.insert(record.identifier.as_str(), record);
        }
    }

    let mut items = Vec::new();
    for record in records {
        if !is_open_at_login(record) && record.record_type != TYPE_LOGIN_ITEM {
            continue;
        }
        let name = if record.name.is_empty() {
            record.identifier.clone()
        } else {
            record.name.clone()
        };
        let mut item = Item {
            id: format!("login-item:{}", record.uuid),
            source: Source::LoginItem,
            name,
            state: State::None,
            pid: 0,
            autostart: if record.enabled {
                Autostart::On
            } else {
                Autostart::Off
            },
            owner: record.developer.clone(),
            path: PathBuf::from(&record.url),
            kind: Some(KIND_APP.to_string()),
            admin: false,
            keep_alive: false,
        };
        if record.record_type == TYPE_LOGIN_ITEM {
            item.kind = Some(KIND_HELPER.to_string());
            if let Some(parent) = parents.get(record.parent.as_str()) {
                item.owner = parent.name.clone();
                let parent_path = Path::new(&parent.url);
                if parent_path.is_absolute() {
                    item.path = parent_path.join(&record.url);
                }
            }
        }
        let pid = process_of(record, state);
        if pid > 0 {
            item.state = State::Running;
            item.pid = pid;
        }
        items.push(item);
    }
    items
}

/// Configuration for [`Provider`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Directory holding the BTM database (`BackgroundItems-v*.btm`).
    pub dir: PathBuf,
    /// Optional cache file path storing what `sfltool dumpbtm` last returned.
    pub cache_path: Option<PathBuf>,
    /// Optional fixed user `GeneratedUID`. When `None`, resolved via `id -un`
    /// and `dscl`.
    pub user_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedDump {
    #[serde(rename = "AtUnix", default)]
    at_unix: i64,
    #[serde(rename = "Records", default)]
    records: Vec<Record>,
}

#[derive(Default)]
struct ProviderState {
    /// Records from the last [`Provider::list`], read by [`Provider::detail`].
    records: Vec<Record>,
    /// What the last [`Provider::list`] left out.
    restriction: Option<Restriction>,
    /// What `sfltool dumpbtm` last answered, and its Unix timestamp.
    dump: Vec<Record>,
    dump_at: Option<i64>,
}

/// The login items provider for the current user.
pub struct Provider {
    config: Config,
    runner: Arc<dyn ProcessRunner>,
    host: Arc<dyn Host>,
    state: Mutex<ProviderState>,
}

impl Provider {
    /// Builds the machine's login items provider using the default system
    /// runner and host.
    pub fn default_provider() -> Self {
        Provider::system(Arc::new(SystemRunner::new()), Arc::new(SystemHost::new()))
    }

    /// Builds a provider pointed at macOS's standard BTM database and user
    /// cache directory with an injected runner and host.
    pub fn system(runner: Arc<dyn ProcessRunner>, host: Arc<dyn Host>) -> Self {
        let cache_path = host
            .home_dir()
            .ok()
            .map(|home| home.join("Library/Caches/xyz.jyeu.devbase/login-items.json"));
        Provider::new(
            Config {
                dir: PathBuf::from(DEFAULT_DATABASE_DIR),
                cache_path,
                user_id: None,
            },
            runner,
            host,
        )
    }

    /// Builds a provider with explicit configuration, runner, and host.
    pub fn new(config: Config, runner: Arc<dyn ProcessRunner>, host: Arc<dyn Host>) -> Self {
        Provider {
            config,
            runner,
            host,
            state: Mutex::new(ProviderState::default()),
        }
    }

    pub fn source(&self) -> Source {
        Source::LoginItem
    }

    pub fn dir(&self) -> &Path {
        &self.config.dir
    }

    pub fn cache_path(&self) -> Option<&Path> {
        self.config.cache_path.as_deref()
    }

    /// Lists the user's login items, returning both any partial items (e.g.
    /// from a prior `read_once` cache) and any error (e.g. refused Automation
    /// permission), matching Go's `([]service.Item, error)`.
    pub fn list_with_partial(&self) -> (Vec<Item>, Option<LoginItemError>) {
        let (records, restriction, list_error) = match self.read_database() {
            Ok(records) => (records, None, None),
            Err(db_error) => {
                let restriction = self.restriction_of(&db_error);
                let (fallback_records, fallback_error) = self.fallback();
                (fallback_records, Some(restriction), fallback_error)
            }
        };

        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.records = records.clone();
            state.restriction = restriction;
        }

        // The processes are a nicety: without them, the items are still right.
        let domain = format!("gui/{}", self.host.uid());
        let domain_state = match self.runner.run("launchctl", &["print", &domain]) {
            Ok(out) => parse_print(&out),
            Err(_) => DomainState::default(),
        };

        (items_of(&records, &domain_state), list_error)
    }

    /// Lists the user's login items.
    pub fn list(&self) -> Result<Vec<Item>, LoginItemError> {
        let (items, error) = self.list_with_partial();
        match error {
            Some(error) => Err(error),
            None => Ok(items),
        }
    }

    /// Says what the last [`Provider::list`] left out, or `None` if nothing.
    pub fn restriction(&self) -> Option<Restriction> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .restriction
            .clone()
    }

    /// Reports whether macOS refuses the app permission to read the BTM
    /// database directory. Runs no subprocesses and never prompts the user.
    pub fn needs_access(&self) -> bool {
        matches!(
            std::fs::read_dir(&self.config.dir),
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied
        )
    }

    /// Opens System Settings at Full Disk Access.
    pub fn open_settings(&self) -> Result<(), LoginItemError> {
        self.runner.run("open", &[FULL_DISK_ACCESS_SETTINGS_URL])?;
        Ok(())
    }

    /// Runs `sfltool dumpbtm`, for which macOS prompts for an administrator
    /// each time, and caches its answer for subsequent [`Provider::list`] calls.
    pub fn read_once(&self) -> Result<(), LoginItemError> {
        let out = self.runner.run("sfltool", &["dumpbtm"])?;
        let dump = parse_btm(&out, i64::from(self.host.uid()));
        let at_unix = unix_now();
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.dump = dump.clone();
            state.dump_at = Some(at_unix);
        }
        if let Some(cache_path) = &self.config.cache_path
            && let Ok(data) = serde_json::to_vec(&CachedDump {
                at_unix,
                records: dump,
            })
            && let Some(parent) = cache_path.parent()
            && std::fs::create_dir_all(parent).is_ok()
        {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
            }
            if std::fs::write(cache_path, data).is_ok() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(
                        cache_path,
                        std::fs::Permissions::from_mode(0o600),
                    );
                }
            }
        }
        Ok(())
    }

    pub fn detail(&self, item: &Item) -> Result<Detail, LoginItemError> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for record in &state.records {
            if format!("login-item:{}", record.uuid) == item.id {
                return Ok(detail_of(record, item));
            }
        }
        Err(LoginItemError::not_found(format!(
            "{} is no longer a login item",
            item.name
        )))
    }

    /// An app that opens at login can be removed from the list. A background
    /// helper belongs to its app, which alone can register or unregister it.
    pub fn actions(&self, item: &Item) -> Vec<Action> {
        if item.kind.as_deref() == Some(KIND_APP) && item.path.is_absolute() {
            vec![Action::Remove]
        } else {
            Vec::new()
        }
    }

    pub fn confirm(&self, item: &Item, action: Action) -> Option<String> {
        if action == Action::Remove {
            Some(format!(
                "Stop {} from opening at login? The app itself is not deleted.",
                item.name
            ))
        } else {
            None
        }
    }

    /// Removes an app from System Events login items. An app that registered
    /// itself (`SMAppService`) appears in System Settings but not System Events,
    /// so this reports that instead of claiming success.
    pub fn do_action(&self, item: &Item, action: Action) -> Result<(), LoginItemError> {
        if action != Action::Remove {
            return Err(LoginItemError::unknown_action(format!(
                "login items: unknown action \"{action}\""
            )));
        }
        let path = apple_script_string(&item.path.to_string_lossy());
        let script = format!(
            "tell application \"System Events\"\n\
             \tset found to count of (every login item whose path is {path})\n\
             \tif found > 0 then delete (every login item whose path is {path})\n\
             \treturn found\n\
             end tell"
        );
        let out = self
            .runner
            .run("osascript", &["-e", &script])
            .map_err(automation_error)?;
        if out.trim() == "0" {
            return Err(LoginItemError::validation(format!(
                "{} registered itself: turn it off in System Settings or in the app",
                item.name
            )));
        }
        Ok(())
    }

    /// Makes the `.app` bundle at `path` open at login via System Events.
    pub fn add(&self, path: &Path) -> Result<(), LoginItemError> {
        let path_str = path.to_string_lossy();
        if !path_str.ends_with(".app") {
            return Err(LoginItemError::validation("choose an app (a .app bundle)"));
        }
        if std::fs::metadata(path).is_err() {
            return Err(LoginItemError::not_found(format!(
                "{path_str} does not exist"
            )));
        }
        let script = format!(
            "tell application \"System Events\" to make login item at end with properties {{path:{}, hidden:false}}",
            apple_script_string(&path_str)
        );
        self.runner
            .run("osascript", &["-e", &script])
            .map_err(automation_error)?;
        Ok(())
    }

    fn resolve_user_id(&self) -> Result<String, LoginItemError> {
        if let Some(user_id) = &self.config.user_id {
            return Ok(user_id.clone());
        }
        database::generated_uid(self.runner.as_ref())
    }

    fn read_database(&self) -> Result<Vec<Record>, LoginItemError> {
        let user = self.resolve_user_id()?;
        let path = database::database_file(&self.config.dir, &user)?;
        let data = std::fs::read(&path).map_err(database::map_io_error)?;
        parse_database(&data, &user)
    }

    fn restriction_of(&self, error: &LoginItemError) -> Restriction {
        let denied = error.is_permission_denied();
        let why = if denied {
            "macOS lets only apps with Full Disk Access read them"
        } else {
            "the app cannot read where this version of macOS keeps them"
        };
        let (_, dump_at) = self.remembered();
        let text = match dump_at {
            Some(at_unix) => format!(
                "Helpers that apps registered are as of {}: {why}.",
                format_local_d_mon_y_hm(at_unix)
            ),
            None => format!("Helpers that apps registered are not listed: {why}."),
        };
        Restriction {
            text,
            settings: denied,
        }
    }

    /// Returns what `sfltool dumpbtm` last answered, and when, in this launch
    /// or an earlier one.
    fn remembered(&self) -> (Vec<Record>, Option<i64>) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.dump_at.is_none()
            && let Some(cache_path) = &self.config.cache_path
            && let Ok(data) = std::fs::read(cache_path)
            && let Ok(cached) = serde_json::from_slice::<CachedDump>(&data)
            && cached.at_unix > 0
        {
            state.dump = cached.records;
            state.dump_at = Some(cached.at_unix);
        }
        (state.dump.clone(), state.dump_at)
    }

    /// Reads items when the database cannot be read: System Events lists apps
    /// that open at login now, and the last `sfltool dumpbtm` answer (if any)
    /// adds helpers and richer metadata. Never runs `sfltool` itself.
    fn fallback(&self) -> (Vec<Record>, Option<LoginItemError>) {
        let (dump, _) = self.remembered();
        let apps = match self.open_at_login() {
            Ok(apps) => apps,
            Err(error) => return (dump, Some(error)),
        };

        // System Events is the newer of the two about the apps.
        let mut by_path: HashMap<String, &Record> = HashMap::with_capacity(apps.len());
        for record in &apps {
            by_path.insert(record.url.clone(), record);
        }
        let mut records = Vec::with_capacity(dump.len() + apps.len());
        for mut record in dump {
            if record.record_type == TYPE_APP {
                record.enabled = by_path.remove(&record.url).is_some();
            }
            records.push(record);
        }
        for record in &apps {
            if by_path.contains_key(&record.url) {
                records.push(record.clone());
            }
        }
        (records, None)
    }

    /// Reads the apps that open at login from System Events. Paths are joined
    /// with linefeeds because AppleScript's default list delimiter `", "` can
    /// appear inside an app's filename.
    fn open_at_login(&self) -> Result<Vec<Record>, LoginItemError> {
        let out = self
            .runner
            .run(
                "osascript",
                &[
                    "-e",
                    "tell application \"System Events\" to set paths to the path of every login item\n\
                     set AppleScript's text item delimiters to linefeed\n\
                     return paths as text",
                ],
            )
            .map_err(automation_error)?;
        let mut records = Vec::new();
        for path in out.trim().split('\n') {
            let path = path.trim_end_matches('\r');
            if path.is_empty() {
                continue;
            }
            let base = Path::new(path)
                .file_name()
                .map(|name| name.to_string_lossy())
                .unwrap_or_default();
            let name = base.strip_suffix(".app").unwrap_or(&base).to_string();
            records.push(Record {
                uuid: path.to_string(),
                name,
                record_type: TYPE_APP.to_string(),
                url: path.to_string(),
                enabled: true,
                ..Record::default()
            });
        }
        Ok(records)
    }
}

fn detail_of(record: &Record, item: &Item) -> Detail {
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
    const ABOUT: &str = "About";
    const FILES: &str = "Files";

    if item.pid > 0 {
        add(
            &mut detail.rows,
            STATUS,
            "Process ID",
            &item.pid.to_string(),
        );
    }
    add(
        &mut detail.rows,
        STATUS,
        "Enabled",
        if record.enabled { "Yes" } else { "No" },
    );
    add(&mut detail.rows, STATUS, "Last used", &record.last_use);
    if record.record_type == TYPE_APP {
        add(&mut detail.rows, ABOUT, "Kind", "App that opens at login");
    } else {
        add(
            &mut detail.rows,
            ABOUT,
            "Kind",
            "Helper that runs in the background",
        );
        add(&mut detail.rows, ABOUT, "Part of", &item.owner);
    }
    add(&mut detail.rows, ABOUT, "Developer", &record.developer);
    add(&mut detail.rows, ABOUT, "Team ID", &record.team);
    add(&mut detail.rows, ABOUT, "Bundle identifier", &record.bundle);
    let path_str = item.path.to_string_lossy();
    add(&mut detail.rows, FILES, "Path", &path_str);
    detail
}

/// Translates AppleEvent error `-1743` (refused Automation permission) into a
/// clear user-facing message pointing to System Settings.
fn automation_error(error: CommandError) -> LoginItemError {
    let message = error.message();
    if message.contains("-1743") {
        LoginItemError::automation_denied(
            "macOS does not let this app control System Events: allow it in System Settings, under Privacy & Security, Automation",
        )
    } else {
        LoginItemError::command(message)
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

/// Formats a Unix timestamp in local time as `"2 Jan 2006, 15:04"`, matching
/// Go's `at.Format("2 Jan 2006, 15:04")`.
#[cfg(unix)]
fn format_local_d_mon_y_hm(unix_secs: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    // SAFETY: `raw` is a valid time_t and `tm` is properly zero-initialized.
    unsafe {
        let raw = unix_secs as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&raw, &mut tm).is_null() {
            return String::new();
        }
        let month = MONTHS
            .get(usize::try_from(tm.tm_mon).unwrap_or(0))
            .copied()
            .unwrap_or("Jan");
        format!(
            "{} {} {:04}, {:02}:{:02}",
            tm.tm_mday,
            month,
            tm.tm_year + 1900,
            tm.tm_hour,
            tm.tm_min,
        )
    }
}

#[cfg(not(unix))]
fn format_local_d_mon_y_hm(_unix_secs: i64) -> String {
    String::new()
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

    fn as_restricted(&self) -> Option<&dyn Restricted> {
        Some(self)
    }
}

impl service::Restricted for Provider {
    fn restriction(&self) -> Option<Restriction> {
        Provider::restriction(self)
    }

    fn needs_access(&self) -> bool {
        Provider::needs_access(self)
    }

    fn open_settings(&self) -> service::Result<()> {
        Provider::open_settings(self).map_err(|error| Box::new(error) as service::BoxError)
    }

    fn read_once(&self) -> service::Result<()> {
        Provider::read_once(self).map_err(|error| Box::new(error) as service::BoxError)
    }
}

//! Signal-backed state and off-thread load/save orchestration for the plist
//! editor.
//!
//! Each form field lives in its own fine-grained [`Signal`] so typing in one
//! input never re-evaluates unrelated inputs, toggles, or conditional sections.
//! All filesystem, `plutil`, `launchctl`, and privileged-helper calls go
//! through [`EditorProvider`] on a blocking thread via `Owner::detached()` +
//! `spawn(...)` + `blocking(...)`.

use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use devcore::{
    helper::DefaultHelper,
    launchd::{
        Calendar, Context, Launchd, PrivilegedHelper, Spec, SystemHost, SystemRunner, format_env,
        load_spec, parse_env, parse_lines, parse_list, save_file, shell_args, shell_command,
    },
    service::{Action, Autostart, Item, Source, State},
};
use nsh_base::alert_dialog;
use nsh_core::prelude::*;

/// The launchd sources that can be created or edited in the plist editor.
pub const JOB_KINDS: [Source; 3] = [Source::UserAgent, Source::SystemAgent, Source::Daemon];

/// How the command field is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CommandMode {
    /// Runs via `/bin/sh -c <command>` (or preserves an existing `sh` path).
    #[default]
    Shell,
    /// `ProgramArguments`, one item per line.
    Program,
}

/// What starts the job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScheduleMode {
    /// Long-lived / event-driven service (`RunAtLoad`, `KeepAlive`, mount, queue).
    #[default]
    Service,
    /// Periodic `StartInterval` in seconds.
    Interval,
    /// Cron-like `StartCalendarInterval`.
    Calendar,
}

/// How launchd restarts a service when it exits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RestartPolicy {
    /// `KeepAlive` omitted / false.
    #[default]
    Never,
    /// `KeepAlive: { SuccessfulExit: false }` (also sets `RunAtLoad`).
    OnCrash,
    /// `KeepAlive: true`.
    Always,
}

/// The outcome of saving a job plist (and optionally loading/reloading it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveResult {
    /// The path written on disk.
    pub path: PathBuf,
    /// `None` when saved and loaded cleanly; `Some` when the plist file was
    /// saved on disk but loading/reloading it into launchd failed.
    pub load_error: Option<String>,
}

/// Emitted after a plist file is written so callers (e.g. Services) can refresh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedEvent {
    pub source: Source,
    pub path: PathBuf,
    pub label: String,
    pub loaded: bool,
}

/// Injectable I/O seam for loading, diffing, and saving launchd plists.
///
/// Production uses [`SystemEditorProvider`]; headless tests install a fake
/// provider via [`use_fake_provider`] + [`install_provider`] so tests never
/// touch real `~/Library/LaunchAgents`, `/Library/LaunchAgents`, or
/// `/Library/LaunchDaemons`.
pub trait EditorProvider: Send + Sync {
    /// Reads the editable [`Spec`] from `path`.
    fn load(&self, path: &Path) -> Result<Spec, String>;

    /// Reads the raw bytes of an existing plist at `path` (used to compute
    /// `Spec::changes` before confirming root-owned edits).
    fn read_raw(&self, path: &Path) -> Result<Vec<u8>, String>;

    /// Writes `spec` for `source` (editing `existing` when `Some`, or creating
    /// a new plist when `None`) and optionally loads/reloads it when `load` is
    /// `true`.
    fn save(
        &self,
        source: Source,
        spec: Spec,
        existing: Option<PathBuf>,
        load: bool,
    ) -> Result<SaveResult, String>;

    /// The user's home directory, used for computing the default `~/Library/Logs`
    /// path for `UserAgent` jobs.
    fn home_dir(&self) -> Option<PathBuf> {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

/// Production provider backed by `devcore::launchd` and the privileged helper.
struct SystemEditorProvider {
    runner: Arc<SystemRunner>,
    launchd: Launchd,
}

impl SystemEditorProvider {
    fn new() -> Self {
        let runner = Arc::new(SystemRunner::new());
        let ctx = Arc::new(Context::new(
            Arc::clone(&runner) as Arc<dyn devcore::launchd::ProcessRunner>,
            Arc::new(DefaultHelper) as Arc<dyn PrivilegedHelper>,
            Arc::new(SystemHost::new()),
        ));
        let launchd = Launchd::new(ctx);
        SystemEditorProvider { runner, launchd }
    }
}

impl EditorProvider for SystemEditorProvider {
    fn load(&self, path: &Path) -> Result<Spec, String> {
        load_spec(path).map_err(|error| error.to_string())
    }

    fn read_raw(&self, path: &Path) -> Result<Vec<u8>, String> {
        std::fs::read(path).map_err(|error| error.to_string())
    }

    fn save(
        &self,
        source: Source,
        spec: Spec,
        existing: Option<PathBuf>,
        load: bool,
    ) -> Result<SaveResult, String> {
        let provider = self
            .launchd
            .provider(source)
            .ok_or_else(|| format!("{} cannot be saved", source.title()))?;

        if source != Source::UserAgent {
            let outcome = provider
                .save_as_root(&spec, existing.as_deref(), load)
                .map_err(|error| error.to_string())?;
            return Ok(SaveResult {
                path: outcome.path,
                load_error: outcome.load_error,
            });
        }

        let path = save_file(
            &provider.config().dir,
            &spec,
            existing.as_deref(),
            &*self.runner,
        )
        .map_err(|error| error.to_string())?;

        if !load {
            return Ok(SaveResult {
                path,
                load_error: None,
            });
        }

        let item = Item {
            id: format!("{}:{}", source.as_str(), path.display()),
            source,
            name: spec.label.clone(),
            state: State::Stopped,
            pid: 0,
            autostart: Autostart::On,
            owner: provider.config().domain.clone(),
            path: path.clone(),
            kind: None,
            admin: false,
            keep_alive: spec.keep_alive,
        };
        if existing.is_some() {
            // Loaded agents keep the plist they were loaded with: unload before loading again.
            let _ = provider.do_action(&item, Action::Unload);
        }
        match provider.do_action(&item, Action::Load) {
            Ok(()) => Ok(SaveResult {
                path,
                load_error: None,
            }),
            Err(error) => Ok(SaveResult {
                path,
                load_error: Some(format!("saved, but it did not load: {error}")),
            }),
        }
    }
}

/// Test provider installed when fake mode is enabled without a custom provider.
struct MissingFakeProvider;

impl EditorProvider for MissingFakeProvider {
    fn load(&self, _path: &Path) -> Result<Spec, String> {
        Err("no editor provider is installed".to_string())
    }

    fn read_raw(&self, _path: &Path) -> Result<Vec<u8>, String> {
        Err("no editor provider is installed".to_string())
    }

    fn save(
        &self,
        _source: Source,
        _spec: Spec,
        _existing: Option<PathBuf>,
        _load: bool,
    ) -> Result<SaveResult, String> {
        Err("no editor provider is installed".to_string())
    }

    fn home_dir(&self) -> Option<PathBuf> {
        None
    }
}

static FAKE_MODE: AtomicBool = AtomicBool::new(false);
static FAKE_PROVIDER: LazyLock<Mutex<Option<Arc<dyn EditorProvider>>>> =
    LazyLock::new(|| Mutex::new(None));
static SYSTEM_PROVIDER: LazyLock<Arc<dyn EditorProvider>> =
    LazyLock::new(|| Arc::new(SystemEditorProvider::new()));

type SavedCallback = Arc<dyn Fn(&SavedEvent) + Send + Sync>;

static SAVED_CALLBACKS: LazyLock<Mutex<Vec<SavedCallback>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

fn provider() -> Arc<dyn EditorProvider> {
    if let Some(fake) = FAKE_PROVIDER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
    {
        return fake;
    }
    if FAKE_MODE.load(Ordering::Relaxed) {
        return Arc::new(MissingFakeProvider);
    }
    Arc::clone(&SYSTEM_PROVIDER)
}

/// Switches the editor into fake-provider mode for tests and clears any
/// previously installed test provider and save callbacks.
pub fn use_fake_provider() {
    FAKE_MODE.store(true, Ordering::Relaxed);
    *FAKE_PROVIDER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    clear_saved_callbacks();
}

/// Installs a fake [`EditorProvider`] for headless tests.
pub fn install_provider(provider: Arc<dyn EditorProvider>) {
    FAKE_MODE.store(true, Ordering::Relaxed);
    *FAKE_PROVIDER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(provider);
}

/// Registers a callback invoked on the UI thread whenever a plist is saved.
pub fn on_saved(callback: impl Fn(&SavedEvent) + Send + Sync + 'static) {
    SAVED_CALLBACKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(Arc::new(callback));
}

/// Removes all callbacks registered via [`on_saved`].
pub fn clear_saved_callbacks() {
    SAVED_CALLBACKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
}

fn notify_saved(event: &SavedEvent) {
    let callbacks = SAVED_CALLBACKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    for callback in callbacks {
        callback(event);
    }
}

// --- Fine-grained reactive state ---------------------------------------------

/// Per-engine handle holding one fine-grained [`Signal`] per editor field.
///
/// Stored once via `global::<EditorSignals>()` (created inside
/// `Owner::detached()`). Because every field is its own `PartialEq` signal,
/// editing one text field never triggers re-renders of other inputs or
/// structural `when(...)` branches.
#[derive(Clone, Copy)]
pub(crate) struct EditorSignals {
    pub open: Signal<bool>,
    pub loading: Signal<bool>,
    pub saving: Signal<bool>,
    pub sequence: Signal<u64>,

    pub source: Signal<Source>,
    pub existing: Signal<Option<PathBuf>>,

    pub label: Signal<String>,
    pub command_mode: Signal<CommandMode>,
    pub shell: Signal<String>,
    pub args: Signal<String>,
    pub working_dir: Signal<String>,
    pub env: Signal<String>,

    pub schedule_mode: Signal<ScheduleMode>,
    pub when_loaded: Signal<ScheduleMode>,
    pub run_at_load: Signal<bool>,
    pub restart: Signal<RestartPolicy>,
    pub keep_alive_custom: Signal<bool>,
    pub start_on_mount: Signal<bool>,
    pub queue_dirs: Signal<String>,
    pub interval: Signal<String>,

    pub cal_minute: Signal<String>,
    pub cal_hour: Signal<String>,
    pub cal_day: Signal<String>,
    pub cal_month: Signal<String>,
    pub cal_weekday: Signal<String>,
    pub calendar_custom: Signal<bool>,

    pub user_name: Signal<String>,
    pub group_name: Signal<String>,
    pub root_dir: Signal<String>,

    pub stdout: Signal<String>,
    pub stderr: Signal<String>,
    pub log_default: Signal<bool>,
    pub apply: Signal<bool>,

    pub error: Signal<Option<String>>,
    pub notice: Signal<Option<String>>,
    pub revision: Signal<u64>,
    pub last_saved: Signal<Option<SavedEvent>>,
}

impl Default for EditorSignals {
    fn default() -> Self {
        EditorSignals {
            open: signal(false),
            loading: signal(false),
            saving: signal(false),
            sequence: signal(0),

            source: signal(Source::UserAgent),
            existing: signal(None),

            label: signal(String::new()),
            command_mode: signal(CommandMode::Shell),
            shell: signal(String::new()),
            args: signal(String::new()),
            working_dir: signal(String::new()),
            env: signal(String::new()),

            schedule_mode: signal(ScheduleMode::Service),
            when_loaded: signal(ScheduleMode::Service),
            run_at_load: signal(true),
            restart: signal(RestartPolicy::Never),
            keep_alive_custom: signal(false),
            start_on_mount: signal(false),
            queue_dirs: signal(String::new()),
            interval: signal(String::new()),

            cal_minute: signal(String::new()),
            cal_hour: signal(String::new()),
            cal_day: signal(String::new()),
            cal_month: signal(String::new()),
            cal_weekday: signal(String::new()),
            calendar_custom: signal(false),

            user_name: signal(String::new()),
            group_name: signal(String::new()),
            root_dir: signal(String::new()),

            stdout: signal(String::new()),
            stderr: signal(String::new()),
            log_default: signal(true),
            apply: signal(true),

            error: signal(None),
            notice: signal(None),
            revision: signal(0),
            last_saved: signal(None),
        }
    }
}

pub(crate) fn signals() -> EditorSignals {
    global::<EditorSignals>().get_untracked()
}

fn task(future: impl Future<Output = ()> + 'static) {
    Owner::detached().run(move || {
        spawn(future);
    });
}

/// Normalizes any [`Source`] to one of the three editable launchd job kinds.
pub fn normalize_job_source(source: Source) -> Source {
    match source {
        Source::SystemAgent => Source::SystemAgent,
        Source::Daemon => Source::Daemon,
        _ => Source::UserAgent,
    }
}

/// Human-readable label for an editable launchd job kind.
pub const fn kind_label(source: Source) -> &'static str {
    match source {
        Source::UserAgent => "User Agent",
        Source::SystemAgent => "System Agent",
        Source::Daemon => "Daemon",
        _ => "User Agent",
    }
}

fn clear_fields(s: EditorSignals) {
    s.label.set(String::new());
    s.command_mode.set(CommandMode::Shell);
    s.shell.set(String::new());
    s.args.set(String::new());
    s.working_dir.set(String::new());
    s.env.set(String::new());

    s.schedule_mode.set(ScheduleMode::Service);
    s.when_loaded.set(ScheduleMode::Service);
    s.run_at_load.set(true);
    s.restart.set(RestartPolicy::Never);
    s.keep_alive_custom.set(false);
    s.start_on_mount.set(false);
    s.queue_dirs.set(String::new());
    s.interval.set(String::new());

    s.cal_minute.set(String::new());
    s.cal_hour.set(String::new());
    s.cal_day.set(String::new());
    s.cal_month.set(String::new());
    s.cal_weekday.set(String::new());
    s.calendar_custom.set(false);

    s.user_name.set(String::new());
    s.group_name.set(String::new());
    s.root_dir.set(String::new());

    s.stdout.set(String::new());
    s.stderr.set(String::new());
    s.error.set(None);
}

fn bump_sequence(s: EditorSignals) -> u64 {
    let next = s.sequence.get_untracked().wrapping_add(1);
    s.sequence.set(next);
    next
}

// --- Public lifecycle API ----------------------------------------------------

/// Opens the editor for creating a new job targeting `source`.
pub fn open_new(source: Source) {
    let s = signals();
    bump_sequence(s);
    clear_fields(s);
    s.source.set(normalize_job_source(source));
    s.existing.set(None);
    s.run_at_load.set(true);
    s.apply.set(true);
    s.log_default.set(true);
    s.loading.set(false);
    s.saving.set(false);
    s.open.set(true);
}

/// Opens the editor on an existing property list at `path` (defaulting
/// "Reload it after saving" to `true`).
pub fn open_edit(source: Source, path: PathBuf) {
    open_edit_with_apply(source, path, true);
}

/// Opens the editor on an existing property list at `path`, initializing the
/// reload-on-save toggle to `apply` (e.g. `item.state != State::Stopped`).
pub fn open_edit_with_apply(source: Source, path: PathBuf, apply: bool) {
    let s = signals();
    let seq = bump_sequence(s);
    clear_fields(s);
    s.source.set(normalize_job_source(source));
    s.existing.set(Some(path.clone()));
    s.apply.set(apply);
    s.log_default.set(false);
    s.saving.set(false);
    s.loading.set(true);
    s.open.set(true);

    let prov = provider();
    task(async move {
        let loaded = match blocking(move || prov.load(&path)).await {
            Ok(result) => result,
            Err(error) => Err(format!("could not read property list: {error}")),
        };
        if s.sequence.get_untracked() != seq {
            return;
        }
        s.loading.set(false);
        match loaded {
            Ok(spec) => populate_from_spec(s, &spec),
            Err(message) => s.error.set(Some(message)),
        }
    });
}

/// Closes the editor and cancels any in-flight load/save updates.
pub fn close() {
    let s = signals();
    bump_sequence(s);
    s.open.set(false);
    s.loading.set(false);
    s.saving.set(false);
    s.error.set(None);
}

/// Whether the editor is currently open.
pub fn is_open() -> bool {
    signals().open.get()
}

/// Monotonically increasing counter bumped after every successful plist write.
pub fn revision() -> u64 {
    signals().revision.get()
}

/// Metadata from the most recent successful save, if any.
pub fn last_saved() -> Option<SavedEvent> {
    signals().last_saved.get()
}

/// Status notice from the last save / no-op save, if any.
pub fn notice() -> Option<String> {
    signals().notice.get()
}

// --- Internal state readers & setters ----------------------------------------

fn populate_from_spec(s: EditorSignals, spec: &Spec) {
    s.label.set(spec.label.clone());
    s.working_dir.set(spec.working_dir.clone());
    s.env.set(format_env(&spec.env));
    s.stdout.set(spec.stdout.clone());
    s.stderr.set(spec.stderr.clone());
    s.run_at_load.set(spec.run_at_load);
    s.keep_alive_custom.set(spec.keep_alive_custom);
    s.start_on_mount.set(spec.start_on_mount);
    s.queue_dirs.set(spec.queue_dirs.join(", "));
    s.cal_minute.set(spec.calendar.minute.clone());
    s.cal_hour.set(spec.calendar.hour.clone());
    s.cal_day.set(spec.calendar.day.clone());
    s.cal_month.set(spec.calendar.month.clone());
    s.cal_weekday.set(spec.calendar.weekday.clone());
    s.calendar_custom.set(spec.calendar_custom);
    s.user_name.set(spec.user_name.clone());
    s.group_name.set(spec.group_name.clone());
    s.root_dir.set(spec.root_dir.clone());
    s.log_default.set(false);

    if let Some(command) = shell_command(&spec.args) {
        s.command_mode.set(CommandMode::Shell);
        s.args.set(command.to_string());
        s.shell.set(spec.args[0].clone());
    } else {
        s.command_mode.set(CommandMode::Program);
        s.args.set(spec.args.join("\n"));
        s.shell.set(String::new());
    }

    let restart = if spec.keep_alive {
        RestartPolicy::Always
    } else if spec.restart_on_crash {
        RestartPolicy::OnCrash
    } else {
        RestartPolicy::Never
    };
    s.restart.set(restart);

    let schedule = if spec.start_interval > 0 {
        s.interval.set(spec.start_interval.to_string());
        ScheduleMode::Interval
    } else if spec.calendar_custom || !spec.calendar.empty() {
        s.interval.set(String::new());
        ScheduleMode::Calendar
    } else {
        s.interval.set(String::new());
        ScheduleMode::Service
    };
    s.schedule_mode.set(schedule);
    s.when_loaded.set(schedule);
    s.error.set(None);
}

pub(crate) fn is_existing() -> bool {
    signals().existing.with(Option::is_some)
}

pub(crate) fn existing_display() -> Option<String> {
    signals()
        .existing
        .with(|path| path.as_ref().map(|p| p.display().to_string()))
}

pub(crate) fn source() -> Source {
    signals().source.get()
}

pub(crate) fn set_source(next: Source) {
    let s = signals();
    if s.existing.with_untracked(Option::is_some) {
        return;
    }
    s.source.set(normalize_job_source(next));
}

pub(crate) fn is_loading() -> bool {
    signals().loading.get()
}

pub(crate) fn is_saving() -> bool {
    signals().saving.get()
}

pub(crate) fn error() -> Option<String> {
    signals().error.get()
}

pub(crate) fn has_error() -> bool {
    signals().error.with(Option::is_some)
}

pub(crate) fn subtitle() -> String {
    let existing = existing_display();
    let root_hint = source() != Source::UserAgent;
    match (existing, root_hint) {
        (Some(path), true) => format!("{path} · Asks for in-app confirmation before saving"),
        (Some(path), false) => path,
        (None, true) => "Asks for in-app confirmation before saving".to_string(),
        (None, false) => String::new(),
    }
}

pub(crate) fn has_subtitle() -> bool {
    is_existing() || source() != Source::UserAgent
}

/// Switches schedule mode, clearing service-only options when moving to a
/// schedule the file did not originally have (matching Go's `whenChanged`).
pub(crate) fn set_schedule_mode(next: ScheduleMode) {
    let s = signals();
    if s.calendar_custom.get_untracked() {
        return;
    }
    s.schedule_mode.set(next);
    if next == ScheduleMode::Service {
        s.run_at_load.set(true);
        return;
    }
    s.run_at_load.set(false);
    if s.when_loaded.get_untracked() != next {
        s.start_on_mount.set(false);
        s.queue_dirs.set(String::new());
        if !s.keep_alive_custom.get_untracked() {
            s.restart.set(RestartPolicy::Never);
        }
    }
}

/// Computes the default log file path for `source`, `user_name`, and `label`,
/// or `None` when no single default path is appropriate (system agents run per
/// user, and non-root daemons may not write to `/Library/Logs`).
pub fn log_default_path(
    source: Source,
    user_name: &str,
    label: &str,
    home_dir: Option<&Path>,
) -> Option<String> {
    match source {
        Source::UserAgent => {
            let home = home_dir?;
            Some(
                home.join("Library/Logs")
                    .join(format!("{label}.log"))
                    .display()
                    .to_string(),
            )
        }
        Source::Daemon if user_name.trim().is_empty() => Some(format!("/Library/Logs/{label}.log")),
        _ => None,
    }
}

/// Formats `path` with a leading `~/` when it resides under `home_dir`.
pub fn tilde_path(path: &str, home_dir: Option<&Path>) -> String {
    if let Some(home) = home_dir {
        let prefix = format!("{}/", home.display());
        if let Some(rest) = path.strip_prefix(&prefix) {
            return format!("~/{rest}");
        }
    }
    path.to_string()
}

pub(crate) fn log_default_preview_path() -> Option<String> {
    let s = signals();
    let src = s.source.get();
    let user = s.user_name.get();
    let home = provider().home_dir();
    let raw = log_default_path(src, &user, "<label>", home.as_deref())?;
    Some(tilde_path(&raw, home.as_deref()))
}

pub(crate) fn has_log_default_option() -> bool {
    !is_existing() && log_default_preview_path().is_some()
}

/// Pure form snapshot built into a validated [`Spec`], suitable for unit tests
/// as well as the live editor store.
#[derive(Debug, Clone)]
pub struct FormSnapshot {
    pub source: Source,
    pub existing: Option<PathBuf>,
    pub label: String,
    pub command_mode: CommandMode,
    pub shell: String,
    pub args: String,
    pub working_dir: String,
    pub env: String,
    pub schedule_mode: ScheduleMode,
    pub run_at_load: bool,
    pub restart: RestartPolicy,
    pub keep_alive_custom: bool,
    pub start_on_mount: bool,
    pub queue_dirs: String,
    pub interval: String,
    pub cal_minute: String,
    pub cal_hour: String,
    pub cal_day: String,
    pub cal_month: String,
    pub cal_weekday: String,
    pub calendar_custom: bool,
    pub user_name: String,
    pub group_name: String,
    pub root_dir: String,
    pub stdout: String,
    pub stderr: String,
    pub log_default: bool,
}

impl Default for FormSnapshot {
    fn default() -> Self {
        FormSnapshot {
            source: Source::UserAgent,
            existing: None,
            label: String::new(),
            command_mode: CommandMode::default(),
            shell: String::new(),
            args: String::new(),
            working_dir: String::new(),
            env: String::new(),
            schedule_mode: ScheduleMode::default(),
            run_at_load: false,
            restart: RestartPolicy::default(),
            keep_alive_custom: false,
            start_on_mount: false,
            queue_dirs: String::new(),
            interval: String::new(),
            cal_minute: String::new(),
            cal_hour: String::new(),
            cal_day: String::new(),
            cal_month: String::new(),
            cal_weekday: String::new(),
            calendar_custom: false,
            user_name: String::new(),
            group_name: String::new(),
            root_dir: String::new(),
            stdout: String::new(),
            stderr: String::new(),
            log_default: false,
        }
    }
}

impl FormSnapshot {
    /// Converts the form fields into a validated [`Spec`].
    pub fn build_spec(&self, home_dir: Option<&Path>) -> Result<Spec, String> {
        let mut spec = Spec {
            label: self.label.trim().to_string(),
            existing: self.existing.is_some(),
            working_dir: self.working_dir.trim().to_string(),
            run_at_load: self.run_at_load,
            keep_alive: self.restart == RestartPolicy::Always || self.keep_alive_custom,
            keep_alive_custom: self.keep_alive_custom,
            restart_on_crash: self.restart == RestartPolicy::OnCrash,
            calendar_custom: self.calendar_custom,
            start_on_mount: self.start_on_mount,
            queue_dirs: parse_list(&self.queue_dirs),
            user_name: self.user_name.trim().to_string(),
            group_name: self.group_name.trim().to_string(),
            root_dir: self.root_dir.trim().to_string(),
            stdout: self.stdout.trim().to_string(),
            stderr: self.stderr.trim().to_string(),
            ..Spec::default()
        };

        // Check label first when empty so validation surfaces label guidance
        // before command guidance (matching Go's editorState.spec()).
        if !spec.label.is_empty() {
            match self.command_mode {
                CommandMode::Shell => {
                    let command = self.args.trim();
                    if command.is_empty() {
                        return Err("give the command to run".to_string());
                    }
                    let mut args = shell_args(command);
                    if !self.shell.is_empty() {
                        args[0] = self.shell.clone();
                    }
                    spec.args = args;
                }
                CommandMode::Program => {
                    spec.args = parse_lines(&self.args);
                }
            }
        }

        match self.schedule_mode {
            ScheduleMode::Service => {}
            ScheduleMode::Interval => {
                let seconds = self.interval.trim().parse::<i32>().unwrap_or(0);
                if seconds <= 0 {
                    return Err("the interval is a number of seconds".to_string());
                }
                spec.start_interval = seconds;
            }
            ScheduleMode::Calendar => {
                spec.calendar = Calendar {
                    minute: self.cal_minute.clone(),
                    hour: self.cal_hour.clone(),
                    day: self.cal_day.clone(),
                    month: self.cal_month.clone(),
                    weekday: self.cal_weekday.clone(),
                };
                if spec.calendar.empty() && !self.calendar_custom {
                    return Err("give at least one field of the calendar".to_string());
                }
            }
        }

        spec.env = parse_env(&self.env).map_err(|error| error.to_string())?;

        if self.log_default
            && self.existing.is_none()
            && spec.stdout.is_empty()
            && spec.stderr.is_empty()
            && !spec.label.is_empty()
            && let Some(path) =
                log_default_path(self.source, &spec.user_name, &spec.label, home_dir)
        {
            spec.stdout = path.clone();
            spec.stderr = path;
        }

        spec.validate().map_err(|error| error.to_string())?;
        Ok(spec)
    }
}

fn snapshot_form(s: EditorSignals) -> FormSnapshot {
    FormSnapshot {
        source: s.source.get_untracked(),
        existing: s.existing.get_untracked(),
        label: s.label.get_untracked(),
        command_mode: s.command_mode.get_untracked(),
        shell: s.shell.get_untracked(),
        args: s.args.get_untracked(),
        working_dir: s.working_dir.get_untracked(),
        env: s.env.get_untracked(),
        schedule_mode: s.schedule_mode.get_untracked(),
        run_at_load: s.run_at_load.get_untracked(),
        restart: s.restart.get_untracked(),
        keep_alive_custom: s.keep_alive_custom.get_untracked(),
        start_on_mount: s.start_on_mount.get_untracked(),
        queue_dirs: s.queue_dirs.get_untracked(),
        interval: s.interval.get_untracked(),
        cal_minute: s.cal_minute.get_untracked(),
        cal_hour: s.cal_hour.get_untracked(),
        cal_day: s.cal_day.get_untracked(),
        cal_month: s.cal_month.get_untracked(),
        cal_weekday: s.cal_weekday.get_untracked(),
        calendar_custom: s.calendar_custom.get_untracked(),
        user_name: s.user_name.get_untracked(),
        group_name: s.group_name.get_untracked(),
        root_dir: s.root_dir.get_untracked(),
        stdout: s.stdout.get_untracked(),
        stderr: s.stderr.get_untracked(),
        log_default: s.log_default.get_untracked(),
    }
}

/// Builds and validates the current form's [`Spec`].
pub fn build_current_spec() -> Result<Spec, String> {
    let s = signals();
    let home = provider().home_dir();
    snapshot_form(s).build_spec(home.as_deref())
}

// --- Save flow & confirmation ------------------------------------------------

/// Warning message shown before creating a root-owned system agent or daemon.
pub fn create_warning(source: Source, load: bool) -> String {
    let target = if source == Source::Daemon {
        "/Library/LaunchDaemons"
    } else {
        "/Library/LaunchAgents"
    };
    let mut msg = format!("This creates a root-owned plist file in {target}. ");
    if load {
        msg.push_str("It also loads the job now. ");
    }
    msg.push_str("The privileged helper will save it without a macOS password prompt.");
    msg
}

/// Warning message shown before modifying an existing root-owned plist.
pub fn change_warning(changes: &[String], reload: bool) -> String {
    let mut msg = format!("This changes {}. ", changes.join(", "));
    if reload {
        msg.push_str("The job loads again, which restarts it if it runs. ");
    }
    msg.push_str(
        "The software that installed it may depend on what it says, or undo this when it updates. ",
    );
    msg.push_str("The privileged helper will save it without a macOS password prompt.");
    msg
}

/// Validates and saves the current editor form.
///
/// User agents save immediately off-thread; system agents and daemons ask for
/// in-app confirmation first (computing changed keys off-thread on edits, and
/// closing without prompting when nothing changed).
pub fn save() {
    let s = signals();
    if s.saving.get_untracked() || s.loading.get_untracked() {
        return;
    }
    let prov = provider();
    let home = prov.home_dir();
    let snapshot = snapshot_form(s);
    let spec = match snapshot.build_spec(home.as_deref()) {
        Ok(spec) => spec,
        Err(message) => {
            s.error.set(Some(message));
            return;
        }
    };
    s.error.set(None);

    let src = snapshot.source;
    let apply = s.apply.get_untracked();
    let seq = s.sequence.get_untracked();

    if src == Source::UserAgent {
        write_job(s, prov, src, spec, snapshot.existing, apply, seq);
        return;
    }

    let Some(existing_path) = snapshot.existing else {
        let answer = alert_dialog()
            .title(format!("Create system job {}?", spec.label))
            .description(create_warning(src, apply))
            .cancel("cancel", "Cancel")
            .action("confirm", "Create Job")
            .show();
        task(async move {
            let confirmed = matches!(
                answer.await,
                Ok(result) if result.action.as_deref() == Some("confirm")
            );
            if confirmed && s.sequence.get_untracked() == seq {
                write_job(s, prov, src, spec, None, apply, seq);
            }
        });
        return;
    };

    s.saving.set(true);
    let diff_prov = Arc::clone(&prov);
    let diff_path = existing_path.clone();
    let diff_spec = spec.clone();
    task(async move {
        let changes_result = match blocking(move || {
            let current = diff_prov.read_raw(&diff_path)?;
            diff_spec
                .changes(&current)
                .map_err(|error| error.to_string())
        })
        .await
        {
            Ok(result) => result,
            Err(error) => Err(format!("could not compare property list: {error}")),
        };
        if s.sequence.get_untracked() != seq {
            return;
        }
        s.saving.set(false);

        let changes = match changes_result {
            Ok(changes) => changes,
            Err(message) => {
                s.error.set(Some(message));
                return;
            }
        };
        if changes.is_empty() {
            let message = format!("{} has no changes to save", spec.label);
            s.open.set(false);
            s.notice.set(Some(message.clone()));
            crate::toast::info(message);
            return;
        }

        let answer = alert_dialog()
            .title(format!("Save changes to {}?", spec.label))
            .description(change_warning(&changes, apply))
            .cancel("cancel", "Cancel")
            .action("confirm", "Save Changes")
            .show();
        let confirmed = matches!(
            answer.await,
            Ok(result) if result.action.as_deref() == Some("confirm")
        );
        if confirmed && s.sequence.get_untracked() == seq {
            write_job(s, prov, src, spec, Some(existing_path), apply, seq);
        }
    });
}

fn write_job(
    s: EditorSignals,
    prov: Arc<dyn EditorProvider>,
    src: Source,
    spec: Spec,
    existing: Option<PathBuf>,
    apply: bool,
    seq: u64,
) {
    s.saving.set(true);
    s.error.set(None);
    let label = spec.label.clone();
    task(async move {
        let outcome = match blocking(move || prov.save(src, spec, existing, apply)).await {
            Ok(result) => result,
            Err(error) => Err(format!("save task failed: {error}")),
        };
        if s.sequence.get_untracked() != seq {
            return;
        }
        s.saving.set(false);
        match outcome {
            Ok(SaveResult {
                path,
                load_error: Some(load_err),
            }) => {
                // File was saved on disk, only loading failed: subsequent save edits it.
                s.existing.set(Some(path.clone()));
                s.error.set(Some(load_err));
                let event = SavedEvent {
                    source: src,
                    path,
                    label,
                    loaded: false,
                };
                s.last_saved.set(Some(event.clone()));
                s.revision.update(|rev| *rev += 1);
                notify_saved(&event);
            }
            Ok(SaveResult {
                path,
                load_error: None,
            }) => {
                s.open.set(false);
                s.error.set(None);
                s.notice.set(Some(format!("{label} saved")));
                let event = SavedEvent {
                    source: src,
                    path,
                    label,
                    loaded: apply,
                };
                s.last_saved.set(Some(event.clone()));
                s.revision.update(|rev| *rev += 1);
                notify_saved(&event);
            }
            Err(message) => {
                s.error.set(Some(message));
            }
        }
    });
}

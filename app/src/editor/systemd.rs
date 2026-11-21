//! Native systemd `.service` (and companion `.timer`) unit editor for Linux.
//!
//! Uses fine-grained signals per form field, live `.service` / `.timer`
//! preview with real-time validation via [`devcore::systemd::UnitSpec`], and
//! off-thread load/save through [`SystemdEditorProvider`].

use std::{
    collections::BTreeMap,
    future::Future,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use devcore::{
    launchd::{SystemHost, SystemRunner},
    service::Source,
    systemd::{
        DefaultSystemdHelper, SystemdProvider, UnitSpec, parse_unit, quote_systemd_arg,
        split_systemd_words,
    },
};
use nsh_base::{
    alert_dialog, button, checkbox, dialog, input, tab, tabs, theme, toggle, toggle_group,
};
use nsh_core::{
    desktop::{WindowAttributes, WindowKind},
    element::{AutoCapitalize, desktop},
    prelude::*,
};

use crate::theme as tokens;

use super::store::{SavedEvent, record_external_save};

/// The systemd sources that can be created or edited in the unit editor.
pub const SYSTEMD_KINDS: [Source; 2] = [Source::SystemdUser, Source::SystemdSystem];

/// Supported `Restart=` presets in the segmented control.
const RESTART_OPTIONS: [(&str, &str); 4] = [
    ("no", "No"),
    ("on-failure", "On failure"),
    ("always", "Always"),
    ("on-abnormal", "On abnormal"),
];

const EDITOR_MIN_SIZE: (f32, f32) = (520.0, 520.0);
const EDITOR_MAX_WIDTH: f32 = 640.0;

/// Injectable I/O seam for loading and saving systemd unit files.
pub trait SystemdEditorProvider: Send + Sync {
    /// Loads and parses an existing `.service` unit from `path`.
    fn load(&self, source: Source, path: &Path) -> Result<UnitSpec, String>;

    /// Validates, writes, reloads, and optionally enables/starts `spec`.
    fn save(
        &self,
        source: Source,
        spec: UnitSpec,
        enable_and_start: bool,
    ) -> Result<PathBuf, String>;
}

/// Production [`SystemdEditorProvider`] backed by [`devcore::systemd::SystemdProvider`].
pub struct DefaultSystemdEditorProvider {
    user: SystemdProvider,
    system: SystemdProvider,
}

impl DefaultSystemdEditorProvider {
    pub fn new() -> Self {
        let runner = Arc::new(SystemRunner::new());
        let host = Arc::new(SystemHost::new());
        let user = SystemdProvider::user(runner.clone(), host.clone());
        let system = SystemdProvider::system(runner, host, Arc::new(DefaultSystemdHelper));
        Self { user, system }
    }

    /// Builds a provider from explicit user and system [`SystemdProvider`] instances
    /// (used in unit tests with isolated `TempDir` directories and fake runners).
    pub fn with_providers(user: SystemdProvider, system: SystemdProvider) -> Self {
        Self { user, system }
    }

    fn provider_for(&self, source: Source) -> Result<&SystemdProvider, String> {
        match source {
            Source::SystemdUser => Ok(&self.user),
            Source::SystemdSystem => Ok(&self.system),
            other => Err(format!("{} is not a systemd unit source", other.title())),
        }
    }
}

impl Default for DefaultSystemdEditorProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemdEditorProvider for DefaultSystemdEditorProvider {
    fn load(&self, source: Source, path: &Path) -> Result<UnitSpec, String> {
        let provider = self.provider_for(source)?;
        let mut spec = provider
            .load_unit(path)
            .map_err(|error| error.to_string())?;
        // If a companion `.timer` sits beside the `.service`, merge its schedule fields.
        if let Ok(timer_name) = spec.timer_filename()
            && let Some(parent) = path.parent()
        {
            let timer_path = parent.join(timer_name);
            if let Ok(timer_raw) = std::fs::read_to_string(&timer_path)
                && let Ok(timer_spec) = parse_unit(&spec.name, &timer_raw)
            {
                if spec.on_calendar.is_empty() {
                    spec.on_calendar = timer_spec.on_calendar;
                }
                if spec.on_unit_active_sec.is_none() {
                    spec.on_unit_active_sec = timer_spec.on_unit_active_sec;
                }
            }
        }
        Ok(spec)
    }

    fn save(
        &self,
        source: Source,
        spec: UnitSpec,
        enable_and_start: bool,
    ) -> Result<PathBuf, String> {
        self.provider_for(source)?
            .save_unit(&spec, enable_and_start)
            .map_err(|error| error.to_string())
    }
}

struct MissingFakeProvider;

impl SystemdEditorProvider for MissingFakeProvider {
    fn load(&self, _source: Source, _path: &Path) -> Result<UnitSpec, String> {
        Err("no systemd editor provider is installed".to_string())
    }

    fn save(
        &self,
        _source: Source,
        _spec: UnitSpec,
        _enable_and_start: bool,
    ) -> Result<PathBuf, String> {
        Err("no systemd editor provider is installed".to_string())
    }
}

static FAKE_MODE: AtomicBool = AtomicBool::new(false);
static FAKE_PROVIDER: LazyLock<Mutex<Option<Arc<dyn SystemdEditorProvider>>>> =
    LazyLock::new(|| Mutex::new(None));
static SYSTEM_PROVIDER: LazyLock<Arc<dyn SystemdEditorProvider>> =
    LazyLock::new(|| Arc::new(DefaultSystemdEditorProvider::new()));

fn provider() -> Arc<dyn SystemdEditorProvider> {
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

/// Switches the systemd editor into fake-provider mode for tests.
pub fn use_fake_provider() {
    FAKE_MODE.store(true, Ordering::Relaxed);
    *FAKE_PROVIDER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

/// Installs a fake [`SystemdEditorProvider`] for tests.
pub fn install_provider(provider: Arc<dyn SystemdEditorProvider>) {
    FAKE_MODE.store(true, Ordering::Relaxed);
    *FAKE_PROVIDER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(provider);
}

// --- Fine-grained reactive state ---------------------------------------------

#[derive(Clone, Copy)]
struct UnitEditorSignals {
    open: Signal<bool>,
    loading: Signal<bool>,
    saving: Signal<bool>,
    sequence: Signal<u64>,

    source: Signal<Source>,
    existing: Signal<Option<PathBuf>>,

    name: Signal<String>,
    description: Signal<String>,
    exec_start: Signal<String>,
    working_dir: Signal<String>,
    env: Signal<String>,
    restart: Signal<String>,
    restart_sec: Signal<String>,
    wanted_by: Signal<String>,
    user_name: Signal<String>,
    group_name: Signal<String>,
    on_calendar: Signal<String>,
    on_unit_active_sec: Signal<String>,
    stdout_path: Signal<String>,
    stderr_path: Signal<String>,
    apply: Signal<bool>,

    error: Signal<Option<String>>,
}

impl Default for UnitEditorSignals {
    fn default() -> Self {
        Self {
            open: signal(false),
            loading: signal(false),
            saving: signal(false),
            sequence: signal(0),

            source: signal(Source::SystemdUser),
            existing: signal(None),

            name: signal(String::new()),
            description: signal(String::new()),
            exec_start: signal(String::new()),
            working_dir: signal(String::new()),
            env: signal(String::new()),
            restart: signal("on-failure".to_string()),
            restart_sec: signal(String::new()),
            wanted_by: signal(default_wanted_by(Source::SystemdUser).to_string()),
            user_name: signal(String::new()),
            group_name: signal(String::new()),
            on_calendar: signal(String::new()),
            on_unit_active_sec: signal(String::new()),
            stdout_path: signal(String::new()),
            stderr_path: signal(String::new()),
            apply: signal(true),

            error: signal(None),
        }
    }
}

fn signals() -> UnitEditorSignals {
    global::<UnitEditorSignals>().get_untracked()
}

fn task(future: impl Future<Output = ()> + 'static) {
    Owner::detached().run(move || {
        spawn(future);
    });
}

/// Whether `source` targets the systemd unit editor rather than the macOS plist editor.
pub const fn is_systemd_source(source: Source) -> bool {
    matches!(source, Source::SystemdUser | Source::SystemdSystem)
}

/// Normalizes a source to either `SystemdUser` or `SystemdSystem`.
pub const fn normalize_systemd_source(source: Source) -> Source {
    match source {
        Source::SystemdSystem => Source::SystemdSystem,
        _ => Source::SystemdUser,
    }
}

/// Human-readable label for a systemd unit scope.
pub const fn kind_label(source: Source) -> &'static str {
    match source {
        Source::SystemdSystem => "System Unit",
        _ => "User Unit",
    }
}

/// Default `[Install] WantedBy=` target for a systemd unit scope.
pub const fn default_wanted_by(source: Source) -> &'static str {
    match source {
        Source::SystemdSystem => "multi-user.target",
        _ => "default.target",
    }
}

fn bump_sequence(s: UnitEditorSignals) -> u64 {
    let next = s.sequence.get_untracked().wrapping_add(1);
    s.sequence.set(next);
    next
}

fn clear_fields(s: UnitEditorSignals, source: Source) {
    s.name.set(String::new());
    s.description.set(String::new());
    s.exec_start.set(String::new());
    s.working_dir.set(String::new());
    s.env.set(String::new());
    s.restart.set("on-failure".to_string());
    s.restart_sec.set(String::new());
    s.wanted_by.set(default_wanted_by(source).to_string());
    s.user_name.set(String::new());
    s.group_name.set(String::new());
    s.on_calendar.set(String::new());
    s.on_unit_active_sec.set(String::new());
    s.stdout_path.set(String::new());
    s.stderr_path.set(String::new());
    s.error.set(None);
}

fn populate_from_spec(s: UnitEditorSignals, spec: &UnitSpec) {
    s.name.set(spec.name.clone());
    s.description.set(spec.description.clone());
    let exec = spec
        .exec_start
        .iter()
        .map(|arg| quote_systemd_arg(arg))
        .collect::<Vec<_>>()
        .join(" ");
    s.exec_start.set(exec);
    s.working_dir.set(spec.working_directory.clone());
    let env = spec
        .environment
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("\n");
    s.env.set(env);
    s.restart.set(if spec.restart.trim().is_empty() {
        "no".to_string()
    } else {
        spec.restart.clone()
    });
    s.restart_sec
        .set(spec.restart_sec.map(|v| v.to_string()).unwrap_or_default());
    s.wanted_by.set(spec.wanted_by.join(" "));
    s.user_name.set(spec.user.clone());
    s.group_name.set(spec.group.clone());
    s.on_calendar.set(spec.on_calendar.clone());
    s.on_unit_active_sec.set(
        spec.on_unit_active_sec
            .map(|v| v.to_string())
            .unwrap_or_default(),
    );
    s.stdout_path.set(spec.stdout_path.clone());
    s.stderr_path.set(spec.stderr_path.clone());
    s.error.set(None);
}

// --- Public lifecycle API ----------------------------------------------------

/// Opens the systemd unit editor for creating a new unit targeting `source`.
pub fn open_new(source: Source) {
    let source = normalize_systemd_source(source);
    let s = signals();
    bump_sequence(s);
    clear_fields(s, source);
    s.source.set(source);
    s.existing.set(None);
    s.apply.set(true);
    s.loading.set(false);
    s.saving.set(false);
    s.open.set(true);
}

/// Opens the systemd unit editor on an existing unit file at `path`.
pub fn open_edit_with_apply(source: Source, path: PathBuf, apply: bool) {
    let source = normalize_systemd_source(source);
    let s = signals();
    let seq = bump_sequence(s);
    clear_fields(s, source);
    s.source.set(source);
    s.existing.set(Some(path.clone()));
    s.apply.set(apply);
    s.saving.set(false);
    s.loading.set(true);
    s.open.set(true);

    let prov = provider();
    task(async move {
        let loaded = match blocking(move || prov.load(source, &path)).await {
            Ok(result) => result,
            Err(error) => Err(format!("could not read unit file: {error}")),
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

/// Closes the systemd unit editor.
pub fn close() {
    let s = signals();
    bump_sequence(s);
    s.open.set(false);
    s.loading.set(false);
    s.saving.set(false);
    s.error.set(None);
}

/// Whether the systemd unit editor is currently open.
pub fn is_open() -> bool {
    signals().open.get()
}

fn is_existing() -> bool {
    signals().existing.with(Option::is_some)
}

fn existing_display() -> Option<String> {
    signals()
        .existing
        .with(|path| path.as_ref().map(|p| p.display().to_string()))
}

fn is_loading() -> bool {
    signals().loading.get()
}

fn is_saving() -> bool {
    signals().saving.get()
}

fn has_error() -> bool {
    signals().error.with(Option::is_some)
}

fn subtitle() -> String {
    let existing = existing_display();
    let elevated = signals().source.get() == Source::SystemdSystem;
    match (existing, elevated) {
        (Some(path), true) => format!("{path} · Saved via privileged helper"),
        (Some(path), false) => path,
        (None, true) => {
            "Creates a system unit in /etc/systemd/system via privileged helper".to_string()
        }
        (None, false) => String::new(),
    }
}

fn has_subtitle() -> bool {
    is_existing() || signals().source.get() == Source::SystemdSystem
}

fn set_source(next: Source) {
    let s = signals();
    if s.existing.with_untracked(Option::is_some) {
        return;
    }
    let prev = s.source.get_untracked();
    let next = normalize_systemd_source(next);
    if prev == next {
        return;
    }
    // Update WantedBy automatically when it still holds the previous scope's default.
    if s.wanted_by.get_untracked().trim() == default_wanted_by(prev) {
        s.wanted_by.set(default_wanted_by(next).to_string());
    }
    s.source.set(next);
}

// --- Pure form snapshot, validation & live preview ---------------------------

/// Pure form snapshot built into a validated [`UnitSpec`], testable without UI.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SystemdFormSnapshot {
    pub source: Option<Source>,
    pub name: String,
    pub description: String,
    pub exec_start: String,
    pub working_dir: String,
    pub env: String,
    pub restart: String,
    pub restart_sec: String,
    pub wanted_by: String,
    pub user_name: String,
    pub group_name: String,
    pub on_calendar: String,
    pub on_unit_active_sec: String,
    pub stdout_path: String,
    pub stderr_path: String,
}

impl SystemdFormSnapshot {
    /// Parses and validates all form fields into a [`UnitSpec`].
    pub fn build_spec(&self) -> Result<UnitSpec, String> {
        let name = self.name.trim().to_string();
        if name.is_empty() {
            return Err("Unit name is required (e.g. my-worker.service)".to_string());
        }

        let exec_start = split_systemd_words(self.exec_start.trim());
        if exec_start.is_empty() {
            return Err("ExecStart command is required".to_string());
        }

        let environment = parse_env_lines(&self.env)?;

        let restart_sec = parse_optional_u32(&self.restart_sec, "RestartSec")?;
        let on_unit_active_sec = parse_optional_u64(&self.on_unit_active_sec, "OnUnitActiveSec")?;

        let wanted_by: Vec<String> = self
            .wanted_by
            .split(|c: char| c.is_whitespace() || c == ',')
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(str::to_owned)
            .collect();

        let is_system = self.source == Some(Source::SystemdSystem);
        let spec = UnitSpec {
            name,
            description: self.description.trim().to_string(),
            exec_start,
            working_directory: self.working_dir.trim().to_string(),
            environment,
            restart: self.restart.trim().to_string(),
            restart_sec,
            wanted_by,
            user: if is_system {
                self.user_name.trim().to_string()
            } else {
                String::new()
            },
            group: if is_system {
                self.group_name.trim().to_string()
            } else {
                String::new()
            },
            stdout_path: self.stdout_path.trim().to_string(),
            stderr_path: self.stderr_path.trim().to_string(),
            on_calendar: self.on_calendar.trim().to_string(),
            on_unit_active_sec,
        };
        spec.validate().map_err(|error| error.to_string())?;
        Ok(spec)
    }

    /// Renders a live `.service` (and companion `.timer`, when scheduled) preview,
    /// or returns the validation error explaining what still needs fixing.
    pub fn preview(&self) -> Result<String, String> {
        let spec = self.build_spec()?;
        let mut rendered = spec.render_unit();
        if let Some(timer) = spec.render_timer() {
            let timer_name = spec
                .timer_filename()
                .unwrap_or_else(|_| "companion.timer".to_string());
            rendered.push_str(&format!("\n# --- {timer_name} ---\n"));
            rendered.push_str(&timer);
        }
        Ok(rendered)
    }
}

fn parse_env_lines(raw: &str) -> Result<BTreeMap<String, String>, String> {
    let mut map = BTreeMap::new();
    for (idx, line) in raw.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            return Err(format!("Environment line {} must be KEY=value", idx + 1));
        };
        let key = key.trim();
        if key.is_empty() {
            return Err(format!(
                "Environment line {} is missing a variable name",
                idx + 1
            ));
        }
        map.insert(key.to_string(), value.to_string());
    }
    Ok(map)
}

fn parse_optional_u32(raw: &str, field: &str) -> Result<Option<u32>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let value = trimmed
        .parse::<u32>()
        .map_err(|_| format!("{field} must be a whole number of seconds"))?;
    Ok((value > 0).then_some(value))
}

fn parse_optional_u64(raw: &str, field: &str) -> Result<Option<u64>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let value = trimmed
        .parse::<u64>()
        .map_err(|_| format!("{field} must be a whole number of seconds"))?;
    if value == 0 {
        return Err(format!("{field} must be greater than 0"));
    }
    Ok(Some(value))
}

fn snapshot_tracked(s: UnitEditorSignals) -> SystemdFormSnapshot {
    SystemdFormSnapshot {
        source: Some(s.source.get()),
        name: s.name.get(),
        description: s.description.get(),
        exec_start: s.exec_start.get(),
        working_dir: s.working_dir.get(),
        env: s.env.get(),
        restart: s.restart.get(),
        restart_sec: s.restart_sec.get(),
        wanted_by: s.wanted_by.get(),
        user_name: s.user_name.get(),
        group_name: s.group_name.get(),
        on_calendar: s.on_calendar.get(),
        on_unit_active_sec: s.on_unit_active_sec.get(),
        stdout_path: s.stdout_path.get(),
        stderr_path: s.stderr_path.get(),
    }
}

fn snapshot_untracked(s: UnitEditorSignals) -> SystemdFormSnapshot {
    SystemdFormSnapshot {
        source: Some(s.source.get_untracked()),
        name: s.name.get_untracked(),
        description: s.description.get_untracked(),
        exec_start: s.exec_start.get_untracked(),
        working_dir: s.working_dir.get_untracked(),
        env: s.env.get_untracked(),
        restart: s.restart.get_untracked(),
        restart_sec: s.restart_sec.get_untracked(),
        wanted_by: s.wanted_by.get_untracked(),
        user_name: s.user_name.get_untracked(),
        group_name: s.group_name.get_untracked(),
        on_calendar: s.on_calendar.get_untracked(),
        on_unit_active_sec: s.on_unit_active_sec.get_untracked(),
        stdout_path: s.stdout_path.get_untracked(),
        stderr_path: s.stderr_path.get_untracked(),
    }
}

// --- Save flow ---------------------------------------------------------------

/// Validates and saves the current systemd unit form.
pub fn save() {
    let s = signals();
    if s.saving.get_untracked() || s.loading.get_untracked() {
        return;
    }
    let snapshot = snapshot_untracked(s);
    let spec = match snapshot.build_spec() {
        Ok(spec) => spec,
        Err(message) => {
            s.error.set(Some(message));
            return;
        }
    };
    s.error.set(None);

    let source = normalize_systemd_source(s.source.get_untracked());
    let apply = s.apply.get_untracked();
    let seq = s.sequence.get_untracked();
    let prov = provider();

    if source == Source::SystemdUser {
        write_unit(s, prov, source, spec, apply, seq);
        return;
    }

    let unit_name = spec.unit_filename().unwrap_or_else(|_| spec.name.clone());
    let existing = s.existing.with_untracked(Option::is_some);
    let title = if existing {
        format!("Save system unit {unit_name}?")
    } else {
        format!("Create system unit {unit_name}?")
    };
    let mut description =
        "This writes a root-owned unit file in /etc/systemd/system and reloads systemd."
            .to_string();
    if apply {
        description.push_str(" It will also enable and start the unit now.");
    }
    let answer = alert_dialog()
        .title(title)
        .description(description)
        .cancel("cancel", "Cancel")
        .action(
            "confirm",
            if existing { "Save Unit" } else { "Create Unit" },
        )
        .show();
    task(async move {
        let confirmed = matches!(
            answer.await,
            Ok(result) if result.action.as_deref() == Some("confirm")
        );
        if confirmed && s.sequence.get_untracked() == seq {
            write_unit(s, prov, source, spec, apply, seq);
        }
    });
}

fn write_unit(
    s: UnitEditorSignals,
    prov: Arc<dyn SystemdEditorProvider>,
    source: Source,
    spec: UnitSpec,
    apply: bool,
    seq: u64,
) {
    s.saving.set(true);
    s.error.set(None);
    let label = spec.unit_filename().unwrap_or_else(|_| spec.name.clone());
    task(async move {
        let outcome = match blocking(move || prov.save(source, spec, apply)).await {
            Ok(result) => result,
            Err(error) => Err(format!("save task failed: {error}")),
        };
        if s.sequence.get_untracked() != seq {
            return;
        }
        s.saving.set(false);
        match outcome {
            Ok(path) => {
                s.open.set(false);
                s.error.set(None);
                record_external_save(SavedEvent {
                    source,
                    path,
                    label,
                    loaded: apply,
                });
            }
            Err(message) => {
                s.error.set(Some(message));
            }
        }
    });
}

// --- Native UI ---------------------------------------------------------------

fn unit_window_attributes() -> WindowAttributes {
    let mut attrs = WindowAttributes::default();
    attrs.close_on_escape = Some(true);
    attrs.min_size = Some(Some(size(px(EDITOR_MIN_SIZE.0), px(EDITOR_MIN_SIZE.1))));
    attrs
}

/// Presents the systemd unit editor as a modal dialog window on desktop or an
/// inline modal overlay off-desktop.
pub fn unit_editor_window() -> impl IntoElement {
    if os().is_desktop() {
        desktop::window()
            .id("systemd-unit-editor")
            .kind(WindowKind::Dialog)
            .parent("main")
            .modal(true)
            .attributes(unit_window_attributes())
            .title(move || {
                if is_existing() {
                    "Edit Systemd Unit".to_string()
                } else {
                    "New Systemd Unit".to_string()
                }
            })
            .default_size(size(px(640.0), px(740.0)))
            .open(is_open)
            .on_open_change(|event| {
                if !event.open {
                    close();
                }
            })
            .content(|| unit_editor_panel_with_title(false))
            .into_element()
    } else {
        when(is_open, || {
            dialog()
                .label(move || {
                    if is_existing() {
                        "Edit Systemd Unit".to_string()
                    } else {
                        "New Systemd Unit".to_string()
                    }
                })
                .on_cancel(|_: &()| close())
                .child(
                    view()
                        .size_full()
                        .items_center()
                        .justify_center()
                        .bg(move || theme().background.opacity(0.6))
                        .child(
                            view()
                                .w(px(640.0))
                                .max_h(px(740.0))
                                .rounded(tokens::panel_radius)
                                .bg(tokens::panel)
                                .border_1()
                                .border_color(tokens::hairline)
                                .overflow_hidden()
                                .child(unit_editor_panel_with_title(true)),
                        ),
                )
        })
        .into_element()
    }
}

/// Full systemd unit editor panel, directly mountable in headless tests.
pub fn unit_editor_panel() -> impl IntoElement {
    unit_editor_panel_with_title(true)
}

fn unit_editor_panel_with_title(show_title: bool) -> impl IntoElement {
    let s = signals();

    on_key("mod+s", |_| {
        if !is_saving() && !is_loading() {
            save();
        }
    });

    view()
        .test_id("systemd-unit-editor")
        .flex_1()
        .flex_col()
        .min_h(px(0.0))
        .w_full()
        .bg(tokens::sheet)
        .text_color(move || theme().foreground)
        .child(when(
            move || show_title || has_subtitle() || !is_existing(),
            move || {
                view()
                    .w_full()
                    .flex_none()
                    .flex_col()
                    .items_center()
                    .px(tokens::space(5.0))
                    .py(tokens::space(3.0))
                    .border_b_1()
                    .border_color(tokens::hairline)
                    .child(
                        view()
                            .w_full()
                            .max_w(px(EDITOR_MAX_WIDTH))
                            .flex_col()
                            .gap(tokens::space(3.0))
                            .child(when(
                                move || show_title || has_subtitle(),
                                move || header(show_title),
                            ))
                            .child(when(move || !is_existing(), kind_selector)),
                    )
            },
        ))
        .child(
            scroll().flex_1().min_h(px(0.0)).child(
                view()
                    .w_full()
                    .flex_col()
                    .items_center()
                    .px(tokens::space(5.0))
                    .py(tokens::space(4.0))
                    .child(
                        view()
                            .w_full()
                            .max_w(px(EDITOR_MAX_WIDTH))
                            .flex_col()
                            .gap(tokens::space(4.0))
                            .child(when(is_loading, loading_banner))
                            .child(unit_section(s))
                            .child(service_section(s))
                            .child(when(
                                move || s.source.get() == Source::SystemdSystem,
                                move || runs_as_section(s),
                            ))
                            .child(timer_section(s))
                            .child(logs_section(s))
                            .child(preview_section(s)),
                    ),
            ),
        )
        .child(when(has_error, move || {
            view()
                .w_full()
                .flex_none()
                .flex_col()
                .items_center()
                .px(tokens::space(5.0))
                .pb(tokens::space(3.0))
                .child(
                    view()
                        .w_full()
                        .max_w(px(EDITOR_MAX_WIDTH))
                        .child(error_banner(s)),
                )
        }))
        .child(footer(s))
}

fn header(show_title: bool) -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(1.0))
        .child(when(show_title, || {
            text(move || {
                if is_existing() {
                    "Edit Systemd Unit"
                } else {
                    "New Systemd Unit"
                }
            })
            .test_id("systemd-editor-title")
            .text_size(px(15.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(move || theme().foreground)
        }))
        .child(when(has_subtitle, || {
            text(subtitle)
                .test_id("systemd-editor-subtitle")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
                .truncate()
        }))
}

fn kind_selector() -> impl IntoElement {
    let s = signals();
    tabs()
        .test_id("systemd-editor-kind-tabs")
        .accessibility_label("Unit scope")
        .flex_row()
        .self_start()
        .p(px(2.0))
        .gap(px(2.0))
        .rounded(move || theme().radius)
        .bg(tokens::inset)
        .border_1()
        .border_color(tokens::hairline)
        .children(SYSTEMD_KINDS.into_iter().map(move |kind| {
            let id = format!("systemd-kind-{}", kind.as_str());
            tab()
                .test_id(&id)
                .selected(move || s.source.get() == kind)
                .on_press(move |_: &()| set_source(kind))
                .h(px(24.0))
                .px(tokens::space(2.5))
                .items_center()
                .justify_center()
                .rounded(move || px((theme().radius.0 - 2.0).max(2.0)))
                .bg(move || {
                    if s.source.get() == kind {
                        tokens::panel()
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .border_1()
                .border_color(move || {
                    if s.source.get() == kind {
                        tokens::hairline()
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .hover(move |style| {
                    if s.source.get() == kind {
                        style.bg(tokens::panel())
                    } else {
                        style.bg(tokens::row_hover())
                    }
                })
                .child(
                    text(kind_label(kind))
                        .text_size(px(tokens::TEXT_SMALL))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(move || {
                            if s.source.get() == kind {
                                theme().foreground
                            } else {
                                theme().muted_foreground
                            }
                        }),
                )
        }))
}

fn loading_banner() -> impl IntoElement {
    view()
        .px(tokens::space(3.0))
        .py(tokens::space(2.0))
        .rounded(move || theme().radius)
        .bg(tokens::inset)
        .child(
            text("Loading…")
                .test_id("systemd-editor-loading")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground),
        )
}

fn unit_section(s: UnitEditorSignals) -> impl IntoElement {
    fieldset(
        "Unit",
        view()
            .flex_col()
            .gap(tokens::space(2.5))
            .child(field_row(
                "Name",
                text_input_box(
                    "systemd-name",
                    "Unit name",
                    "my-worker.service",
                    move || s.name.get(),
                    move |next| s.name.set(next),
                    is_existing,
                ),
            ))
            .child(field_row(
                "Description",
                text_input_box(
                    "systemd-description",
                    "Description",
                    "Local API background worker",
                    move || s.description.get(),
                    move |next| s.description.set(next),
                    || false,
                ),
            )),
    )
}

fn service_section(s: UnitEditorSignals) -> impl IntoElement {
    fieldset(
        "Service",
        view()
            .flex_col()
            .gap(tokens::space(2.5))
            .child(field_row(
                "ExecStart",
                text_input_box(
                    "systemd-exec-start",
                    "ExecStart",
                    "/usr/bin/node /home/me/app/server.js",
                    move || s.exec_start.get(),
                    move |next| s.exec_start.set(next),
                    || false,
                ),
            ))
            .child(field_row(
                "WorkingDirectory",
                text_input_box(
                    "systemd-workdir",
                    "WorkingDirectory",
                    "/home/me/app",
                    move || s.working_dir.get(),
                    move |next| s.working_dir.set(next),
                    || false,
                ),
            ))
            .child(field_row(
                "Environment",
                multiline_input_box(
                    "systemd-env",
                    "Environment",
                    px(56.0),
                    "KEY=value",
                    move || s.env.get(),
                    move |next| s.env.set(next),
                ),
            ))
            .child(field_row("Restart", restart_segmented(s)))
            .child(
                view()
                    .flex_row()
                    .gap(tokens::space(2.5))
                    .child(view().flex_1().min_w(px(0.0)).child(field_row(
                        "RestartSec",
                        text_input_box(
                            "systemd-restart-sec",
                            "RestartSec",
                            "5",
                            move || s.restart_sec.get(),
                            move |next| s.restart_sec.set(next),
                            || false,
                        ),
                    )))
                    .child(view().flex_1().min_w(px(0.0)).child(field_row(
                        "WantedBy",
                        text_input_box(
                            "systemd-wanted-by",
                            "WantedBy",
                            "default.target",
                            move || s.wanted_by.get(),
                            move |next| s.wanted_by.set(next),
                            || false,
                        ),
                    ))),
            ),
    )
}

fn restart_segmented(s: UnitEditorSignals) -> impl IntoElement {
    toggle_group()
        .test_id("systemd-restart-policy")
        .flex_row()
        .self_start()
        .p(px(2.0))
        .gap(px(2.0))
        .rounded(move || theme().radius)
        .bg(tokens::inset)
        .border_1()
        .border_color(tokens::hairline)
        .children(RESTART_OPTIONS.into_iter().map(move |(value, label)| {
            let test_id = format!("systemd-restart-{value}");
            toggle()
                .test_id(&test_id)
                .pressed(move || s.restart.get() == value)
                .on_change(move |_: &bool| s.restart.set(value.to_string()))
                .h(px(24.0))
                .px(tokens::space(2.5))
                .items_center()
                .justify_center()
                .rounded(move || px((theme().radius.0 - 2.0).max(2.0)))
                .bg(move || {
                    if s.restart.get() == value {
                        tokens::panel()
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .border_1()
                .border_color(move || {
                    if s.restart.get() == value {
                        tokens::hairline()
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .hover(move |style| {
                    if s.restart.get() == value {
                        style.bg(tokens::panel())
                    } else {
                        style.bg(tokens::row_hover())
                    }
                })
                .child(
                    text(label)
                        .text_size(px(tokens::TEXT_SMALL))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(move || {
                            if s.restart.get() == value {
                                theme().foreground
                            } else {
                                theme().muted_foreground
                            }
                        }),
                )
        }))
}

fn runs_as_section(s: UnitEditorSignals) -> impl IntoElement {
    fieldset(
        "Runs as",
        view()
            .flex_row()
            .gap(tokens::space(2.5))
            .child(view().flex_1().min_w(px(0.0)).child(field_row(
                "User",
                text_input_box(
                    "systemd-user",
                    "User",
                    "root",
                    move || s.user_name.get(),
                    move |next| s.user_name.set(next),
                    || false,
                ),
            )))
            .child(view().flex_1().min_w(px(0.0)).child(field_row(
                "Group",
                text_input_box(
                    "systemd-group",
                    "Group",
                    "root",
                    move || s.group_name.get(),
                    move |next| s.group_name.set(next),
                    || false,
                ),
            ))),
    )
}

fn timer_section(s: UnitEditorSignals) -> impl IntoElement {
    fieldset(
        "Timer (optional)",
        view()
            .flex_row()
            .gap(tokens::space(2.5))
            .child(view().flex_1().min_w(px(0.0)).child(field_row(
                "OnCalendar",
                text_input_box(
                    "systemd-on-calendar",
                    "OnCalendar",
                    "daily or *-*-* 03:00:00",
                    move || s.on_calendar.get(),
                    move |next| s.on_calendar.set(next),
                    || false,
                ),
            )))
            .child(view().flex_1().min_w(px(0.0)).child(field_row(
                "OnUnitActiveSec (seconds)",
                text_input_box(
                    "systemd-on-unit-active-sec",
                    "OnUnitActiveSec",
                    "300",
                    move || s.on_unit_active_sec.get(),
                    move |next| s.on_unit_active_sec.set(next),
                    || false,
                ),
            ))),
    )
}

fn logs_section(s: UnitEditorSignals) -> impl IntoElement {
    fieldset(
        "Logs (optional)",
        view()
            .flex_col()
            .gap(tokens::space(2.5))
            .child(field_row(
                "StandardOutput",
                text_input_box(
                    "systemd-stdout",
                    "StandardOutput",
                    "/path/to/stdout.log (leave empty for journald)",
                    move || s.stdout_path.get(),
                    move |next| s.stdout_path.set(next),
                    || false,
                ),
            ))
            .child(field_row(
                "StandardError",
                text_input_box(
                    "systemd-stderr",
                    "StandardError",
                    "/path/to/stderr.log (leave empty for journald)",
                    move || s.stderr_path.get(),
                    move |next| s.stderr_path.set(next),
                    || false,
                ),
            )),
    )
}

fn preview_section(s: UnitEditorSignals) -> impl IntoElement {
    let preview = computed(move || snapshot_tracked(s).preview());
    fieldset(
        "Live .service Preview",
        view().flex_col().gap(tokens::space(1.5)).child(
            view()
                .w_full()
                .p(tokens::space(2.5))
                .rounded(move || theme().radius)
                .bg(tokens::inset)
                .border_1()
                .border_color(tokens::hairline)
                .child(
                    text(move || match preview.get() {
                        Ok(rendered) => rendered,
                        Err(message) => format!("# Validation: {message}"),
                    })
                    .test_id("systemd-preview")
                    .font_family("monospace")
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || {
                        if preview.with(Result::is_ok) {
                            theme().foreground
                        } else {
                            theme().muted_foreground
                        }
                    })
                    .selectable(true),
                ),
        ),
    )
}

fn error_banner(s: UnitEditorSignals) -> impl IntoElement {
    view()
        .px(tokens::space(3.0))
        .py(tokens::space(2.0))
        .rounded(move || theme().radius)
        .bg(move || theme().destructive.opacity(0.1))
        .child(
            text(move || s.error.get().unwrap_or_default())
                .test_id("systemd-editor-error")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().destructive),
        )
}

fn footer(s: UnitEditorSignals) -> impl IntoElement {
    let disabled = move || is_saving() || is_loading();
    view()
        .w_full()
        .flex_none()
        .flex_col()
        .items_center()
        .px(tokens::space(5.0))
        .py(tokens::space(3.0))
        .bg(tokens::panel)
        .border_t_1()
        .border_color(tokens::hairline)
        .child(
            view()
                .w_full()
                .max_w(px(EDITOR_MAX_WIDTH))
                .flex_row()
                .items_center()
                .gap(tokens::space(2.0))
                .child(checkbox_row(
                    "systemd-editor-apply",
                    move || {
                        if is_existing() {
                            "Reload & enable after saving".to_string()
                        } else {
                            "Enable & start now".to_string()
                        }
                    },
                    move || s.apply.get(),
                    move |next| s.apply.set(next),
                ))
                .child(view().flex_1())
                .child(action_button(
                    "systemd-editor-cancel",
                    "Cancel",
                    false,
                    || false,
                    close,
                ))
                .child(action_button(
                    "systemd-editor-save",
                    "Save",
                    true,
                    disabled,
                    save,
                )),
        )
}

// --- Shared control helpers --------------------------------------------------

fn fieldset(title: &'static str, body: impl IntoElement) -> impl IntoElement {
    view()
        .w_full()
        .flex_col()
        .gap(tokens::space(1.5))
        .child(
            text(title)
                .px(tokens::space(1.5))
                .text_size(px(tokens::TEXT_CAPTION))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().muted_foreground),
        )
        .child(
            view()
                .w_full()
                .flex_col()
                .p(tokens::space(3.5))
                .rounded(tokens::panel_radius)
                .bg(tokens::panel)
                .border_1()
                .border_color(tokens::hairline)
                .child(body),
        )
}

fn field_row(label: &'static str, control: impl IntoElement) -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(1.0))
        .child(
            text(label)
                .text_size(px(tokens::TEXT_SMALL))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || theme().foreground),
        )
        .child(control)
}

fn text_input_box(
    test_id: &'static str,
    label: &'static str,
    placeholder: &'static str,
    value: impl Fn() -> String + 'static,
    on_change: impl Fn(String) + 'static,
    disabled: impl Fn() -> bool + Copy + 'static,
) -> impl IntoElement {
    let on_change = Rc::new(on_change);
    let value = Rc::new(value);
    view()
        .w_full()
        .h(px(28.0))
        .flex_row()
        .items_center()
        .px(tokens::space(2.0))
        .rounded(move || theme().radius)
        .bg(move || {
            if disabled() {
                tokens::inset()
            } else {
                tokens::control()
            }
        })
        .border_1()
        .border_color(tokens::hairline)
        .focus_within(|s| s.border_color(move || theme().ring))
        .child(
            input()
                .test_id(test_id)
                .label(label)
                .placeholder(placeholder)
                .value(move || value())
                .disabled(disabled)
                .auto_capitalize(AutoCapitalize::None)
                .auto_correct(false)
                .flex_1()
                .min_w(px(0.0))
                .h(px(26.0))
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || {
                    if disabled() {
                        theme().muted_foreground
                    } else {
                        theme().foreground
                    }
                })
                .on_change(move |text: &str| on_change(text.to_owned())),
        )
}

fn multiline_input_box(
    test_id: &'static str,
    label: &'static str,
    height: Points,
    placeholder: &'static str,
    value: impl Fn() -> String + 'static,
    on_change: impl Fn(String) + 'static,
) -> impl IntoElement {
    let on_change = Rc::new(on_change);
    let value = Rc::new(value);
    view()
        .w_full()
        .min_h(height)
        .p(tokens::space(2.0))
        .rounded(move || theme().radius)
        .bg(tokens::control)
        .border_1()
        .border_color(tokens::hairline)
        .focus_within(|s| s.border_color(move || theme().ring))
        .child(
            input()
                .test_id(test_id)
                .label(label)
                .placeholder(placeholder)
                .value(move || value())
                .multiline(true)
                .auto_capitalize(AutoCapitalize::None)
                .auto_correct(false)
                .w_full()
                .min_h(px((height.0 - 8.0).max(36.0)))
                .font_family("monospace")
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().foreground)
                .on_change(move |text: &str| on_change(text.to_owned())),
        )
}

fn checkbox_row(
    test_id: &'static str,
    label: impl Fn() -> String + 'static,
    checked: impl Fn() -> bool + Copy + 'static,
    on_change: impl Fn(bool) + 'static,
) -> impl IntoElement {
    let label = Rc::new(label);
    let label_a11y = Rc::clone(&label);
    checkbox()
        .test_id(test_id)
        .checked(checked)
        .accessibility_label(move || Some(SharedString::from(label_a11y())))
        .flex_row()
        .items_center()
        .gap(tokens::space(2.0))
        .on_change(move |next: &bool| on_change(*next))
        .indicator(
            view()
                .size(px(14.0))
                .flex_none()
                .items_center()
                .justify_center()
                .rounded(px(3.5))
                .bg(move || {
                    if checked() {
                        theme().primary
                    } else {
                        tokens::control()
                    }
                })
                .border_1()
                .border_color(move || {
                    if checked() {
                        theme().primary
                    } else {
                        tokens::hairline()
                    }
                }),
        )
        .child(
            text(move || label())
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().foreground),
        )
}

fn action_button(
    test_id: &'static str,
    label: &'static str,
    primary: bool,
    disabled: impl Fn() -> bool + Copy + 'static,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    button()
        .test_id(test_id)
        .h(px(28.0))
        .px(tokens::space(3.5))
        .items_center()
        .justify_center()
        .rounded(move || theme().radius)
        .bg(move || {
            if primary {
                theme().primary
            } else {
                tokens::control()
            }
        })
        .border_1()
        .border_color(move || {
            if primary {
                theme().primary
            } else {
                tokens::hairline()
            }
        })
        .hover(move |style| {
            if primary {
                style.bg(move || theme().primary.opacity(0.86))
            } else {
                style.bg(tokens::row_hover())
            }
        })
        .focus_visible(|style| style.ring(px(2.0)).ring_color(move || theme().ring))
        .disabled(disabled)
        .opacity(move || if disabled() { 0.45_f32 } else { 1.0_f32 })
        .on_press(on_press)
        .child(
            text(label)
                .text_size(px(tokens::TEXT_SMALL))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || {
                    if primary {
                        theme().primary_foreground
                    } else {
                        theme().foreground
                    }
                }),
        )
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use devcore::{
        launchd::{CommandError, Host, LaunchdError, ProcessRunner},
        systemd::{SystemdConfig, SystemdError, SystemdHelper, SystemdUnitRemove, SystemdUnitSave},
    };

    use super::*;

    struct FakeRunner {
        calls: Mutex<Vec<(String, Vec<String>)>>,
    }

    impl FakeRunner {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl ProcessRunner for FakeRunner {
        fn run(&self, program: &str, args: &[&str]) -> Result<String, CommandError> {
            self.calls.lock().unwrap().push((
                program.to_string(),
                args.iter().map(|s| (*s).to_string()).collect(),
            ));
            Ok(String::new())
        }
    }

    struct FakeHost(PathBuf);

    impl Host for FakeHost {
        fn uid(&self) -> u32 {
            1000
        }
        fn home_dir(&self) -> Result<PathBuf, LaunchdError> {
            Ok(self.0.clone())
        }
        fn temp_dir(&self) -> PathBuf {
            self.0.join("tmp")
        }
        fn current_exe(&self) -> Result<PathBuf, LaunchdError> {
            Ok(PathBuf::from("/usr/bin/devbase"))
        }
        fn eval_symlinks(&self, path: &Path) -> Option<PathBuf> {
            Some(path.to_path_buf())
        }
        fn trash(&self, path: &Path) -> Result<(), LaunchdError> {
            std::fs::remove_file(path).map_err(|e| LaunchdError::io(e.to_string()))
        }
    }

    #[derive(Default)]
    struct FakeSystemdHelper {
        saves: Mutex<Vec<SystemdUnitSave>>,
    }

    impl SystemdHelper for FakeSystemdHelper {
        fn systemd_control(&self, _: &[Vec<String>]) -> Result<(), SystemdError> {
            Ok(())
        }
        fn systemd_save(&self, request: SystemdUnitSave) -> Result<(), SystemdError> {
            self.saves.lock().unwrap().push(request);
            Ok(())
        }
        fn systemd_remove(&self, _: SystemdUnitRemove) -> Result<(), SystemdError> {
            Ok(())
        }
    }

    #[test]
    fn systemd_snapshot_builds_and_previews_service_and_timer() {
        let form = SystemdFormSnapshot {
            source: Some(Source::SystemdSystem),
            name: "my-backup".into(),
            description: "Nightly backup".into(),
            exec_start: "/usr/bin/env bash \"/opt/my scripts/backup.sh\"".into(),
            working_dir: "/srv/backups".into(),
            env: "NODE_ENV=production\nGREETING=hello world\n# comment\n".into(),
            restart: "on-failure".into(),
            restart_sec: "10".into(),
            wanted_by: "multi-user.target".into(),
            user_name: "backup".into(),
            group_name: "backup".into(),
            on_calendar: "daily".into(),
            on_unit_active_sec: "3600".into(),
            stdout_path: "/var/log/backup.log".into(),
            stderr_path: "/var/log/backup.err.log".into(),
        };

        let spec = form.build_spec().expect("valid spec");
        assert_eq!(spec.unit_filename().unwrap(), "my-backup.service");
        assert_eq!(
            spec.exec_start,
            vec!["/usr/bin/env", "bash", "/opt/my scripts/backup.sh"]
        );
        assert_eq!(
            spec.environment.get("GREETING").map(String::as_str),
            Some("hello world")
        );
        assert_eq!(spec.user, "backup");
        assert_eq!(spec.group, "backup");
        assert_eq!(spec.on_calendar, "daily");
        assert_eq!(spec.on_unit_active_sec, Some(3600));

        let preview = form.preview().expect("preview renders");
        assert!(preview.contains("Description=Nightly backup"));
        assert!(preview.contains("Type=oneshot"));
        assert!(preview.contains("User=backup"));
        assert!(preview.contains("StandardOutput=append:/var/log/backup.log"));
        assert!(preview.contains("# --- my-backup.timer ---"));
        assert!(preview.contains("OnCalendar=daily"));
        assert!(preview.contains("OnUnitActiveSec=3600s"));
    }

    #[test]
    fn systemd_snapshot_validates_required_and_malformed_fields() {
        let empty = SystemdFormSnapshot::default();
        assert!(empty.build_spec().unwrap_err().contains("Unit name"));

        let no_exec = SystemdFormSnapshot {
            name: "worker".into(),
            ..SystemdFormSnapshot::default()
        };
        assert!(no_exec.build_spec().unwrap_err().contains("ExecStart"));

        let bad_name = SystemdFormSnapshot {
            name: "../escape".into(),
            exec_start: "/bin/true".into(),
            ..SystemdFormSnapshot::default()
        };
        assert!(bad_name.build_spec().is_err());

        let bad_env = SystemdFormSnapshot {
            name: "worker".into(),
            exec_start: "/bin/true".into(),
            env: "NO_EQUALS".into(),
            ..SystemdFormSnapshot::default()
        };
        assert!(bad_env.build_spec().unwrap_err().contains("KEY=value"));

        let bad_restart_sec = SystemdFormSnapshot {
            name: "worker".into(),
            exec_start: "/bin/true".into(),
            restart_sec: "abc".into(),
            ..SystemdFormSnapshot::default()
        };
        assert!(
            bad_restart_sec
                .build_spec()
                .unwrap_err()
                .contains("RestartSec")
        );

        let zero_timer = SystemdFormSnapshot {
            name: "worker".into(),
            exec_start: "/bin/true".into(),
            on_unit_active_sec: "0".into(),
            ..SystemdFormSnapshot::default()
        };
        assert!(
            zero_timer
                .build_spec()
                .unwrap_err()
                .contains("greater than 0")
        );

        // User units strip User=/Group= even if populated.
        let user_form = SystemdFormSnapshot {
            source: Some(Source::SystemdUser),
            name: "api".into(),
            exec_start: "/usr/bin/api".into(),
            user_name: "root".into(),
            group_name: "wheel".into(),
            ..SystemdFormSnapshot::default()
        };
        let spec = user_form.build_spec().unwrap();
        assert!(spec.user.is_empty());
        assert!(spec.group.is_empty());
    }

    #[test]
    fn systemd_editor_provider_round_trips_user_unit_and_timer_and_routes_system_via_helper() {
        let dir = std::env::temp_dir().join(format!(
            "devbase-systemd-editor-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let user_dir = dir.join("user");
        let sys_dir = dir.join("system");
        std::fs::create_dir_all(&user_dir).unwrap();
        std::fs::create_dir_all(&sys_dir).unwrap();

        let runner = Arc::new(FakeRunner::new());
        let host = Arc::new(FakeHost(dir.clone()));
        let helper = Arc::new(FakeSystemdHelper::default());

        let user_provider = SystemdProvider::new(
            SystemdConfig {
                source: Source::SystemdUser,
                scan_dirs: vec![user_dir.clone()],
                write_dir: user_dir.clone(),
                system: false,
                elevated: false,
            },
            runner.clone(),
            host.clone(),
            helper.clone(),
        );
        let system_provider = SystemdProvider::new(
            SystemdConfig {
                source: Source::SystemdSystem,
                scan_dirs: vec![sys_dir.clone()],
                write_dir: sys_dir.clone(),
                system: true,
                elevated: true,
            },
            runner.clone(),
            host,
            helper.clone(),
        );
        let editor_prov =
            DefaultSystemdEditorProvider::with_providers(user_provider, system_provider);

        let spec = SystemdFormSnapshot {
            source: Some(Source::SystemdUser),
            name: "devbase-demo".into(),
            description: "Demo Worker".into(),
            exec_start: "/usr/bin/env node /srv/demo.js".into(),
            working_dir: "/srv".into(),
            env: "PORT=3000".into(),
            restart: "on-failure".into(),
            restart_sec: "3".into(),
            wanted_by: "default.target".into(),
            on_calendar: "hourly".into(),
            ..SystemdFormSnapshot::default()
        }
        .build_spec()
        .unwrap();

        let saved_path = editor_prov
            .save(Source::SystemdUser, spec.clone(), true)
            .unwrap();
        assert_eq!(saved_path, user_dir.join("devbase-demo.service"));
        assert!(user_dir.join("devbase-demo.timer").is_file());

        let reloaded = editor_prov.load(Source::SystemdUser, &saved_path).unwrap();
        assert_eq!(reloaded.description, "Demo Worker");
        assert_eq!(
            reloaded.exec_start,
            vec!["/usr/bin/env", "node", "/srv/demo.js"]
        );
        assert_eq!(reloaded.on_calendar, "hourly");

        let sys_spec = SystemdFormSnapshot {
            source: Some(Source::SystemdSystem),
            name: "sys-worker".into(),
            exec_start: "/usr/sbin/worker".into(),
            wanted_by: "multi-user.target".into(),
            ..SystemdFormSnapshot::default()
        }
        .build_spec()
        .unwrap();
        let sys_path = editor_prov
            .save(Source::SystemdSystem, sys_spec, true)
            .unwrap();
        assert_eq!(sys_path, sys_dir.join("sys-worker.service"));
        assert_eq!(helper.saves.lock().unwrap().len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }
}

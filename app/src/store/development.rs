//! The development workspace's state: the runtimes Devbase manages, their
//! default versions, the shell startup integration for zsh, Bash and fish,
//! and the runtime terminal that opens a folder with a Node.js version on
//! `PATH`.
//!
//! Ported from `view_development.go`, `view_runtime_terminal.go`,
//! `view_java.go` and `view_additional_runtimes.go`, backed by
//! [`devcore::shell`]'s `Shell` facade. Installed versions come from
//! `state.json` (and uv's own list); defaults are resolved through the same
//! facade the Go manager used, including `.node-version`, `.nvmrc` and
//! `engines.node` walking.
//!
//! Every provider call does disk, process or network work, so each one runs on
//! a blocking thread and writes the store when it answers. Render code reads
//! the store; it never resolves a directory, edits a startup file or launches
//! Terminal itself.
//!
//! Tests install a provider of their own before the surface starts with
//! [`use_fake_provider`] and [`install_provider`]; production builds the
//! devcore `Shell` facade on first use.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use devcore::{
    model::Package,
    shell::{
        InstalledNode, NODE_BLOCK_BEGIN, NODE_BLOCK_END, NodeRelease, Shell, node::installed_nodes,
        runtime_block,
    },
    software::Software,
    state::Store,
};
use nsh_base::alert_dialog;
use nsh_core::prelude::*;

/// The workspace's tabs, in the Go order with the runtime workspace the Go
/// Software tab grew.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    /// The software workspace: install, verify and remove versions.
    #[default]
    Software,
    /// Node.js, Java and Python (uv): what is installed and what is default.
    Runtimes,
    /// The zsh, Bash and fish startup integration.
    ShellIntegration,
    /// Defaults, shell state and the runtime terminal.
    Overview,
}

impl Tab {
    pub(crate) const ALL: [Tab; 4] = [
        Tab::Software,
        Tab::Runtimes,
        Tab::ShellIntegration,
        Tab::Overview,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Tab::Software => "Software",
            Tab::Runtimes => "Runtimes",
            Tab::ShellIntegration => "Shell integration",
            Tab::Overview => "Overview",
        }
    }

    pub(crate) fn key(self) -> &'static str {
        match self {
            Tab::Software => "software",
            Tab::Runtimes => "runtimes",
            Tab::ShellIntegration => "shell",
            Tab::Overview => "overview",
        }
    }
}

/// Whether the workspace shows its tabs or the runtime terminal page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Workspace,
    Terminal,
}

/// One managed Node.js version, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeVersion {
    pub id: String,
    pub version: String,
    pub lts: String,
    pub default: bool,
}

/// One installed JDK, as the Go Java view listed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaVersion {
    pub id: String,
    pub version: String,
    pub product: String,
    pub default: bool,
}

/// One uv-managed Python interpreter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PythonVersion {
    pub key: String,
    pub version: String,
    pub path: String,
    pub pin: bool,
}

/// One shell's startup integration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellRow {
    /// `zsh`, `bash` or `fish`.
    pub shell: String,
    pub label: String,
    pub available: bool,
    /// The startup files this shell reads that the integration edits.
    pub files: Vec<String>,
    pub node_applied: bool,
    pub java_applied: bool,
}

/// One read of the development home.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub node: Vec<NodeVersion>,
    pub node_default: String,
    pub java: Vec<JavaVersion>,
    pub java_default: String,
    pub python: Vec<PythonVersion>,
    pub python_available: Vec<PythonVersion>,
    pub python_pin: String,
    pub uv_description: String,
    pub uv_path: String,
    /// Why uv could not be listed; empty when it answered.
    pub uv_error: String,
    pub shells: Vec<ShellRow>,
    pub node_shell_enabled: bool,
    pub java_shell_enabled: bool,
    pub bash_command: String,
}

impl Snapshot {
    pub fn node_version(&self, id: &str) -> Option<&NodeVersion> {
        self.node.iter().find(|row| row.id == id)
    }

    pub fn java_version(&self, id: &str) -> Option<&JavaVersion> {
        self.java.iter().find(|row| row.id == id)
    }

    pub fn python_version(&self, key: &str) -> Option<&PythonVersion> {
        self.python.iter().find(|row| row.key == key)
    }

    fn node_default_version(&self) -> String {
        self.node
            .iter()
            .find(|row| row.id == self.node_default)
            .map(|row| row.version.clone())
            .unwrap_or_default()
    }

    fn java_default_version(&self) -> String {
        self.java
            .iter()
            .find(|row| row.id == self.java_default)
            .map(|row| row.version.clone())
            .unwrap_or_default()
    }
}

/// The Node.js releases nodejs.org last listed, or the cache's copy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReleaseList {
    pub releases: Vec<NodeRelease>,
    pub stale: bool,
}

/// The version a folder runs: the result of devcore's selection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    pub id: String,
    pub version: String,
    pub dir: String,
    pub prefix: String,
    /// What the version is put ahead of `PATH` with.
    pub path: String,
    pub spec: String,
    /// `.node-version`, `.nvmrc`, `package.json`, `default` or `system`.
    pub source: String,
    pub system: bool,
}

/// The development operations the screen needs. The production implementation
/// wraps devcore's [`Shell`] and [`Software`]; a headless test implements it
/// with fixtures and never edits a startup file, runs a shell or launches
/// Terminal.
pub trait Provider: Send + Sync {
    fn snapshot(&self) -> Result<Snapshot, String>;

    fn node_releases(&self, refresh: bool) -> Result<ReleaseList, String>;
    fn set_node_default(&self, id: &str) -> Result<(), String>;
    fn set_java_default(&self, id: &str) -> Result<(), String>;

    fn node_binary(&self, id: &str) -> Result<String, String>;

    fn install_python(&self, key: &str) -> Result<(), String>;
    fn python_action(&self, key: &str, action: &str) -> Result<(), String>;
    fn set_uv_path(&self, path: &str) -> Result<(), String>;

    fn java_catalog(&self) -> Result<Vec<Package>, String>;
    fn install_java(&self, package: Package) -> Result<(), String>;

    fn node_shell(&self, enable: bool) -> Result<(), String>;
    fn java_shell(&self, enable: bool) -> Result<(), String>;

    fn select_node(&self, dir: &str, override_spec: Option<&str>) -> Result<Selection, String>;
    fn launch_terminal(&self, selection: &Selection) -> Result<(), String>;
}

// --- The production provider -------------------------------------------------

/// The development home, opened once.
struct Facade {
    store: Store,
}

impl Facade {
    fn new() -> Self {
        Facade {
            store: Store::new(devcore::state::default_root()),
        }
    }

    fn shell(&self) -> Shell {
        let software = Software::new(self.store.clone());
        Shell::new(self.store.clone(), software)
    }

    fn shell_rows(&self, shell: &Shell) -> Vec<ShellRow> {
        let files = shell.shell_startup_files(false).unwrap_or_default();
        let node = (NODE_BLOCK_BEGIN.to_string(), NODE_BLOCK_END.to_string());
        let java = runtime_block("java");
        ["zsh", "bash", "fish"]
            .into_iter()
            .map(|name| {
                let mut paths: Vec<String> = Vec::new();
                let mut node_applied = false;
                let mut java_applied = false;
                for file in &files {
                    let path = file.path.to_string_lossy().into_owned();
                    let shell_of = if file.fish {
                        "fish"
                    } else if path.ends_with(".zshenv")
                        || path.ends_with(".zprofile")
                        || path.ends_with(".zshrc")
                    {
                        "zsh"
                    } else {
                        "bash"
                    };
                    if shell_of != name {
                        continue;
                    }
                    if !paths.contains(&path) {
                        paths.push(path.clone());
                    }
                    let Ok(bytes) = std::fs::read(&file.path) else {
                        continue;
                    };
                    let contains = |needle: &str| {
                        !needle.is_empty()
                            && bytes
                                .windows(needle.len())
                                .any(|window| window == needle.as_bytes())
                    };
                    if contains(&node.0) && contains(&node.1) {
                        node_applied = true;
                    }
                    if contains(&java.0) && contains(&java.1) {
                        java_applied = true;
                    }
                }
                ShellRow {
                    shell: name.to_string(),
                    label: match name {
                        "zsh" => "zsh".to_string(),
                        "bash" => "Bash".to_string(),
                        _ => "fish".to_string(),
                    },
                    available: shell.has_shell(name),
                    files: paths,
                    node_applied,
                    java_applied,
                }
            })
            .collect()
    }
}

impl Provider for Facade {
    fn snapshot(&self) -> Result<Snapshot, String> {
        let state = self.store.read().map_err(|error| error.to_string())?;
        let shell = self.shell();
        let releases = shell.cached_node_releases();
        let node: Vec<NodeVersion> = installed_nodes(&state, &releases)
            .into_iter()
            .map(|node: InstalledNode| NodeVersion {
                id: node.id,
                version: node.version,
                lts: node.lts,
                default: node.default,
            })
            .collect();
        let mut java: Vec<JavaVersion> = state
            .packages
            .values()
            .filter(|package| package.name == "java")
            .map(|package| JavaVersion {
                id: package.id(),
                version: package.version.clone(),
                product: package.java_product.clone(),
                default: state.runtime_defaults.get("java") == Some(&package.id()),
            })
            .collect();
        java.sort_by(|a, b| a.version.cmp(&b.version));
        let java_default = state
            .runtime_defaults
            .get("java")
            .cloned()
            .unwrap_or_default();

        let mut python_error = String::new();
        let python: Vec<PythonVersion> = match shell.uv_pythons(true) {
            Ok(rows) => rows
                .into_iter()
                .map(|row| PythonVersion {
                    key: row.key,
                    version: row.version,
                    path: row.path,
                    pin: false,
                })
                .collect(),
            Err(error) => {
                python_error = error.to_string();
                Vec::new()
            }
        };
        let python_pin = shell.uv_global_pin().unwrap_or_default();
        let python: Vec<PythonVersion> = python
            .into_iter()
            .map(|row| PythonVersion {
                pin: !python_pin.is_empty() && (python_pin == row.key || python_pin == row.version),
                ..row
            })
            .collect();
        let python_available: Vec<PythonVersion> = match shell.uv_pythons(false) {
            Ok(rows) => rows
                .into_iter()
                .filter(|row| row.os_name == "macos" && row.arch == "aarch64")
                .map(|row| PythonVersion {
                    key: row.key,
                    version: row.version,
                    path: row.path,
                    pin: false,
                })
                .collect(),
            Err(error) => {
                if python_error.is_empty() {
                    python_error = error.to_string();
                }
                Vec::new()
            }
        };
        let uv_description = shell.uv_description().unwrap_or_default();
        let uv_path = shell
            .uv_path()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let shells = self.shell_rows(&shell);

        Ok(Snapshot {
            node,
            node_default: state.node_default.clone(),
            java,
            java_default,
            python,
            python_available,
            python_pin,
            uv_description,
            uv_path,
            uv_error: python_error,
            shells,
            node_shell_enabled: shell.node_shell_enabled(),
            java_shell_enabled: shell.runtime_shell_enabled("java"),
            bash_command: shell.bash_command(),
        })
    }

    fn node_releases(&self, refresh: bool) -> Result<ReleaseList, String> {
        let shell = self.shell();
        let cancelled = || false;
        shell
            .node_releases(&cancelled, refresh)
            .map(|(releases, stale)| ReleaseList { releases, stale })
            .map_err(|error| error.to_string())
    }

    fn set_node_default(&self, id: &str) -> Result<(), String> {
        self.shell()
            .set_node_default(id)
            .map_err(|error| error.to_string())
    }

    fn set_java_default(&self, id: &str) -> Result<(), String> {
        self.shell()
            .set_runtime_default("java", id)
            .map_err(|error| error.to_string())
    }

    fn node_binary(&self, id: &str) -> Result<String, String> {
        let state = self.store.read().map_err(|error| error.to_string())?;
        self.shell()
            .software()
            .binary(&state, id, "node")
            .map(|path| path.to_string_lossy().into_owned())
            .map_err(|error| error.to_string())
    }

    fn install_python(&self, key: &str) -> Result<(), String> {
        self.shell()
            .install_uv_python(key)
            .map_err(|error| error.to_string())
    }

    fn python_action(&self, key: &str, action: &str) -> Result<(), String> {
        self.shell()
            .uv_python_action(key, action)
            .map_err(|error| error.to_string())
    }

    fn set_uv_path(&self, path: &str) -> Result<(), String> {
        self.shell()
            .set_uv_path(Path::new(path))
            .map_err(|error| error.to_string())
    }

    fn java_catalog(&self) -> Result<Vec<Package>, String> {
        let transport = devcore::software::CurlTransport::new();
        devcore::shell::java_catalog(&transport).map_err(|error| error.to_string())
    }

    fn install_java(&self, package: Package) -> Result<(), String> {
        let cancelled = || false;
        self.shell()
            .software()
            .install(&package, &cancelled)
            .map_err(|error| error.to_string())
    }

    fn node_shell(&self, enable: bool) -> Result<(), String> {
        let shell = self.shell();
        let result = if enable {
            shell.enable_node_shell().map(|_| ())
        } else {
            shell.disable_node_shell().map(|_| ())
        };
        result.map_err(|error| error.to_string())
    }

    fn java_shell(&self, enable: bool) -> Result<(), String> {
        self.shell()
            .configure_runtime_shell("java", enable)
            .map_err(|error| error.to_string())
    }

    fn select_node(&self, dir: &str, override_spec: Option<&str>) -> Result<Selection, String> {
        let path = if dir.trim().is_empty() {
            std::env::current_dir().map_err(|error| error.to_string())?
        } else {
            PathBuf::from(dir)
        };
        self.shell()
            .select_node(&path, override_spec)
            .map(|selection| {
                let path = selection.path();
                Selection {
                    id: selection.id,
                    version: selection.version,
                    dir: selection.dir.to_string_lossy().into_owned(),
                    prefix: selection.prefix.to_string_lossy().into_owned(),
                    path,
                    spec: selection.spec,
                    source: selection.source,
                    system: selection.system,
                }
            })
            .map_err(|error| error.to_string())
    }

    fn launch_terminal(&self, selection: &Selection) -> Result<(), String> {
        // The launcher script puts the selected Node.js first on PATH and
        // starts the developer's login shell in the folder. No user command
        // and no secret enters argv.
        let directory = self.store.root().join("runtimes").join("node");
        std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        let script = directory.join("terminal.sh");
        let dir = devcore::shell::shell_quote(&selection.dir);
        let path = if selection.system || selection.path.is_empty() {
            String::new()
        } else {
            format!(
                "export PATH={}:$PATH\n",
                devcore::shell::shell_quote(&selection.path)
            )
        };
        let body = format!(
            "#!/bin/sh\n# Written by Devbase. Changes are overwritten.\ncd {dir}\n{path}exec \"${{SHELL:-/bin/zsh}}\" -l\n"
        );
        std::fs::write(&script, body).map_err(|error| error.to_string())?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
        let status = std::process::Command::new("/usr/bin/open")
            .arg("-a")
            .arg("Terminal")
            .arg(&script)
            .status()
            .map_err(|error| error.to_string())?;
        if !status.success() {
            return Err("Terminal could not be opened".to_string());
        }
        Ok(())
    }
}

// --- Providers ---------------------------------------------------------------

static FAKE: LazyLock<Mutex<Option<Arc<dyn Provider>>>> = LazyLock::new(|| Mutex::new(None));
static FAKE_MODE: AtomicBool = AtomicBool::new(false);

fn fake() -> &'static Mutex<Option<Arc<dyn Provider>>> {
    &FAKE
}

static FACADE: LazyLock<Facade> = LazyLock::new(Facade::new);

fn provider_of() -> Option<Arc<dyn Provider>> {
    if let Some(provider) = fake()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
    {
        return Some(provider);
    }
    if FAKE_MODE.load(Ordering::Relaxed) {
        return None;
    }
    Some(Arc::new(Facade {
        store: FACADE.store.clone(),
    }) as Arc<dyn Provider>)
}

/// Makes the surface use a test provider instead of devcore. Call it before
/// starting the headless surface, then [`install_provider`]. Tests only.
pub fn use_fake_provider() {
    FAKE_MODE.store(true, Ordering::Relaxed);
    *fake()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

/// Gives the surface a provider of the test's own. Tests only.
pub fn install_provider(provider: Arc<dyn Provider>) {
    FAKE_MODE.store(true, Ordering::Relaxed);
    *fake()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(provider);
}

// --- State -------------------------------------------------------------------

/// One read of the development home.
#[derive(Debug, Clone, Default)]
struct LoadState {
    snapshot: Snapshot,
    loading: bool,
    loaded: bool,
    error: Option<String>,
    /// Bumped for every read, so an older answer never replaces a newer one.
    sequence: u64,
}

/// The release list and its refresh.
#[derive(Debug, Clone, Default)]
struct ReleaseState {
    list: Option<ReleaseList>,
    loading: bool,
    loaded: bool,
    error: Option<String>,
    sequence: u64,
}

/// The JetBrains JDK catalog and its refresh.
#[derive(Debug, Clone, Default)]
struct CatalogState {
    packages: Vec<Package>,
    loading: bool,
    loaded: bool,
    error: Option<String>,
    sequence: u64,
}

/// The folder's Node.js resolution.
#[derive(Debug, Clone, Default)]
struct SelectionState {
    selection: Option<Selection>,
    loading: bool,
    error: Option<String>,
    sequence: u64,
}

#[derive(Debug, Clone, Default)]
struct UvPathDraft(String);

#[derive(Debug, Clone, Default)]
struct TerminalDirDraft(String);

#[derive(Debug, Clone, Default)]
struct Feedback {
    action_error: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct Workspace {
    mode: Mode,
    tab: Tab,
    load: LoadState,
    releases: ReleaseState,
    catalog: CatalogState,
    selection: SelectionState,
    /// Actions in flight, by a stable key.
    busy: BTreeSet<String>,
    override_spec: Option<String>,
    python_choice: String,
    java_choice: String,
}

fn workspace() -> Signal<Workspace> {
    global::<Workspace>()
}

fn uv_path_draft() -> Signal<UvPathDraft> {
    global::<UvPathDraft>()
}

fn terminal_dir_draft() -> Signal<TerminalDirDraft> {
    global::<TerminalDirDraft>()
}

fn feedback() -> Signal<Feedback> {
    global::<Feedback>()
}

/// Runs store work outside the view that asked for it: a pane the store
/// change rebuilds must not cancel the task that is changing it.
fn task(future: impl Future<Output = ()> + 'static) {
    Owner::detached().run(move || {
        spawn(future);
    });
}

fn next_sequence() -> u64 {
    workspace().update(|state| state.load.sequence += 1);
    workspace().with(|state| state.load.sequence)
}

fn clear_feedback() {
    if feedback().with_untracked(|fb| fb.action_error.is_some()) {
        feedback().update(|fb| {
            fb.action_error = None;
        });
    }
}

fn ctrl_error(message: String) {
    feedback().update(|fb| {
        fb.action_error = Some(message);
    });
}

fn show_notice(message: impl Into<String>) {
    clear_feedback();
    crate::toast::success(message);
}

fn show_info_notice(message: impl Into<String>) {
    clear_feedback();
    crate::toast::info(message);
}

fn start_busy(key: &str) {
    let key = key.to_string();
    workspace().update(|state| {
        state.busy.insert(key);
    });
    if feedback().with_untracked(|fb| fb.action_error.is_some()) {
        feedback().update(|fb| fb.action_error = None);
    }
}

fn finish_busy(key: &str) {
    let key = key.to_string();
    workspace().update(|state| {
        state.busy.remove(&key);
    });
}

fn apply_snapshot(sequence: u64, snapshot: Snapshot) {
    workspace().update(|state| {
        if state.load.sequence != sequence {
            return;
        }
        state.load.snapshot = snapshot;
        state.load.loading = false;
        state.load.loaded = true;
        state.load.error = None;
    });
}

/// Reads the development home. Called once by the page; a second call while a
/// read is in flight does nothing.
pub fn load() {
    if workspace().with(|state| state.load.loaded || state.load.loading) {
        return;
    }
    let Some(provider) = provider_of() else {
        ctrl_error("no development provider is available".to_string());
        return;
    };
    let sequence = next_sequence();
    workspace().update(|state| {
        state.load.loading = true;
        state.load.error = None;
    });
    task(async move {
        let answer = blocking(move || provider.snapshot()).await;
        let answer = answer
            .map_err(|error| error.to_string())
            .and_then(|answer| answer);
        match answer {
            Ok(snapshot) => apply_snapshot(sequence, snapshot),
            Err(error) => {
                let message = if error.is_empty() {
                    "the development home could not be read".to_string()
                } else {
                    error
                };
                workspace().update(|state| {
                    if state.load.sequence != sequence {
                        return;
                    }
                    state.load.loading = false;
                    state.load.error = Some(message);
                });
            }
        }
    });
}

/// Forces a fresh read.
pub fn refresh() {
    if workspace().with(|state| state.load.loading) {
        return;
    }
    let Some(provider) = provider_of() else {
        ctrl_error("no development provider is available".to_string());
        return;
    };
    let sequence = next_sequence();
    workspace().update(|state| {
        state.load.loading = true;
        state.load.error = None;
    });
    task(async move {
        let answer = blocking(move || provider.snapshot()).await;
        let answer = answer
            .map_err(|error| error.to_string())
            .and_then(|answer| answer);
        match answer {
            Ok(snapshot) => apply_snapshot(sequence, snapshot),
            Err(error) => {
                let message = if error.is_empty() {
                    "the development home could not be read".to_string()
                } else {
                    error
                };
                workspace().update(|state| {
                    if state.load.sequence != sequence {
                        return;
                    }
                    state.load.loading = false;
                    state.load.error = Some(message);
                });
            }
        }
    });
}

/// Runs an action that changes the home, then reads it again.
fn perform(
    key: &'static str,
    notice: String,
    op: impl FnOnce(Arc<dyn Provider>) -> Result<(), String> + Send + 'static,
) {
    if workspace().with(|state| state.busy.contains(key)) {
        return;
    }
    let Some(provider) = provider_of() else {
        ctrl_error("no development provider is available".to_string());
        return;
    };
    start_busy(key);
    let sequence = next_sequence();
    task(async move {
        let answer = blocking(move || {
            op(provider.clone())?;
            provider.snapshot()
        })
        .await;
        finish_busy(key);
        let answer = answer
            .map_err(|error| error.to_string())
            .and_then(|answer| answer);
        match answer {
            Ok(snapshot) => {
                apply_snapshot(sequence, snapshot);
                show_notice(notice);
            }
            Err(error) => {
                let message = if error.is_empty() {
                    "the operation did not complete".to_string()
                } else {
                    error
                };
                ctrl_error(message);
            }
        }
    });
}

// --- Reads -------------------------------------------------------------------

pub(crate) fn tab() -> Tab {
    workspace().with(|state| state.tab)
}

pub(crate) fn mode() -> Mode {
    workspace().with(|state| state.mode)
}

/// Shows a tab; the runtime tabs read the home when they first show.
pub(crate) fn select_tab(next: Tab) {
    workspace().update(|state| {
        state.tab = next;
        state.mode = Mode::Workspace;
    });
    clear_feedback();
    if next != Tab::Software {
        load();
    }
}

/// Shows the runtime terminal page.
pub(crate) fn open_terminal() {
    let cwd = std::env::current_dir()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    if terminal_dir_draft().with_untracked(|draft| draft.0.is_empty()) {
        terminal_dir_draft().update(|draft| draft.0 = cwd);
    }
    workspace().update(|state| {
        state.mode = Mode::Terminal;
        state.selection = SelectionState::default();
    });
    clear_feedback();
    load();
}

/// Returns to the workspace tabs.
pub(crate) fn close_terminal() {
    workspace().update(|state| {
        state.mode = Mode::Workspace;
    });
    clear_feedback();
}

pub(crate) fn loading() -> bool {
    workspace().with(|state| state.load.loading)
}

pub(crate) fn load_error() -> Option<String> {
    workspace().with(|state| state.load.error.clone())
}

pub(crate) fn action_error() -> Option<String> {
    feedback().with(|fb| fb.action_error.clone())
}

pub(crate) fn busy() -> bool {
    workspace().with(|state| !state.busy.is_empty())
}

pub(crate) fn is_busy(key: &str) -> bool {
    workspace().with(|state| state.busy.contains(key))
}

pub(crate) fn node_versions() -> Vec<NodeVersion> {
    workspace().with(|state| state.load.snapshot.node.clone())
}

pub(crate) fn java_versions() -> Vec<JavaVersion> {
    workspace().with(|state| state.load.snapshot.java.clone())
}

pub(crate) fn python_versions() -> Vec<PythonVersion> {
    workspace().with(|state| state.load.snapshot.python.clone())
}

pub(crate) fn python_available() -> Vec<PythonVersion> {
    workspace().with(|state| state.load.snapshot.python_available.clone())
}

pub(crate) fn python_pin() -> String {
    workspace().with(|state| state.load.snapshot.python_pin.clone())
}

pub(crate) fn uv_description() -> String {
    workspace().with(|state| state.load.snapshot.uv_description.clone())
}

pub(crate) fn uv_path() -> String {
    let draft = uv_path_draft().with(|draft| draft.0.clone());
    if draft.is_empty() {
        workspace().with(|state| state.load.snapshot.uv_path.clone())
    } else {
        draft
    }
}

pub(crate) fn uv_error() -> String {
    workspace().with(|state| state.load.snapshot.uv_error.clone())
}

pub(crate) fn shells() -> Vec<ShellRow> {
    workspace().with(|state| state.load.snapshot.shells.clone())
}

pub(crate) fn node_shell_enabled() -> bool {
    workspace().with(|state| state.load.snapshot.node_shell_enabled)
}

pub(crate) fn java_shell_enabled() -> bool {
    workspace().with(|state| state.load.snapshot.java_shell_enabled)
}

pub(crate) fn bash_command() -> String {
    workspace().with(|state| state.load.snapshot.bash_command.clone())
}

pub(crate) fn node_default_version() -> String {
    workspace().with(|state| state.load.snapshot.node_default_version())
}

pub(crate) fn java_default_version() -> String {
    workspace().with(|state| state.load.snapshot.java_default_version())
}

/// The version pickers' labels.
pub(crate) fn node_default_id() -> String {
    workspace().with(|state| state.load.snapshot.node_default.clone())
}

pub(crate) fn java_default_id() -> String {
    workspace().with(|state| state.load.snapshot.java_default.clone())
}

// --- Releases ----------------------------------------------------------------

pub(crate) fn releases() -> Option<ReleaseList> {
    workspace().with(|state| state.releases.list.clone())
}

pub(crate) fn releases_loading() -> bool {
    workspace().with(|state| state.releases.loading)
}

pub(crate) fn releases_error() -> Option<String> {
    workspace().with(|state| state.releases.error.clone())
}

/// Reads the release index once, from the cache when it is fresh.
pub(crate) fn load_releases() {
    if workspace().with(|state| state.releases.loaded || state.releases.loading) {
        return;
    }
    fetch_releases(false);
}

/// Forces a fresh release index, like the Go "Refresh available versions".
pub(crate) fn refresh_releases() {
    if workspace().with(|state| state.releases.loading) {
        return;
    }
    fetch_releases(true);
}

fn fetch_releases(refresh: bool) {
    let Some(provider) = provider_of() else {
        ctrl_error("no development provider is available".to_string());
        return;
    };
    let sequence = workspace().with(|state| state.releases.sequence + 1);
    workspace().update(|state| {
        state.releases.loading = true;
        state.releases.error = None;
        state.releases.sequence = sequence;
    });
    start_busy("releases");
    task(async move {
        let answer = blocking(move || provider.node_releases(refresh)).await;
        finish_busy("releases");
        let answer = answer
            .map_err(|error| error.to_string())
            .and_then(|answer| answer);
        match answer {
            Ok(list) => workspace().update(|state| {
                if state.releases.sequence != sequence {
                    return;
                }
                state.releases.list = Some(list);
                state.releases.loading = false;
                state.releases.loaded = true;
                state.releases.error = None;
            }),
            Err(error) => {
                let message = if error.is_empty() {
                    "the release list could not be read".to_string()
                } else {
                    error
                };
                workspace().update(|state| {
                    if state.releases.sequence != sequence {
                        return;
                    }
                    state.releases.loading = false;
                    state.releases.error = Some(message);
                });
            }
        }
    });
}

// --- Java catalog ------------------------------------------------------------

pub(crate) fn java_catalog() -> Vec<Package> {
    workspace().with(|state| state.catalog.packages.clone())
}

pub(crate) fn java_catalog_loading() -> bool {
    workspace().with(|state| state.catalog.loading)
}

pub(crate) fn java_catalog_error() -> Option<String> {
    workspace().with(|state| state.catalog.error.clone())
}

/// Reads the JetBrains JDK catalog once when the Runtimes tab first shows.
pub(crate) fn load_java_catalog() {
    if workspace().with(|state| state.catalog.loaded || state.catalog.loading) {
        return;
    }
    fetch_java_catalog();
}

/// Reads the JetBrains JDK catalog again.
pub(crate) fn refresh_java_catalog() {
    if workspace().with(|state| state.catalog.loading) {
        return;
    }
    fetch_java_catalog();
}

fn fetch_java_catalog() {
    let Some(provider) = provider_of() else {
        ctrl_error("no development provider is available".to_string());
        return;
    };
    let sequence = workspace().with(|state| state.catalog.sequence + 1);
    workspace().update(|state| {
        state.catalog.loading = true;
        state.catalog.error = None;
        state.catalog.sequence = sequence;
    });
    start_busy("java-catalog");
    task(async move {
        let answer = blocking(move || provider.java_catalog()).await;
        finish_busy("java-catalog");
        let answer = answer
            .map_err(|error| error.to_string())
            .and_then(|answer| answer);
        match answer {
            Ok(packages) => workspace().update(|state| {
                if state.catalog.sequence != sequence {
                    return;
                }
                state.catalog.packages = packages;
                state.catalog.loading = false;
                state.catalog.loaded = true;
                state.catalog.error = None;
            }),
            Err(error) => {
                let message = if error.is_empty() {
                    "the JetBrains JDK catalog could not be read".to_string()
                } else {
                    error
                };
                workspace().update(|state| {
                    if state.catalog.sequence != sequence {
                        return;
                    }
                    state.catalog.loading = false;
                    state.catalog.error = Some(message);
                });
            }
        }
    });
}

pub(crate) fn java_choice() -> String {
    let choice = workspace().with(|state| state.java_choice.clone());
    if !choice.is_empty() {
        return choice;
    }
    java_catalog().first().map(Package::id).unwrap_or_default()
}

pub(crate) fn select_java(id: &str) {
    let id = id.to_string();
    workspace().update(|state| state.java_choice = id);
}

/// Downloads and installs the chosen JDK.
pub(crate) fn install_java() {
    let choice = java_choice();
    let package = java_catalog()
        .into_iter()
        .find(|package| package.id() == choice);
    let Some(package) = package else {
        ctrl_error("select a JDK version to install".to_string());
        return;
    };
    let version = package.version.clone();
    let notice = format!("Installed Java {version}");
    perform("java-install", notice, move |provider| {
        provider.install_java(package.clone())
    });
}

// --- Defaults ----------------------------------------------------------------

pub(crate) fn set_node_default(id: &str) {
    let id = id.to_string();
    let notice = format!(
        "Default is Node.js {}",
        workspace()
            .with(|state| state
                .load
                .snapshot
                .node_version(&id)
                .map(|row| row.version.clone()))
            .unwrap_or_else(|| id.clone())
    );
    perform("node-default", notice, move |provider| {
        provider.set_node_default(&id)
    });
}

pub(crate) fn set_java_default(id: &str) {
    let id = id.to_string();
    let version = workspace()
        .with(|state| {
            state
                .load
                .snapshot
                .java_version(&id)
                .map(|row| row.version.clone())
        })
        .unwrap_or_else(|| id.clone());
    let notice = format!("Java default is {version}; enable terminal integration to use it");
    perform("java-default", notice, move |provider| {
        provider.set_java_default(&id)
    });
}

pub(crate) fn set_python_default(key: &str) {
    let key = key.to_string();
    let notice = format!("Python {key} is the uv default");
    perform("python-default", notice, move |provider| {
        provider.python_action(&key, "default")
    });
}

pub(crate) fn uninstall_python(key: &str) {
    let key = key.to_string();
    let notice = format!("Uninstalled Python {key}");
    perform("python-uninstall", notice, move |provider| {
        provider.python_action(&key, "uninstall")
    });
}

pub(crate) fn python_choice() -> String {
    let choice = workspace().with(|state| state.python_choice.clone());
    if !choice.is_empty() {
        return choice;
    }
    python_available()
        .first()
        .map(|row| row.key.clone())
        .unwrap_or_default()
}

pub(crate) fn select_python(key: &str) {
    let key = key.to_string();
    workspace().update(|state| state.python_choice = key);
}

pub(crate) fn install_python() {
    let key = python_choice();
    if key.is_empty() {
        ctrl_error("select an available macOS arm64 Python version".to_string());
        return;
    }
    let notice = format!("Installed Python {key} with uv");
    perform("python-install", notice, move |provider| {
        provider.install_python(&key)
    });
}

pub(crate) fn set_uv_path_draft(next: String) {
    uv_path_draft().update(|draft| draft.0 = next);
}

pub(crate) fn use_uv_path() {
    let path = uv_path_draft().with_untracked(|draft| draft.0.trim().to_string());
    if path.is_empty() {
        ctrl_error("choose an absolute uv executable path".to_string());
        return;
    }
    perform(
        "uv-path",
        "uv executable selected".to_string(),
        move |provider| provider.set_uv_path(&path),
    );
    uv_path_draft().update(|draft| draft.0.clear());
}

pub(crate) fn copy_python_path(path: &str) {
    if path.is_empty() {
        return;
    }
    set_clipboard(path.to_string());
    show_info_notice("Copied Python executable path.");
}

// --- Shell integration -------------------------------------------------------

/// Asks before changing every installed shell's startup files.
pub(crate) fn request_node_shell(enable: bool) {
    let (title, description, label) = if enable {
        (
            "Enable Node.js in your terminal?",
            "Adds a Devbase block to zsh, Bash and fish startup files. Each folder runs the Node.js version its .node-version, .nvmrc or package.json asks for, else the default.",
            "Enable Node.js",
        )
    } else {
        (
            "Remove Node.js shell integration?",
            "Removes only the Devbase Node.js blocks from shell startup files. Installed Node.js versions and the default are kept.",
            "Remove Node.js",
        )
    };
    let dialog = alert_dialog()
        .title(title)
        .description(description)
        .cancel("cancel", "Cancel");
    let dialog = if enable {
        dialog.action("confirm", label)
    } else {
        dialog.destructive("confirm", label)
    };
    let answer = dialog.show();
    task(async move {
        let chosen = matches!(
            answer.await,
            Ok(result) if result.action.as_deref() == Some("confirm")
        );
        if !chosen {
            return;
        }
        let notice = if enable {
            "Enabled for installed shells. Start a new terminal or AI session."
        } else {
            "Removed the Devbase Node.js blocks from all supported shells."
        };
        perform("node-shell", notice.to_string(), move |provider| {
            provider.node_shell(enable)
        });
    });
}

/// Asks before changing the named runtime's shell startup integration.
pub(crate) fn request_runtime_shell(name: &str, enable: bool) {
    if name == "java" {
        request_java_shell(enable);
    } else {
        request_node_shell(enable);
    }
}

/// Asks before changing every installed shell's Java startup integration.
pub(crate) fn request_java_shell(enable: bool) {
    let (title, description, label) = if enable {
        (
            "Enable Java in your terminal?",
            "Adds a Devbase block to zsh, Bash and fish startup files. Java follows .java-version and .sdkmanrc in each folder, else the default.",
            "Enable Java",
        )
    } else {
        (
            "Remove Java shell integration?",
            "Removes only the Devbase Java blocks from shell startup files. Installed JDKs and the default are kept.",
            "Remove Java",
        )
    };
    let dialog = alert_dialog()
        .title(title)
        .description(description)
        .cancel("cancel", "Cancel");
    let dialog = if enable {
        dialog.action("confirm", label)
    } else {
        dialog.destructive("confirm", label)
    };
    let answer = dialog.show();
    task(async move {
        let chosen = matches!(
            answer.await,
            Ok(result) if result.action.as_deref() == Some("confirm")
        );
        if !chosen {
            return;
        }
        let notice = if enable {
            "Terminal configuration updated; open a new terminal."
        } else {
            "Removed the Devbase Java blocks from all supported shells."
        };
        perform("java-shell", notice.to_string(), move |provider| {
            provider.java_shell(enable)
        });
    });
}

/// Copies the managed Node.js executable path after checking it is runnable.
pub(crate) fn copy_node_binary(id: &str) {
    let Some(provider) = provider_of() else {
        ctrl_error("no development provider is available".to_string());
        return;
    };
    let id = id.to_string();
    start_busy("node-binary");
    task(async move {
        let answer = blocking(move || provider.node_binary(&id)).await;
        finish_busy("node-binary");
        let answer = answer
            .map_err(|error| error.to_string())
            .and_then(|answer| answer);
        match answer {
            Ok(path) => {
                set_clipboard(path);
                show_info_notice("Copied Node.js executable path.");
            }
            Err(error) => {
                ctrl_error(if error.is_empty() {
                    "Node.js executable is unavailable. Repair the installation before copying its path."
                        .to_string()
                } else {
                    error
                });
            }
        }
    });
}

/// Copies the `BASH_ENV=… /bin/bash -c 'YOUR_COMMAND'` template once the
/// runtime is enabled in the terminal.
pub(crate) fn copy_bash(name: &str) {
    let enabled = if name == "java" {
        java_shell_enabled()
    } else {
        node_shell_enabled()
    };
    if !enabled {
        ctrl_error("Enable in terminal first to prepare the Bash environment.".to_string());
        return;
    }
    let command = bash_command();
    if command.is_empty() {
        ctrl_error("Enable in terminal first to prepare the Bash environment.".to_string());
        return;
    }
    set_clipboard(command);
    show_info_notice("Copied Bash command. Replace YOUR_COMMAND with the AI task's command.");
}

// --- Runtime terminal --------------------------------------------------------

pub(crate) fn terminal_dir() -> String {
    terminal_dir_draft().with(|draft| draft.0.clone())
}

pub(crate) fn set_terminal_dir(next: String) {
    terminal_dir_draft().update(|draft| draft.0 = next);
}

pub(crate) fn override_spec() -> Option<String> {
    workspace().with(|state| state.override_spec.clone())
}

pub(crate) fn set_override_spec(next: Option<String>) {
    workspace().update(|state| {
        state.override_spec = next;
        state.selection.selection = None;
        state.selection.error = None;
    });
}

pub(crate) fn selection() -> Option<Selection> {
    workspace().with(|state| state.selection.selection.clone())
}

pub(crate) fn selection_loading() -> bool {
    workspace().with(|state| state.selection.loading)
}

pub(crate) fn selection_error() -> Option<String> {
    workspace().with(|state| state.selection.error.clone())
}

/// Resolves the folder's Node.js version through the shell facade.
pub(crate) fn resolve() {
    if workspace().with(|state| state.selection.loading) {
        return;
    }
    let Some(provider) = provider_of() else {
        ctrl_error("no development provider is available".to_string());
        return;
    };
    let dir = terminal_dir_draft().with_untracked(|draft| draft.0.trim().to_string());
    let override_spec = workspace().with_untracked(|state| state.override_spec.clone());
    let sequence = workspace().with(|state| state.selection.sequence + 1);
    workspace().update(|state| {
        state.selection.loading = true;
        state.selection.error = None;
        state.selection.sequence = sequence;
    });
    start_busy("selection");
    task(async move {
        let answer = blocking(move || provider.select_node(&dir, override_spec.as_deref())).await;
        finish_busy("selection");
        let answer = answer
            .map_err(|error| error.to_string())
            .and_then(|answer| answer);
        match answer {
            Ok(selection) => workspace().update(|state| {
                if state.selection.sequence != sequence {
                    return;
                }
                state.selection.selection = Some(selection);
                state.selection.loading = false;
                state.selection.error = None;
            }),
            Err(error) => {
                let message = if error.is_empty() {
                    "the folder's Node.js version could not be resolved".to_string()
                } else {
                    error
                };
                workspace().update(|state| {
                    if state.selection.sequence != sequence {
                        return;
                    }
                    state.selection.loading = false;
                    state.selection.error = Some(message);
                });
            }
        }
    });
}

/// Opens Terminal in the folder with the resolved runtime on `PATH`.
pub(crate) fn launch_terminal() {
    let Some(selection) = selection() else {
        ctrl_error("Resolve the folder's Node.js version first.".to_string());
        return;
    };
    let Some(provider) = provider_of() else {
        ctrl_error("no development provider is available".to_string());
        return;
    };
    start_busy("launch");
    let version = selection.version.clone();
    task(async move {
        let answer = blocking(move || provider.launch_terminal(&selection)).await;
        finish_busy("launch");
        let answer = answer
            .map_err(|error| error.to_string())
            .and_then(|answer| answer);
        match answer {
            Ok(()) => show_info_notice(format!("Opened Terminal with Node.js {version}.")),
            Err(error) => ctrl_error(if error.is_empty() {
                "Terminal could not be opened".to_string()
            } else {
                error
            }),
        }
    });
}

// --- Formatting --------------------------------------------------------------

/// A folder short enough for a row.
pub(crate) fn shorten_path(path: &str) -> String {
    let Some(home) = std::env::var_os("HOME") else {
        return path.to_string();
    };
    let home = home.to_string_lossy();
    if let Some(rest) = path.strip_prefix(home.as_ref()) {
        format!("~{rest}")
    } else {
        path.to_string()
    }
}

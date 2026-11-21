//! Shell integration, ported from the Go `runtime_shell.go`,
//! `runtime_defaults.go`, `runtime_fish.go`, `node_manage.go`,
//! `node_releases.go`, `java_catalog.go` and `uv_python.go`.
//!
//! This layer owns the Development screen's backing API: the shell startup
//! blocks that make Node/Java visible to zsh, Bash and fish, Node version
//! resolution and selection, the JetBrains JDK catalog, and uv discovery.
//!
//! Every path is confined to the development home or the fixture home, and
//! every external command goes through [`crate::engine::CommandRunner`]. Tests
//! use a temp home and fake runner/HTTP seams; nothing executes a real shell,
//! `uv` or touches the network.

pub mod block;
pub mod ca_env;
pub mod java;
pub mod node;
pub mod uv;

use std::collections::BTreeMap;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::engine::CommandRunner;
use crate::launchd::CommandError;
use crate::software::{HttpTransport, Software, SoftwareError};
use crate::state::{StateError, Store};

pub use block::update_shell_block;
pub use ca_env::{
    CA_ENV_BLOCK_BEGIN, CA_ENV_BLOCK_END, ShellCaEnvConfig, ca_env_block_fish, ca_env_block_posix,
    ca_env_exports_fish, ca_env_exports_posix,
};
pub use java::{JavaCatalog, java_catalog, resolve_java};
pub use node::{
    InstalledNode, NodeRelease, NodeSelection, NodeVersionFile, node_index_path, parse_range,
    parse_semver, pick_node, resolve_node_spec,
};
pub use uv::UVPython;

/// Everything shell integration can fail with.
#[derive(Debug)]
pub enum ShellError {
    State(StateError),
    Software(SoftwareError),
    Io(io::Error),
    Json(serde_json::Error),
    Invalid(String),
    Command(CommandError),
    Missing(String),
}

impl ShellError {
    pub fn invalid(message: impl Into<String>) -> Self {
        ShellError::Invalid(message.into())
    }
}

impl std::fmt::Display for ShellError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShellError::State(error) => error.fmt(f),
            ShellError::Software(error) => error.fmt(f),
            ShellError::Io(error) => error.fmt(f),
            ShellError::Json(error) => error.fmt(f),
            ShellError::Invalid(message) | ShellError::Missing(message) => f.write_str(message),
            ShellError::Command(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ShellError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ShellError::State(error) => Some(error),
            ShellError::Software(error) => Some(error),
            ShellError::Io(error) => Some(error),
            ShellError::Json(error) => Some(error),
            ShellError::Command(error) => Some(error),
            ShellError::Invalid(_) | ShellError::Missing(_) => None,
        }
    }
}

impl From<StateError> for ShellError {
    fn from(error: StateError) -> Self {
        ShellError::State(error)
    }
}

impl From<SoftwareError> for ShellError {
    fn from(error: SoftwareError) -> Self {
        ShellError::Software(error)
    }
}

impl From<io::Error> for ShellError {
    fn from(error: io::Error) -> Self {
        ShellError::Io(error)
    }
}

impl From<serde_json::Error> for ShellError {
    fn from(error: serde_json::Error) -> Self {
        ShellError::Json(error)
    }
}

impl From<CommandError> for ShellError {
    fn from(error: CommandError) -> Self {
        ShellError::Command(error)
    }
}

/// The shell lookup seam: whether a shell binary is installed and executable.
pub type ShellAvailable = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Shell integration over one development home.
#[derive(Clone)]
pub struct Shell {
    store: Store,
    software: Software,
    runner: Arc<dyn CommandRunner>,
    http: Arc<dyn HttpTransport>,
    home: PathBuf,
    /// Only honor `ZDOTDIR`/`XDG_CONFIG_HOME` when the home is the real user
    /// home, never a test fixture.
    honor_environment_paths: bool,
    shell_env: Option<PathBuf>,
    path_dirs: Vec<PathBuf>,
    shell_available: Option<ShellAvailable>,
    node_dist_base: String,
    uv_candidates: Option<Vec<PathBuf>>,
    program: PathBuf,
}

impl std::fmt::Debug for Shell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shell")
            .field("root", &self.store.root())
            .field("home", &self.home)
            .finish()
    }
}

impl Shell {
    /// Production seams from the software manager.
    pub fn new(store: Store, software: Software) -> Self {
        let runner = Arc::new(crate::engine::SystemCommandRunner::new());
        let http = Arc::new(crate::software::CurlTransport::new());
        Self::with_seams(store, software, runner, http)
    }

    /// Explicit command/HTTP seams; used by tests and by callers that already
    /// hold a software manager's seams.
    pub fn with_seams(
        store: Store,
        software: Software,
        runner: Arc<dyn CommandRunner>,
        http: Arc<dyn HttpTransport>,
    ) -> Self {
        let home = store.home().unwrap_or_else(|_| PathBuf::from("/"));
        Self {
            store,
            software,
            runner,
            http,
            home,
            honor_environment_paths: true,
            shell_env: std::env::var_os("SHELL").map(PathBuf::from),
            path_dirs: std::env::var_os("PATH")
                .map(|value| std::env::split_paths(&value).collect())
                .unwrap_or_default(),
            shell_available: None,
            node_dist_base: "https://nodejs.org/dist/".into(),
            uv_candidates: None,
            program: crate::platform::paths::default_program_path(),
        }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn software(&self) -> &Software {
        &self.software
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Use a fixture home. Environment overrides (`ZDOTDIR`,
    /// `XDG_CONFIG_HOME`, `$SHELL`, `PATH`) are then ignored.
    pub fn with_home(mut self, home: impl Into<PathBuf>) -> Self {
        self.home = home.into();
        self.honor_environment_paths = false;
        self.shell_env = None;
        self.path_dirs.clear();
        self
    }

    pub fn with_shell_available(mut self, available: ShellAvailable) -> Self {
        self.shell_available = Some(available);
        self
    }

    /// The directories `LookPath` would search, in order, before the fixed
    /// shell directories.
    pub fn with_path_dirs(mut self, dirs: Vec<PathBuf>) -> Self {
        self.path_dirs = dirs;
        self
    }

    pub fn with_shell_env(mut self, shell: Option<PathBuf>) -> Self {
        self.shell_env = shell;
        self
    }

    pub fn with_node_dist_base(mut self, base: impl Into<String>) -> Self {
        self.node_dist_base = base.into();
        self
    }

    /// Override the uv lookup candidates (tests use a temp home only).
    pub fn with_uv_candidates(mut self, candidates: Vec<PathBuf>) -> Self {
        self.uv_candidates = Some(candidates);
        self
    }

    pub fn with_program(mut self, program: impl Into<PathBuf>) -> Self {
        self.program = program.into();
        self
    }

    pub(crate) fn runner(&self) -> &dyn CommandRunner {
        self.runner.as_ref()
    }

    pub(crate) fn http(&self) -> &dyn HttpTransport {
        self.http.as_ref()
    }

    pub(crate) fn node_dist_base(&self) -> &str {
        &self.node_dist_base
    }

    pub(crate) fn path_dirs(&self) -> &[PathBuf] {
        &self.path_dirs
    }

    pub(crate) fn uv_candidate_override(&self) -> Option<&[PathBuf]> {
        self.uv_candidates.as_deref()
    }

    /// The candidate paths for a shell binary, in Go's order: `LookPath`,
    /// `$SHELL`, `/bin`, `/usr/bin`, `/opt/homebrew/bin`, `/usr/local/bin`.
    fn shell_candidates(&self, name: &str) -> Vec<PathBuf> {
        let mut candidates = Vec::new();
        if let Some(path) = self.look_path(name) {
            candidates.push(path);
        }
        if let Some(shell) = &self.shell_env
            && shell.is_absolute()
            && shell
                .file_name()
                .is_some_and(|file| file == std::ffi::OsStr::new(name))
        {
            candidates.push(shell.clone());
        }
        for dir in ["/bin", "/usr/bin", "/opt/homebrew/bin", "/usr/local/bin"] {
            candidates.push(Path::new(dir).join(name));
        }
        #[cfg(not(target_os = "macos"))]
        candidates.push(Path::new("/home/linuxbrew/.linuxbrew/bin").join(name));
        candidates
    }

    fn look_path(&self, name: &str) -> Option<PathBuf> {
        // `exec.LookPath` only returns a regular, executable file.
        self.path_dirs
            .iter()
            .map(|dir| dir.join(name))
            .find(|path| {
                std::fs::metadata(path).is_ok_and(|metadata| {
                    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
                })
            })
    }

    /// Whether `name` resolves to a regular executable, matching
    /// `runtime_shell.go:75-95`.
    pub fn has_shell(&self, name: &str) -> bool {
        if let Some(available) = &self.shell_available {
            return available(name);
        }
        for candidate in self.shell_candidates(name) {
            if let Ok(metadata) = std::fs::metadata(&candidate)
                && metadata.is_file()
                && metadata.permissions().mode() & 0o111 != 0
            {
                return true;
            }
        }
        false
    }

    fn zsh_startup_files(&self) -> Result<Vec<String>, ShellError> {
        let mut home = self.home.clone();
        if self.honor_environment_paths
            && let Some(dir) = std::env::var_os("ZDOTDIR")
        {
            let dir = PathBuf::from(dir);
            if !dir.is_absolute() {
                return Err(ShellError::invalid("ZDOTDIR must be an absolute path"));
            }
            home = dir;
        }
        Ok(vec![
            home.join(".zshenv").to_string_lossy().into_owned(),
            home.join(".zprofile").to_string_lossy().into_owned(),
            home.join(".zshrc").to_string_lossy().into_owned(),
        ])
    }

    fn config_home(&self) -> Result<PathBuf, ShellError> {
        if self.honor_environment_paths
            && let Some(dir) = std::env::var_os("XDG_CONFIG_HOME")
        {
            let dir = PathBuf::from(dir);
            if !dir.is_absolute() {
                return Err(ShellError::invalid(
                    "XDG_CONFIG_HOME must be an absolute path",
                ));
            }
            return Ok(dir);
        }
        Ok(self.home.join(".config"))
    }

    /// Startup files that receive a block. With `enable`, only installed
    /// shells are considered; without it, legacy locations are cleaned too.
    pub fn shell_startup_files(&self, enable: bool) -> Result<Vec<ShellStartupFile>, ShellError> {
        let mut files = Vec::new();
        if !enable || self.has_shell("zsh") {
            for path in self.zsh_startup_files()? {
                files.push(ShellStartupFile::new(PathBuf::from(path)));
            }
            if !enable {
                for name in [".zshenv", ".zprofile", ".zshrc"] {
                    files.push(ShellStartupFile::new(self.home.join(name)));
                }
            }
        }
        if !enable || self.has_shell("bash") {
            files.push(ShellStartupFile::new(self.home.join(".bashrc")));
            let mut found = false;
            for name in [".bash_profile", ".bash_login", ".profile"] {
                let path = self.home.join(name);
                match std::fs::symlink_metadata(&path) {
                    Ok(_) => {
                        files.push(ShellStartupFile::new(path));
                        found = true;
                        if enable {
                            break;
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
            if enable && !found {
                files.push(ShellStartupFile::new(self.home.join(".bash_profile")));
            }
        }
        if !enable || self.has_shell("fish") {
            let config_home = self.config_home()?;
            files.push(
                ShellStartupFile::new(config_home.join("fish").join("conf.d").join("devbase.fish"))
                    .fish(),
            );
            if !enable {
                files.push(
                    ShellStartupFile::new(
                        config_home
                            .join("fish")
                            .join("conf.d")
                            .join("service-manager.fish"),
                    )
                    .fish(),
                );
                files.push(
                    ShellStartupFile::new(
                        self.home
                            .join(".config")
                            .join("fish")
                            .join("conf.d")
                            .join("devbase.fish"),
                    )
                    .fish(),
                );
                files.push(
                    ShellStartupFile::new(
                        self.home
                            .join(".config")
                            .join("fish")
                            .join("conf.d")
                            .join("service-manager.fish"),
                    )
                    .fish(),
                );
            }
        }
        Ok(files)
    }

    pub fn bash_env_path(&self) -> PathBuf {
        self.store.root().join("shell").join("bash-env.sh")
    }

    pub fn bash_setup_path(&self) -> PathBuf {
        self.store.root().join("shell").join("bash-setup.sh")
    }

    pub fn fish_setup_path(&self) -> PathBuf {
        self.store.root().join("shell").join("bash-setup.fish")
    }

    /// The `BASH_ENV=… /bin/bash -c 'YOUR_COMMAND'` template.
    pub fn bash_command(&self) -> String {
        format!(
            "BASH_ENV={} /bin/bash -c 'YOUR_COMMAND'",
            shell_quote(&self.bash_env_path().to_string_lossy())
        )
    }

    fn write_bash_setup(&self, fish_enabled: bool) -> Result<(), ShellError> {
        let path = shell_quote(&self.bash_env_path().to_string_lossy());
        let setup = format!(
            "# Written by Devbase.\n\
             # Keep dynamic user BASH_ENV expressions intact; never evaluate them here.\n\
             case \"${{BASH_ENV-}}\" in\n  *{dollar}*|*{tick}*) ;;\n  *)\n    if [ \"${{BASH_ENV-}}\" != {path} ]; then\n      export MSM_PREVIOUS_BASH_ENV=\"${{BASH_ENV-}}\"\n    fi\n    export BASH_ENV={path}\n    ;;\nesac\n",
            dollar = shell_quote("$"),
            tick = shell_quote("`"),
        );
        crate::fs::atomic_write(&self.bash_setup_path(), setup.as_bytes(), 0o600)?;
        if !fish_enabled {
            return Ok(());
        }
        let fish = format!(
            "# Written by Devbase.\n\
             if not string match -rq {pattern} -- \"$BASH_ENV\"\n  if not set -q BASH_ENV; or test \"$BASH_ENV\" != {path}\n    set -gx MSM_PREVIOUS_BASH_ENV \"$BASH_ENV\"\n  end\n  set -gx BASH_ENV {path}\nend\n",
            pattern = fish_quote("[$`]"),
            path = fish_quote(&self.bash_env_path().to_string_lossy()),
        );
        crate::fs::atomic_write(&self.fish_setup_path(), fish.as_bytes(), 0o600)?;
        Ok(())
    }

    fn bash_env_header(&self) -> String {
        format!(
            "# Written by Devbase. BASH_ENV is read by non-interactive Bash.\n\
             # Preserve the caller's existing startup file; do not evaluate it as shell code.\n\
             case $- in\n  *i*) ;;\n  *)\n    if [ \"${{__MSM_BASH_ENV_PID-}}\" != \"$$\" ]; then\n      __MSM_BASH_ENV_PID=$$\n      if [ -n \"${{MSM_PREVIOUS_BASH_ENV-}}\" ] && [ \"$MSM_PREVIOUS_BASH_ENV\" != {path} ] && [ -r \"$MSM_PREVIOUS_BASH_ENV\" ]; then\n        . \"$MSM_PREVIOUS_BASH_ENV\"\n      fi\n    fi\n    ;;\nesac\n",
            path = shell_quote(&self.bash_env_path().to_string_lossy()),
        )
    }

    fn runtime_script(&self, name: &str) -> PathBuf {
        self.store
            .root()
            .join("runtimes")
            .join(name)
            .join("init.sh")
    }

    fn node_home(&self) -> PathBuf {
        self.store.root().join("node")
    }

    fn node_init_path(&self) -> PathBuf {
        self.node_home().join("init.sh")
    }

    fn node_wrapper_path(&self) -> PathBuf {
        self.node_home().join("resolve")
    }

    fn node_default_link(&self) -> PathBuf {
        self.node_home().join("default")
    }

    fn node_global_prefix(&self, version: &str) -> PathBuf {
        self.node_home().join("global").join(version)
    }

    /// The shell block editor shared by Node and Java, ported from
    /// `runtime_shell.go:212-343`.
    fn configure_runtime_shell_files(
        &self,
        name: &str,
        begin: &str,
        end: &str,
        enable: bool,
    ) -> Result<String, ShellError> {
        let mut files = self.shell_startup_files(enable)?;
        if enable && files.is_empty() {
            return Err(ShellError::invalid("no supported shell found"));
        }
        let locations_path = self
            .store
            .root()
            .join("shell")
            .join(format!("{name}-locations.json"));
        let locations = read_shell_configuration(&locations_path)?;
        let remembered: Vec<String> = if locations.data.is_empty() {
            Vec::new()
        } else {
            serde_json::from_slice(&locations.data)?
        };
        if !enable {
            for path in &remembered {
                let path = PathBuf::from(path);
                if !path.is_absolute() {
                    return Err(ShellError::invalid(
                        "invalid recorded shell configuration path",
                    ));
                }
                let fish = path
                    .extension()
                    .is_some_and(|extension| extension == "fish");
                let file = if fish {
                    ShellStartupFile::new(path).fish()
                } else {
                    ShellStartupFile::new(path)
                };
                files.push(file);
            }
        }
        files.push(ShellStartupFile::new(self.bash_env_path()));
        let paths: Vec<String> = files
            .iter()
            .map(|file| file.path.to_string_lossy().into_owned())
            .collect();
        let label = paths.join(", ");
        let script = if name == "node" {
            self.node_init_path()
        } else {
            self.runtime_script(name)
        };
        let body = format!(
            "[ ! -r {} ] || . {}\n",
            shell_quote(&script.to_string_lossy()),
            shell_quote(&script.to_string_lossy())
        );
        let bash_block = format!("{begin}\n{body}{end}\n");
        let block = format!(
            "{begin}\nif [ -n \"${{BASH_VERSION-}}${{ZSH_VERSION-}}\" ]; then\n{body}[ ! -r {} ] || . {}\nfi\n{end}\n",
            shell_quote(&self.bash_setup_path().to_string_lossy()),
            shell_quote(&self.bash_setup_path().to_string_lossy())
        );
        let mut fish_script = script.to_string_lossy().into_owned();
        if let Some(stripped) = fish_script.strip_suffix(".sh") {
            fish_script = format!("{stripped}.fish");
        }
        let fish_block = format!(
            "{begin}\nif test -r {script}; and test -r {fish_script}\n  source {fish_script}\nend\nif test -r {setup}\n  source {setup}\nend\n{end}\n",
            script = fish_quote(&script.to_string_lossy()),
            fish_script = fish_quote(&fish_script),
            setup = fish_quote(&self.fish_setup_path().to_string_lossy()),
        );

        struct Change {
            config: ShellConfiguration,
            data: Vec<u8>,
        }
        let mut changes: Vec<Change> = Vec::new();
        let mut seen: Vec<PathBuf> = Vec::new();
        for file in &files {
            let config = read_shell_configuration(&file.path)?;
            if seen.contains(&config.target) {
                continue;
            }
            seen.push(config.target.clone());
            let mut content = config.data.clone();
            let new_block = if file.fish {
                &fish_block
            } else if file.path == self.bash_env_path() {
                if content.is_empty() && enable {
                    content = self.bash_env_header().into_bytes();
                }
                &bash_block
            } else {
                &block
            };
            let data =
                update_shell_block(&content, begin, end, new_block, enable).map_err(|error| {
                    ShellError::invalid(format!(
                        "{}: {error}; review this file",
                        file.path.display()
                    ))
                })?;
            if config.data != data {
                changes.push(Change { config, data });
            }
        }

        if enable {
            let mut unique: Vec<String> = Vec::new();
            let mut saved: Vec<String> = Vec::new();
            for path in remembered.iter().chain(paths.iter()) {
                if !unique.contains(path) {
                    unique.push(path.clone());
                    saved.push(path.clone());
                }
            }
            let data = serde_json::to_vec(&saved)?;
            if locations.data != data {
                changes.push(Change {
                    config: locations,
                    data,
                });
            }
            self.write_bash_setup(self.has_shell("fish"))?;
        }

        for index in 0..changes.len() {
            let error = match changes[index].config.commit(&changes[index].data) {
                Ok(()) => continue,
                Err(error) => error,
            };
            // Roll back earlier writes only while their content and target
            // still match.
            let mut joined = error.to_string();
            for earlier in (0..index).rev() {
                let snapshot = &changes[earlier];
                let rollback = read_shell_configuration(&snapshot.config.rc).and_then(|current| {
                    if current.target != snapshot.config.target || current.data != snapshot.data {
                        return Err(ShellError::invalid("configuration changed during rollback"));
                    }
                    current.commit(&snapshot.data)
                });
                if let Err(error) = rollback {
                    joined.push_str(&format!(
                        "; restore {}: {error}",
                        snapshot.config.rc.display()
                    ));
                }
            }
            return Err(ShellError::invalid(joined));
        }
        Ok(label)
    }

    /// The installed helper executable used by generated wrappers.
    pub fn helper_executable(&self) -> PathBuf {
        self.program.clone()
    }
}

/// One startup file and whether it uses fish syntax.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellStartupFile {
    pub path: PathBuf,
    pub fish: bool,
}

impl ShellStartupFile {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        ShellStartupFile {
            path: path.into(),
            fish: false,
        }
    }

    pub fn fish(mut self) -> Self {
        self.fish = true;
        self
    }
}

/// A startup file's observed identity, so a commit only succeeds while the
/// file still looks exactly as it did when read. Symlinks are resolved and
/// written through; hard links are refused.
#[derive(Debug, Clone)]
struct ShellConfiguration {
    rc: PathBuf,
    target: PathBuf,
    data: Vec<u8>,
    mode: u32,
    entry: Option<std::fs::Metadata>,
    file: Option<std::fs::Metadata>,
}

fn read_shell_configuration(rc: &Path) -> Result<ShellConfiguration, ShellError> {
    use std::os::unix::fs::MetadataExt;
    let mut config = ShellConfiguration {
        rc: rc.to_path_buf(),
        target: rc.to_path_buf(),
        data: Vec::new(),
        mode: 0o644,
        entry: None,
        file: None,
    };
    let entry = match std::fs::symlink_metadata(rc) {
        Ok(entry) => entry,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(config),
        Err(error) => return Err(error.into()),
    };
    config.entry = Some(entry);
    let target = match std::fs::canonicalize(rc) {
        Ok(target) => target,
        Err(error) => {
            return Err(ShellError::invalid(format!(
                "cannot resolve shell configuration; repair the link and retry: {error}"
            )));
        }
    };
    config.target = target.clone();
    let file = std::fs::symlink_metadata(&target)?;
    if !file.is_file() {
        return Err(ShellError::invalid(
            "shell configuration target must be a regular file",
        ));
    }
    if file.nlink() > 1 {
        return Err(ShellError::invalid(
            "hard-linked shell configuration is not supported; use a single file or symbolic link",
        ));
    }
    config.mode = file.mode() & 0o7777;
    config.file = Some(file);
    config.data = std::fs::read(&target)?;
    Ok(config)
}

impl ShellConfiguration {
    fn commit(&self, data: &[u8]) -> Result<(), ShellError> {
        use std::os::unix::fs::MetadataExt;
        let current = read_shell_configuration(&self.rc).map_err(|error| {
            ShellError::invalid(format!(
                "shell configuration changed; review it and retry: {error}"
            ))
        })?;
        let same_entry = match (&self.entry, &current.entry) {
            (None, None) => true,
            (Some(a), Some(b)) => a.ino() == b.ino() && a.dev() == b.dev(),
            _ => false,
        };
        let same_file = match (&self.file, &current.file) {
            (None, None) => true,
            (Some(a), Some(b)) => a.ino() == b.ino() && a.dev() == b.dev(),
            _ => false,
        };
        if self.target != current.target
            || self.mode != current.mode
            || self.data != current.data
            || !same_entry
            || !same_file
        {
            return Err(ShellError::invalid(
                "shell configuration changed; review it and retry",
            ));
        }
        crate::fs::atomic_write(&self.target, data, self.mode)?;
        Ok(())
    }
}

/// Shell-quote for POSIX shells.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Fish-quote a value.
pub fn fish_quote(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// Fixed keys used by the Node block.
pub const NODE_BLOCK_BEGIN: &str = "# >>> Devbase Node.js >>>";
pub const NODE_BLOCK_END: &str = "# <<< Devbase Node.js <<<";

/// The `runtime` block markers for Java.
pub fn runtime_block(name: &str) -> (String, String) {
    (
        format!("# BEGIN Devbase {name}"),
        format!("# END Devbase {name}"),
    )
}

/// Sorted keys, matching Go's `diagnosticKeys`.
pub fn sorted_keys<V>(map: &BTreeMap<String, V>) -> Vec<String> {
    map.keys().cloned().collect()
}

#[cfg(test)]
mod tests;

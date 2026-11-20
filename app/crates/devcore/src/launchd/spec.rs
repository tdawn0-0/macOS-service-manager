//! The job editor's model of a property list, its validation, and saving.
//!
//! The editor only writes the keys it knows; everything else in the file is
//! preserved, including comments-free unknown keys, explicit `false` values,
//! custom `KeepAlive` conditions, custom calendars, and an untouched
//! `ThrottleInterval`.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Write};
use std::path::{Component, Path, PathBuf};

use plist::{Dictionary, Value};

use super::calendar::{Calendar, calendar_from};
use super::error::LaunchdError;
use super::host::ProcessRunner;
use super::plist_file::{FileModel, read_file};

/// The part of a launch agent's property list that the editor changes.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Spec {
    pub label: String,
    /// The job has a file already, whose label and name stay as they are, even
    /// if a new job could not have them.
    #[serde(default)]
    pub existing: bool,
    /// `ProgramArguments`: the program, then its arguments.
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub working_dir: String,
    #[serde(default)]
    pub run_at_load: bool,
    #[serde(default)]
    pub keep_alive: bool,
    /// The file has `KeepAlive` conditions the editor leaves alone;
    /// `keep_alive` is true for them.
    #[serde(default)]
    pub keep_alive_custom: bool,
    /// Starts the job again when it exits with a failure, which also runs it at
    /// load.
    #[serde(default)]
    pub restart_on_crash: bool,
    /// Runs the job every this many seconds; 0 is never.
    #[serde(default)]
    pub start_interval: i32,
    /// `None` preserves a key this editor did not change. `Some` sets it.
    #[serde(default)]
    pub throttle_interval: Option<i32>,
    #[serde(default)]
    pub calendar: Calendar,
    /// The file has a calendar that a [`Calendar`] cannot say, which the
    /// editor leaves alone.
    #[serde(default)]
    pub calendar_custom: bool,
    #[serde(default)]
    pub start_on_mount: bool,
    /// Keeps the job alive while any of these directories has files.
    #[serde(default)]
    pub queue_dirs: Vec<String>,
    /// Who the job runs as, and where it is chrooted to. launchd only applies
    /// them in the system domain.
    #[serde(default)]
    pub user_name: String,
    #[serde(default)]
    pub group_name: String,
    #[serde(default)]
    pub root_dir: String,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub stdout: String,
    #[serde(default)]
    pub stderr: String,
}

/// Reads the editable part of the property list at `path`.
pub fn load_spec(path: &Path) -> Result<Spec, LaunchdError> {
    Spec::load(path)
}

impl Spec {
    /// Reads the editable part of the property list at `path`.
    pub fn load(path: &Path) -> Result<Self, LaunchdError> {
        let file = read_file(path)?;
        Ok(Self::from_file(&file))
    }

    pub(crate) fn from_file(file: &FileModel) -> Self {
        let mut spec = Spec {
            label: file.label.clone(),
            existing: true,
            args: file.program_arguments.clone().unwrap_or_default(),
            env: file.environment.clone(),
            ..Spec::default()
        };
        // launchd runs Program, if there is one, whatever the first of the
        // arguments says: what its process calls itself.
        if let Some(program) = &file.program {
            let skip = usize::min(1, spec.args.len());
            let mut args = vec![program.clone()];
            args.extend_from_slice(&spec.args[skip..]);
            spec.args = args;
        }
        spec.working_dir = file.working_directory.clone().unwrap_or_default();
        spec.run_at_load = file.run_at_load.unwrap_or(false);
        match &file.keep_alive {
            Some(Value::Boolean(on)) => spec.keep_alive = *on,
            None => {}
            Some(Value::Dictionary(conditions)) => {
                let crashed = conditions.len() == 1
                    && matches!(
                        conditions.get("SuccessfulExit"),
                        Some(Value::Boolean(false))
                    );
                if crashed {
                    spec.restart_on_crash = true;
                } else {
                    spec.keep_alive = true;
                    spec.keep_alive_custom = true;
                }
            }
            Some(_) => {
                spec.keep_alive = true;
                spec.keep_alive_custom = true;
            }
        }
        if let Some(seconds) = file.start_interval {
            spec.start_interval =
                i32::try_from(seconds).unwrap_or(if seconds < 0 { i32::MIN } else { i32::MAX });
        }
        if let Some(value) = &file.start_calendar_interval {
            // The form holds one way of being scheduled: with an interval as
            // well, the calendar stays as it is.
            match calendar_from(value) {
                Some(calendar) if file.start_interval.is_none() => spec.calendar = calendar,
                _ => spec.calendar_custom = true,
            }
        }
        spec.start_on_mount = file.start_on_mount.unwrap_or(false);
        spec.queue_dirs = file.queue_directories.clone();
        spec.user_name = file.user_name.clone().unwrap_or_default();
        spec.group_name = file.group_name.clone().unwrap_or_default();
        spec.root_dir = file.root_directory.clone().unwrap_or_default();
        spec.stdout = file.standard_out_path.clone().unwrap_or_default();
        spec.stderr = file.standard_error_path.clone().unwrap_or_default();
        spec
    }

    /// Checks that the spec makes a launch agent launchd accepts.
    pub fn validate(&self) -> Result<(), LaunchdError> {
        if !valid_label(&self.label) && (!self.existing || self.label.is_empty()) {
            return Err(LaunchdError::validation(
                "the label may hold letters, digits, dots, dashes and underscores, as com.example.job",
            ));
        }
        if self.args.is_empty() {
            return Err(LaunchdError::validation("give the program to run"));
        }
        // launchd looks in its PATH for a program without a path, which a file
        // that works may rely on. What this form writes says where it is.
        if !Path::new(&self.args[0]).is_absolute() && !self.existing {
            return Err(LaunchdError::validation(format!(
                "the program {:?} must be an absolute path",
                self.args[0]
            )));
        }
        if !self.group_name.is_empty() && self.user_name.is_empty() {
            return Err(LaunchdError::validation("a group needs a user to run as"));
        }
        for (name, path) in [
            ("working directory", &self.working_dir),
            ("standard output", &self.stdout),
            ("standard error", &self.stderr),
            ("root directory", &self.root_dir),
        ] {
            if !path.is_empty() && !Path::new(path).is_absolute() {
                return Err(LaunchdError::validation(format!(
                    "the {name} {path:?} must be an absolute path"
                )));
            }
        }
        for dir in &self.queue_dirs {
            if !Path::new(dir).is_absolute() {
                return Err(LaunchdError::validation(format!(
                    "the queue directory {dir:?} must be an absolute path"
                )));
            }
        }
        if self.start_interval < 0 {
            return Err(LaunchdError::validation("the interval cannot be negative"));
        }
        if let Some(throttle) = self.throttle_interval
            && !(1..=3600).contains(&throttle)
        {
            return Err(LaunchdError::validation(
                "the throttle interval must be 1–3600 seconds",
            ));
        }
        self.calendar.entries()?;
        for key in self.env.keys() {
            if key.is_empty() || key.contains('=') {
                return Err(LaunchdError::validation(format!(
                    "the environment variable name {key:?} is not valid"
                )));
            }
        }
        Ok(())
    }

    /// Writes the spec into a property list. Given the current file, keeps the
    /// keys the spec does not hold.
    pub fn marshal(&self, current: Option<&[u8]>) -> Result<Vec<u8>, LaunchdError> {
        let mut dict = current_dictionary(current)?;

        dict.insert("Label".to_string(), Value::String(self.label.clone()));
        if let Some(throttle) = self.throttle_interval {
            dict.insert(
                "ThrottleInterval".to_string(),
                Value::Integer(throttle.into()),
            );
        }
        set_program(&mut dict, &self.args);
        set_bool(
            &mut dict,
            "RunAtLoad",
            self.run_at_load || self.restart_on_crash,
        );
        if self.keep_alive_custom {
            // The editor leaves the file's conditions alone.
        } else if self.restart_on_crash && !self.keep_alive {
            let mut conditions = Dictionary::new();
            conditions.insert("SuccessfulExit".to_string(), Value::Boolean(false));
            dict.insert("KeepAlive".to_string(), Value::Dictionary(conditions));
        } else {
            set_bool(&mut dict, "KeepAlive", self.keep_alive);
        }
        if !self.calendar_custom {
            let entries = self.calendar.entries()?;
            match entries.len() {
                0 => {
                    dict.remove("StartCalendarInterval");
                }
                1 => {
                    dict.insert(
                        "StartCalendarInterval".to_string(),
                        Value::Dictionary(entry_dictionary(&entries[0])),
                    );
                }
                _ => {
                    let values = entries
                        .iter()
                        .map(|entry| Value::Dictionary(entry_dictionary(entry)))
                        .collect();
                    dict.insert("StartCalendarInterval".to_string(), Value::Array(values));
                }
            }
        }
        set_bool(&mut dict, "StartOnMount", self.start_on_mount);
        set_string_array(&mut dict, "QueueDirectories", &self.queue_dirs);
        set_string(&mut dict, "UserName", &self.user_name);
        set_string(&mut dict, "GroupName", &self.group_name);
        set_string(&mut dict, "RootDirectory", &self.root_dir);
        set_string(&mut dict, "WorkingDirectory", &self.working_dir);
        if self.start_interval > 0 {
            dict.insert(
                "StartInterval".to_string(),
                Value::Integer(self.start_interval.into()),
            );
        } else {
            dict.remove("StartInterval");
        }
        if self.env.is_empty() {
            dict.remove("EnvironmentVariables");
        } else {
            let mut env = Dictionary::new();
            for (key, value) in &self.env {
                env.insert(key.clone(), Value::String(value.clone()));
            }
            dict.insert("EnvironmentVariables".to_string(), Value::Dictionary(env));
        }
        set_string(&mut dict, "StandardOutPath", &self.stdout);
        set_string(&mut dict, "StandardErrorPath", &self.stderr);

        let mut out = Vec::new();
        Value::Dictionary(dict)
            .to_writer_xml(&mut out)
            .map_err(|error| LaunchdError::plist(error.to_string()))?;
        Ok(out)
    }

    /// The keys of the property list that saving the spec changes.
    pub fn changes(&self, current: &[u8]) -> Result<Vec<String>, LaunchdError> {
        let data = self.marshal(Some(current))?;
        let before = parse_dictionary(current)?;
        let after = parse_dictionary(&data)?;
        let mut keys: BTreeSet<String> = before.keys().cloned().collect();
        keys.extend(after.keys().cloned());
        let mut changes = Vec::new();
        for key in keys {
            match (before.get(&key), after.get(&key)) {
                (Some(was), Some(is)) if was == is => {}
                _ => changes.push(key),
            }
        }
        Ok(changes)
    }
}

fn entry_dictionary(entry: &BTreeMap<String, i32>) -> Dictionary {
    let mut dict = Dictionary::new();
    for (key, value) in entry {
        dict.insert(key.clone(), Value::Integer((*value).into()));
    }
    dict
}

/// Whether a label could name a new job.
pub(crate) fn valid_label(label: &str) -> bool {
    let mut chars = label.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn current_dictionary(current: Option<&[u8]>) -> Result<Dictionary, LaunchdError> {
    match current {
        None => Ok(Dictionary::new()),
        Some(bytes) => parse_dictionary(bytes).map_err(|error| {
            LaunchdError::plist(format!(
                "the current file is not a property list: {}",
                error.message
            ))
        }),
    }
}

fn parse_dictionary(data: &[u8]) -> Result<Dictionary, LaunchdError> {
    let value = Value::from_reader(Cursor::new(data))
        .map_err(|error| LaunchdError::plist(error.to_string()))?;
    match value {
        Value::Dictionary(dict) => Ok(dict),
        _ => Err(LaunchdError::plist(
            "the top level is not a dictionary".to_string(),
        )),
    }
}

fn set_bool(dict: &mut Dictionary, key: &str, on: bool) {
    // False is what launchd takes a missing key for: a file only says so if it
    // did already.
    if on || dict.contains_key(key) {
        dict.insert(key.to_string(), Value::Boolean(on));
    }
}

fn set_string(dict: &mut Dictionary, key: &str, value: &str) {
    if value.is_empty() {
        dict.remove(key);
    } else {
        dict.insert(key.to_string(), Value::String(value.to_string()));
    }
}

fn set_string_array(dict: &mut Dictionary, key: &str, values: &[String]) {
    if values.is_empty() {
        dict.remove(key);
    } else {
        dict.insert(
            key.to_string(),
            Value::Array(values.iter().cloned().map(Value::String).collect()),
        );
    }
}

/// Writes the program and its arguments. A file with a Program runs it whatever
/// ProgramArguments begins with, so it has to change with the program, and what
/// it says already stays: the first argument can differ from the path, and a
/// Program alone says as much as both keys would.
fn set_program(dict: &mut Dictionary, args: &[String]) {
    let program = dict
        .get("Program")
        .and_then(Value::as_string)
        .map(str::to_string);
    let Some(program) = program else {
        dict.insert(
            "ProgramArguments".to_string(),
            Value::Array(args.iter().cloned().map(Value::String).collect()),
        );
        return;
    };
    if args.is_empty() {
        dict.insert("ProgramArguments".to_string(), Value::Array(Vec::new()));
        return;
    }
    let current: Vec<Value> = dict
        .get("ProgramArguments")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if current.is_empty() && args.len() == 1 && args[0] == program {
        dict.remove("ProgramArguments");
        return;
    }
    let mut written = args.to_vec();
    if args[0] == program
        && !current.is_empty()
        && let Some(first) = current[0].as_string()
    {
        written[0] = first.to_string();
    }
    dict.insert("Program".to_string(), Value::String(args[0].clone()));
    dict.insert(
        "ProgramArguments".to_string(),
        Value::Array(written.into_iter().map(Value::String).collect()),
    );
}

/// Writes the spec as a launch agent in `dir` and returns its path. To edit the
/// file at `existing`, pass its path; for a new one, `None`, which fails if the
/// label is taken.
pub fn save_file(
    dir: &Path,
    spec: &Spec,
    existing: Option<&Path>,
    runner: &dyn ProcessRunner,
) -> Result<PathBuf, LaunchdError> {
    let mut spec = spec.clone();
    let existing = existing.filter(|path| !path.as_os_str().is_empty());
    spec.existing = existing.is_some();
    spec.validate()?;
    let (path, current) = match existing {
        None => {
            let path = dir.join(format!("{}.plist", spec.label));
            if std::fs::metadata(&path).is_ok() {
                return Err(LaunchdError::validation(format!(
                    "{} already exists",
                    file_name(&path)
                )));
            }
            (path, None)
        }
        Some(path) => {
            let data = std::fs::read(path).map_err(|error| LaunchdError::io(error.to_string()))?;
            (path.to_path_buf(), Some(data))
        }
    };
    let data = spec.marshal(current.as_deref())?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|error| LaunchdError::io(error.to_string()))?;
    let tmp = write_checked(parent, &data, runner)?;
    match std::fs::rename(&tmp, &path) {
        Ok(()) => Ok(path),
        Err(error) => {
            let _ = std::fs::remove_file(&tmp);
            Err(LaunchdError::io(error.to_string()))
        }
    }
}

/// Writes a property list to a new file in `dir`, for all to read, if `plutil`
/// finds it valid, and returns its path.
pub(crate) fn write_checked(
    dir: &Path,
    data: &[u8],
    runner: &dyn ProcessRunner,
) -> Result<PathBuf, LaunchdError> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let mut file = None;
    let mut path = PathBuf::new();
    for _ in 0..100 {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        path = dir.join(format!(".msm-{}-{n}.plist", std::process::id()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(created) => {
                file = Some(created);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(LaunchdError::io(error.to_string())),
        }
    }
    let mut file = file.ok_or_else(|| LaunchdError::io("could not create a temporary file"))?;
    if let Err(error) = file.write_all(data) {
        let _ = std::fs::remove_file(&path);
        return Err(LaunchdError::io(error.to_string()));
    }
    drop(file);
    let display = path.to_string_lossy().into_owned();
    if let Err(error) = runner.run("plutil", &["-lint", &display]) {
        let _ = std::fs::remove_file(&path);
        return Err(LaunchdError::validation(format!(
            "the property list is not valid: {}",
            error.message()
        )));
    }
    if let Err(error) = set_mode(&path, 0o644) {
        let _ = std::fs::remove_file(&path);
        return Err(error);
    }
    Ok(path)
}

fn set_mode(path: &Path, mode: u32) -> Result<(), LaunchdError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = std::fs::Permissions::from_mode(mode);
        std::fs::set_permissions(path, permissions)
            .map_err(|error| LaunchdError::io(error.to_string()))?;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
    Ok(())
}

/// Confines a root edit or removal to a direct `.plist` child of the managed
/// directory, rejecting symlinks.
pub(crate) fn check_managed_path(dir: &Path, path: &Path) -> Result<PathBuf, LaunchdError> {
    if dir.as_os_str().is_empty() || !dir.is_absolute() || !path.is_absolute() {
        return Err(LaunchdError::validation(
            "path is not within managed directory",
        ));
    }
    let clean = clean_path(path);
    let clean_dir = clean_path(dir);
    let base = file_name(&clean);
    if clean.parent() != Some(clean_dir.as_path()) || !base.ends_with(".plist") || base == ".plist"
    {
        return Err(LaunchdError::validation(
            "path is not within managed directory",
        ));
    }
    let dir_info = std::fs::symlink_metadata(&clean_dir)
        .map_err(|error| LaunchdError::io(error.to_string()))?;
    if !dir_info.is_dir() || dir_info.file_type().is_symlink() {
        return Err(LaunchdError::validation(
            "managed directory is not a directory or is a symlink",
        ));
    }
    let info =
        std::fs::symlink_metadata(&clean).map_err(|error| LaunchdError::io(error.to_string()))?;
    if !info.is_file() {
        return Err(LaunchdError::validation(format!(
            "{base} is not a regular file"
        )));
    }
    Ok(clean)
}

/// `filepath.Clean` for the paths this module handles.
pub(crate) fn clean_path(path: &Path) -> PathBuf {
    let absolute = path.is_absolute();
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::CurDir => {}
            Component::RootDir => out.push(std::path::MAIN_SEPARATOR.to_string()),
            Component::ParentDir => {
                if !out.pop() && !absolute {
                    out.push("..");
                }
            }
            Component::Normal(part) => out.push(part),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(if absolute { "/" } else { "." });
    }
    out
}

pub(crate) fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The arguments that run a command line with the shell.
pub fn shell_args(command: &str) -> Vec<String> {
    vec!["/bin/sh".to_string(), "-c".to_string(), command.to_string()]
}

/// The command line of arguments that [`shell_args`] could have made.
pub fn shell_command(args: &[String]) -> Option<&str> {
    if args.len() == 3 && (args[0] == "/bin/sh" || args[0] == "sh") && args[1] == "-c" {
        Some(&args[2])
    } else {
        None
    }
}

/// Splits text at its commas into its trimmed, non-empty items.
pub fn parse_list(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

/// Splits text into its trimmed, non-empty lines.
pub fn parse_lines(text: &str) -> Vec<String> {
    text.split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// Reads lines of `KEY=value`.
pub fn parse_env(text: &str) -> Result<BTreeMap<String, String>, LaunchdError> {
    let mut env = BTreeMap::new();
    for line in parse_lines(text) {
        let Some((key, value)) = line.split_once('=') else {
            return Err(LaunchdError::validation(format!(
                "{line:?} is not KEY=value"
            )));
        };
        let key = key.trim();
        if key.is_empty() {
            return Err(LaunchdError::validation(format!(
                "{line:?} is not KEY=value"
            )));
        }
        env.insert(key.to_string(), value.trim().to_string());
    }
    Ok(env)
}

/// Writes env as lines of `KEY=value`, sorted.
pub fn format_env(env: &BTreeMap<String, String>) -> String {
    env.iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

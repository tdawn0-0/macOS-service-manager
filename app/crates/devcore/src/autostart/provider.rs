//! XDG Autostart provider (`~/.config/autostart` and `/etc/xdg/autostart`).

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::fs::atomic_write;
use crate::launchd::Host;
use crate::service::{self, Action, Autostart, Detail, Item, Row, Source, State};

use super::desktop::{
    AutostartError, DesktopEntry, parse_desktop, update_desktop_enabled, validate_desktop_filename,
};

/// File mode for user autostart `.desktop` entries.
const DESKTOP_FILE_MODE: u32 = 0o644;

/// Configuration for [`AutostartProvider`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutostartConfig {
    /// User autostart directory (typically `$XDG_CONFIG_HOME/autostart`).
    pub user_dir: PathBuf,
    /// System-wide autostart directory (typically `/etc/xdg/autostart`).
    pub system_dir: PathBuf,
}

/// Manages Freedesktop `.desktop` autostart entries.
pub struct AutostartProvider {
    config: AutostartConfig,
    host: Arc<dyn Host>,
}

impl AutostartProvider {
    /// Creates a provider using the current user's XDG autostart directory and
    /// `/etc/xdg/autostart`.
    pub fn system_default(host: Arc<dyn Host>) -> Self {
        let home = host.home_dir().unwrap_or_default();
        let user_dir = crate::platform::linux::paths::autostart_directory(&home);
        AutostartProvider::new(
            AutostartConfig {
                user_dir,
                system_dir: PathBuf::from("/etc/xdg/autostart"),
            },
            host,
        )
    }

    pub fn new(config: AutostartConfig, host: Arc<dyn Host>) -> Self {
        AutostartProvider { config, host }
    }

    pub fn config(&self) -> &AutostartConfig {
        &self.config
    }

    /// Creates or replaces a user `.desktop` entry in `user_dir`.
    pub fn add_entry(
        &self,
        filename: &str,
        entry: &DesktopEntry,
    ) -> Result<PathBuf, AutostartError> {
        validate_desktop_filename(filename)?;
        entry.validate()?;
        std::fs::create_dir_all(&self.config.user_dir)?;
        let path = self.config.user_dir.join(filename);
        atomic_write(&path, entry.render().as_bytes(), DESKTOP_FILE_MODE)?;
        Ok(path)
    }

    /// Lists all autostart `.desktop` entries, with `user_dir` overriding
    /// identically named files in `system_dir`.
    pub fn list_entries(&self) -> Result<Vec<Item>, AutostartError> {
        let mut by_name: BTreeMap<String, (PathBuf, bool)> = BTreeMap::new();

        // System entries first; user entries with the same filename win.
        scan_desktop_dir(&self.config.system_dir, false, &mut by_name);
        scan_desktop_dir(&self.config.user_dir, true, &mut by_name);

        let mut items = Vec::with_capacity(by_name.len());
        for (filename, (path, is_user)) in by_name {
            items.push(self.build_item(&filename, &path, is_user));
        }
        items.sort_by_key(|a| a.name.to_lowercase());
        Ok(items)
    }

    fn build_item(&self, filename: &str, path: &Path, is_user: bool) -> Item {
        let owner = if is_user { "user" } else { "system" }.to_string();
        let stem = filename.strip_suffix(".desktop").unwrap_or(filename);
        let id = format!("{}:{}", Source::Autostart.as_str(), path.display());
        match std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| parse_desktop(&raw).ok())
        {
            Some(entry) => {
                let name = if entry.name.trim().is_empty() {
                    stem.to_string()
                } else {
                    entry.name.clone()
                };
                Item {
                    id,
                    source: Source::Autostart,
                    name,
                    state: State::None,
                    pid: 0,
                    autostart: if entry.is_enabled() {
                        Autostart::On
                    } else {
                        Autostart::Off
                    },
                    owner,
                    path: path.to_path_buf(),
                    kind: Some(if is_user {
                        "User Autostart".to_string()
                    } else {
                        "System Autostart".to_string()
                    }),
                    admin: false,
                    keep_alive: false,
                }
            }
            None => Item {
                id,
                source: Source::Autostart,
                name: stem.to_string(),
                state: State::Invalid,
                pid: 0,
                autostart: Autostart::Unknown,
                owner,
                path: path.to_path_buf(),
                kind: Some("Invalid desktop entry".to_string()),
                admin: false,
                keep_alive: false,
            },
        }
    }

    fn is_user_path(&self, path: &Path) -> bool {
        path.parent() == Some(self.config.user_dir.as_path())
    }

    fn detail_entry(&self, item: &Item) -> Result<Detail, AutostartError> {
        let raw = std::fs::read_to_string(&item.path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                AutostartError::not_found(format!(
                    "desktop entry not found: {}",
                    item.path.display()
                ))
            } else {
                AutostartError::from(error)
            }
        })?;
        let is_user = self.is_user_path(&item.path);
        match parse_desktop(&raw) {
            Ok(entry) => {
                let mut rows = vec![
                    Row {
                        group: "Status".into(),
                        key: "Autostart".into(),
                        value: if entry.is_enabled() {
                            "Enabled".into()
                        } else {
                            "Disabled".into()
                        },
                    },
                    Row {
                        group: "Status".into(),
                        key: "Scope".into(),
                        value: if is_user {
                            "User".into()
                        } else {
                            "System".into()
                        },
                    },
                ];
                if !entry.exec.is_empty() {
                    rows.push(Row {
                        group: "Command".into(),
                        key: "Exec".into(),
                        value: entry.exec,
                    });
                }
                if !entry.try_exec.is_empty() {
                    rows.push(Row {
                        group: "Command".into(),
                        key: "TryExec".into(),
                        value: entry.try_exec,
                    });
                }
                if !entry.comment.is_empty() {
                    rows.push(Row {
                        group: "Details".into(),
                        key: "Comment".into(),
                        value: entry.comment,
                    });
                }
                if !entry.icon.is_empty() {
                    rows.push(Row {
                        group: "Details".into(),
                        key: "Icon".into(),
                        value: entry.icon,
                    });
                }
                rows.push(Row {
                    group: "Files".into(),
                    key: "Desktop entry".into(),
                    value: item.path.display().to_string(),
                });
                Ok(Detail {
                    rows,
                    logs: Vec::new(),
                })
            }
            Err(error) => Ok(Detail {
                rows: vec![
                    Row {
                        group: "Status".into(),
                        key: "Problem".into(),
                        value: error.to_string(),
                    },
                    Row {
                        group: "Files".into(),
                        key: "Desktop entry".into(),
                        value: item.path.display().to_string(),
                    },
                ],
                logs: Vec::new(),
            }),
        }
    }

    fn set_enabled(&self, item: &Item, enabled: bool) -> Result<(), AutostartError> {
        let filename = item
            .path
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or_else(|| AutostartError::invalid("desktop entry has no valid filename"))?;
        validate_desktop_filename(filename)?;
        let raw = std::fs::read_to_string(&item.path)?;
        let updated = update_desktop_enabled(&raw, enabled);
        std::fs::create_dir_all(&self.config.user_dir)?;
        let target = self.config.user_dir.join(filename);
        atomic_write(&target, updated.as_bytes(), DESKTOP_FILE_MODE)?;
        Ok(())
    }

    fn remove_entry(&self, item: &Item) -> Result<(), AutostartError> {
        if !self.is_user_path(&item.path) {
            return Err(AutostartError::invalid(
                "system autostart entries cannot be removed; disable them instead",
            ));
        }
        self.host
            .trash(&item.path)
            .map_err(|error| AutostartError::io(error.to_string()))
    }
}

fn scan_desktop_dir(dir: &Path, is_user: bool, out: &mut BTreeMap<String, (PathBuf, bool)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension() != Some(OsStr::new("desktop")) {
            continue;
        }
        let Some(name) = path.file_name().and_then(OsStr::to_str) else {
            continue;
        };
        if validate_desktop_filename(name).is_err() {
            continue;
        }
        out.insert(name.to_string(), (path, is_user));
    }
}

impl service::Provider for AutostartProvider {
    fn source(&self) -> Source {
        Source::Autostart
    }

    fn list(&self) -> service::Result<Vec<Item>> {
        self.list_entries().map_err(Into::into)
    }

    fn detail(&self, item: &Item) -> service::Result<Detail> {
        self.detail_entry(item).map_err(Into::into)
    }

    fn actions(&self, item: &Item) -> Vec<Action> {
        let mut actions = Vec::new();
        if item.state != State::Invalid {
            match item.autostart {
                Autostart::On => actions.push(Action::Disable),
                Autostart::Off | Autostart::Unknown => actions.push(Action::Enable),
            }
        }
        if self.is_user_path(&item.path) {
            actions.push(Action::Remove);
        }
        actions
    }

    fn confirm(&self, item: &Item, action: Action) -> Option<String> {
        match action {
            Action::Remove => Some(format!(
                "Move the autostart entry for {} to Trash?",
                item.name
            )),
            _ => None,
        }
    }

    fn do_action(&self, item: &Item, action: Action) -> service::Result<()> {
        match action {
            Action::Enable => self.set_enabled(item, true).map_err(Into::into),
            Action::Disable => self.set_enabled(item, false).map_err(Into::into),
            Action::Remove => self.remove_entry(item).map_err(Into::into),
            other => Err(Box::new(AutostartError::invalid(format!(
                "unsupported autostart action: {other}"
            )))),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tempfile::TempDir;

    use super::*;
    use crate::launchd::LaunchdError;
    use crate::service::Provider;

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

    fn setup() -> (TempDir, AutostartProvider, Arc<FakeHost>) {
        let temp = TempDir::new().unwrap();
        let user_dir = temp.path().join("user-autostart");
        let system_dir = temp.path().join("system-autostart");
        std::fs::create_dir_all(&user_dir).unwrap();
        std::fs::create_dir_all(&system_dir).unwrap();
        let host = Arc::new(FakeHost {
            home: temp.path().to_path_buf(),
            trashed: Mutex::new(Vec::new()),
        });
        let provider = AutostartProvider::new(
            AutostartConfig {
                user_dir,
                system_dir,
            },
            host.clone(),
        );
        (temp, provider, host)
    }

    #[test]
    fn lists_user_and_system_entries_with_user_override() {
        let (_temp, provider, _host) = setup();
        std::fs::write(
            provider.config().system_dir.join("tracker.desktop"),
            "[Desktop Entry]\nType=Application\nName=Tracker\nExec=/usr/bin/tracker\n",
        )
        .unwrap();
        std::fs::write(
            provider.config().system_dir.join("shared.desktop"),
            "[Desktop Entry]\nType=Application\nName=Shared\nExec=/usr/bin/shared\n",
        )
        .unwrap();
        std::fs::write(
            provider.config().user_dir.join("shared.desktop"),
            "[Desktop Entry]\nType=Application\nName=Shared\nExec=/usr/bin/shared\nHidden=true\n",
        )
        .unwrap();

        let items = provider.list().unwrap();
        assert_eq!(items.len(), 2);
        let shared = items.iter().find(|item| item.name == "Shared").unwrap();
        assert_eq!(shared.autostart, Autostart::Off);
        assert_eq!(shared.owner, "user");
        let tracker = items.iter().find(|item| item.name == "Tracker").unwrap();
        assert_eq!(tracker.autostart, Autostart::On);
        assert_eq!(tracker.owner, "system");
        // System entries cannot be removed, only disabled.
        assert_eq!(provider.actions(tracker), vec![Action::Disable]);
        // User entries can be enabled and removed.
        assert_eq!(
            provider.actions(shared),
            vec![Action::Enable, Action::Remove]
        );
    }

    #[test]
    fn disabling_system_entry_creates_user_override_without_mutating_system() {
        let (_temp, provider, _host) = setup();
        let sys_path = provider.config().system_dir.join("daemon.desktop");
        let original = "[Desktop Entry]\nType=Application\nName=Daemon\nExec=/usr/bin/daemon\n";
        std::fs::write(&sys_path, original).unwrap();

        let items = provider.list().unwrap();
        assert_eq!(items.len(), 1);
        provider.do_action(&items[0], Action::Disable).unwrap();

        // System file untouched.
        assert_eq!(std::fs::read_to_string(&sys_path).unwrap(), original);
        // User override created and disabled.
        let user_override = provider.config().user_dir.join("daemon.desktop");
        assert!(user_override.exists());
        let after = provider.list().unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].autostart, Autostart::Off);
        assert_eq!(after[0].path, user_override);
    }

    #[test]
    fn add_toggle_detail_and_remove_user_entry() {
        let (_temp, provider, host) = setup();
        let entry = DesktopEntry {
            name: "Devbase".into(),
            exec: "/usr/bin/devbase --background".into(),
            comment: "Start Devbase quietly".into(),
            ..DesktopEntry::default()
        };
        let path = provider
            .add_entry("xyz.jyeu.devbase.desktop", &entry)
            .unwrap();
        assert!(path.exists());

        let items = provider.list().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].autostart, Autostart::On);

        let detail = provider.detail(&items[0]).unwrap();
        assert!(
            detail
                .rows
                .iter()
                .any(|r| r.key == "Exec" && r.value == "/usr/bin/devbase --background")
        );

        provider.do_action(&items[0], Action::Disable).unwrap();
        let items = provider.list().unwrap();
        assert_eq!(items[0].autostart, Autostart::Off);

        provider.do_action(&items[0], Action::Enable).unwrap();
        let items = provider.list().unwrap();
        assert_eq!(items[0].autostart, Autostart::On);

        provider.do_action(&items[0], Action::Remove).unwrap();
        assert!(!path.exists());
        assert_eq!(host.trashed.lock().unwrap().as_slice(), &[path]);
    }
}

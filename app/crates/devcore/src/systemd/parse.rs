//! Parsers for `systemctl` tabular (`list-units`, `list-unit-files`) and
//! key-value (`show`) outputs.

use std::collections::BTreeMap;

use crate::health::JobState;
use crate::service::{Autostart, State};

/// One row from `systemctl list-units --type=service --all --no-legend --no-pager --plain`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnitListEntry {
    pub unit: String,
    pub load: String,
    pub active: String,
    pub sub: String,
    pub description: String,
}

/// Parsed properties from `systemctl show <unit> --no-pager -p ...`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnitShow {
    pub id: String,
    pub load_state: String,
    pub active_state: String,
    pub sub_state: String,
    pub main_pid: i32,
    pub exec_main_code: i32,
    pub exec_main_status: i32,
    pub unit_file_state: String,
    pub fragment_path: String,
    pub description: String,
    pub restart: String,
    pub user: String,
    pub group: String,
    pub n_restarts: i32,
    pub result: String,
}

/// Parses `systemctl [--user] list-unit-files --type=service --no-legend --no-pager --plain`.
///
/// Returns a map from unit filename (e.g. `"foo.service"`) to unit file state
/// (e.g. `"enabled"`, `"disabled"`, `"static"`, `"masked"`).
pub fn parse_list_unit_files(output: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut parts = trimmed.split_whitespace();
        let Some(unit) = parts.next() else {
            continue;
        };
        let Some(state) = parts.next() else {
            continue;
        };
        if unit.ends_with(".service") || unit.ends_with(".timer") {
            map.insert(unit.to_string(), state.to_string());
        }
    }
    map
}

/// Parses `systemctl [--user] list-units --type=service --all --no-legend --no-pager --plain`.
pub fn parse_list_units(output: &str) -> BTreeMap<String, UnitListEntry> {
    let mut map = BTreeMap::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut parts = trimmed.split_whitespace();
        let Some(unit) = parts.next() else {
            continue;
        };
        let Some(load) = parts.next() else {
            continue;
        };
        let Some(active) = parts.next() else {
            continue;
        };
        let Some(sub) = parts.next() else {
            continue;
        };
        let description = parts.collect::<Vec<_>>().join(" ");
        map.insert(
            unit.to_string(),
            UnitListEntry {
                unit: unit.to_string(),
                load: load.to_string(),
                active: active.to_string(),
                sub: sub.to_string(),
                description,
            },
        );
    }
    map
}

/// Parses `systemctl [--user] show --no-pager -p ... <unit>` key=value output.
pub fn parse_show(output: &str) -> UnitShow {
    let mut show = UnitShow::default();
    for line in output.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "Id" => show.id = value.to_string(),
            "LoadState" => show.load_state = value.to_string(),
            "ActiveState" => show.active_state = value.to_string(),
            "SubState" => show.sub_state = value.to_string(),
            "MainPID" => show.main_pid = value.trim().parse().unwrap_or(0),
            "ExecMainCode" => show.exec_main_code = parse_exec_code(value),
            "ExecMainStatus" => show.exec_main_status = value.trim().parse().unwrap_or(0),
            "UnitFileState" => show.unit_file_state = value.to_string(),
            "FragmentPath" => show.fragment_path = value.to_string(),
            "Description" => show.description = value.to_string(),
            "Restart" => show.restart = value.to_string(),
            "User" => show.user = value.to_string(),
            "Group" => show.group = value.to_string(),
            "NRestarts" => show.n_restarts = value.trim().parse().unwrap_or(0),
            "Result" => show.result = value.to_string(),
            _ => {}
        }
    }
    show
}

/// Converts a `systemctl show` output into the engine's [`JobState`].
///
/// Signal / exit codes follow POSIX `siginfo_t` `si_code` values systemd emits
/// in `ExecMainCode`: `1` (`CLD_EXITED`), `2` (`CLD_KILLED`), `3` (`CLD_DUMPED`).
pub fn parse_show_job_state(output: &str) -> JobState {
    let show = parse_show(output);
    show_to_job_state(&show)
}

/// Converts a parsed [`UnitShow`] into [`JobState`].
pub fn show_to_job_state(show: &UnitShow) -> JobState {
    let running = matches!(
        show.active_state.as_str(),
        "active" | "activating" | "reloading"
    ) && show.sub_state != "dead";
    let mut state = JobState {
        running,
        pid: if running { show.main_pid } else { 0 },
        runs: if show.n_restarts > 0 {
            show.n_restarts + 1
        } else if running || show.exec_main_code != 0 || show.active_state == "failed" {
            1
        } else {
            0
        },
        last_exit_code: None,
        last_signal: String::new(),
        exit_reason: false,
    };

    if !running {
        match show.exec_main_code {
            // CLD_EXITED = 1
            1 => {
                state.last_exit_code = Some(show.exec_main_status);
            }
            // CLD_KILLED = 2, CLD_DUMPED = 3
            2 | 3 if show.exec_main_status > 0 => {
                state.last_signal = show.exec_main_status.to_string();
            }
            _ => {
                if show.active_state == "failed" {
                    if show.exec_main_status > 0 {
                        state.last_exit_code = Some(show.exec_main_status);
                    } else {
                        state.exit_reason = true;
                    }
                }
            }
        }
    }
    state
}

/// Maps systemd `LoadState`/`ActiveState`/`SubState` to Devbase [`State`].
pub fn map_unit_state(load: &str, active: &str, sub: &str, has_timer: bool) -> State {
    match load {
        "error" | "bad-setting" => return State::Invalid,
        _ => {}
    }
    match active {
        "active" => {
            if sub == "exited" && has_timer {
                State::Scheduled
            } else {
                State::Running
            }
        }
        "activating" | "reloading" => State::Running,
        "failed" => State::Failed,
        "inactive" | "deactivating" => {
            if has_timer {
                State::Scheduled
            } else {
                State::Stopped
            }
        }
        _ => State::Stopped,
    }
}

/// Maps systemd `UnitFileState` to Devbase [`Autostart`].
pub fn map_autostart(unit_file_state: &str) -> Autostart {
    match unit_file_state.trim() {
        "enabled" | "enabled-runtime" => Autostart::On,
        "disabled" | "masked" | "masked-runtime" => Autostart::Off,
        _ => Autostart::Unknown,
    }
}

fn parse_exec_code(value: &str) -> i32 {
    match value.trim() {
        "exited" => 1,
        "killed" => 2,
        "dumped" => 3,
        other => other.parse().unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_list_unit_files_and_list_units() {
        let files = parse_list_unit_files(
            "devbase-api.service enabled enabled\nfoo.service disabled disabled\nbar.socket static -\n",
        );
        assert_eq!(
            files.get("devbase-api.service").map(String::as_str),
            Some("enabled")
        );
        assert_eq!(
            files.get("foo.service").map(String::as_str),
            Some("disabled")
        );
        assert!(!files.contains_key("bar.socket"));

        let units = parse_list_units(
            "devbase-api.service loaded active running Devbase API server\nfoo.service loaded failed failed Broken worker\n",
        );
        let api = &units["devbase-api.service"];
        assert_eq!(api.active, "active");
        assert_eq!(api.sub, "running");
        assert_eq!(api.description, "Devbase API server");
        assert_eq!(
            map_unit_state(&api.load, &api.active, &api.sub, false),
            State::Running
        );
        let foo = &units["foo.service"];
        assert_eq!(
            map_unit_state(&foo.load, &foo.active, &foo.sub, false),
            State::Failed
        );
    }

    #[test]
    fn parses_show_output_into_job_state() {
        let running = parse_show_job_state(
            "LoadState=loaded\nActiveState=active\nSubState=running\nMainPID=4321\nExecMainCode=0\nExecMainStatus=0\n",
        );
        assert!(running.running);
        assert_eq!(running.pid, 4321);
        assert_eq!(running.last_exit_code, None);

        let failed = parse_show_job_state(
            "LoadState=loaded\nActiveState=failed\nSubState=failed\nMainPID=0\nExecMainCode=1\nExecMainStatus=3\n",
        );
        assert!(!failed.running);
        assert_eq!(failed.pid, 0);
        assert_eq!(failed.last_exit_code, Some(3));
        assert!(failed.finished());

        let killed = parse_show_job_state(
            "LoadState=loaded\nActiveState=failed\nSubState=failed\nMainPID=0\nExecMainCode=2\nExecMainStatus=9\n",
        );
        assert_eq!(killed.last_signal, "9");
        assert!(killed.finished());
        assert!(!killed.successful_exit());
    }

    #[test]
    fn maps_autostart_states() {
        assert_eq!(map_autostart("enabled"), Autostart::On);
        assert_eq!(map_autostart("enabled-runtime"), Autostart::On);
        assert_eq!(map_autostart("disabled"), Autostart::Off);
        assert_eq!(map_autostart("masked"), Autostart::Off);
        assert_eq!(map_autostart("static"), Autostart::Unknown);
    }
}

//! Parsing `launchctl print <domain>`.
//!
//! The parser is deliberately line-based and checks exact leading tabs, as the
//! Go code does: output indented with spaces does not parse, and the
//! `unmanaged processes` block is ignored.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::service::State;

/// What launchd says of a loaded job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    /// 0 if no process runs.
    pub pid: i32,
    /// The last exit status: negative for the signal that ended it, 0 if it
    /// never ran or launchd prints no number.
    pub exit: i32,
}

/// What launchd says of a domain: its loaded jobs, and the overrides that
/// disable jobs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DomainState {
    pub jobs: BTreeMap<String, Job>,
    pub disabled: BTreeMap<String, bool>,
}

impl DomainState {
    /// The state of the job with the label, and its process ID. A job ended by
    /// a signal did not fail: that is how a job is stopped.
    pub fn of(&self, label: &str) -> (State, i32) {
        match self.jobs.get(label) {
            None => (State::Stopped, 0),
            Some(job) if job.pid > 0 => (State::Running, job.pid),
            Some(job) if job.exit > 0 => (State::Failed, 0),
            Some(_) => (State::Loaded, 0),
        }
    }
}

/// Reads the `services` and `disabled services` blocks of
/// `launchctl print <domain>`, which an ordinary user can run for the system
/// domain as well as their own.
pub fn parse_print(out: &str) -> DomainState {
    #[derive(Clone, Copy, PartialEq)]
    enum Block {
        None,
        Services,
        Disabled,
    }

    let mut state = DomainState::default();
    let mut block = Block::None;
    for line in out.split('\n') {
        let trimmed = line.trim();
        if line.starts_with("\tservices = {") {
            block = Block::Services;
            continue;
        }
        if line.starts_with("\tdisabled services = {") {
            block = Block::Disabled;
            continue;
        }
        if line.starts_with("\t}") {
            block = Block::None;
            continue;
        }
        match block {
            Block::Services => {
                let fields: Vec<&str> = trimmed.split_whitespace().collect();
                if fields.len() < 3 {
                    continue;
                }
                let pid = fields[0].parse::<i32>().unwrap_or(0);
                let exit = fields[1].parse::<i32>().unwrap_or(0);
                let label = fields[fields.len() - 1];
                state.jobs.insert(label.to_string(), Job { pid, exit });
            }
            Block::Disabled => {
                if let Some((label, status)) = trimmed.split_once(" => ") {
                    state
                        .disabled
                        .insert(label.trim_matches('"').to_string(), status == "disabled");
                }
            }
            Block::None => {}
        }
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launchd::test_support::PRINT_OUT;

    #[test]
    fn parse_print_reads_services_and_disabled_overrides() {
        let state = parse_print(PRINT_OUT);
        assert_eq!(state.jobs["com.example.running"].pid, 409);
        assert_eq!(state.jobs["com.example.idle"].pid, 0);
        assert!(state.jobs.contains_key("com.example.pending"));
        assert!(!state.jobs.contains_key("not.a.service"));
        assert_eq!(state.jobs["com.example.signaled"].exit, -15);
        assert_eq!(state.jobs["com.example.failed"].exit, 1);
        assert!(state.disabled["com.example.off"]);
        assert!(state.disabled.contains_key("com.example.idle"));
        assert!(!state.disabled["com.example.idle"]);

        for (label, want) in [
            ("com.example.running", State::Running),
            ("com.example.idle", State::Loaded),
            ("com.example.pending", State::Loaded),
            ("com.example.failed", State::Failed),
            ("com.example.signaled", State::Loaded),
            ("com.example.missing", State::Stopped),
        ] {
            assert_eq!(state.of(label).0, want, "{label}");
        }
        assert_eq!(state.of("com.example.running").1, 409);
    }

    #[test]
    fn spaces_and_unknown_blocks_do_not_parse() {
        let spaces = "  services = {\n  0 - a\n  }\n";
        assert!(parse_print(spaces).jobs.is_empty());
        let unmanaged = "\tservices = {\n\t}\n\tunmanaged processes = {\n  1 - x\n\t}\n";
        assert!(parse_print(unmanaged).jobs.is_empty());
    }
}

//! Journal log reading via `journalctl`.

use crate::launchd::ProcessRunner;

use super::unit::{SystemdError, validate_any_unit_name};

/// Default line count when callers pass 0.
pub const DEFAULT_JOURNAL_LINES: usize = 400;

/// Reads the most recent `lines` log lines for `unit` from the systemd journal.
///
/// When `system` is `false`, queries the user journal (`journalctl --user -u <unit>`);
/// when `system` is `true`, queries the system journal (`journalctl -u <unit>`).
pub fn read_journal_logs(
    runner: &dyn ProcessRunner,
    unit: &str,
    system: bool,
    lines: usize,
) -> Result<String, SystemdError> {
    validate_any_unit_name(unit)?;
    let count = if lines == 0 {
        DEFAULT_JOURNAL_LINES
    } else {
        lines
    };
    let count_str = count.to_string();
    let args: Vec<&str> = if system {
        vec!["-u", unit, "-n", &count_str, "--no-pager"]
    } else {
        vec!["--user", "-u", unit, "-n", &count_str, "--no-pager"]
    };
    runner.run("journalctl", &args).map_err(SystemdError::from)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::launchd::CommandError;

    struct RecordingRunner {
        calls: Mutex<Vec<(String, Vec<String>)>>,
        response: String,
    }

    impl ProcessRunner for RecordingRunner {
        fn run(&self, program: &str, args: &[&str]) -> Result<String, CommandError> {
            self.calls.lock().unwrap().push((
                program.to_string(),
                args.iter().map(|a| (*a).to_string()).collect(),
            ));
            Ok(self.response.clone())
        }
    }

    #[test]
    fn reads_user_and_system_journal_with_validated_unit() {
        let runner = RecordingRunner {
            calls: Mutex::new(Vec::new()),
            response: "Nov 22 01:00:00 host app[12]: started\n".into(),
        };
        let out = read_journal_logs(&runner, "devbase-api.service", false, 100).unwrap();
        assert!(out.contains("started"));

        let _ = read_journal_logs(&runner, "nginx.service", true, 0).unwrap();

        let calls = runner.calls.lock().unwrap();
        assert_eq!(
            calls[0],
            (
                "journalctl".into(),
                vec![
                    "--user".into(),
                    "-u".into(),
                    "devbase-api.service".into(),
                    "-n".into(),
                    "100".into(),
                    "--no-pager".into(),
                ]
            )
        );
        assert_eq!(
            calls[1],
            (
                "journalctl".into(),
                vec![
                    "-u".into(),
                    "nginx.service".into(),
                    "-n".into(),
                    "400".into(),
                    "--no-pager".into(),
                ]
            )
        );

        assert!(read_journal_logs(&runner, "--evil", false, 10).is_err());
        assert!(read_journal_logs(&runner, "../bad.service", false, 10).is_err());
    }
}

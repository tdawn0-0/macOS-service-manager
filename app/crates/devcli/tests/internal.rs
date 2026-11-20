//! `--dev` dispatch tests: the exact accepted argv, the Go refusal, and the
//! parameter forwarding into the injected subsystem seams. No test starts a
//! listener, a launchd job, or the helper.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use devcli::internal::{InternalRunner, NO_COMMAND_LINE, run_internal_with};
use devcli::{CliError, CliResult};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    Serve(PathBuf),
    Tunnel(PathBuf),
    Maintain(PathBuf),
    DatabaseWorker(PathBuf, String),
    NodePath(PathBuf),
    Helper(i64),
}

#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<Call>>,
}

impl Fake {
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
}

impl InternalRunner for Fake {
    fn serve(&self, root: &Path) -> CliResult<()> {
        self.calls.lock().unwrap().push(Call::Serve(root.into()));
        Ok(())
    }

    fn tunnel(&self, root: &Path) -> CliResult<()> {
        self.calls.lock().unwrap().push(Call::Tunnel(root.into()));
        Ok(())
    }

    fn maintain(&self, root: &Path) -> CliResult<()> {
        self.calls.lock().unwrap().push(Call::Maintain(root.into()));
        Ok(())
    }

    fn database_worker(&self, root: &Path, id: &str) -> CliResult<()> {
        self.calls
            .lock()
            .unwrap()
            .push(Call::DatabaseWorker(root.into(), id.to_string()));
        Ok(())
    }

    fn node_path(&self, dir: &Path, out: &mut dyn Write, _err: &mut dyn Write) -> CliResult<()> {
        self.calls.lock().unwrap().push(Call::NodePath(dir.into()));
        out.write_all(b"/fake/prefix/bin:/fake/node/bin\n/fake/prefix\n")?;
        Ok(())
    }

    fn helper(&self, owner_uid: i64) -> CliResult<()> {
        self.calls.lock().unwrap().push(Call::Helper(owner_uid));
        Ok(())
    }
}

fn run(args: &[&str]) -> (Result<(), CliError>, String, String, Vec<Call>) {
    let fake = Fake::default();
    let args: Vec<String> = args.iter().map(|value| value.to_string()).collect();
    let mut out = Vec::new();
    let mut err = Vec::new();
    let result = run_internal_with(&args, &fake, &mut out, &mut err);
    (
        result,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
        fake.calls(),
    )
}

#[test]
fn no_arguments_and_unknown_commands_are_refused_with_the_go_message() {
    for args in [
        vec![],
        vec!["help"],
        vec!["status"],
        vec!["unknown"],
        vec!["--json"],
    ] {
        let (result, out, err, calls) = run(&args);
        let error = result.unwrap_err();
        assert_eq!(error.message(), NO_COMMAND_LINE, "{args:?}");
        assert_eq!(error.exit_code(), 1);
        assert!(out.is_empty());
        assert!(err.is_empty());
        assert!(calls.is_empty());
    }
}

#[test]
fn serve_tunnel_and_maintain_forward_the_root() {
    for (name, expected) in [
        ("serve", Call::Serve(PathBuf::from("/tmp/dev"))),
        ("tunnel", Call::Tunnel(PathBuf::from("/tmp/dev"))),
        ("maintain", Call::Maintain(PathBuf::from("/tmp/dev"))),
    ] {
        let (result, out, err, calls) = run(&[name, "/tmp/dev"]);
        assert!(result.is_ok(), "{name}");
        assert_eq!(calls, vec![expected]);
        assert!(out.is_empty());
        assert!(err.is_empty());
    }
    // Go's `need(2)` only checks the lower bound; extra argv is ignored.
    let (result, _, _, calls) = run(&["serve", "/tmp/dev", "ignored"]);
    assert!(result.is_ok());
    assert_eq!(calls, vec![Call::Serve(PathBuf::from("/tmp/dev"))]);
}

#[test]
fn each_rooted_subcommand_refuses_a_missing_root() {
    for name in ["serve", "tunnel", "maintain"] {
        let (result, out, _, calls) = run(&[name]);
        let error = result.unwrap_err();
        assert_eq!(
            error.message(),
            "missing arguments; run --dev help",
            "{name}"
        );
        assert_eq!(error.exit_code(), 1);
        assert!(out.is_empty());
        assert!(calls.is_empty());
    }
}

#[test]
fn database_worker_takes_root_and_id_and_refuses_missing_pieces() {
    let (result, _, _, calls) = run(&["database-worker", "/tmp/dev", "task-1"]);
    assert!(result.is_ok());
    assert_eq!(
        calls,
        vec![Call::DatabaseWorker(
            PathBuf::from("/tmp/dev"),
            "task-1".to_string()
        )]
    );

    for args in [vec!["database-worker"], vec!["database-worker", "/tmp/dev"]] {
        let (result, _, _, calls) = run(&args);
        let error = result.unwrap_err();
        assert_eq!(error.message(), "missing arguments; run --dev help");
        assert_eq!(error.exit_code(), 1);
        assert!(calls.is_empty());
    }
}

#[test]
fn node_path_forwards_the_directory_and_prints_two_lines() {
    let (result, out, err, calls) = run(&["node", "path", "/work/project"]);
    assert!(result.is_ok());
    assert_eq!(calls, vec![Call::NodePath(PathBuf::from("/work/project"))]);
    assert_eq!(out, "/fake/prefix/bin:/fake/node/bin\n/fake/prefix\n");
    assert!(err.is_empty());
}

#[test]
fn node_refuses_every_other_subcommand() {
    for args in [vec!["node"], vec!["node", "install"], vec!["node", "23"]] {
        let (result, out, _, calls) = run(&args);
        let error = result.unwrap_err();
        assert_eq!(
            error.message(),
            "node versions are managed in the window: Software, Node.js"
        );
        assert_eq!(error.exit_code(), 1);
        assert!(out.is_empty());
        assert!(calls.is_empty());
    }
}

#[test]
fn helper_parses_owner_uid_anywhere_and_defaults_to_minus_one() {
    let (result, _, _, calls) = run(&["helper", "--owner-uid", "501"]);
    assert!(result.is_ok());
    assert_eq!(calls, vec![Call::Helper(501)]);

    let (result, _, _, calls) = run(&["helper", "ignored", "--owner-uid", "0", "after"]);
    assert!(result.is_ok());
    assert_eq!(calls, vec![Call::Helper(0)]);

    // A trailing flag without a value is ignored, exactly like Go.
    let (result, _, _, calls) = run(&["helper", "--owner-uid"]);
    assert!(result.is_ok());
    assert_eq!(calls, vec![Call::Helper(-1)]);
}

#[test]
fn helper_rejects_a_malformed_owner_uid_with_the_go_strconv_message() {
    let (result, out, _, calls) = run(&["helper", "--owner-uid", "abc"]);
    let error = result.unwrap_err();
    assert_eq!(
        error.message(),
        "invalid --owner-uid \"abc\": strconv.Atoi: parsing \"abc\": invalid syntax"
    );
    assert_eq!(error.exit_code(), 1);
    assert!(out.is_empty());
    assert!(calls.is_empty());

    let overflow = "999999999999999999999999";
    let (result, _, _, _) = run(&["helper", "--owner-uid", overflow]);
    assert_eq!(
        result.unwrap_err().message(),
        format!(
            "invalid --owner-uid \"{overflow}\": strconv.Atoi: parsing \"{overflow}\": value out of range"
        )
    );
}

#[test]
fn run_internal_default_entry_refuses_people_commands() {
    for command in ["help", "status", "install"] {
        let error = devcli::run_internal(&[command.to_string()]).unwrap_err();
        assert_eq!(error.message(), NO_COMMAND_LINE);
        assert_eq!(error.exit_code(), 1);
    }
}

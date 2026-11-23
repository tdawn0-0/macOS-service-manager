//! Contract tests for the ported public CLI.
//!
//! Everything runs against a temp workspace through `CliEnv`; no test starts
//! `launchctl`, `brew`, `security`, a helper, or a network request. The
//! expected JSON strings are built from the Go struct layouts cited in the
//! port spec.

use std::io::Write;
use std::path::Path;

use devcli::args::PASSWORD_STDIN_ADVICE;
use devcli::output::write_action_error_json;
use devcli::{CliEnv, run_cli_with_env};

struct Output {
    code: i32,
    out: String,
    err: String,
}

fn run_with(root: &Path, args: &[&str], input: &[u8]) -> Output {
    let args: Vec<String> = args.iter().map(|value| value.to_string()).collect();
    let mut env = CliEnv::default_env(Box::new(std::io::Cursor::new(input.to_vec())));
    env.root = root.to_path_buf();
    env.cli_bin_dir = root.join("bin");
    env.json = devcli::args::wants_json_flag(&args);
    let mut out = Vec::new();
    let mut err = Vec::new();
    match run_cli_with_env(&args, &mut env, &mut out, &mut err) {
        Ok(()) => Output {
            code: 0,
            out: String::from_utf8(out).unwrap(),
            err: String::from_utf8(err).unwrap(),
        },
        Err(error) => {
            if env.json {
                write_action_error_json(&mut err, error.message(), error.exit_code()).unwrap();
            } else {
                writeln!(err, "Error: {}", error.message()).unwrap();
            }
            Output {
                code: error.exit_code(),
                out: String::from_utf8(out).unwrap(),
                err: String::from_utf8(err).unwrap(),
            }
        }
    }
}

fn run(root: &Path, args: &[&str]) -> Output {
    run_with(root, args, b"")
}

fn temp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[test]
fn version_json_is_the_sorted_go_map() {
    let dir = temp();
    let output = run(dir.path(), &["version", "--json"]);
    assert_eq!(output.code, 0);
    #[cfg(target_os = "macos")]
    assert_eq!(
        output.out,
        "{\n  \"arch\": \"darwin/arm64\",\n  \"name\": \"devbase\",\n  \"version\": \"0.1.0\"\n}\n"
    );
    #[cfg(not(target_os = "macos"))]
    assert_eq!(
        output.out,
        format!(
            "{{\n  \"arch\": \"{}\",\n  \"name\": \"devbase\",\n  \"version\": \"0.1.0\"\n}}\n",
            devcli::schema::CLI_ARCH
        )
    );
    assert!(output.err.is_empty());
}

#[test]
fn version_human_and_unexpected_argument() {
    let dir = temp();
    let output = run(dir.path(), &["--version"]);
    assert_eq!(output.code, 0);
    #[cfg(target_os = "macos")]
    assert_eq!(output.out, "devbase 0.1.0 (darwin/arm64)\n");
    #[cfg(not(target_os = "macos"))]
    assert_eq!(
        output.out,
        format!("devbase 0.1.0 ({})\n", devcli::schema::CLI_ARCH)
    );
    let output = run(dir.path(), &["version", "extra"]);
    assert_eq!(output.code, 2);
    assert_eq!(
        output.err,
        "Error: unexpected argument \"extra\" for version\n"
    );
}

#[test]
fn schema_is_json_even_without_the_flag_and_filters_aliases() {
    let dir = temp();
    let output = run(dir.path(), &["schema", "log-rotation"]);
    assert_eq!(output.code, 0);
    assert!(output.out.starts_with("{\n  \"schemaVersion\": 1,"));
    assert!(output.out.contains("    \"name\": \"log-rotation\","));
    // Go's SetEscapeHTML(false): the invariant text keeps literal < and >.
    assert!(output.out.contains("--confirm <id> --fingerprint <fp>"));
    assert!(!output.out.contains("\\u003c"));
    let output = run(dir.path(), &["schema", "--command", "database"]);
    assert_eq!(output.code, 0);
    assert!(
        output
            .out
            .contains("  \"commands\": [\n    {\n      \"name\": \"db\",")
    );
    let output = run(dir.path(), &["schema", "nope"]);
    assert_eq!(output.code, 2);
}

#[test]
fn help_lists_every_ported_command() {
    let dir = temp();
    let output = run(dir.path(), &["help"]);
    assert_eq!(output.code, 0);
    for command in [
        "status",
        "diagnostics",
        "tasks",
        "logs",
        "ports",
        "shutdown",
        "software",
        "install",
        "db",
        "https",
        "tunnel",
        "ca",
        "services",
        "settings",
        "log-rotation",
        "cli",
        "schema",
        "version",
        "help",
        "gui",
    ] {
        assert!(output.out.contains(command), "missing {command}");
    }
    let output = run(dir.path(), &["help", "services"]);
    assert_eq!(output.code, 0);
    assert!(output.out.starts_with("services - "));
    assert!(output.out.contains("job-save"));
    let output = run(dir.path(), &["help", "nope"]);
    assert_eq!(output.code, 2);
}

#[test]
fn argv_secrets_fail_all_six_forms_with_exit_two_and_no_stdout() {
    let dir = temp();
    for flag in [
        "--password",
        "--password=x",
        "--token",
        "--token=x",
        "--api-token",
        "--api-token=x",
    ] {
        let output = run(dir.path(), &["--json", "status", flag]);
        assert_eq!(output.code, 2, "{flag}");
        assert!(output.out.is_empty(), "{flag} wrote stdout");
        assert!(
            output.err.contains(PASSWORD_STDIN_ADVICE)
                || output.err.contains("pipe the token on stdin")
                || output.err.contains("pipe the API token on stdin"),
            "{flag}: {}",
            output.err
        );
        assert!(output.err.contains("\"exitCode\": 2"), "{flag}");
    }
}

#[test]
fn json_error_shape_and_exit_codes() {
    let dir = temp();
    let output = run(dir.path(), &["--json", "nope"]);
    assert_eq!(output.code, 2);
    assert_eq!(
        output.err,
        "{\n  \"error\": \"unknown command \\\"nope\\\" (run \\\"devbase help\\\" or \\\"devbase schema --json\\\")\",\n  \"exitCode\": 2\n}\n"
    );
    let output = run(dir.path(), &["nope"]);
    assert_eq!(output.code, 2);
    assert_eq!(
        output.err,
        "Error: unknown command \"nope\" (run \"devbase help\" or \"devbase schema --json\")\n"
    );
    // An operational error exits 1, not 2.
    let output = run(dir.path(), &["--json", "software", "versions", "java"]);
    assert_eq!(output.code, 1);
    assert!(
        output
            .err
            .starts_with("{\n  \"error\": \"JDK discovery is unavailable")
    );
    assert!(output.err.contains("\"exitCode\": 1"));
}

#[test]
fn workspace_status_empty_is_an_ordered_flattened_state() {
    let dir = temp();
    let output = run(dir.path(), &["--json", "db", "list"]);
    assert_eq!(output.code, 0);
    assert_eq!(output.out, "{\n  \"instances\": []\n}\n");

    let output = run(dir.path(), &["--json", "db", "profile", "list"]);
    assert_eq!(output.code, 0);
    assert_eq!(output.out, "{\n  \"profiles\": []\n}\n");

    let output = run(dir.path(), &["--json", "https", "status"]);
    assert_eq!(output.code, 0);
    assert_eq!(
        output.out,
        "{\n  \"listenerPort\": 443,\n  \"publicationState\": \"Unknown\",\n  \"revision\": 0,\n  \"rules\": [],\n  \"status\": \"0 rules\"\n}\n"
    );

    let output = run(dir.path(), &["--json", "tunnel", "status"]);
    assert_eq!(output.code, 0);
    assert_eq!(
        output.out,
        "{\n  \"accountId\": \"\",\n  \"clientId\": \"\",\n  \"detail\": \"\",\n  \"dns\": [],\n  \"hasToken\": false,\n  \"rules\": [],\n  \"status\": \"\",\n  \"subdomain\": \"\",\n  \"workers\": {},\n  \"zones\": []\n}\n"
    );
}

#[test]
fn services_list_reports_the_unported_sources_honestly() {
    let dir = temp();
    let output = run(
        dir.path(),
        &["--json", "services", "list", "--source", "brew"],
    );
    assert_eq!(output.code, 0);
    #[cfg(target_os = "macos")]
    assert_eq!(
        output.out,
        "{\n  \"items\": [],\n  \"warnings\": [\n    {\n      \"restriction\": \"The Homebrew provider is not part of this build yet; only launchd jobs are listed.\",\n      \"source\": \"brew\"\n    }\n  ]\n}\n"
    );
    #[cfg(not(target_os = "macos"))]
    assert_eq!(
        output.out,
        "{\n  \"items\": [],\n  \"warnings\": [\n    {\n      \"restriction\": \"The Homebrew provider is only available on macOS.\",\n      \"source\": \"brew\"\n    }\n  ]\n}\n"
    );
    let output = run(dir.path(), &["services", "list", "--source", "nope"]);
    assert_eq!(output.code, 2);
}

#[test]
fn software_uv_path_and_list_report_uv_status() {
    let dir = temp();
    // The shape holds whether or not this machine has uv in PATH.
    let output = run(dir.path(), &["--json", "software", "uv-path"]);
    if output.code == 0 {
        let value: serde_json::Value = serde_json::from_str(&output.out).unwrap();
        assert!(value["path"].as_str().is_some());
        assert!(value["description"].as_str().is_some());
    } else {
        assert_eq!(output.code, 1);
        assert!(output.err.contains("uv was not found"));
    }

    let output = run(dir.path(), &["--json", "software", "list"]);
    assert_eq!(output.code, 0);
    let value: serde_json::Value = serde_json::from_str(&output.out).unwrap();
    assert!(value["uv"]["path"].is_string());
    assert!(value["uv"]["installed"].is_array());
    assert_eq!(value["catalog"][0], "node");

    // A selected executable yields the path shape; description stays empty
    // because the fake binary is not runnable as uv.
    let uv = dir.path().join("uv");
    std::fs::write(&uv, "#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&uv, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let store = devcore::state::Store::new(dir.path());
    store
        .with_state(|state| {
            state.uv_executable = uv.to_string_lossy().into_owned();
            Ok(())
        })
        .unwrap();
    let output = run(dir.path(), &["--json", "software", "uv-path"]);
    assert_eq!(output.code, 0, "{}", output.err);
    let expected = format!(
        "{{\n  \"description\": \"\",\n  \"path\": \"{}\"\n}}\n",
        uv.display()
    );
    assert_eq!(output.out, expected);

    let output = run(dir.path(), &["software", "uv-path", "relative"]);
    assert_eq!(output.code, 2);
}

#[test]
fn log_rotation_and_cache_settings_shapes() {
    let dir = temp();
    let output = run(dir.path(), &["--json", "log-rotation", "status"]);
    assert_eq!(output.code, 0);
    assert_eq!(output.out, "{\n  \"enabled\": true\n}\n");
    let output = run(dir.path(), &["--json", "settings", "log-rotation", "off"]);
    assert_eq!(output.out, "{\n  \"enabled\": false\n}\n");
    let output = run(
        dir.path(),
        &["--json", "settings", "log-rotation", "status"],
    );
    assert_eq!(output.out, "{\n  \"enabled\": false\n}\n");

    let output = run(dir.path(), &["--json", "settings", "cache", "status"]);
    assert_eq!(output.code, 0);
    let value: serde_json::Value = serde_json::from_str(&output.out).unwrap();
    assert_eq!(value["files"], 0);
    assert_eq!(value["bytes"], 0);
    // DownloadCacheInfo is a Go struct: declaration order, not map order.
    assert!(output.out.starts_with("{\n  \"directory\": "));
    assert!(
        output
            .out
            .ends_with("\n  \"files\": 0,\n  \"bytes\": 0\n}\n")
    );
}

#[test]
fn destructive_commands_require_their_confirmation() {
    let dir = temp();
    let cases: &[&[&str]] = &[
        &["software", "uninstall", "node@22.14.0-arm64"],
        &["db", "stop-all"],
        &["db", "remove", "app"],
        &["db", "drop-db", "app", "data"],
        &["db", "drop-user", "app", "user"],
        &["db", "profile", "delete", "app", "profile"],
        &["db", "backup", "restore", "app", "backup"],
        &["db", "backup", "delete", "app", "backup"],
        &["shutdown", "stop"],
        &["tunnel", "remove-config"],
        &["settings", "helper", "uninstall"],
        &["settings", "cache", "clear"],
        &["ca", "untrust"],
        &[
            "ca",
            "remove-archived-trust",
            "archive",
            "--fingerprint",
            "abc",
        ],
        &["ports", "kill", "3000"],
        &["ports", "kill", "--pid", "99999"],
    ];
    for case in cases {
        let output = run(dir.path(), case);
        assert_eq!(output.code, 2, "{case:?}: {}", output.err);
        assert!(
            output.err.contains("requires --yes"),
            "{case:?}: {}",
            output.err
        );
    }

    let output = run(dir.path(), &["db", "create-user", "app", "user"]);
    assert_eq!(output.code, 2);
    assert!(output.err.contains("--password-stdin"));

    let output = run(dir.path(), &["tasks", "finish-deletion", "id", "rev"]);
    assert_eq!(output.code, 2);
    assert!(output.err.contains("--confirm <instance-id>"));

    let output = run(dir.path(), &["db", "delete-data", "app"]);
    assert_eq!(output.code, 2);
    assert!(output.err.contains("--confirm <id> and --fingerprint"));
}

#[test]
fn password_stdin_is_required_and_never_echoed() {
    let dir = temp();
    let output = run_with(
        dir.path(),
        &[
            "--json",
            "db",
            "create-user",
            "app",
            "user",
            "--password-stdin",
        ],
        b"secret-value\r\n",
    );
    // The state has no instance, so the operation fails after reading stdin;
    // the important contract is that the secret is not in the error or argv.
    assert_eq!(output.code, 1);
    assert!(!output.err.contains("secret-value"));
    let output = run_with(
        dir.path(),
        &["db", "create-user", "app", "user", "--password-stdin"],
        b"\n",
    );
    assert_eq!(output.code, 2);
    assert!(output.err.contains("cannot be empty"));
}

#[test]
fn cli_wrapper_install_writes_the_exact_managed_script() {
    let dir = temp();
    let output = run(dir.path(), &["--json", "cli", "install"]);
    assert_eq!(output.code, 0, "{}", output.err);
    let wrapper = dir.path().join("bin/devbase");
    let script = std::fs::read_to_string(&wrapper).unwrap();
    assert!(script.starts_with("#!/bin/sh\n# Managed by Devbase CLI\nset -eu\n"));
    #[cfg(target_os = "macos")]
    assert!(script.contains("if [ \"$1\" = \"gui\" ] || [ \"$1\" = \"open\" ]; then"));
    #[cfg(not(target_os = "macos"))]
    {
        assert!(script.contains("exec \"$TARGET\" \"$@\""));
        assert!(!script.contains("/usr/bin/open"));
        assert!(!script.contains("Devbase.app"));
    }
    assert!(script.ends_with("exit 1\n"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&wrapper).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }
    let output = run(dir.path(), &["--json", "cli", "status"]);
    assert_eq!(output.code, 0);
    assert!(output.out.contains("\"installed\": true"));
    assert!(output.out.contains("\"managedByDevbase\": true"));
    let output = run(dir.path(), &["--json", "cli", "uninstall"]);
    assert_eq!(output.code, 0);
    assert!(!wrapper.exists());
}

#[test]
fn cli_wrapper_refuses_a_foreign_file() {
    let dir = temp();
    std::fs::create_dir_all(dir.path().join("bin")).unwrap();
    std::fs::write(dir.path().join("bin/devbase"), "#!/bin/sh\n").unwrap();
    let output = run(dir.path(), &["cli", "install"]);
    assert_eq!(output.code, 1);
    assert!(output.err.contains("not managed by Devbase"));
    let output = run(dir.path(), &["cli", "uninstall"]);
    assert_eq!(output.code, 1);
    assert!(dir.path().join("bin/devbase").exists());
}

#[test]
fn json_false_and_last_flag_wins() {
    let dir = temp();
    let output = run(dir.path(), &["--json", "--json=false", "version"]);
    assert_eq!(output.code, 0);
    #[cfg(target_os = "macos")]
    assert_eq!(output.out, "devbase 0.1.0 (darwin/arm64)\n");
    #[cfg(not(target_os = "macos"))]
    assert_eq!(
        output.out,
        format!("devbase 0.1.0 ({})\n", devcli::schema::CLI_ARCH)
    );
    let output = run(dir.path(), &["--json=false", "--json", "version"]);
    #[cfg(target_os = "macos")]
    assert_eq!(
        output.out,
        "{\n  \"arch\": \"darwin/arm64\",\n  \"name\": \"devbase\",\n  \"version\": \"0.1.0\"\n}\n"
    );
    #[cfg(not(target_os = "macos"))]
    assert_eq!(
        output.out,
        format!(
            "{{\n  \"arch\": \"{}\",\n  \"name\": \"devbase\",\n  \"version\": \"0.1.0\"\n}}\n",
            devcli::schema::CLI_ARCH
        )
    );
}

#[test]
fn internal_dev_commands_are_reserved() {
    let result = devcli::run_internal(&["node".to_string()]);
    assert!(result.is_err());
    let result = devcli::run_internal(&["nope".to_string()]);
    assert_eq!(result.unwrap_err().exit_code(), 1);
}

#[test]
fn status_flattens_state_then_runtime_helper_cli_and_root() {
    let dir = temp();
    let args: Vec<String> = vec!["--json".into(), "status".into()];
    let mut env = CliEnv::default_env(Box::new(std::io::empty()));
    env.root = dir.path().to_path_buf();
    env.cli_bin_dir = dir.path().join("bin");
    env.login_program = "/opt/devbase/bin/devbase".into();
    env.runtime_states = Some(std::collections::BTreeMap::new());
    env.json = true;
    let mut out = Vec::new();
    let mut err = Vec::new();
    run_cli_with_env(&args, &mut env, &mut out, &mut err).unwrap();
    let out = String::from_utf8(out).unwrap();
    let expected = format!(
        "{{\n  \"connectionProfiles\": {{}},\n  \"proxyRules\": [],\n  \"proxyListenerPort\": 443,\n  \"proxyRulesRevision\": 0,\n  \"schemaVersion\": 2,\n  \"packages\": {{}},\n  \"instances\": {{}},\n  \"runtime\": {{\n    \"states\": {{}}\n  }},\n  \"proxyPublicationState\": \"Unknown\",\n  \"helper\": {{\n    \"State\": \"not-installed\",\n    \"Message\": \"Privileged helper is unavailable in this build.\"\n  }},\n  \"cli\": {{\n    \"installed\": false,\n    \"wrapperPath\": \"{}/bin/devbase\",\n    \"targetExecutable\": \"/opt/devbase/bin/devbase\",\n    \"inPath\": false,\n    \"managedByDevbase\": false\n  }},\n  \"root\": \"{}\"\n}}\n",
        dir.path().display(),
        dir.path().display()
    );
    assert_eq!(out, expected);
    assert!(err.is_empty());
}

#[test]
fn tasks_and_diagnostics_empty_workspace_shapes() {
    let dir = temp();
    for (args, expected) in [
        (
            &["--json", "tasks", "list"][..],
            "{\n  \"operations\": []\n}\n",
        ),
        (
            &["--json", "tasks", "work"][..],
            "{\n  \"databaseWork\": []\n}\n",
        ),
        (
            &["--json", "tasks", "deletions"][..],
            "{\n  \"deletions\": []\n}\n",
        ),
        (
            &["--json", "tasks", "initializations"][..],
            "{\n  \"initializations\": []\n}\n",
        ),
    ] {
        let output = run(dir.path(), args);
        assert_eq!(output.code, 0, "{args:?}: {}", output.err);
        assert_eq!(output.out, expected, "{args:?}");
    }

    let output = run(dir.path(), &["--json", "diagnostics", "run"]);
    assert_eq!(output.code, 0, "{}", output.err);
    let value: serde_json::Value = serde_json::from_str(&output.out).unwrap();
    assert_eq!(value["schemaVersion"], 1);
    assert_eq!(value["redacted"], false);
    assert!(value["generatedAt"].as_str().unwrap().contains('T'));
    let checks = value["checks"].as_array().unwrap();
    assert_eq!(checks.len(), 2);
    assert_eq!(checks[0]["layer"], "CA");
    assert_eq!(checks[1]["layer"], "Trust");
    assert!(
        output
            .out
            .starts_with("{\n  \"schemaVersion\": 1,\n  \"generatedAt\": ")
    );
    assert!(output.out.contains(
        "\"layer\": \"CA\",\n      \"resource\": \"Local CA\",\n      \"status\": \"Unknown\",\n      \"summary\":"
    ));
}

#[test]
fn diagnostics_export_writes_a_private_redacted_report() {
    let dir = temp();
    let output = run(dir.path(), &["--json", "diagnostics", "export"]);
    assert_eq!(output.code, 0, "{}", output.err);
    let value: serde_json::Value = serde_json::from_str(&output.out).unwrap();
    let path = std::path::PathBuf::from(value["path"].as_str().unwrap());
    assert!(path.starts_with(dir.path().join("reports")));
    assert_eq!(value["report"]["redacted"], false);
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.ends_with(b"\n"));
    let exported: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(exported["redacted"], true);
    assert_eq!(exported["checks"][0]["resource"], "resource-1");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[test]
fn software_shell_status_uses_the_shell_contract_shape() {
    let dir = temp();
    let output = run(
        dir.path(),
        &["--json", "software", "shell", "node", "status"],
    );
    assert_eq!(output.code, 0, "{}", output.err);
    let value: serde_json::Value = serde_json::from_str(&output.out).unwrap();
    assert_eq!(value["software"], "node");
    assert!(value["enabled"].is_boolean());
    assert!(value["bashCommand"].as_str().unwrap().contains("BASH_ENV="));
    let output = run(
        dir.path(),
        &["--json", "software", "shell", "java", "bash-command"],
    );
    assert_eq!(output.code, 0);
    let value: serde_json::Value = serde_json::from_str(&output.out).unwrap();
    assert_eq!(value["software"], "java");
    assert!(
        value["bashCommand"]
            .as_str()
            .unwrap()
            .contains("bash-env.sh")
    );
    let output = run(dir.path(), &["software", "shell", "python"]);
    assert_eq!(output.code, 2);
}

#[test]
fn software_default_requires_an_installed_package() {
    let dir = temp();
    let output = run(
        dir.path(),
        &["--json", "software", "default", "node", "22.14.0"],
    );
    assert_eq!(output.code, 1);
    assert!(output.err.contains("not installed"));
    let output = run(dir.path(), &["--json", "db", "port", "app", "--repair"]);
    assert_eq!(output.code, 1);
    assert!(output.err.contains("instance"));
}

#[test]
fn mcp_is_registered_in_schema_and_help_and_handles_stdio_session() {
    let dir = temp();

    let schema = run(dir.path(), &["schema", "mcp"]);
    assert_eq!(schema.code, 0, "{}", schema.err);
    assert!(schema.out.contains("\"name\": \"mcp\""));

    let help = run(dir.path(), &["help", "mcp"]);
    assert_eq!(help.code, 0, "{}", help.err);
    assert!(help.out.contains("devbase mcp"));

    let bad_arg = run(dir.path(), &["mcp", "extra"]);
    assert_eq!(bad_arg.code, 2);

    // Full JSON-RPC session over mock stdin/stdout through `run_cli_with_env`.
    let session = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2024-11-05\"}}\n",
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/list\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/call\",\"params\":{\"name\":\"devbase_diagnostics\",\"arguments\":{}}}\n",
    );
    let output = run_with(dir.path(), &["mcp"], session.as_bytes());
    assert_eq!(output.code, 0, "{}", output.err);
    assert!(output.err.is_empty());

    let frames: Vec<serde_json::Value> = output
        .out
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(frames.len(), 4);
    assert_eq!(frames[0]["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(frames[0]["result"]["serverInfo"]["name"], "devbase");
    assert_eq!(frames[1]["result"], serde_json::json!({}));
    assert_eq!(frames[2]["result"]["tools"].as_array().unwrap().len(), 7);
    assert_eq!(frames[3]["result"]["isError"], false);
    let report: serde_json::Value =
        serde_json::from_str(frames[3]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(report["redacted"], true);
}

#[test]
fn run_cli_streams_mcp_without_blocking_on_read_to_end() {
    let args = vec!["mcp".to_string()];
    let mut input =
        std::io::Cursor::new(b"{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"ping\"}\n".to_vec());
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = devcli::run_cli(&args, &mut input, &mut out, &mut err);
    assert_eq!(code, 0);
    assert!(err.is_empty());
    let resp: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(resp["id"], 9);
    assert_eq!(resp["result"], serde_json::json!({}));
}

#[test]
fn ports_star_watch_and_kill_guards_work() {
    let dir = temp();

    let starred = run(dir.path(), &["--json", "ports", "star", "3000"]);
    assert_eq!(starred.code, 0, "{}", starred.err);
    let starred_json: serde_json::Value = serde_json::from_str(&starred.out).unwrap();
    assert_eq!(starred_json["port"], 3000);
    assert_eq!(starred_json["starred"], true);
    assert_eq!(starred_json["favoritePorts"], serde_json::json!([3000]));

    let watched = run(dir.path(), &["--json", "ports", "watch", "3000"]);
    assert_eq!(watched.code, 0, "{}", watched.err);
    let watched_json: serde_json::Value = serde_json::from_str(&watched.out).unwrap();
    assert_eq!(watched_json["watched"], true);
    assert_eq!(watched_json["watchedPorts"], serde_json::json!([3000]));

    let unstarred = run(dir.path(), &["--json", "ports", "unstar", "3000"]);
    assert_eq!(unstarred.code, 0, "{}", unstarred.err);
    let unstarred_json: serde_json::Value = serde_json::from_str(&unstarred.out).unwrap();
    assert_eq!(unstarred_json["favoritePorts"], serde_json::json!([]));

    let unwatched = run(dir.path(), &["--json", "ports", "unwatch", "3000"]);
    assert_eq!(unwatched.code, 0, "{}", unwatched.err);
    let unwatched_json: serde_json::Value = serde_json::from_str(&unwatched.out).unwrap();
    assert_eq!(unwatched_json["watchedPorts"], serde_json::json!([]));

    // Protected PID 1 is refused even with --yes.
    let pid1 = run(dir.path(), &["ports", "kill", "--pid", "1", "--yes"]);
    assert_eq!(pid1.code, 2);
    assert!(pid1.err.contains("protected system PID"));

    // Invalid category is refused cleanly.
    let bad_cat = run(dir.path(), &["ports", "active", "--category", "invalid"]);
    assert_eq!(bad_cat.code, 2);
    assert!(bad_cat.err.contains("unknown category"));
}

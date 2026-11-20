use std::path::{Path, PathBuf};
use std::sync::Arc;
use plist::Value;
use super::test_support::{
    FakeHelper, FakeHost, FakeRunner, TempDir, VENDOR_JOB, binary_vendor_plist, context, write_file,
};
use super::*;
use crate::service::Source;

fn decode(data: &[u8]) -> plist::Dictionary {
    match Value::from_reader(std::io::Cursor::new(data)).expect("a plist") {
        Value::Dictionary(dict) => dict,
        other => panic!("not a dictionary: {other:?}"),
    }
}
use plist::Value;

fn spec_from_bytes(data: &[u8]) -> Spec {
    let dir = TempDir::new();
    let path = write_file(dir.path(), "x.plist", data);
    Spec::load(&path).expect("load spec")
}

fn root_setup(
    dir: &Path,
    installed: bool,
) -> (Arc<FakeRunner>, FakeHelper, Arc<FakeHost>, Arc<Context>) {
    let runner = FakeRunner::new("");
    let helper = FakeHelper::new(installed);
    let host = FakeHost::new(dir, dir);
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    (runner, helper, host, ctx)
}

fn provider(
    dir: &Path,
    source: Source,
    domain: &str,
    elevated: bool,
    ctx: Arc<Context>,
) -> LaunchdProvider {
    LaunchdProvider::new(
        Config {
            source,
            dir: dir.to_path_buf(),
            domain: domain.to_string(),
            elevated,
        },
        ctx,
    )
}

fn temp_leftovers(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with(prefix))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn an_edit_of_nothing_changes_nothing() {
    let fixtures: [(&str, Vec<u8>); 2] = [
        ("xml", VENDOR_JOB.as_bytes().to_vec()),
        ("binary", binary_vendor_plist()),
    ];
    for (name, data) in fixtures {
        for _ in 0..20 {
            let spec = spec_from_bytes(&data);
            let changes = spec.changes(&data).expect("changes");
            assert!(changes.is_empty(), "{name}: changes = {changes:?}");
        }
    }
}

#[test]
fn changes_names_what_changed() {
    let data = VENDOR_JOB.as_bytes();
    let mut spec = spec_from_bytes(data);
    spec.args = vec!["/opt/vendor/helper".into(), "--verbose".into()];
    spec.start_interval = 0;
    spec.run_at_load = true;
    spec.user_name = "_vendor".into();
    assert_eq!(
        spec.changes(data).expect("changes"),
        vec!["ProgramArguments", "RunAtLoad", "StartInterval", "UserName"]
    );
}

#[test]
fn a_file_with_a_program_runs_what_the_form_says() {
    let data = br#"<plist version="1.0"><dict>
<key>Label</key><string>com.vendor.O+Connect</string>
<key>Program</key><string>/opt/vendor/helper</string>
<key>ProgramArguments</key><array><string>helper</string><string>--old</string></array>
</dict></plist>"#;
    let mut spec = spec_from_bytes(data);
    assert_eq!(spec.args, vec!["/opt/vendor/helper", "--old"]);
    spec.validate().expect("an existing job");

    spec.existing = false;
    assert!(spec.validate().is_err(), "a new job cannot have that label");
    spec.existing = true;
    assert!(spec.changes(data).expect("changes").is_empty());

    // New arguments keep what the process calls itself.
    spec.args = vec!["/opt/vendor/helper".into(), "--new".into()];
    let out = spec.marshal(Some(data)).expect("marshal");
    let dict = decode(&out);
    assert_eq!(
        dict.get("Program").and_then(Value::as_string),
        Some("/opt/vendor/helper")
    );
    let args = dict
        .get("ProgramArguments")
        .and_then(Value::as_array)
        .expect("arguments");
    assert_eq!(args[0].as_string(), Some("helper"));
    assert_eq!(args[1].as_string(), Some("--new"));

    // A new program runs: launchd would go on running the old one.
    spec.args = vec!["/opt/other/tool".into(), "--new".into()];
    let out = spec.marshal(Some(data)).expect("marshal");
    let dict = decode(&out);
    assert_eq!(
        dict.get("Program").and_then(Value::as_string),
        Some("/opt/other/tool")
    );
    let args = dict
        .get("ProgramArguments")
        .and_then(Value::as_array)
        .expect("arguments");
    assert_eq!(args[0].as_string(), Some("/opt/other/tool"));

    let got = spec_from_bytes(&out);
    assert_eq!(got.args, spec.args);
}

#[test]
fn turning_an_option_off_keeps_it_false() {
    let data = br#"<plist version="1.0"><dict>
<key>Label</key><string>a</string>
<key>ProgramArguments</key><array><string>/bin/date</string></array>
<key>RunAtLoad</key><true/>
<key>KeepAlive</key><true/>
<key>StartOnMount</key><true/>
</dict></plist>"#;
    let mut spec = spec_from_bytes(data);
    spec.run_at_load = false;
    spec.keep_alive = false;
    spec.start_on_mount = false;
    let out = spec.marshal(Some(data)).expect("marshal");
    let dict = decode(&out);
    for key in ["RunAtLoad", "KeepAlive", "StartOnMount"] {
        assert_eq!(dict.get(key), Some(&Value::Boolean(false)), "{key}");
    }

    let fresh = Spec {
        label: "a".into(),
        args: vec!["/bin/date".into()],
        ..Spec::default()
    }
    .marshal(None)
    .expect("marshal new");
    let dict = decode(&fresh);
    for key in ["RunAtLoad", "KeepAlive", "StartOnMount"] {
        assert!(!dict.contains_key(key), "a new file says nothing of {key}");
    }
}

#[test]
fn editing_a_daemon_replaces_then_loads_again_under_one_authentication() {
    let dir = TempDir::new();
    let path = dir.path().join("my jobs");
    std::fs::create_dir_all(&path).expect("job dir");
    let (_runner, helper, _host, ctx) = root_setup(dir.path(), true);
    let path = write_file(&path, "com.vendor.helper.plist", VENDOR_JOB.as_bytes());
    let provider = provider(path.parent().unwrap(), Source::Daemon, "system", true, ctx);
    let mut spec = Spec::load(&path).expect("load");
    spec.user_name.clear();
    let outcome = provider
        .save_as_root(&spec, Some(&path), true)
        .expect("save daemon");
    assert_eq!(outcome.path, path);
    assert_eq!(outcome.load_error, None);
    assert_eq!(
        helper.save_requests()[0].existing_path.as_deref(),
        Some(path.as_path())
    );
    assert_eq!(
        helper.calls(),
        vec![
            vec!["/bin/launchctl", "print", "system/com.vendor.helper",],
            vec!["/bin/launchctl", "bootout", "system/com.vendor.helper",],
            vec![
                "/bin/launchctl",
                "bootstrap",
                "system",
                path.to_str().unwrap(),
            ],
        ]
    );
}

#[test]
fn editing_a_system_agent_loads_again_as_the_user() {
    let dir = TempDir::new();
    let (runner, helper, _host, ctx) = root_setup(dir.path(), true);
    let path = write_file(dir.path(), "a.plist", VENDOR_JOB.as_bytes());
    let provider = provider(dir.path(), Source::SystemAgent, "gui/501", false, ctx);
    let spec = Spec::load(&path).expect("load");
    provider
        .save_as_root(&spec, Some(&path), true)
        .expect("save agent");
    assert!(helper.calls().is_empty(), "{:?}", helper.calls());
    let calls: Vec<String> = runner.calls().iter().map(|call| call.join(" ")).collect();
    let bootout = calls
        .iter()
        .position(|call| call == "launchctl bootout gui/501/com.vendor.helper");
    let bootstrap = calls
        .iter()
        .position(|call| call == &format!("launchctl bootstrap gui/501 {}", path.display()));
    assert!(
        bootout.is_some() && bootstrap.is_some() && bootout < bootstrap,
        "{calls:?}"
    );
}

#[test]
fn saving_over_what_is_not_a_file_asks_nothing() {
    let dir = TempDir::new();
    let (_runner, helper, _host, ctx) = root_setup(dir.path(), false);
    let real = write_file(dir.path(), "real.plist", VENDOR_JOB.as_bytes());
    let link = dir.path().join("link.plist");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");
    let provider = provider(dir.path(), Source::Daemon, "system", true, ctx);
    let spec = Spec {
        label: "com.vendor.helper".into(),
        args: vec!["/bin/date".into()],
        ..Spec::default()
    };
    let error = provider
        .save_as_root(&spec, Some(&link), true)
        .expect_err("symlink");
    assert!(error.message.contains("not a regular file"), "{error}");

    let error = provider
        .save_as_root(&spec, Some(&dir.path().join("gone.plist")), true)
        .expect_err("missing");
    assert_ne!(error.code, ErrorCode::HelperNotInstalled);
    assert!(helper.save_requests().is_empty());
}

#[test]
fn a_failure_says_whether_the_file_was_saved() {
    let dir = TempDir::new();
    let path = write_file(dir.path(), "a.plist", VENDOR_JOB.as_bytes());
    let (_runner, helper, _host, ctx) = root_setup(dir.path(), false);
    let provider = provider(dir.path(), Source::Daemon, "system", true, ctx);
    let mut spec = Spec::load(&path).expect("load");
    spec.run_at_load = true;

    let error = provider
        .save_as_root(&spec, Some(&path), true)
        .expect_err("no helper");
    assert_eq!(error.code, ErrorCode::HelperNotInstalled);

    helper.install();
    helper.fail("bootstrap", "Bootstrap failed: 5: Input/output error");
    let outcome = provider
        .save_as_root(&spec, Some(&path), true)
        .expect("saved");
    assert_eq!(outcome.path, path);
    let load_error = outcome.load_error.expect("load error");
    assert!(
        load_error.contains("saved, but it did not load"),
        "{load_error}"
    );
}

#[test]
fn removing_a_root_job_puts_it_in_the_trash() {
    let dir = TempDir::new();
    let job_dir = dir.path().join("my jobs");
    std::fs::create_dir_all(&job_dir).expect("job dir");
    let runtime = TempDir::new();
    std::fs::create_dir_all(dir.path().join(".Trash")).expect("trash");
    let runner = FakeRunner::new("");
    let helper = FakeHelper::installed();
    helper.set_print_succeeds(false);
    let host = FakeHost::new(dir.path(), runtime.path());
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    let provider = provider(&job_dir, Source::Daemon, "gui/501", true, ctx);

    let path = write_file(&job_dir, "com.vendor.it's.plist", VENDOR_JOB.as_bytes());
    let item = Item {
        name: "com.example.msm-test.no-such-job".into(),
        path: path.clone(),
        ..item_stub("com.example.msm-test.no-such-job", &path)
    };
    provider.do_action(&item, Action::Remove).expect("remove");
    assert!(!path.exists(), "still there");
    let trashed = host.trashed();
    assert_eq!(trashed.len(), 1);
    assert_eq!(
        trashed[0].file_name(),
        Some(std::ffi::OsStr::new("com.vendor.it's.plist"))
    );
    assert_eq!(std::fs::read(&trashed[0]).unwrap(), VENDOR_JOB.as_bytes());
    assert!(
        temp_leftovers(runtime.path(), "msm-removed-").is_empty(),
        "the temporary directory stays"
    );
    assert_eq!(helper.remove_requests().len(), 1);
    assert_eq!(
        helper.calls(),
        vec![vec![
            "/bin/launchctl",
            "print",
            "gui/501/com.example.msm-test.no-such-job"
        ],]
    );
}

#[test]
fn a_job_launchd_will_not_unload_is_not_removed() {
    let dir = TempDir::new();
    let runtime = TempDir::new();
    let runner = FakeRunner::new("");
    let helper = FakeHelper::installed();
    helper.fail("bootout", "bootout failed");
    let host = FakeHost::new(dir.path(), runtime.path());
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    let provider = provider(dir.path(), Source::Daemon, "system", true, ctx);

    let path = write_file(dir.path(), "a.plist", VENDOR_JOB.as_bytes());
    let target = item_stub("com.vendor.helper", &path);
    let error = provider
        .do_action(&target, Action::Remove)
        .expect_err("bootout fails");
    assert!(error.message.contains("bootout failed"), "{error}");
    assert_eq!(std::fs::read(&path).unwrap(), VENDOR_JOB.as_bytes());
    assert!(host.trashed().is_empty());
    assert!(
        temp_leftovers(runtime.path(), "msm-removed-").is_empty(),
        "left behind"
    );
}

#[test]
fn a_replaced_file_is_saved_whether_or_not_the_job_loads_again() {
    for (name, subcommand, message) in [
        ("launchd will not unload it", "bootout", "bootout failed"),
        (
            "launchd will not load it",
            "bootstrap",
            "Bootstrap failed: 5: Input/output error",
        ),
    ] {
        let dir = TempDir::new();
        let (runner, helper, _host, ctx) = root_setup(dir.path(), true);
        helper.fail(subcommand, message);
        let path = write_file(dir.path(), "a.plist", VENDOR_JOB.as_bytes());
        let provider = provider(dir.path(), Source::Daemon, "system", true, ctx);
        let mut spec = Spec::load(&path).expect("load");
        spec.run_at_load = true;
        let outcome = provider
            .save_as_root(&spec, Some(&path), true)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(outcome.path, path, "{name}");
        let load_error = outcome.load_error.expect("load error");
        assert!(
            load_error.contains("saved, but it did not load"),
            "{name}: {load_error}"
        );
        let saved = Spec::load(&path).expect("reload");
        assert!(saved.run_at_load, "{name}: the file was not replaced");
        let _ = runner;
    }
}

#[test]
fn removing_a_root_job_keeps_it_if_anything_fails() {
    let dir = TempDir::new();
    let runtime = TempDir::new();
    let runner = FakeRunner::new("");
    let helper = FakeHelper::missing();
    let host = FakeHost::new(dir.path(), runtime.path());
    host.fail_trash("no Trash");
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    let provider = provider(dir.path(), Source::Daemon, "system", true, ctx);

    let path = write_file(dir.path(), "a.plist", VENDOR_JOB.as_bytes());
    let target = item_stub("com.vendor.helper", &path);

    let error = provider
        .do_action(&target, Action::Remove)
        .expect_err("no helper");
    assert_eq!(error.code, ErrorCode::HelperNotInstalled);
    assert!(host.trashed().is_empty());
    assert!(path.exists(), "missing helper keeps the file");

    helper.install();
    let error = provider
        .do_action(&target, Action::Remove)
        .expect_err("trash fails");
    assert!(error.message.contains("not in the Trash"), "{error}");
    let want_bootout = vec![
        "/bin/launchctl".to_string(),
        "bootout".to_string(),
        "system/com.vendor.helper".to_string(),
    ];
    assert!(
        helper.calls().contains(&want_bootout),
        "helper calls missing bootout: {:?}",
        helper.calls()
    );
}

#[test]
fn optional_throttle_preserves_existing_job_policy() {
    let data = br#"<plist version="1.0"><dict><key>Label</key><string>a</string><key>ProgramArguments</key><array><string>/bin/date</string></array><key>ThrottleInterval</key><integer>45</integer></dict></plist>"#;
    let mut spec = spec_from_bytes(data);
    let out = spec.marshal(Some(data)).expect("marshal");
    assert_eq!(
        decode(&out)
            .get("ThrottleInterval")
            .and_then(Value::as_unsigned_integer),
        Some(45)
    );
    spec.throttle_interval = Some(3);
    let out = spec.marshal(Some(data)).expect("marshal");
    assert_eq!(
        decode(&out)
            .get("ThrottleInterval")
            .and_then(Value::as_unsigned_integer),
        Some(3)
    );
}

#[test]
fn authorized_job_payload_keeps_label_and_arguments() {
    for edit in [false, true] {
        for load in [false, true] {
            let dir = TempDir::new();
            let (_runner, helper, _host, ctx) = root_setup(dir.path(), true);
            let provider = provider(dir.path(), Source::Daemon, "system", true, ctx);
            let spec = Spec {
                label: "com.example.fixed".into(),
                args: vec!["/bin/echo".into(), "quoted ' Unicode 字段".into()],
                ..Spec::default()
            };
            let target_path = dir.path().join("com.example.fixed.plist");
            let existing = if edit {
                let bytes = spec.marshal(None).expect("marshal");
                std::fs::write(&target_path, bytes).expect("write existing");
                Some(target_path.clone())
            } else {
                None
            };
            provider
                .save_as_root(&spec, existing.as_deref(), load)
                .unwrap_or_else(|error| panic!("edit {edit} load {load}: {error}"));
            let decoded = decode(&std::fs::read(&target_path).expect("read saved"));
            assert_eq!(
                decoded.get("Label").and_then(Value::as_string),
                Some("com.example.fixed")
            );
            let args = decoded
                .get("ProgramArguments")
                .and_then(Value::as_array)
                .expect("arguments");
            assert_eq!(args[1].as_string(), Some("quoted ' Unicode 字段"));

            if load {
                let want = [
                    "/bin/launchctl",
                    "bootstrap",
                    "system",
                    target_path.to_str().unwrap(),
                ];
                assert!(
                    helper
                        .calls()
                        .iter()
                        .any(|call| call.iter().map(String::as_str).eq(want.iter().copied())),
                    "expected bootstrap in {:?}",
                    helper.calls()
                );
            }
        }
    }
}

#[test]
fn authorized_job_payload_size_limit() {
    let dir = TempDir::new();
    let (_runner, helper, _host, ctx) = root_setup(dir.path(), true);
    let provider = provider(dir.path(), Source::Daemon, "system", true, ctx);
    let error = provider
        .save_as_root(
            &Spec {
                label: "com.example.large".into(),
                args: vec!["/bin/echo".into(), "x".repeat(65 << 10)],
                ..Spec::default()
            },
            None,
            true,
        )
        .expect_err("oversized");
    assert!(error.message.contains("size limit"), "{error}");
    assert!(helper.save_requests().is_empty());
}

#[test]
fn root_operations_reject_paths_outside_the_provider_directory() {
    let dir = TempDir::new();
    let (_runner, helper, _host, ctx) = root_setup(dir.path(), true);
    let provider = provider(dir.path(), Source::Daemon, "system", true, ctx);

    let other = TempDir::new();
    let outside = write_file(other.path(), "outside.plist", VENDOR_JOB.as_bytes());

    let spec = Spec::load(&outside).expect("load outside");
    assert!(provider.save_as_root(&spec, Some(&outside), false).is_err());
    assert!(helper.calls().is_empty(), "{:?}", helper.calls());

    let target = write_file(dir.path(), "target.plist", VENDOR_JOB.as_bytes());
    let link = dir.path().join("link.plist");
    std::os::unix::fs::symlink(&target, &link).expect("symlink");
    let spec = Spec::load(&link).expect("load symlink");
    assert!(provider.save_as_root(&spec, Some(&link), false).is_err());
    assert!(helper.calls().is_empty(), "{:?}", helper.calls());

    let item = item_stub("com.vendor.helper", &outside);
    assert!(provider.do_action(&item, Action::Remove).is_err());
    assert!(helper.calls().is_empty(), "{:?}", helper.calls());
}

fn item_stub(name: &str, path: &Path) -> Item {
    Item {
        id: format!("daemon:{}", path.display()),
        source: Source::Daemon,
        name: name.to_string(),
        state: crate::service::State::Stopped,
        pid: 0,
        autostart: Autostart::On,
        owner: "system".into(),
        path: path.to_path_buf(),
        kind: None,
        admin: true,
        keep_alive: false,
    }
}

use super::test_support::{
    FakeHelper, FakeHost, FakeRunner, TempDir, VENDOR_JOB, binary_vendor_plist, context, write_file,
};
use super::*;
use crate::service::Source;

fn decode(data: &[u8]) -> plist::Dictionary {
    match Value::from_reader(std::io::Cursor::new(data)).expect("a plist") {
        Value::Dictionary(dict) => dict,
        other => panic!("not a dictionary: {other:?}"),
    }
}

fn spec_from_bytes(data: &[u8]) -> Spec {
    let dir = TempDir::new();
    let path = write_file(dir.path(), "x.plist", data);
    Spec::load(&path).expect("load spec")
}

fn root_setup(
    dir: &Path,
    installed: bool,
) -> (Arc<FakeRunner>, FakeHelper, Arc<FakeHost>, Arc<Context>) {
    let runner = FakeRunner::new("");
    let helper = FakeHelper::new(installed);
    let host = FakeHost::new(dir, dir);
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    (runner, helper, host, ctx)
}

fn provider(
    dir: &Path,
    source: Source,
    domain: &str,
    elevated: bool,
    ctx: Arc<Context>,
) -> LaunchdProvider {
    LaunchdProvider::new(
        Config {
            source,
            dir: dir.to_path_buf(),
            domain: domain.to_string(),
            elevated,
        },
        ctx,
    )
}

fn temp_leftovers(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with(prefix))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn an_edit_of_nothing_changes_nothing() {
    let fixtures: [(&str, Vec<u8>); 2] = [
        ("xml", VENDOR_JOB.as_bytes().to_vec()),
        ("binary", binary_vendor_plist()),
    ];
    for (name, data) in fixtures {
        for _ in 0..20 {
            let spec = spec_from_bytes(&data);
            let changes = spec.changes(&data).expect("changes");
            assert!(changes.is_empty(), "{name}: changes = {changes:?}");
        }
    }
}

#[test]
fn changes_names_what_changed() {
    let data = VENDOR_JOB.as_bytes();
    let mut spec = spec_from_bytes(data);
    spec.args = vec!["/opt/vendor/helper".into(), "--verbose".into()];
    spec.start_interval = 0;
    spec.run_at_load = true;
    spec.user_name = "_vendor".into();
    assert_eq!(
        spec.changes(data).expect("changes"),
        vec!["ProgramArguments", "RunAtLoad", "StartInterval", "UserName"]
    );
}

#[test]
fn a_file_with_a_program_runs_what_the_form_says() {
    let data = br#"<plist version="1.0"><dict>
<key>Label</key><string>com.vendor.O+Connect</string>
<key>Program</key><string>/opt/vendor/helper</string>
<key>ProgramArguments</key><array><string>helper</string><string>--old</string></array>
</dict></plist>"#;
    let mut spec = spec_from_bytes(data);
    assert_eq!(spec.args, vec!["/opt/vendor/helper", "--old"]);
    spec.validate().expect("an existing job");

    spec.existing = false;
    assert!(spec.validate().is_err(), "a new job cannot have that label");
    spec.existing = true;
    assert!(spec.changes(data).expect("changes").is_empty());

    // New arguments keep what the process calls itself.
    spec.args = vec!["/opt/vendor/helper".into(), "--new".into()];
    let out = spec.marshal(Some(data)).expect("marshal");
    let dict = decode(&out);
    assert_eq!(
        dict.get("Program").and_then(Value::as_string),
        Some("/opt/vendor/helper")
    );
    let args = dict
        .get("ProgramArguments")
        .and_then(Value::as_array)
        .expect("arguments");
    assert_eq!(args[0].as_string(), Some("helper"));
    assert_eq!(args[1].as_string(), Some("--new"));

    // A new program runs: launchd would go on running the old one.
    spec.args = vec!["/opt/other/tool".into(), "--new".into()];
    let out = spec.marshal(Some(data)).expect("marshal");
    let dict = decode(&out);
    assert_eq!(
        dict.get("Program").and_then(Value::as_string),
        Some("/opt/other/tool")
    );
    let args = dict
        .get("ProgramArguments")
        .and_then(Value::as_array)
        .expect("arguments");
    assert_eq!(args[0].as_string(), Some("/opt/other/tool"));

    let got = spec_from_bytes(&out);
    assert_eq!(got.args, spec.args);
}

#[test]
fn turning_an_option_off_keeps_it_false() {
    let data = br#"<plist version="1.0"><dict>
<key>Label</key><string>a</string>
<key>ProgramArguments</key><array><string>/bin/date</string></array>
<key>RunAtLoad</key><true/>
<key>KeepAlive</key><true/>
<key>StartOnMount</key><true/>
</dict></plist>"#;
    let mut spec = spec_from_bytes(data);
    spec.run_at_load = false;
    spec.keep_alive = false;
    spec.start_on_mount = false;
    let out = spec.marshal(Some(data)).expect("marshal");
    let dict = decode(&out);
    for key in ["RunAtLoad", "KeepAlive", "StartOnMount"] {
        assert_eq!(dict.get(key), Some(&Value::Boolean(false)), "{key}");
    }

    let fresh = Spec {
        label: "a".into(),
        args: vec!["/bin/date".into()],
        ..Spec::default()
    }
    .marshal(None)
    .expect("marshal new");
    let dict = decode(&fresh);
    for key in ["RunAtLoad", "KeepAlive", "StartOnMount"] {
        assert!(!dict.contains_key(key), "a new file says nothing of {key}");
    }
}

#[test]
fn editing_a_daemon_replaces_then_loads_again_under_one_authentication() {
    let dir = TempDir::new();
    let path = dir.path().join("my jobs");
    std::fs::create_dir_all(&path).expect("job dir");
    let (_runner, helper, _host, ctx) = root_setup(dir.path(), true);
    let path = write_file(&path, "com.vendor.helper.plist", VENDOR_JOB.as_bytes());
    let provider = provider(path.parent().unwrap(), Source::Daemon, "system", true, ctx);
    let mut spec = Spec::load(&path).expect("load");
    spec.user_name.clear();
    let outcome = provider
        .save_as_root(&spec, Some(&path), true)
        .expect("save daemon");
    assert_eq!(outcome.path, path);
    assert_eq!(outcome.load_error, None);
    assert_eq!(
        helper.save_requests()[0].existing_path.as_deref(),
        Some(path.as_path())
    );
    assert_eq!(
        helper.calls(),
        vec![
            vec!["/bin/launchctl", "print", "system/com.vendor.helper",],
            vec!["/bin/launchctl", "bootout", "system/com.vendor.helper",],
            vec![
                "/bin/launchctl",
                "bootstrap",
                "system",
                path.to_str().unwrap(),
            ],
        ]
    );
}

#[test]
fn editing_a_system_agent_loads_again_as_the_user() {
    let dir = TempDir::new();
    let (runner, helper, _host, ctx) = root_setup(dir.path(), true);
    let path = write_file(dir.path(), "a.plist", VENDOR_JOB.as_bytes());
    let provider = provider(dir.path(), Source::SystemAgent, "gui/501", false, ctx);
    let spec = Spec::load(&path).expect("load");
    provider
        .save_as_root(&spec, Some(&path), true)
        .expect("save agent");
    assert!(helper.calls().is_empty(), "{:?}", helper.calls());
    let calls: Vec<String> = runner.calls().iter().map(|call| call.join(" ")).collect();
    let bootout = calls
        .iter()
        .position(|call| call == "launchctl bootout gui/501/com.vendor.helper");
    let bootstrap = calls
        .iter()
        .position(|call| call == &format!("launchctl bootstrap gui/501 {}", path.display()));
    assert!(
        bootout.is_some() && bootstrap.is_some() && bootout < bootstrap,
        "{calls:?}"
    );
}

#[test]
fn saving_over_what_is_not_a_file_asks_nothing() {
    let dir = TempDir::new();
    let (_runner, helper, _host, ctx) = root_setup(dir.path(), false);
    let real = write_file(dir.path(), "real.plist", VENDOR_JOB.as_bytes());
    let link = dir.path().join("link.plist");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");
    let provider = provider(dir.path(), Source::Daemon, "system", true, ctx);
    let spec = Spec {
        label: "com.vendor.helper".into(),
        args: vec!["/bin/date".into()],
        ..Spec::default()
    };
    let error = provider
        .save_as_root(&spec, Some(&link), true)
        .expect_err("symlink");
    assert!(error.message.contains("not a regular file"), "{error}");

    let error = provider
        .save_as_root(&spec, Some(&dir.path().join("gone.plist")), true)
        .expect_err("missing");
    assert_ne!(error.code, ErrorCode::HelperNotInstalled);
    assert!(helper.save_requests().is_empty());
}

#[test]
fn a_failure_says_whether_the_file_was_saved() {
    let dir = TempDir::new();
    let path = write_file(dir.path(), "a.plist", VENDOR_JOB.as_bytes());
    let (_runner, helper, _host, ctx) = root_setup(dir.path(), false);
    let provider = provider(dir.path(), Source::Daemon, "system", true, ctx);
    let mut spec = Spec::load(&path).expect("load");
    spec.run_at_load = true;

    let error = provider
        .save_as_root(&spec, Some(&path), true)
        .expect_err("no helper");
    assert_eq!(error.code, ErrorCode::HelperNotInstalled);

    helper.install();
    helper.fail("bootstrap", "Bootstrap failed: 5: Input/output error");
    let outcome = provider
        .save_as_root(&spec, Some(&path), true)
        .expect("saved");
    assert_eq!(outcome.path, path);
    let load_error = outcome.load_error.expect("load error");
    assert!(
        load_error.contains("saved, but it did not load"),
        "{load_error}"
    );
}

#[test]
fn removing_a_root_job_puts_it_in_the_trash() {
    let dir = TempDir::new();
    let job_dir = dir.path().join("my jobs");
    std::fs::create_dir_all(&job_dir).expect("job dir");
    let runtime = TempDir::new();
    std::fs::create_dir_all(dir.path().join(".Trash")).expect("trash");
    let runner = FakeRunner::new("");
    let helper = FakeHelper::installed();
    helper.set_print_succeeds(false);
    let host = FakeHost::new(dir.path(), runtime.path());
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    let provider = provider(&job_dir, Source::Daemon, "gui/501", true, ctx);

    let path = write_file(&job_dir, "com.vendor.it's.plist", VENDOR_JOB.as_bytes());
    let item = Item {
        name: "com.example.msm-test.no-such-job".into(),
        path: path.clone(),
        ..item_stub("com.example.msm-test.no-such-job", &path)
    };
    provider.do_action(&item, Action::Remove).expect("remove");
    assert!(!path.exists(), "still there");
    let trashed = host.trashed();
    assert_eq!(trashed.len(), 1);
    assert_eq!(
        trashed[0].file_name(),
        Some(std::ffi::OsStr::new("com.vendor.it's.plist"))
    );
    assert_eq!(std::fs::read(&trashed[0]).unwrap(), VENDOR_JOB.as_bytes());
    assert!(
        temp_leftovers(runtime.path(), "msm-removed-").is_empty(),
        "the temporary directory stays"
    );
    assert_eq!(helper.remove_requests().len(), 1);
    assert_eq!(
        helper.calls(),
        vec![vec![
            "/bin/launchctl",
            "print",
            "gui/501/com.example.msm-test.no-such-job"
        ],]
    );
}

#[test]
fn a_job_launchd_will_not_unload_is_not_removed() {
    let dir = TempDir::new();
    let runtime = TempDir::new();
    let runner = FakeRunner::new("");
    let helper = FakeHelper::installed();
    helper.fail("bootout", "bootout failed");
    let host = FakeHost::new(dir.path(), runtime.path());
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    let provider = provider(dir.path(), Source::Daemon, "system", true, ctx);

    let path = write_file(dir.path(), "a.plist", VENDOR_JOB.as_bytes());
    let target = item_stub("com.vendor.helper", &path);
    let error = provider
        .do_action(&target, Action::Remove)
        .expect_err("bootout fails");
    assert!(error.message.contains("bootout failed"), "{error}");
    assert_eq!(std::fs::read(&path).unwrap(), VENDOR_JOB.as_bytes());
    assert!(host.trashed().is_empty());
    assert!(
        temp_leftovers(runtime.path(), "msm-removed-").is_empty(),
        "left behind"
    );
}

#[test]
fn a_replaced_file_is_saved_whether_or_not_the_job_loads_again() {
    for (name, subcommand, message) in [
        ("launchd will not unload it", "bootout", "bootout failed"),
        (
            "launchd will not load it",
            "bootstrap",
            "Bootstrap failed: 5: Input/output error",
        ),
    ] {
        let dir = TempDir::new();
        let (runner, helper, _host, ctx) = root_setup(dir.path(), true);
        helper.fail(subcommand, message);
        let path = write_file(dir.path(), "a.plist", VENDOR_JOB.as_bytes());
        let provider = provider(dir.path(), Source::Daemon, "system", true, ctx);
        let mut spec = Spec::load(&path).expect("load");
        spec.run_at_load = true;
        let outcome = provider
            .save_as_root(&spec, Some(&path), true)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(outcome.path, path, "{name}");
        let load_error = outcome.load_error.expect("load error");
        assert!(
            load_error.contains("saved, but it did not load"),
            "{name}: {load_error}"
        );
        let saved = Spec::load(&path).expect("reload");
        assert!(saved.run_at_load, "{name}: the file was not replaced");
        let _ = runner;
    }
}

#[test]
fn removing_a_root_job_keeps_it_if_anything_fails() {
    let dir = TempDir::new();
    let runtime = TempDir::new();
    let runner = FakeRunner::new("");
    let helper = FakeHelper::missing();
    let host = FakeHost::new(dir.path(), runtime.path());
    host.fail_trash("no Trash");
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    let provider = provider(dir.path(), Source::Daemon, "system", true, ctx);

    let path = write_file(dir.path(), "a.plist", VENDOR_JOB.as_bytes());
    let target = item_stub("com.vendor.helper", &path);

    let error = provider
        .do_action(&target, Action::Remove)
        .expect_err("no helper");
    assert_eq!(error.code, ErrorCode::HelperNotInstalled);
    assert!(host.trashed().is_empty());
    assert!(path.exists(), "missing helper keeps the file");

    helper.install();
    let error = provider
        .do_action(&target, Action::Remove)
        .expect_err("trash fails");
    assert!(error.message.contains("not in the Trash"), "{error}");
    let want_bootout = vec![
        "/bin/launchctl".to_string(),
        "bootout".to_string(),
        "system/com.vendor.helper".to_string(),
    ];
    assert!(
        helper.calls().contains(&want_bootout),
        "helper calls missing bootout: {:?}",
        helper.calls()
    );
}

#[test]
fn optional_throttle_preserves_existing_job_policy() {
    let data = br#"<plist version="1.0"><dict><key>Label</key><string>a</string><key>ProgramArguments</key><array><string>/bin/date</string></array><key>ThrottleInterval</key><integer>45</integer></dict></plist>"#;
    let mut spec = spec_from_bytes(data);
    let out = spec.marshal(Some(data)).expect("marshal");
    assert_eq!(
        decode(&out)
            .get("ThrottleInterval")
            .and_then(Value::as_unsigned_integer),
        Some(45)
    );
    spec.throttle_interval = Some(3);
    let out = spec.marshal(Some(data)).expect("marshal");
    assert_eq!(
        decode(&out)
            .get("ThrottleInterval")
            .and_then(Value::as_unsigned_integer),
        Some(3)
    );
}

#[test]
fn authorized_job_payload_keeps_label_and_arguments() {
    for edit in [false, true] {
        for load in [false, true] {
            let dir = TempDir::new();
            let (_runner, helper, _host, ctx) = root_setup(dir.path(), true);
            let provider = provider(dir.path(), Source::Daemon, "system", true, ctx);
            let spec = Spec {
                label: "com.example.fixed".into(),
                args: vec!["/bin/echo".into(), "quoted ' Unicode 字段".into()],
                ..Spec::default()
            };
            let target_path = dir.path().join("com.example.fixed.plist");
            let existing = if edit {
                let bytes = spec.marshal(None).expect("marshal");
                std::fs::write(&target_path, bytes).expect("write existing");
                Some(target_path.clone())
            } else {
                None
            };
            provider
                .save_as_root(&spec, existing.as_deref(), load)
                .unwrap_or_else(|error| panic!("edit {edit} load {load}: {error}"));
            let decoded = decode(&std::fs::read(&target_path).expect("read saved"));
            assert_eq!(
                decoded.get("Label").and_then(Value::as_string),
                Some("com.example.fixed")
            );
            let args = decoded
                .get("ProgramArguments")
                .and_then(Value::as_array)
                .expect("arguments");
            assert_eq!(args[1].as_string(), Some("quoted ' Unicode 字段"));

            if load {
                let want = [
                    "/bin/launchctl",
                    "bootstrap",
                    "system",
                    target_path.to_str().unwrap(),
                ];
                assert!(
                    helper
                        .calls()
                        .iter()
                        .any(|call| call.iter().map(String::as_str).eq(want.iter().copied())),
                    "expected bootstrap in {:?}",
                    helper.calls()
                );
            }
        }
    }
}

#[test]
fn authorized_job_payload_size_limit() {
    let dir = TempDir::new();
    let (_runner, helper, _host, ctx) = root_setup(dir.path(), true);
    let provider = provider(dir.path(), Source::Daemon, "system", true, ctx);
    let error = provider
        .save_as_root(
            &Spec {
                label: "com.example.large".into(),
                args: vec!["/bin/echo".into(), "x".repeat(65 << 10)],
                ..Spec::default()
            },
            None,
            true,
        )
        .expect_err("oversized");
    assert!(error.message.contains("size limit"), "{error}");
    assert!(helper.save_requests().is_empty());
}

#[test]
fn root_operations_reject_paths_outside_the_provider_directory() {
    let dir = TempDir::new();
    let (_runner, helper, _host, ctx) = root_setup(dir.path(), true);
    let provider = provider(dir.path(), Source::Daemon, "system", true, ctx);

    let other = TempDir::new();
    let outside = write_file(other.path(), "outside.plist", VENDOR_JOB.as_bytes());

    let spec = Spec::load(&outside).expect("load outside");
    assert!(provider.save_as_root(&spec, Some(&outside), false).is_err());
    assert!(helper.calls().is_empty(), "{:?}", helper.calls());

    let target = write_file(dir.path(), "target.plist", VENDOR_JOB.as_bytes());
    let link = dir.path().join("link.plist");
    std::os::unix::fs::symlink(&target, &link).expect("symlink");
    let spec = Spec::load(&link).expect("load symlink");
    assert!(provider.save_as_root(&spec, Some(&link), false).is_err());
    assert!(helper.calls().is_empty(), "{:?}", helper.calls());

    let item = item_stub("com.vendor.helper", &outside);
    assert!(provider.do_action(&item, Action::Remove).is_err());
    assert!(helper.calls().is_empty(), "{:?}", helper.calls());
}

fn item_stub(name: &str, path: &Path) -> Item {
    Item {
        id: format!("daemon:{}", path.display()),
        source: Source::Daemon,
        name: name.to_string(),
        state: crate::service::State::Stopped,
        pid: 0,
        autostart: Autostart::On,
        owner: "system".into(),
        path: path.to_path_buf(),
        kind: None,
        admin: true,
        keep_alive: false,
    }
}

use super::test_support::{
    FakeHelper, FakeHost, FakeRunner, TempDir, VENDOR_JOB, binary_vendor_plist, context, write_file,
};
use super::*;
use crate::service::Source;

fn decode(data: &[u8]) -> plist::Dictionary {
    match Value::from_reader(std::io::Cursor::new(data)).expect("a plist") {
        Value::Dictionary(dict) => dict,
        other => panic!("not a dictionary: {other:?}"),
    }
}

fn spec_from_bytes(data: &[u8]) -> Spec {
    let dir = TempDir::new();
    let path = write_file(dir.path(), "x.plist", data);
    Spec::load(&path).expect("load spec")
}

fn root_setup(
    dir: &Path,
    installed: bool,
) -> (Arc<FakeRunner>, FakeHelper, Arc<FakeHost>, Arc<Context>) {
    let runner = FakeRunner::new("");
    let helper = FakeHelper::new(installed);
    let host = FakeHost::new(dir, dir);
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    (runner, helper, host, ctx)
}

fn provider(
    dir: &Path,
    source: Source,
    domain: &str,
    elevated: bool,
    ctx: Arc<Context>,
) -> LaunchdProvider {
    LaunchdProvider::new(
        Config {
            source,
            dir: dir.to_path_buf(),
            domain: domain.to_string(),
            elevated,
        },
        ctx,
    )
}

fn temp_leftovers(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with(prefix))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn an_edit_of_nothing_changes_nothing() {
    let fixtures: [(&str, Vec<u8>); 2] = [
        ("xml", VENDOR_JOB.as_bytes().to_vec()),
        ("binary", binary_vendor_plist()),
    ];
    for (name, data) in fixtures {
        for _ in 0..20 {
            let spec = spec_from_bytes(&data);
            let changes = spec.changes(&data).expect("changes");
            assert!(changes.is_empty(), "{name}: changes = {changes:?}");
        }
    }
}

#[test]
fn changes_names_what_changed() {
    let data = VENDOR_JOB.as_bytes();
    let mut spec = spec_from_bytes(data);
    spec.args = vec!["/opt/vendor/helper".into(), "--verbose".into()];
    spec.start_interval = 0;
    spec.run_at_load = true;
    spec.user_name = "_vendor".into();
    assert_eq!(
        spec.changes(data).expect("changes"),
        vec!["ProgramArguments", "RunAtLoad", "StartInterval", "UserName"]
    );
}

#[test]
fn a_file_with_a_program_runs_what_the_form_says() {
    let data = br#"<plist version="1.0"><dict>
<key>Label</key><string>com.vendor.O+Connect</string>
<key>Program</key><string>/opt/vendor/helper</string>
<key>ProgramArguments</key><array><string>helper</string><string>--old</string></array>
</dict></plist>"#;
    let mut spec = spec_from_bytes(data);
    assert_eq!(spec.args, vec!["/opt/vendor/helper", "--old"]);
    spec.validate().expect("an existing job");

    spec.existing = false;
    assert!(spec.validate().is_err(), "a new job cannot have that label");
    spec.existing = true;
    assert!(spec.changes(data).expect("changes").is_empty());

    // New arguments keep what the process calls itself.
    spec.args = vec!["/opt/vendor/helper".into(), "--new".into()];
    let out = spec.marshal(Some(data)).expect("marshal");
    let dict = decode(&out);
    assert_eq!(
        dict.get("Program").and_then(Value::as_string),
        Some("/opt/vendor/helper")
    );
    let args = dict
        .get("ProgramArguments")
        .and_then(Value::as_array)
        .expect("arguments");
    assert_eq!(args[0].as_string(), Some("helper"));
    assert_eq!(args[1].as_string(), Some("--new"));

    // A new program runs: launchd would go on running the old one.
    spec.args = vec!["/opt/other/tool".into(), "--new".into()];
    let out = spec.marshal(Some(data)).expect("marshal");
    let dict = decode(&out);
    assert_eq!(
        dict.get("Program").and_then(Value::as_string),
        Some("/opt/other/tool")
    );
    let args = dict
        .get("ProgramArguments")
        .and_then(Value::as_array)
        .expect("arguments");
    assert_eq!(args[0].as_string(), Some("/opt/other/tool"));

    let got = spec_from_bytes(&out);
    assert_eq!(got.args, spec.args);
}

#[test]
fn turning_an_option_off_keeps_it_false() {
    let data = br#"<plist version="1.0"><dict>
<key>Label</key><string>a</string>
<key>ProgramArguments</key><array><string>/bin/date</string></array>
<key>RunAtLoad</key><true/>
<key>KeepAlive</key><true/>
<key>StartOnMount</key><true/>
</dict></plist>"#;
    let mut spec = spec_from_bytes(data);
    spec.run_at_load = false;
    spec.keep_alive = false;
    spec.start_on_mount = false;
    let out = spec.marshal(Some(data)).expect("marshal");
    let dict = decode(&out);
    for key in ["RunAtLoad", "KeepAlive", "StartOnMount"] {
        assert_eq!(dict.get(key), Some(&Value::Boolean(false)), "{key}");
    }

    let fresh = Spec {
        label: "a".into(),
        args: vec!["/bin/date".into()],
        ..Spec::default()
    }
    .marshal(None)
    .expect("marshal new");
    let dict = decode(&fresh);
    for key in ["RunAtLoad", "KeepAlive", "StartOnMount"] {
        assert!(!dict.contains_key(key), "a new file says nothing of {key}");
    }
}

#[test]
fn editing_a_daemon_replaces_then_loads_again_under_one_authentication() {
    let dir = TempDir::new();
    let path = dir.path().join("my jobs");
    std::fs::create_dir_all(&path).expect("job dir");
    let (_runner, helper, _host, ctx) = root_setup(dir.path(), true);
    let path = write_file(&path, "com.vendor.helper.plist", VENDOR_JOB.as_bytes());
    let provider = provider(path.parent().unwrap(), Source::Daemon, "system", true, ctx);
    let mut spec = Spec::load(&path).expect("load");
    spec.user_name.clear();
    let outcome = provider
        .save_as_root(&spec, Some(&path), true)
        .expect("save daemon");
    assert_eq!(outcome.path, path);
    assert_eq!(outcome.load_error, None);
    assert_eq!(
        helper.save_requests()[0].existing_path.as_deref(),
        Some(path.as_path())
    );
    assert_eq!(
        helper.calls(),
        vec![
            vec!["/bin/launchctl", "print", "system/com.vendor.helper",],
            vec!["/bin/launchctl", "bootout", "system/com.vendor.helper",],
            vec![
                "/bin/launchctl",
                "bootstrap",
                "system",
                path.to_str().unwrap(),
            ],
        ]
    );
}

#[test]
fn editing_a_system_agent_loads_again_as_the_user() {
    let dir = TempDir::new();
    let (runner, helper, _host, ctx) = root_setup(dir.path(), true);
    let path = write_file(dir.path(), "a.plist", VENDOR_JOB.as_bytes());
    let provider = provider(dir.path(), Source::SystemAgent, "gui/501", false, ctx);
    let spec = Spec::load(&path).expect("load");
    provider
        .save_as_root(&spec, Some(&path), true)
        .expect("save agent");
    assert!(helper.calls().is_empty(), "{:?}", helper.calls());
    let calls: Vec<String> = runner.calls().iter().map(|call| call.join(" ")).collect();
    let bootout = calls
        .iter()
        .position(|call| call == "launchctl bootout gui/501/com.vendor.helper");
    let bootstrap = calls
        .iter()
        .position(|call| call == &format!("launchctl bootstrap gui/501 {}", path.display()));
    assert!(
        bootout.is_some() && bootstrap.is_some() && bootout < bootstrap,
        "{calls:?}"
    );
}

#[test]
fn saving_over_what_is_not_a_file_asks_nothing() {
    let dir = TempDir::new();
    let (_runner, helper, _host, ctx) = root_setup(dir.path(), false);
    let real = write_file(dir.path(), "real.plist", VENDOR_JOB.as_bytes());
    let link = dir.path().join("link.plist");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");
    let provider = provider(dir.path(), Source::Daemon, "system", true, ctx);
    let spec = Spec {
        label: "com.vendor.helper".into(),
        args: vec!["/bin/date".into()],
        ..Spec::default()
    };
    let error = provider
        .save_as_root(&spec, Some(&link), true)
        .expect_err("symlink");
    assert!(error.message.contains("not a regular file"), "{error}");

    let error = provider
        .save_as_root(&spec, Some(&dir.path().join("gone.plist")), true)
        .expect_err("missing");
    assert_ne!(error.code, ErrorCode::HelperNotInstalled);
    assert!(helper.save_requests().is_empty());
}

#[test]
fn a_failure_says_whether_the_file_was_saved() {
    let dir = TempDir::new();
    let path = write_file(dir.path(), "a.plist", VENDOR_JOB.as_bytes());
    let (_runner, helper, _host, ctx) = root_setup(dir.path(), false);
    let provider = provider(dir.path(), Source::Daemon, "system", true, ctx);
    let mut spec = Spec::load(&path).expect("load");
    spec.run_at_load = true;

    let error = provider
        .save_as_root(&spec, Some(&path), true)
        .expect_err("no helper");
    assert_eq!(error.code, ErrorCode::HelperNotInstalled);

    helper.install();
    helper.fail("bootstrap", "Bootstrap failed: 5: Input/output error");
    let outcome = provider
        .save_as_root(&spec, Some(&path), true)
        .expect("saved");
    assert_eq!(outcome.path, path);
    let load_error = outcome.load_error.expect("load error");
    assert!(
        load_error.contains("saved, but it did not load"),
        "{load_error}"
    );
}

#[test]
fn removing_a_root_job_puts_it_in_the_trash() {
    let dir = TempDir::new();
    let job_dir = dir.path().join("my jobs");
    std::fs::create_dir_all(&job_dir).expect("job dir");
    let runtime = TempDir::new();
    std::fs::create_dir_all(dir.path().join(".Trash")).expect("trash");
    let runner = FakeRunner::new("");
    let helper = FakeHelper::installed();
    helper.set_print_succeeds(false);
    let host = FakeHost::new(dir.path(), runtime.path());
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    let provider = provider(&job_dir, Source::Daemon, "gui/501", true, ctx);

    let path = write_file(&job_dir, "com.vendor.it's.plist", VENDOR_JOB.as_bytes());
    let item = Item {
        name: "com.example.msm-test.no-such-job".into(),
        path: path.clone(),
        ..item_stub("com.example.msm-test.no-such-job", &path)
    };
    provider.do_action(&item, Action::Remove).expect("remove");
    assert!(!path.exists(), "still there");
    let trashed = host.trashed();
    assert_eq!(trashed.len(), 1);
    assert_eq!(
        trashed[0].file_name(),
        Some(std::ffi::OsStr::new("com.vendor.it's.plist"))
    );
    assert_eq!(std::fs::read(&trashed[0]).unwrap(), VENDOR_JOB.as_bytes());
    assert!(
        temp_leftovers(runtime.path(), "msm-removed-").is_empty(),
        "the temporary directory stays"
    );
    assert_eq!(helper.remove_requests().len(), 1);
    assert_eq!(
        helper.calls(),
        vec![vec![
            "/bin/launchctl",
            "print",
            "gui/501/com.example.msm-test.no-such-job"
        ],]
    );
}

#[test]
fn a_job_launchd_will_not_unload_is_not_removed() {
    let dir = TempDir::new();
    let runtime = TempDir::new();
    let runner = FakeRunner::new("");
    let helper = FakeHelper::installed();
    helper.fail("bootout", "bootout failed");
    let host = FakeHost::new(dir.path(), runtime.path());
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    let provider = provider(dir.path(), Source::Daemon, "system", true, ctx);

    let path = write_file(dir.path(), "a.plist", VENDOR_JOB.as_bytes());
    let target = item_stub("com.vendor.helper", &path);
    let error = provider
        .do_action(&target, Action::Remove)
        .expect_err("bootout fails");
    assert!(error.message.contains("bootout failed"), "{error}");
    assert_eq!(std::fs::read(&path).unwrap(), VENDOR_JOB.as_bytes());
    assert!(host.trashed().is_empty());
    assert!(
        temp_leftovers(runtime.path(), "msm-removed-").is_empty(),
        "left behind"
    );
}

#[test]
fn a_replaced_file_is_saved_whether_or_not_the_job_loads_again() {
    for (name, subcommand, message) in [
        ("launchd will not unload it", "bootout", "bootout failed"),
        (
            "launchd will not load it",
            "bootstrap",
            "Bootstrap failed: 5: Input/output error",
        ),
    ] {
        let dir = TempDir::new();
        let (runner, helper, _host, ctx) = root_setup(dir.path(), true);
        helper.fail(subcommand, message);
        let path = write_file(dir.path(), "a.plist", VENDOR_JOB.as_bytes());
        let provider = provider(dir.path(), Source::Daemon, "system", true, ctx);
        let mut spec = Spec::load(&path).expect("load");
        spec.run_at_load = true;
        let outcome = provider
            .save_as_root(&spec, Some(&path), true)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(outcome.path, path, "{name}");
        let load_error = outcome.load_error.expect("load error");
        assert!(
            load_error.contains("saved, but it did not load"),
            "{name}: {load_error}"
        );
        let saved = Spec::load(&path).expect("reload");
        assert!(saved.run_at_load, "{name}: the file was not replaced");
        let _ = runner;
    }
}

#[test]
fn removing_a_root_job_keeps_it_if_anything_fails() {
    let dir = TempDir::new();
    let runtime = TempDir::new();
    let runner = FakeRunner::new("");
    let helper = FakeHelper::missing();
    let host = FakeHost::new(dir.path(), runtime.path());
    host.fail_trash("no Trash");
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    let provider = provider(dir.path(), Source::Daemon, "system", true, ctx);

    let path = write_file(dir.path(), "a.plist", VENDOR_JOB.as_bytes());
    let target = item_stub("com.vendor.helper", &path);

    let error = provider
        .do_action(&target, Action::Remove)
        .expect_err("no helper");
    assert_eq!(error.code, ErrorCode::HelperNotInstalled);
    assert!(host.trashed().is_empty());
    assert!(path.exists(), "missing helper keeps the file");

    helper.install();
    let error = provider
        .do_action(&target, Action::Remove)
        .expect_err("trash fails");
    assert!(error.message.contains("not in the Trash"), "{error}");
    let want_bootout = vec![
        "/bin/launchctl".to_string(),
        "bootout".to_string(),
        "system/com.vendor.helper".to_string(),
    ];
    assert!(
        helper.calls().contains(&want_bootout),
        "helper calls missing bootout: {:?}",
        helper.calls()
    );
}

#[test]
fn optional_throttle_preserves_existing_job_policy() {
    let data = br#"<plist version="1.0"><dict><key>Label</key><string>a</string><key>ProgramArguments</key><array><string>/bin/date</string></array><key>ThrottleInterval</key><integer>45</integer></dict></plist>"#;
    let mut spec = spec_from_bytes(data);
    let out = spec.marshal(Some(data)).expect("marshal");
    assert_eq!(
        decode(&out)
            .get("ThrottleInterval")
            .and_then(Value::as_unsigned_integer),
        Some(45)
    );
    spec.throttle_interval = Some(3);
    let out = spec.marshal(Some(data)).expect("marshal");
    assert_eq!(
        decode(&out)
            .get("ThrottleInterval")
            .and_then(Value::as_unsigned_integer),
        Some(3)
    );
}

#[test]
fn authorized_job_payload_keeps_label_and_arguments() {
    for edit in [false, true] {
        for load in [false, true] {
            let dir = TempDir::new();
            let (_runner, helper, _host, ctx) = root_setup(dir.path(), true);
            let provider = provider(dir.path(), Source::Daemon, "system", true, ctx);
            let spec = Spec {
                label: "com.example.fixed".into(),
                args: vec!["/bin/echo".into(), "quoted ' Unicode 字段".into()],
                ..Spec::default()
            };
            let target_path = dir.path().join("com.example.fixed.plist");
            let existing = if edit {
                let bytes = spec.marshal(None).expect("marshal");
                std::fs::write(&target_path, bytes).expect("write existing");
                Some(target_path.clone())
            } else {
                None
            };
            provider
                .save_as_root(&spec, existing.as_deref(), load)
                .unwrap_or_else(|error| panic!("edit {edit} load {load}: {error}"));
            let decoded = decode(&std::fs::read(&target_path).expect("read saved"));
            assert_eq!(
                decoded.get("Label").and_then(Value::as_string),
                Some("com.example.fixed")
            );
            let args = decoded
                .get("ProgramArguments")
                .and_then(Value::as_array)
                .expect("arguments");
            assert_eq!(args[1].as_string(), Some("quoted ' Unicode 字段"));

            if load {
                let want = [
                    "/bin/launchctl",
                    "bootstrap",
                    "system",
                    target_path.to_str().unwrap(),
                ];
                assert!(
                    helper
                        .calls()
                        .iter()
                        .any(|call| call.iter().map(String::as_str).eq(want.iter().copied())),
                    "expected bootstrap in {:?}",
                    helper.calls()
                );
            }
        }
    }
}

#[test]
fn authorized_job_payload_size_limit() {
    let dir = TempDir::new();
    let (_runner, helper, _host, ctx) = root_setup(dir.path(), true);
    let provider = provider(dir.path(), Source::Daemon, "system", true, ctx);
    let error = provider
        .save_as_root(
            &Spec {
                label: "com.example.large".into(),
                args: vec!["/bin/echo".into(), "x".repeat(65 << 10)],
                ..Spec::default()
            },
            None,
            true,
        )
        .expect_err("oversized");
    assert!(error.message.contains("size limit"), "{error}");
    assert!(helper.save_requests().is_empty());
}

#[test]
fn root_operations_reject_paths_outside_the_provider_directory() {
    let dir = TempDir::new();
    let (_runner, helper, _host, ctx) = root_setup(dir.path(), true);
    let provider = provider(dir.path(), Source::Daemon, "system", true, ctx);

    let other = TempDir::new();
    let outside = write_file(other.path(), "outside.plist", VENDOR_JOB.as_bytes());

    let spec = Spec::load(&outside).expect("load outside");
    assert!(provider.save_as_root(&spec, Some(&outside), false).is_err());
    assert!(helper.calls().is_empty(), "{:?}", helper.calls());

    let target = write_file(dir.path(), "target.plist", VENDOR_JOB.as_bytes());
    let link = dir.path().join("link.plist");
    std::os::unix::fs::symlink(&target, &link).expect("symlink");
    let spec = Spec::load(&link).expect("load symlink");
    assert!(provider.save_as_root(&spec, Some(&link), false).is_err());
    assert!(helper.calls().is_empty(), "{:?}", helper.calls());

    let item = item_stub("com.vendor.helper", &outside);
    assert!(provider.do_action(&item, Action::Remove).is_err());
    assert!(helper.calls().is_empty(), "{:?}", helper.calls());
}

fn item_stub(name: &str, path: &Path) -> Item {
    Item {
        id: format!("daemon:{}", path.display()),
        source: Source::Daemon,
        name: name.to_string(),
        state: crate::service::State::Stopped,
        pid: 0,
        autostart: Autostart::On,
        owner: "system".into(),
        path: path.to_path_buf(),
        kind: None,
        admin: true,
        keep_alive: false,
    }
}

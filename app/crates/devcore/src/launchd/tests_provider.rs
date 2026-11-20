use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::test_support::{
    FakeHelper, FakeHost, FakeRunner, PRINT_OUT, TempDir, XML_PLIST, binary_off_plist, context,
    write_file,
};
use super::*;
use crate::service::{Item, Source, State};

fn fixture_dir() -> TempDir {
    let dir = TempDir::new();
    write_file(dir.path(), "running.plist", XML_PLIST.as_bytes());
    write_file(dir.path(), "off.plist", &binary_off_plist());
    write_file(
        dir.path(),
        "nolabel.plist",
        br#"<?xml version="1.0"?><plist version="1.0"><dict><key>X</key><string>y</string></dict></plist>"#,
    );
    write_file(dir.path(), "broken.plist", b"not a plist");
    write_file(dir.path(), "readme.txt", b"ignored");
    dir
}

struct Fixture {
    _dir: TempDir,
    _home: TempDir,
    runner: Arc<FakeRunner>,
    helper: FakeHelper,
    provider: LaunchdProvider,
}

fn fixture() -> Fixture {
    let dir = fixture_dir();
    let home = TempDir::new();
    let runner = FakeRunner::new(PRINT_OUT);
    let host = FakeHost::new(home.path(), dir.path());
    let helper = FakeHelper::missing();
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    let provider = LaunchdProvider::new(
        Config {
            source: Source::UserAgent,
            dir: dir.path().to_path_buf(),
            domain: "gui/501".into(),
            elevated: false,
        },
        ctx,
    );
    Fixture {
        _dir: dir,
        _home: home,
        runner,
        helper,
        provider,
    }
}

fn item(name: &str, path: &str) -> Item {
    Item {
        id: String::new(),
        source: Source::UserAgent,
        name: name.to_string(),
        state: State::None,
        pid: 0,
        autostart: Autostart::On,
        owner: String::new(),
        path: PathBuf::from(path),
        kind: None,
        admin: false,
        keep_alive: false,
    }
}

fn by_name<'a>(items: &'a [Item], name: &str) -> &'a Item {
    items
        .iter()
        .find(|item| item.name == name)
        .unwrap_or_else(|| panic!("no item {name:?} in {items:?}"))
}

fn call(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_string()).collect()
}

#[test]
fn disabled_and_scheduled() {
    let fixture = fixture();
    let provider = LaunchdProvider::new(
        Config {
            source: Source::UserAgent,
            dir: PathBuf::from("/x"),
            domain: "gui/501".into(),
            elevated: false,
        },
        fixture.provider.ctx(),
    );
    let state = parse_print(PRINT_OUT);
    let file = |label: &str| plist_file::FileModel {
        label: label.to_string(),
        ..plist_file::FileModel::default()
    };

    let mut disabled = file("com.example.new");
    disabled.disabled = Some(true);
    assert_eq!(
        provider
            .item(Path::new("/x/a"), &disabled, &state)
            .autostart,
        Autostart::Off
    );

    let mut overridden_on = file("com.example.idle");
    overridden_on.disabled = Some(true);
    assert_eq!(
        provider
            .item(Path::new("/x/a"), &overridden_on, &state)
            .autostart,
        Autostart::On
    );

    let mut overridden_off = file("com.example.off");
    overridden_off.disabled = Some(false);
    assert_eq!(
        provider
            .item(Path::new("/x/a"), &overridden_off, &state)
            .autostart,
        Autostart::Off
    );

    let mut interval = file("com.example.idle");
    interval.start_interval = Some(60);
    assert_eq!(
        provider.item(Path::new("/x/a"), &interval, &state).state,
        State::Scheduled
    );

    let mut calendar = file("com.example.idle");
    calendar.start_calendar_interval = Some(plist::Value::Boolean(true));
    assert_eq!(
        provider.item(Path::new("/x/a"), &calendar, &state).state,
        State::Scheduled
    );
}

#[test]
fn list_reads_state_once_and_lists_invalid_files() {
    let fixture = fixture();
    let items = fixture.provider.list().expect("list");
    assert_eq!(
        fixture.runner.calls(),
        vec![vec!["launchctl", "print", "gui/501"]]
    );
    assert_eq!(items.len(), 4, "{items:?}");

    let off = by_name(&items, "com.example.off");
    assert_eq!(off.state, State::Stopped);
    assert_eq!(off.autostart, Autostart::Off);
    assert!(!off.keep_alive);

    let running = by_name(&items, "com.example.running");
    assert_eq!(running.state, State::Running);
    assert_eq!(running.pid, 409);
    assert_eq!(running.autostart, Autostart::On);
    assert!(running.keep_alive);
    assert_eq!(running.id, format!("user-agent:{}", running.path.display()));

    for name in ["broken", "nolabel"] {
        let invalid = by_name(&items, name);
        assert_eq!(invalid.state, State::Invalid);
        assert_eq!(invalid.kind.as_deref(), Some(INVALID_KIND));
        assert_eq!(
            invalid.path,
            fixture.provider.config().dir.join(format!("{name}.plist"))
        );
        assert_eq!(invalid.autostart, Autostart::Unknown);
    }

    let names: Vec<&str> = items.iter().map(|item| item.name.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
}

#[test]
fn list_fails_when_launchctl_does_not_answer() {
    let fixture = fixture();
    fixture.runner.fail("launchctl", "launchctl: exit status 1");
    let error = fixture.provider.list().expect_err("list must fail");
    assert_eq!(error.message, "launchctl: exit status 1");
}

#[test]
fn invalid_detail_says_why() {
    let fixture = fixture();
    let items = fixture.provider.list().expect("list");
    for (name, want) in [("broken", "failed to parse plist"), ("nolabel", "no Label")] {
        let detail = fixture
            .provider
            .detail(by_name(&items, name))
            .expect("invalid detail");
        let rows: BTreeMap<&str, &str> = detail
            .rows
            .iter()
            .map(|row| (row.key.as_str(), row.value.as_str()))
            .collect();
        assert!(rows["Problem"].contains(want), "{rows:?}");
        assert_eq!(
            rows["Property list"],
            fixture
                .provider
                .config()
                .dir
                .join(format!("{name}.plist"))
                .display()
                .to_string()
        );
    }
}

#[test]
fn detail_shows_program_runs_environment_and_one_log() {
    let fixture = fixture();
    let items = fixture.provider.list().expect("list");
    let detail = fixture
        .provider
        .detail(by_name(&items, "com.example.running"))
        .expect("detail");
    let rows: BTreeMap<&str, &str> = detail
        .rows
        .iter()
        .map(|row| (row.key.as_str(), row.value.as_str()))
        .collect();
    for (key, value) in [
        ("Command", "/bin/echo hi"),
        ("Runs", "When loaded, kept alive on conditions, every 60 s"),
        ("A", "1"),
        ("B", "2"),
        ("Process ID", "409"),
    ] {
        assert_eq!(rows[key], value, "row {key}");
    }
    assert_eq!(detail.logs.len(), 1, "both paths are the same");
    assert_eq!(detail.logs[0].path, PathBuf::from("/tmp/x.log"));

    let missing = item("com.example.missing", "/nope/missing.plist");
    let error = fixture.provider.detail(&missing).expect_err("missing file");
    assert!(error.message.contains("not found"), "{error}");
}

#[test]
fn actions_matrix() {
    let fixture = fixture();
    let user = &fixture.provider;
    let system = LaunchdProvider::new(
        Config {
            source: Source::SystemAgent,
            dir: PathBuf::from("/x"),
            domain: "gui/501".into(),
            elevated: false,
        },
        fixture.provider.ctx(),
    );
    let cases: Vec<(&LaunchdProvider, Item, Vec<Action>)> = vec![
        (
            user,
            Item {
                state: State::Running,
                autostart: Autostart::On,
                ..item("x", "/x/x.plist")
            },
            vec![
                Action::Stop,
                Action::Restart,
                Action::Unload,
                Action::Disable,
                Action::Remove,
            ],
        ),
        (
            user,
            Item {
                state: State::Loaded,
                autostart: Autostart::On,
                ..item("x", "/x/x.plist")
            },
            vec![
                Action::Start,
                Action::Unload,
                Action::Disable,
                Action::Remove,
            ],
        ),
        (
            user,
            Item {
                state: State::Failed,
                autostart: Autostart::On,
                ..item("x", "/x/x.plist")
            },
            vec![
                Action::Start,
                Action::Unload,
                Action::Disable,
                Action::Remove,
            ],
        ),
        (
            &system,
            Item {
                state: State::Stopped,
                autostart: Autostart::Off,
                ..item("x", "/x/x.plist")
            },
            vec![Action::Load, Action::Enable, Action::Remove],
        ),
        (
            user,
            Item {
                state: State::Running,
                autostart: Autostart::On,
                keep_alive: true,
                ..item("x", "/x/x.plist")
            },
            vec![
                Action::Stop,
                Action::Restart,
                Action::Disable,
                Action::Remove,
            ],
        ),
        (
            user,
            Item {
                state: State::Loaded,
                autostart: Autostart::On,
                keep_alive: true,
                ..item("x", "/x/x.plist")
            },
            vec![
                Action::Start,
                Action::Unload,
                Action::Disable,
                Action::Remove,
            ],
        ),
        (
            user,
            Item {
                state: State::Invalid,
                ..item("x", "/x/x.plist")
            },
            vec![Action::Remove],
        ),
        (
            &system,
            Item {
                state: State::Invalid,
                ..item("x", "/x/x.plist")
            },
            Vec::new(),
        ),
    ];
    for (provider, target, want) in cases {
        assert_eq!(provider.actions(&target), want, "{:?}", target.state);
    }
}

#[test]
fn do_runs_the_exact_launchctl_arguments() {
    let fixture = fixture();
    let daemon_dir = TempDir::new();
    let user = LaunchdProvider::new(
        Config {
            source: Source::UserAgent,
            dir: PathBuf::from("/x"),
            domain: "gui/501".into(),
            elevated: false,
        },
        fixture.provider.ctx(),
    );
    let daemon = LaunchdProvider::new(
        Config {
            source: Source::Daemon,
            dir: daemon_dir.path().to_path_buf(),
            domain: "system".into(),
            elevated: true,
        },
        fixture.provider.ctx(),
    );
    let base = item("lbl", "/x/lbl.plist");

    let error = daemon
        .do_action(&base, Action::Unload)
        .expect_err("no helper");
    assert_eq!(error.code, ErrorCode::HelperNotInstalled);
    fixture.helper.install();

    struct Case {
        provider: &'static str,
        which: &'static str,
        action: Action,
        want_user: Vec<Vec<String>>,
        want_helper: Vec<Vec<String>>,
    }
    let daemon_path = daemon_dir.path().join("lbl.plist").display().to_string();
    let daemon_path = daemon_path.as_str();
    let cases = vec![
        Case {
            provider: "user",
            which: "base",
            action: Action::Load,
            want_user: vec![call(&["launchctl", "bootstrap", "gui/501", "/x/lbl.plist"])],
            want_helper: vec![],
        },
        Case {
            provider: "user",
            which: "off",
            action: Action::Load,
            want_user: vec![
                call(&["launchctl", "enable", "gui/501/lbl"]),
                call(&["launchctl", "bootstrap", "gui/501", "/x/lbl.plist"]),
            ],
            want_helper: vec![],
        },
        Case {
            provider: "user",
            which: "base",
            action: Action::Unload,
            want_user: vec![call(&["launchctl", "bootout", "gui/501/lbl"])],
            want_helper: vec![],
        },
        Case {
            provider: "user",
            which: "base",
            action: Action::Start,
            want_user: vec![call(&["launchctl", "kickstart", "gui/501/lbl"])],
            want_helper: vec![],
        },
        Case {
            provider: "user",
            which: "base",
            action: Action::Restart,
            want_user: vec![call(&["launchctl", "kickstart", "-k", "gui/501/lbl"])],
            want_helper: vec![],
        },
        Case {
            provider: "user",
            which: "base",
            action: Action::Stop,
            want_user: vec![call(&["launchctl", "kill", "SIGTERM", "gui/501/lbl"])],
            want_helper: vec![],
        },
        Case {
            provider: "user",
            which: "kept",
            action: Action::Stop,
            want_user: vec![call(&["launchctl", "bootout", "gui/501/lbl"])],
            want_helper: vec![],
        },
        Case {
            provider: "user",
            which: "base",
            action: Action::Disable,
            want_user: vec![call(&["launchctl", "disable", "gui/501/lbl"])],
            want_helper: vec![],
        },
        Case {
            provider: "daemon",
            which: "base",
            action: Action::Unload,
            want_user: vec![],
            want_helper: vec![call(&["/bin/launchctl", "bootout", "system/lbl"])],
        },
        Case {
            provider: "daemon",
            which: "base",
            action: Action::Stop,
            want_user: vec![],
            want_helper: vec![call(&["/bin/launchctl", "kill", "SIGTERM", "system/lbl"])],
        },
        Case {
            provider: "daemon",
            which: "kept",
            action: Action::Stop,
            want_user: vec![],
            want_helper: vec![call(&["/bin/launchctl", "bootout", "system/lbl"])],
        },
        Case {
            provider: "daemon",
            which: "off",
            action: Action::Load,
            want_user: vec![],
            want_helper: vec![
                call(&["/bin/launchctl", "enable", "system/lbl"]),
                call(&["/bin/launchctl", "bootstrap", "system", daemon_path]),
            ],
        },
    ];
    for case in cases {
        fixture.runner.take_calls();
        fixture.helper.take_calls();
        let (provider, mut target) = if case.provider == "user" {
            (&user, base.clone())
        } else {
            (&daemon, base.clone())
        };
        target = match case.which {
            "off" => Item {
                autostart: Autostart::Off,
                ..target
            },
            "kept" => Item {
                state: State::Running,
                keep_alive: true,
                ..target
            },
            _ => target,
        };
        if case.provider == "daemon" {
            target.path = daemon_dir.path().join("lbl.plist");
        }
        provider
            .do_action(&target, case.action)
            .unwrap_or_else(|error| panic!("{:?}: {error}", case.action));
        assert_eq!(fixture.runner.calls(), case.want_user, "{:?}", case.action);
        assert_eq!(
            fixture.helper.calls(),
            case.want_helper,
            "{:?}",
            case.action
        );
    }
}

#[test]
fn do_all_asks_the_helper_once() {
    let fixture = fixture();
    let daemon_dir = TempDir::new();
    fixture.helper.install();
    let daemon = LaunchdProvider::new(
        Config {
            source: Source::Daemon,
            dir: daemon_dir.path().to_path_buf(),
            domain: "system".into(),
            elevated: true,
        },
        fixture.provider.ctx(),
    );
    daemon
        .do_all(
            &[item("a", "/x/a.plist"), item("b", "/x/b.plist")],
            Action::Start,
        )
        .expect("do all");
    assert_eq!(
        fixture.helper.calls(),
        vec![
            vec!["/bin/launchctl", "kickstart", "system/a"],
            vec!["/bin/launchctl", "kickstart", "system/b"],
        ]
    );
    assert_eq!(fixture.helper.control_requests().len(), 1);
}

#[test]
fn remove_moves_a_user_agent_to_the_trash_after_best_effort_unload() {
    let dir = fixture_dir();
    let home = TempDir::new();
    std::fs::create_dir_all(home.path().join(".Trash")).expect("trash");
    let runner = FakeRunner::new(PRINT_OUT);
    let host = FakeHost::new(home.path(), dir.path());
    let helper = FakeHelper::missing();
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    let provider = LaunchdProvider::new(
        Config {
            source: Source::UserAgent,
            dir: dir.path().to_path_buf(),
            domain: "gui/501".into(),
            elevated: false,
        },
        ctx,
    );
    let path = dir.path().join("running.plist");
    let target = Item {
        state: State::Running,
        ..item("com.example.running", &path.to_string_lossy())
    };
    provider.do_action(&target, Action::Remove).expect("remove");
    assert!(!path.exists());
    assert!(home.path().join(".Trash/running.plist").exists());
    assert_eq!(
        runner.calls(),
        vec![vec!["launchctl", "bootout", "gui/501/com.example.running"]]
    );
    assert_eq!(
        host.trashed(),
        vec![home.path().join(".Trash/running.plist")]
    );
}

#[test]
fn an_invalid_file_is_only_trashed() {
    let dir = fixture_dir();
    let home = TempDir::new();
    std::fs::create_dir_all(home.path().join(".Trash")).expect("trash");
    let runner = FakeRunner::new(PRINT_OUT);
    let host = FakeHost::new(home.path(), dir.path());
    let helper = FakeHelper::missing();
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    let provider = LaunchdProvider::new(
        Config {
            source: Source::UserAgent,
            dir: dir.path().to_path_buf(),
            domain: "gui/501".into(),
            elevated: false,
        },
        Arc::clone(&ctx),
    );
    let path = dir.path().join("broken.plist");
    let invalid = provider.invalid(&path);
    for action in [
        Action::Start,
        Action::Stop,
        Action::Load,
        Action::Unload,
        Action::Disable,
    ] {
        assert!(
            provider.do_action(&invalid, action).is_err(),
            "{action:?} ran on an invalid file"
        );
    }
    assert!(
        provider
            .do_all(std::slice::from_ref(&invalid), Action::Start)
            .is_err()
    );
    provider
        .do_action(&invalid, Action::Remove)
        .expect("remove invalid user file");
    assert!(
        runner.calls().is_empty(),
        "launchctl ran for an invalid file"
    );
    assert!(home.path().join(".Trash/broken.plist").exists());

    let daemon = LaunchdProvider::new(
        Config {
            source: Source::Daemon,
            dir: dir.path().to_path_buf(),
            domain: "system".into(),
            elevated: true,
        },
        ctx,
    );
    assert!(
        daemon
            .do_action(
                &daemon.invalid(&dir.path().join("nolabel.plist")),
                Action::Remove
            )
            .is_err(),
        "an invalid daemon file was removed as root"
    );
}

#[test]
fn confirmations_match_the_go_strings() {
    let fixture = fixture();
    let user = &fixture.provider;
    let daemon = LaunchdProvider::new(
        Config {
            source: Source::Daemon,
            dir: PathBuf::from("/x"),
            domain: "system".into(),
            elevated: true,
        },
        fixture.provider.ctx(),
    );
    let system = LaunchdProvider::new(
        Config {
            source: Source::SystemAgent,
            dir: PathBuf::from("/x"),
            domain: "gui/501".into(),
            elevated: false,
        },
        fixture.provider.ctx(),
    );
    let target = item("x", "/a/x.plist");

    assert!(user.confirm(&target, Action::Remove).is_some());
    assert!(daemon.confirm(&target, Action::Unload).is_some());
    assert!(daemon.confirm(&target, Action::Disable).is_some());

    for provider in [&daemon, &system] {
        let message = provider.confirm(&target, Action::Remove).expect("message");
        for want in [
            "may stop working",
            "not an undo",
            "Put Back cannot return it to the a folder",
            "reinstall",
        ] {
            assert!(message.contains(want), "{message:?} lacks {want:?}");
        }
        assert!(!message.contains("password"));
    }
    assert!(
        !user
            .confirm(&target, Action::Remove)
            .unwrap()
            .contains("password")
    );
    assert_eq!(user.confirm(&target, Action::Stop), None);
    assert_eq!(user.confirm(&target, Action::Unload), None);
    assert_eq!(daemon.confirm(&target, Action::Start), None);

    let invalid = Item {
        state: State::Invalid,
        ..target.clone()
    };
    assert_eq!(
        user.confirm(&invalid, Action::Remove).as_deref(),
        Some("Move x.plist to the Trash? launchd cannot load it as it is.")
    );
}

#[test]
fn scan_lists_duplicates_and_ignores_other_names_and_extensions() {
    let dir = TempDir::new();
    write_file(dir.path(), "a.plist", XML_PLIST.as_bytes());
    write_file(dir.path(), "b.plist", XML_PLIST.as_bytes());
    write_file(dir.path(), "upper.PLIST", XML_PLIST.as_bytes());
    write_file(dir.path(), "readme.txt", b"ignored");
    std::fs::create_dir_all(dir.path().join("folder.plist")).expect("directory");
    let runner = FakeRunner::new(PRINT_OUT);
    let host = FakeHost::new(dir.path(), dir.path());
    let helper = FakeHelper::missing();
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    let provider = LaunchdProvider::new(
        Config {
            source: Source::UserAgent,
            dir: dir.path().to_path_buf(),
            domain: "gui/501".into(),
            elevated: false,
        },
        ctx,
    );
    let items = provider.list().expect("list");
    // Two files with the same label are both listed; uppercase is not a plist.
    assert_eq!(items.len(), 3, "{items:?}");
    assert_eq!(
        items
            .iter()
            .filter(|item| item.name == "com.example.running")
            .count(),
        2
    );
    let folder = by_name(&items, "folder");
    assert_eq!(folder.state, State::Invalid);
    assert!(folder.kind.as_deref() == Some(INVALID_KIND));
}

#[test]
fn a_missing_directory_lists_nothing_but_still_reads_state() {
    let dir = TempDir::new();
    let missing = dir.path().join("not-there");
    let runner = FakeRunner::new(PRINT_OUT);
    let host = FakeHost::new(dir.path(), dir.path());
    let helper = FakeHelper::missing();
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    let provider = LaunchdProvider::new(
        Config {
            source: Source::UserAgent,
            dir: missing,
            domain: "gui/501".into(),
            elevated: false,
        },
        ctx,
    );
    assert!(provider.list().expect("list").is_empty());
    assert_eq!(runner.calls(), vec![vec!["launchctl", "print", "gui/501"]]);
}

#[test]
fn invalid_detail_of_a_missing_file_says_not_found() {
    let fixture = fixture();
    let missing = item("x", "/nope/x.plist");
    let invalid = Item {
        state: State::Invalid,
        ..missing
    };
    let error = fixture
        .provider
        .detail(&invalid)
        .expect_err("missing invalid file");
    assert!(error.message.contains("plist file not found"), "{error}");
}

#[test]
fn do_all_does_not_remove() {
    let fixture = fixture();
    let error = fixture
        .provider
        .do_all(&[item("x", "/x/x.plist")], Action::Remove)
        .expect_err("remove is not a bulk action");
    assert_eq!(error.message, "launchd: unknown action \"remove\"");
    assert!(fixture.runner.calls().is_empty());
}

#[test]
fn the_facade_holds_the_three_launchd_sources_in_order() {
    let home = TempDir::new();
    let runner = FakeRunner::new(PRINT_OUT);
    let host = FakeHost::new(home.path(), home.path());
    let helper = FakeHelper::missing();
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    let launchd = Launchd::new(ctx);
    let sources: Vec<Source> = launchd.providers().map(|(source, _)| *source).collect();
    assert_eq!(
        sources,
        vec![Source::UserAgent, Source::SystemAgent, Source::Daemon]
    );
    let user = launchd.provider(Source::UserAgent).expect("user agents");
    assert_eq!(user.config().dir, home.path().join("Library/LaunchAgents"));
    assert_eq!(user.config().domain, "gui/501");
    assert_eq!(
        launchd.provider(Source::Daemon).unwrap().config().domain,
        "system"
    );
    assert!(launchd.provider(Source::Brew).is_none());
}

#[test]
fn type_mismatches_are_invalid_but_unknown_keys_are_fine() {
    let dir = TempDir::new();
    write_file(
        dir.path(),
        "badlabel.plist",
        br#"<?xml version="1.0"?><plist version="1.0"><dict><key>Label</key><integer>5</integer></dict></plist>"#,
    );
    write_file(
        dir.path(),
        "badenv.plist",
        br#"<?xml version="1.0"?><plist version="1.0"><dict><key>Label</key><string>com.example.badenv</string><key>EnvironmentVariables</key><dict><key>A</key><integer>1</integer></dict></dict></plist>"#,
    );
    write_file(
        dir.path(),
        "badargs.plist",
        br#"<?xml version="1.0"?><plist version="1.0"><dict><key>Label</key><string>com.example.badargs</string><key>ProgramArguments</key><array><string>/bin/echo</string><integer>2</integer></array></dict></plist>"#,
    );
    write_file(
        dir.path(),
        "unknown.plist",
        br#"<?xml version="1.0"?><plist version="1.0"><dict><key>Label</key><string>com.example.unknown</string><key>Weird</key><dict><key>Nested</key><array><integer>1</integer><string>x</string></array></dict></dict></plist>"#,
    );
    let runner = FakeRunner::new(PRINT_OUT);
    let host = FakeHost::new(dir.path(), dir.path());
    let helper = FakeHelper::missing();
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    let provider = LaunchdProvider::new(
        Config {
            source: Source::UserAgent,
            dir: dir.path().to_path_buf(),
            domain: "gui/501".into(),
            elevated: false,
        },
        ctx,
    );
    let items = provider.list().expect("list");
    assert_eq!(items.len(), 4, "{items:?}");
    for name in ["badlabel", "badenv", "badargs"] {
        assert_eq!(by_name(&items, name).state, State::Invalid, "{name}");
    }
    let unknown = by_name(&items, "com.example.unknown");
    assert_eq!(unknown.state, State::Stopped);
    assert_eq!(unknown.kind, None);
}

#[test]
fn command_arguments_replace_program_and_empty_arguments_are_skipped() {
    let dir = TempDir::new();
    write_file(
        dir.path(),
        "a.plist",
        br#"<?xml version="1.0"?><plist version="1.0"><dict><key>Label</key><string>com.example.command</string><key>Program</key><string>/opt/a</string><key>ProgramArguments</key><array><string>/bin/b</string><string>x</string></array></dict></plist>"#,
    );
    write_file(
        dir.path(),
        "b.plist",
        br#"<?xml version="1.0"?><plist version="1.0"><dict><key>Label</key><string>com.example.empty</string><key>Program</key><string>/opt/a</string><key>ProgramArguments</key><array></array></dict></plist>"#,
    );
    let runner = FakeRunner::new(PRINT_OUT);
    let host = FakeHost::new(dir.path(), dir.path());
    let helper = FakeHelper::missing();
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    let provider = LaunchdProvider::new(
        Config {
            source: Source::UserAgent,
            dir: dir.path().to_path_buf(),
            domain: "gui/501".into(),
            elevated: false,
        },
        ctx,
    );
    let items = provider.list().expect("list");
    let command = provider
        .detail(by_name(&items, "com.example.command"))
        .expect("detail");
    let command: BTreeMap<&str, &str> = command
        .rows
        .iter()
        .map(|row| (row.key.as_str(), row.value.as_str()))
        .collect();
    assert_eq!(command["Command"], "/bin/b x");

    let empty = provider
        .detail(by_name(&items, "com.example.empty"))
        .expect("detail");
    assert!(
        empty.rows.iter().all(|row| row.key != "Command"),
        "{empty:?}"
    );
}

#[test]
fn detail_shows_a_negative_last_exit() {
    let dir = TempDir::new();
    write_file(
        dir.path(),
        "signaled.plist",
        br#"<?xml version="1.0"?><plist version="1.0"><dict><key>Label</key><string>com.example.signaled</string><key>ProgramArguments</key><array><string>/bin/date</string></array></dict></plist>"#,
    );
    let runner = FakeRunner::new(PRINT_OUT);
    let host = FakeHost::new(dir.path(), dir.path());
    let helper = FakeHelper::missing();
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    let provider = LaunchdProvider::new(
        Config {
            source: Source::UserAgent,
            dir: dir.path().to_path_buf(),
            domain: "gui/501".into(),
            elevated: false,
        },
        ctx,
    );
    let items = provider.list().expect("list");
    let signaled = by_name(&items, "com.example.signaled");
    assert_eq!(signaled.state, State::Loaded);
    let detail = provider.detail(signaled).expect("detail");
    let rows: BTreeMap<&str, &str> = detail
        .rows
        .iter()
        .map(|row| (row.key.as_str(), row.value.as_str()))
        .collect();
    assert_eq!(rows["Last exit code"], "-15");
}

#[test]
fn works_behind_the_service_provider_trait() {
    let fixture = fixture();
    let provider: Arc<dyn crate::service::Provider> = Arc::new(LaunchdProvider::new(
        Config {
            source: Source::UserAgent,
            dir: fixture.provider.config().dir.clone(),
            domain: "gui/501".into(),
            elevated: false,
        },
        fixture.provider.ctx(),
    ));
    assert_eq!(provider.source(), Source::UserAgent);
    let items = provider.list().expect("list through the trait");
    assert_eq!(items.len(), 4);
    let running = items
        .iter()
        .find(|item| item.name == "com.example.running")
        .expect("running item");
    let detail = provider.detail(running).expect("detail through the trait");
    assert!(detail.rows.iter().any(|row| row.key == "Command"));
    assert!(provider.do_action(running, Action::Stop).is_ok());
}

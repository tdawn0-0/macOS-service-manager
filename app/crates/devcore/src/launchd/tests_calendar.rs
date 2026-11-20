use std::sync::Arc;
use super::test_support::{FakeHelper, FakeHost, FakeRunner, TempDir, context, write_file};
use super::*;
use crate::service::Source;
#[test]
fn calendar_entries_expand_like_cron() {
    struct Case {
        name: &'static str,
        calendar: Calendar,
        want: usize,
        key: Option<&'static str>,
        values: Option<Vec<i32>>,
    }
    let cases = vec![
        Case {
            name: "none",
            calendar: Calendar::default(),
            want: 0,
            key: None,
            values: None,
        },
        Case {
            name: "a number",
            calendar: Calendar {
                minute: "5".into(),
                ..Calendar::default()
            },
            want: 1,
            key: Some("Minute"),
            values: Some(vec![5]),
        },
        Case {
            name: "a list",
            calendar: Calendar {
                hour: "1, 3,5".into(),
                ..Calendar::default()
            },
            want: 3,
            key: Some("Hour"),
            values: Some(vec![1, 3, 5]),
        },
        Case {
            name: "a range",
            calendar: Calendar {
                weekday: "1-5".into(),
                ..Calendar::default()
            },
            want: 5,
            key: Some("Weekday"),
            values: Some(vec![1, 2, 3, 4, 5]),
        },
        Case {
            name: "an interval from the start",
            calendar: Calendar {
                minute: "*/15".into(),
                ..Calendar::default()
            },
            want: 4,
            key: Some("Minute"),
            values: Some(vec![0, 15, 30, 45]),
        },
        Case {
            name: "an interval from day 1",
            calendar: Calendar {
                day: "*/10".into(),
                ..Calendar::default()
            },
            want: 4,
            key: Some("Day"),
            values: Some(vec![1, 11, 21, 31]),
        },
        Case {
            name: "duplicates once",
            calendar: Calendar {
                hour: "1,1-2".into(),
                ..Calendar::default()
            },
            want: 2,
            key: Some("Hour"),
            values: Some(vec![1, 2]),
        },
        Case {
            name: "a product",
            calendar: Calendar {
                minute: "0,30".into(),
                hour: "9-17".into(),
                ..Calendar::default()
            },
            want: 18,
            key: None,
            values: None,
        },
        Case {
            name: "star leaves the field out",
            calendar: Calendar {
                minute: "0".into(),
                hour: "*".into(),
                ..Calendar::default()
            },
            want: 1,
            key: Some("Minute"),
            values: Some(vec![0]),
        },
        Case {
            name: "every minute",
            calendar: Calendar {
                minute: "*".into(),
                ..Calendar::default()
            },
            want: 60,
            key: Some("Minute"),
            values: None,
        },
        Case {
            name: "every minute by interval",
            calendar: Calendar {
                hour: "*/1".into(),
                ..Calendar::default()
            },
            want: 60,
            key: Some("Minute"),
            values: None,
        },
        Case {
            name: "sunday as 7",
            calendar: Calendar {
                weekday: "7".into(),
                ..Calendar::default()
            },
            want: 1,
            key: Some("Weekday"),
            values: Some(vec![7]),
        },
    ];
    for case in cases {
        let entries = case.calendar.entries().expect(case.name);
        assert_eq!(entries.len(), case.want, "{}: {entries:?}", case.name);
        if let (Some(key), Some(want)) = (case.key, case.values) {
            let got: Vec<i32> = entries.iter().map(|entry| entry[key]).collect();
            assert_eq!(got, want, "{}", case.name);
        }
    }
}

#[test]
fn calendar_rejects_bad_fields() {
    let cases: Vec<(&str, Calendar)> = vec![
        (
            "outside 0-59",
            Calendar {
                minute: "60".into(),
                ..Calendar::default()
            },
        ),
        (
            "outside 0-23",
            Calendar {
                hour: "24".into(),
                ..Calendar::default()
            },
        ),
        (
            "outside 1-31",
            Calendar {
                day: "0".into(),
                ..Calendar::default()
            },
        ),
        (
            "outside 1-12",
            Calendar {
                month: "13".into(),
                ..Calendar::default()
            },
        ),
        (
            "outside 0-7",
            Calendar {
                weekday: "8".into(),
                ..Calendar::default()
            },
        ),
        (
            "goes backwards",
            Calendar {
                hour: "5-1".into(),
                ..Calendar::default()
            },
        ),
        (
            "is not a number",
            Calendar {
                minute: "abc".into(),
                ..Calendar::default()
            },
        ),
        (
            "not a positive",
            Calendar {
                minute: "*/0".into(),
                ..Calendar::default()
            },
        ),
        (
            "start times",
            Calendar {
                minute: "0-59".into(),
                hour: "0-23".into(),
                ..Calendar::default()
            },
        ),
    ];
    for (want, calendar) in cases {
        let error = calendar.entries().expect_err(want);
        assert!(
            error.message.contains(want),
            "{calendar:?}: {} wants {want:?}",
            error.message
        );
    }
}
use super::test_support::{FakeHelper, FakeHost, FakeRunner, TempDir, context, write_file};
use super::*;
use crate::service::Source;

#[test]
fn calendars_round_trip_through_a_file() {
    let runner = FakeRunner::new("");
    let dir = TempDir::new();
    let calendars = [
        (
            "single",
            Calendar {
                minute: "30".into(),
                hour: "2".into(),
                ..Calendar::default()
            },
        ),
        (
            "interval",
            Calendar {
                minute: "*/15".into(),
                ..Calendar::default()
            },
        ),
        (
            "day-interval",
            Calendar {
                day: "*/10".into(),
                ..Calendar::default()
            },
        ),
        (
            "product",
            Calendar {
                minute: "0,30".into(),
                hour: "9-17".into(),
                weekday: "1-5".into(),
                ..Calendar::default()
            },
        ),
        (
            "sparse",
            Calendar {
                day: "1,15".into(),
                month: "1,7".into(),
                ..Calendar::default()
            },
        ),
    ];
    for (name, calendar) in calendars {
        let spec = Spec {
            label: format!("com.example.{name}"),
            args: shell_args("date"),
            calendar: calendar.clone(),
            ..Spec::default()
        };
        let path = save_file(dir.path(), &spec, None, &*runner).expect("save");
        let loaded = Spec::load(&path).expect("load");
        assert!(!loaded.calendar_custom, "{name}");
        assert_eq!(loaded.calendar, calendar, "{name}");
        assert_eq!(
            shell_command(&loaded.args),
            Some("date"),
            "{name}: {:?}",
            loaded.args
        );
    }

    // Two different times are not all combinations of their values.
    let custom = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>Label</key><string>com.example.custom</string>
<key>ProgramArguments</key><array><string>/bin/date</string></array>
<key>StartCalendarInterval</key><array>
<dict><key>Hour</key><integer>9</integer><key>Minute</key><integer>0</integer></dict>
<dict><key>Hour</key><integer>17</integer><key>Minute</key><integer>30</integer></dict>
</array>
</dict></plist>"#;
    let path = write_file(dir.path(), "custom.plist", custom.as_bytes());
    let spec = Spec::load(&path).expect("load custom");
    assert!(spec.calendar_custom, "{spec:?}");
    save_file(dir.path(), &spec, Some(&path), &*runner).expect("save custom");
    let data = std::fs::read_to_string(&path).expect("read");
    assert!(data.contains("<integer>17</integer>"), "{data}");
    assert!(data.contains("<integer>30</integer>"), "{data}");
}

#[test]
fn service_options_round_trip_and_runs_says_so() {
    let runner = FakeRunner::new("");
    let dir = TempDir::new();
    let spec = Spec {
        label: "com.example.svc".into(),
        args: shell_args("date"),
        restart_on_crash: true,
        start_on_mount: true,
        queue_dirs: vec!["/tmp/a".into(), "/tmp/b".into()],
        user_name: "me".into(),
        group_name: "staff".into(),
        root_dir: "/tmp/root".into(),
        ..Spec::default()
    };
    let path = save_file(dir.path(), &spec, None, &*runner).expect("save");
    let data = std::fs::read_to_string(&path).expect("read");
    assert!(data.contains("<key>SuccessfulExit</key>"), "{data}");

    let loaded = Spec::load(&path).expect("load");
    assert!(loaded.restart_on_crash);
    assert!(!loaded.keep_alive);
    assert!(!loaded.keep_alive_custom);
    assert!(loaded.run_at_load);
    assert!(loaded.start_on_mount);
    assert_eq!(loaded.queue_dirs, spec.queue_dirs);
    assert_eq!(loaded.user_name, "me");
    assert_eq!(loaded.group_name, "staff");
    assert_eq!(loaded.root_dir, "/tmp/root");

    let file = plist_file::read_file(&path).expect("read file");
    assert_eq!(
        file.runs(),
        "When loaded, restarted after a crash, when a disk mounts, while a queue has files"
    );

    let bad: Vec<(&str, Spec)> = vec![
        (
            "queue directory",
            Spec {
                label: "a".into(),
                args: vec!["/bin/date".into()],
                queue_dirs: vec!["inbox".into()],
                ..Spec::default()
            },
        ),
        (
            "root directory",
            Spec {
                label: "a".into(),
                args: vec!["/bin/date".into()],
                root_dir: "jail".into(),
                ..Spec::default()
            },
        ),
        (
            "a group needs",
            Spec {
                label: "a".into(),
                args: vec!["/bin/date".into()],
                group_name: "staff".into(),
                ..Spec::default()
            },
        ),
        (
            "outside",
            Spec {
                label: "a".into(),
                args: vec!["/bin/date".into()],
                calendar: Calendar {
                    hour: "99".into(),
                    ..Calendar::default()
                },
                ..Spec::default()
            },
        ),
    ];
    for (want, spec) in bad {
        let error = spec.validate().expect_err(want);
        assert!(error.message.contains(want), "{want}: {error}");
    }
}

fn root_fixture() -> (TempDir, Arc<FakeRunner>, FakeHelper, LaunchdProvider) {
    let dir = TempDir::new();
    let runner = FakeRunner::new("");
    let helper = FakeHelper::missing();
    let host = FakeHost::new(dir.path(), dir.path());
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    let provider = LaunchdProvider::new(
        Config {
            source: Source::Daemon,
            dir: dir.path().to_path_buf(),
            domain: "system".into(),
            elevated: true,
        },
        ctx,
    );
    (dir, runner, helper, provider)
}

fn joined(call: &[String]) -> String {
    call.join(" ")
}

#[test]
fn a_daemon_installs_and_loads_under_one_authentication() {
    let dir = TempDir::new();
    let job_dir = dir.path().join("my jobs");
    std::fs::create_dir_all(&job_dir).expect("job dir");
    let runner = FakeRunner::new("");
    let helper = FakeHelper::installed();
    let host = FakeHost::new(dir.path(), dir.path());
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    let provider = LaunchdProvider::new(
        Config {
            source: Source::Daemon,
            dir: job_dir.clone(),
            domain: "system".into(),
            elevated: true,
        },
        ctx,
    );
    let outcome = provider
        .save_as_root(
            &Spec {
                label: "com.example.d".into(),
                args: shell_args("date"),
                user_name: "me".into(),
                group_name: "staff".into(),
                ..Spec::default()
            },
            None,
            true,
        )
        .expect("save");
    assert_eq!(outcome.path, job_dir.join("com.example.d.plist"));
    assert!(outcome.path.exists(), "the plist was not written");

    let want_boot = [
        "/bin/launchctl",
        "bootstrap",
        "system",
        outcome.path.to_str().unwrap(),
    ];
    assert!(
        helper.calls().iter().any(|call| call
            .iter()
            .map(String::as_str)
            .eq(want_boot.iter().copied())),
        "helper calls: {:?}",
        helper.calls()
    );
    let runs: Vec<String> = runner.calls().iter().map(|call| joined(call)).collect();
    for want in [
        "id -u me",
        "dscl . -read /Groups/staff PrimaryGroupID",
        "plutil -lint",
    ] {
        assert!(
            runs.iter().any(|run| run.starts_with(want)),
            "no {want:?} in {runs:?}"
        );
    }
}

#[test]
fn a_system_agent_loads_without_a_second_authentication() {
    let dir = TempDir::new();
    let runner = FakeRunner::new("");
    let helper = FakeHelper::installed();
    let host = FakeHost::new(dir.path(), dir.path());
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    let provider = LaunchdProvider::new(
        Config {
            source: Source::SystemAgent,
            dir: dir.path().to_path_buf(),
            domain: "gui/501".into(),
            elevated: false,
        },
        ctx,
    );
    let outcome = provider
        .save_as_root(
            &Spec {
                label: "com.example.a".into(),
                args: shell_args("date"),
                ..Spec::default()
            },
            None,
            true,
        )
        .expect("save");
    assert!(helper.calls().is_empty(), "{:?}", helper.calls());
    let runs: Vec<String> = runner.calls().iter().map(|call| joined(call)).collect();
    assert!(
        runs.contains(&format!(
            "launchctl bootstrap gui/501 {}",
            outcome.path.display()
        )),
        "{runs:?}"
    );
}

#[test]
fn save_as_root_checks_before_asking() {
    let (dir, runner, helper, provider) = {
        let dir = TempDir::new();
        write_file(dir.path(), "com.example.taken.plist", b"");
        let runner = FakeRunner::new("");
        let helper = FakeHelper::missing();
        let host = FakeHost::new(dir.path(), dir.path());
        let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
        let provider = LaunchdProvider::new(
            Config {
                source: Source::Daemon,
                dir: dir.path().to_path_buf(),
                domain: "system".into(),
                elevated: true,
            },
            ctx,
        );
        (dir, runner, helper, provider)
    };
    let _ = &dir;
    let error = provider
        .save_as_root(
            &Spec {
                label: "com.example.taken".into(),
                args: shell_args("date"),
                ..Spec::default()
            },
            None,
            false,
        )
        .expect_err("taken");
    assert!(error.message.contains("already exists"), "{error}");

    assert!(
        provider
            .save_as_root(
                &Spec {
                    label: "bad label".into(),
                    args: shell_args("date"),
                    ..Spec::default()
                },
                None,
                false
            )
            .is_err()
    );
    assert!(
        provider
            .save_as_root(
                &Spec {
                    label: "ok".into(),
                    args: shell_args("date"),
                    calendar: Calendar {
                        hour: "99".into(),
                        ..Calendar::default()
                    },
                    ..Spec::default()
                },
                None,
                false
            )
            .is_err()
    );

    runner.fail("id", "no such user");
    let error = provider
        .save_as_root(
            &Spec {
                label: "ok".into(),
                args: shell_args("date"),
                user_name: "nobody-here".into(),
                ..Spec::default()
            },
            None,
            false,
        )
        .expect_err("missing user");
    assert!(
        error.message.contains(r#"no user "nobody-here""#),
        "{error}"
    );

    let error = provider
        .save_as_root(
            &Spec {
                label: "com.example.ok".into(),
                args: shell_args("date"),
                ..Spec::default()
            },
            None,
            false,
        )
        .expect_err("no helper");
    assert_eq!(error.code, ErrorCode::HelperNotInstalled);
    assert!(helper.save_requests().is_empty());
}

#[test]
fn helper_save_requests_are_built_from_the_spec() {
    let (_dir, _runner, helper, provider) = root_fixture();
    helper.install();
    let outcome = provider
        .save_as_root(
            &Spec {
                label: "com.example.d".into(),
                args: shell_args("date"),
                ..Spec::default()
            },
            None,
            true,
        )
        .expect("save");
    let requests = helper.save_requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.path, outcome.path);
    assert_eq!(request.domain, "system");
    assert_eq!(request.target, "system/com.example.d");
    assert_eq!(request.existing_path, None);
    assert!(request.load);
}

#[test]
fn missing_temporary_directory_is_an_error_not_a_panic() {
    let dir = TempDir::new();
    let missing = dir.path().join("gone");
    let runner = FakeRunner::new("");
    let helper = FakeHelper::installed();
    let host = FakeHost::new(dir.path(), &missing);
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    let provider = LaunchdProvider::new(
        Config {
            source: Source::Daemon,
            dir: dir.path().to_path_buf(),
            domain: "system".into(),
            elevated: true,
        },
        ctx,
    );
    assert!(
        provider
            .save_as_root(
                &Spec {
                    label: "a".into(),
                    args: vec!["/bin/date".into()],
                    ..Spec::default()
                },
                None,
                false
            )
            .is_err()
    );
    assert!(helper.save_requests().is_empty());
}

#[test]
fn a_custom_calendar_round_trips_as_an_array() {
    let dir = TempDir::new();
    let path = dir.path().join("x.plist");
    write_file(
        dir.path(),
        "x.plist",
        br#"<?xml version="1.0"?><plist version="1.0"><dict><key>Label</key><string>a</string><key>ProgramArguments</key><array><string>/bin/date</string></array><key>StartCalendarInterval</key><array><dict><key>Minute</key><integer>0</integer></dict><dict><key>Minute</key><integer>30</integer></dict></array></dict></plist>"#,
    );
    let spec = Spec::load(&path).expect("load");
    assert!(!spec.calendar_custom);
    assert_eq!(spec.calendar.minute, "0,30");
}

#[test]
fn save_as_root_checks_the_group_and_the_directory() {
    let dir = TempDir::new();
    let runner = FakeRunner::new("");
    let helper = FakeHelper::installed();
    let host = FakeHost::new(dir.path(), dir.path());
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    let provider = LaunchdProvider::new(
        Config {
            source: Source::Daemon,
            dir: dir.path().to_path_buf(),
            domain: "system".into(),
            elevated: true,
        },
        ctx,
    );
    runner.fail("dscl", "no such group");
    let error = provider
        .save_as_root(
            &Spec {
                label: "com.example.g".into(),
                args: shell_args("date"),
                user_name: "me".into(),
                group_name: "staff".into(),
                ..Spec::default()
            },
            None,
            false,
        )
        .expect_err("missing group");
    assert!(error.message.contains(r#"no group "staff""#), "{error}");
    assert!(helper.save_requests().is_empty());

    let unconfigured = LaunchdProvider::new(
        Config {
            source: Source::Daemon,
            dir: std::path::PathBuf::new(),
            domain: "system".into(),
            elevated: true,
        },
        provider.ctx(),
    );
    let error = unconfigured
        .save_as_root(
            &Spec {
                label: "com.example.u".into(),
                args: shell_args("date"),
                ..Spec::default()
            },
            None,
            false,
        )
        .expect_err("unconfigured directory");
    assert!(
        error
            .message
            .contains("provider directory is not configured"),
        "{error}"
    );
}

#[test]
fn calendar_from_rejects_shapes_the_form_cannot_say() {
    use plist::{Dictionary, Value};

    fn entry(pairs: &[(&str, Value)]) -> Value {
        let mut dict = Dictionary::new();
        for (key, value) in pairs {
            dict.insert((*key).to_string(), value.clone());
        }
        Value::Dictionary(dict)
    }

    fn entries(values: Vec<Value>) -> Value {
        Value::Array(values)
    }

    let minute = |value: i64| Value::Integer(value.into());
    assert!(calendar_from(&entries(Vec::new())).is_none());
    assert!(calendar_from(&entry(&[])).is_none());
    assert!(
        calendar_from(&entries(vec![
            entry(&[("Minute", minute(0))]),
            entry(&[("Hour", minute(9))]),
        ]))
        .is_none(),
        "the entries must name the same fields"
    );
    assert!(
        calendar_from(&entries(vec![
            entry(&[("Minute", minute(0))]),
            entry(&[("Minute", minute(0))]),
        ]))
        .is_none(),
        "duplicates are not a product"
    );
    assert!(
        calendar_from(&entry(&[("Minute", minute(0)), ("Extra", minute(1))])).is_none(),
        "unknown keys are not a calendar"
    );
    assert!(
        calendar_from(&entry(&[("Minute", Value::String("0".into()))])).is_none(),
        "a string is not a plist integer"
    );
    assert!(
        calendar_from(&entry(&[("Minute", minute(60))])).is_none(),
        "outside the field range"
    );

    let product = calendar_from(&entries(vec![
        entry(&[("Minute", minute(0))]),
        entry(&[("Minute", minute(30))]),
    ]))
    .expect("a complete product");
    assert_eq!(product.minute, "0,30");
    let single =
        calendar_from(&entry(&[("Minute", minute(0)), ("Hour", minute(9))])).expect("one entry");
    assert_eq!(single.minute, "0");
    assert_eq!(single.hour, "9");
}

#[test]
fn calendar_entries_expand_like_cron() {
    struct Case {
        name: &'static str,
        calendar: Calendar,
        want: usize,
        key: Option<&'static str>,
        values: Option<Vec<i32>>,
    }
    let cases = vec![
        Case {
            name: "none",
            calendar: Calendar::default(),
            want: 0,
            key: None,
            values: None,
        },
        Case {
            name: "a number",
            calendar: Calendar {
                minute: "5".into(),
                ..Calendar::default()
            },
            want: 1,
            key: Some("Minute"),
            values: Some(vec![5]),
        },
        Case {
            name: "a list",
            calendar: Calendar {
                hour: "1, 3,5".into(),
                ..Calendar::default()
            },
            want: 3,
            key: Some("Hour"),
            values: Some(vec![1, 3, 5]),
        },
        Case {
            name: "a range",
            calendar: Calendar {
                weekday: "1-5".into(),
                ..Calendar::default()
            },
            want: 5,
            key: Some("Weekday"),
            values: Some(vec![1, 2, 3, 4, 5]),
        },
        Case {
            name: "an interval from the start",
            calendar: Calendar {
                minute: "*/15".into(),
                ..Calendar::default()
            },
            want: 4,
            key: Some("Minute"),
            values: Some(vec![0, 15, 30, 45]),
        },
        Case {
            name: "an interval from day 1",
            calendar: Calendar {
                day: "*/10".into(),
                ..Calendar::default()
            },
            want: 4,
            key: Some("Day"),
            values: Some(vec![1, 11, 21, 31]),
        },
        Case {
            name: "duplicates once",
            calendar: Calendar {
                hour: "1,1-2".into(),
                ..Calendar::default()
            },
            want: 2,
            key: Some("Hour"),
            values: Some(vec![1, 2]),
        },
        Case {
            name: "a product",
            calendar: Calendar {
                minute: "0,30".into(),
                hour: "9-17".into(),
                ..Calendar::default()
            },
            want: 18,
            key: None,
            values: None,
        },
        Case {
            name: "star leaves the field out",
            calendar: Calendar {
                minute: "0".into(),
                hour: "*".into(),
                ..Calendar::default()
            },
            want: 1,
            key: Some("Minute"),
            values: Some(vec![0]),
        },
        Case {
            name: "every minute",
            calendar: Calendar {
                minute: "*".into(),
                ..Calendar::default()
            },
            want: 60,
            key: Some("Minute"),
            values: None,
        },
        Case {
            name: "every minute by interval",
            calendar: Calendar {
                hour: "*/1".into(),
                ..Calendar::default()
            },
            want: 60,
            key: Some("Minute"),
            values: None,
        },
        Case {
            name: "sunday as 7",
            calendar: Calendar {
                weekday: "7".into(),
                ..Calendar::default()
            },
            want: 1,
            key: Some("Weekday"),
            values: Some(vec![7]),
        },
    ];
    for case in cases {
        let entries = case.calendar.entries().expect(case.name);
        assert_eq!(entries.len(), case.want, "{}: {entries:?}", case.name);
        if let (Some(key), Some(want)) = (case.key, case.values) {
            let got: Vec<i32> = entries.iter().map(|entry| entry[key]).collect();
            assert_eq!(got, want, "{}", case.name);
        }
    }
}

#[test]
fn calendar_rejects_bad_fields() {
    let cases: Vec<(&str, Calendar)> = vec![
        (
            "outside 0-59",
            Calendar {
                minute: "60".into(),
                ..Calendar::default()
            },
        ),
        (
            "outside 0-23",
            Calendar {
                hour: "24".into(),
                ..Calendar::default()
            },
        ),
        (
            "outside 1-31",
            Calendar {
                day: "0".into(),
                ..Calendar::default()
            },
        ),
        (
            "outside 1-12",
            Calendar {
                month: "13".into(),
                ..Calendar::default()
            },
        ),
        (
            "outside 0-7",
            Calendar {
                weekday: "8".into(),
                ..Calendar::default()
            },
        ),
        (
            "goes backwards",
            Calendar {
                hour: "5-1".into(),
                ..Calendar::default()
            },
        ),
        (
            "is not a number",
            Calendar {
                minute: "abc".into(),
                ..Calendar::default()
            },
        ),
        (
            "not a positive",
            Calendar {
                minute: "*/0".into(),
                ..Calendar::default()
            },
        ),
        (
            "start times",
            Calendar {
                minute: "0-59".into(),
                hour: "0-23".into(),
                ..Calendar::default()
            },
        ),
    ];
    for (want, calendar) in cases {
        let error = calendar.entries().expect_err(want);
        assert!(
            error.message.contains(want),
            "{calendar:?}: {} wants {want:?}",
            error.message
        );
    }
}

#[test]
fn calendars_round_trip_through_a_file() {
    let runner = FakeRunner::new("");
    let dir = TempDir::new();
    let calendars = [
        (
            "single",
            Calendar {
                minute: "30".into(),
                hour: "2".into(),
                ..Calendar::default()
            },
        ),
        (
            "interval",
            Calendar {
                minute: "*/15".into(),
                ..Calendar::default()
            },
        ),
        (
            "day-interval",
            Calendar {
                day: "*/10".into(),
                ..Calendar::default()
            },
        ),
        (
            "product",
            Calendar {
                minute: "0,30".into(),
                hour: "9-17".into(),
                weekday: "1-5".into(),
                ..Calendar::default()
            },
        ),
        (
            "sparse",
            Calendar {
                day: "1,15".into(),
                month: "1,7".into(),
                ..Calendar::default()
            },
        ),
    ];
    for (name, calendar) in calendars {
        let spec = Spec {
            label: format!("com.example.{name}"),
            args: shell_args("date"),
            calendar: calendar.clone(),
            ..Spec::default()
        };
        let path = save_file(dir.path(), &spec, None, &*runner).expect("save");
        let loaded = Spec::load(&path).expect("load");
        assert!(!loaded.calendar_custom, "{name}");
        assert_eq!(loaded.calendar, calendar, "{name}");
        assert_eq!(
            shell_command(&loaded.args),
            Some("date"),
            "{name}: {:?}",
            loaded.args
        );
    }

    // Two different times are not all combinations of their values.
    let custom = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>Label</key><string>com.example.custom</string>
<key>ProgramArguments</key><array><string>/bin/date</string></array>
<key>StartCalendarInterval</key><array>
<dict><key>Hour</key><integer>9</integer><key>Minute</key><integer>0</integer></dict>
<dict><key>Hour</key><integer>17</integer><key>Minute</key><integer>30</integer></dict>
</array>
</dict></plist>"#;
    let path = write_file(dir.path(), "custom.plist", custom.as_bytes());
    let spec = Spec::load(&path).expect("load custom");
    assert!(spec.calendar_custom, "{spec:?}");
    save_file(dir.path(), &spec, Some(&path), &*runner).expect("save custom");
    let data = std::fs::read_to_string(&path).expect("read");
    assert!(data.contains("<integer>17</integer>"), "{data}");
    assert!(data.contains("<integer>30</integer>"), "{data}");
}

#[test]
fn service_options_round_trip_and_runs_says_so() {
    let runner = FakeRunner::new("");
    let dir = TempDir::new();
    let spec = Spec {
        label: "com.example.svc".into(),
        args: shell_args("date"),
        restart_on_crash: true,
        start_on_mount: true,
        queue_dirs: vec!["/tmp/a".into(), "/tmp/b".into()],
        user_name: "me".into(),
        group_name: "staff".into(),
        root_dir: "/tmp/root".into(),
        ..Spec::default()
    };
    let path = save_file(dir.path(), &spec, None, &*runner).expect("save");
    let data = std::fs::read_to_string(&path).expect("read");
    assert!(data.contains("<key>SuccessfulExit</key>"), "{data}");

    let loaded = Spec::load(&path).expect("load");
    assert!(loaded.restart_on_crash);
    assert!(!loaded.keep_alive);
    assert!(!loaded.keep_alive_custom);
    assert!(loaded.run_at_load);
    assert!(loaded.start_on_mount);
    assert_eq!(loaded.queue_dirs, spec.queue_dirs);
    assert_eq!(loaded.user_name, "me");
    assert_eq!(loaded.group_name, "staff");
    assert_eq!(loaded.root_dir, "/tmp/root");

    let file = plist_file::read_file(&path).expect("read file");
    assert_eq!(
        file.runs(),
        "When loaded, restarted after a crash, when a disk mounts, while a queue has files"
    );

    let bad: Vec<(&str, Spec)> = vec![
        (
            "queue directory",
            Spec {
                label: "a".into(),
                args: vec!["/bin/date".into()],
                queue_dirs: vec!["inbox".into()],
                ..Spec::default()
            },
        ),
        (
            "root directory",
            Spec {
                label: "a".into(),
                args: vec!["/bin/date".into()],
                root_dir: "jail".into(),
                ..Spec::default()
            },
        ),
        (
            "a group needs",
            Spec {
                label: "a".into(),
                args: vec!["/bin/date".into()],
                group_name: "staff".into(),
                ..Spec::default()
            },
        ),
        (
            "outside",
            Spec {
                label: "a".into(),
                args: vec!["/bin/date".into()],
                calendar: Calendar {
                    hour: "99".into(),
                    ..Calendar::default()
                },
                ..Spec::default()
            },
        ),
    ];
    for (want, spec) in bad {
        let error = spec.validate().expect_err(want);
        assert!(error.message.contains(want), "{want}: {error}");
    }
}

fn root_fixture() -> (TempDir, Arc<FakeRunner>, FakeHelper, LaunchdProvider) {
    let dir = TempDir::new();
    let runner = FakeRunner::new("");
    let helper = FakeHelper::missing();
    let host = FakeHost::new(dir.path(), dir.path());
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    let provider = LaunchdProvider::new(
        Config {
            source: Source::Daemon,
            dir: dir.path().to_path_buf(),
            domain: "system".into(),
            elevated: true,
        },
        ctx,
    );
    (dir, runner, helper, provider)
}

fn joined(call: &[String]) -> String {
    call.join(" ")
}

#[test]
fn a_daemon_installs_and_loads_under_one_authentication() {
    let dir = TempDir::new();
    let job_dir = dir.path().join("my jobs");
    std::fs::create_dir_all(&job_dir).expect("job dir");
    let runner = FakeRunner::new("");
    let helper = FakeHelper::installed();
    let host = FakeHost::new(dir.path(), dir.path());
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    let provider = LaunchdProvider::new(
        Config {
            source: Source::Daemon,
            dir: job_dir.clone(),
            domain: "system".into(),
            elevated: true,
        },
        ctx,
    );
    let outcome = provider
        .save_as_root(
            &Spec {
                label: "com.example.d".into(),
                args: shell_args("date"),
                user_name: "me".into(),
                group_name: "staff".into(),
                ..Spec::default()
            },
            None,
            true,
        )
        .expect("save");
    assert_eq!(outcome.path, job_dir.join("com.example.d.plist"));
    assert!(outcome.path.exists(), "the plist was not written");

    let want_boot = [
        "/bin/launchctl",
        "bootstrap",
        "system",
        outcome.path.to_str().unwrap(),
    ];
    assert!(
        helper.calls().iter().any(|call| call
            .iter()
            .map(String::as_str)
            .eq(want_boot.iter().copied())),
        "helper calls: {:?}",
        helper.calls()
    );
    let runs: Vec<String> = runner.calls().iter().map(|call| joined(call)).collect();
    for want in [
        "id -u me",
        "dscl . -read /Groups/staff PrimaryGroupID",
        "plutil -lint",
    ] {
        assert!(
            runs.iter().any(|run| run.starts_with(want)),
            "no {want:?} in {runs:?}"
        );
    }
}

#[test]
fn a_system_agent_loads_without_a_second_authentication() {
    let dir = TempDir::new();
    let runner = FakeRunner::new("");
    let helper = FakeHelper::installed();
    let host = FakeHost::new(dir.path(), dir.path());
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    let provider = LaunchdProvider::new(
        Config {
            source: Source::SystemAgent,
            dir: dir.path().to_path_buf(),
            domain: "gui/501".into(),
            elevated: false,
        },
        ctx,
    );
    let outcome = provider
        .save_as_root(
            &Spec {
                label: "com.example.a".into(),
                args: shell_args("date"),
                ..Spec::default()
            },
            None,
            true,
        )
        .expect("save");
    assert!(helper.calls().is_empty(), "{:?}", helper.calls());
    let runs: Vec<String> = runner.calls().iter().map(|call| joined(call)).collect();
    assert!(
        runs.contains(&format!(
            "launchctl bootstrap gui/501 {}",
            outcome.path.display()
        )),
        "{runs:?}"
    );
}

#[test]
fn save_as_root_checks_before_asking() {
    let (dir, runner, helper, provider) = {
        let dir = TempDir::new();
        write_file(dir.path(), "com.example.taken.plist", b"");
        let runner = FakeRunner::new("");
        let helper = FakeHelper::missing();
        let host = FakeHost::new(dir.path(), dir.path());
        let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
        let provider = LaunchdProvider::new(
            Config {
                source: Source::Daemon,
                dir: dir.path().to_path_buf(),
                domain: "system".into(),
                elevated: true,
            },
            ctx,
        );
        (dir, runner, helper, provider)
    };
    let _ = &dir;
    let error = provider
        .save_as_root(
            &Spec {
                label: "com.example.taken".into(),
                args: shell_args("date"),
                ..Spec::default()
            },
            None,
            false,
        )
        .expect_err("taken");
    assert!(error.message.contains("already exists"), "{error}");

    assert!(
        provider
            .save_as_root(
                &Spec {
                    label: "bad label".into(),
                    args: shell_args("date"),
                    ..Spec::default()
                },
                None,
                false
            )
            .is_err()
    );
    assert!(
        provider
            .save_as_root(
                &Spec {
                    label: "ok".into(),
                    args: shell_args("date"),
                    calendar: Calendar {
                        hour: "99".into(),
                        ..Calendar::default()
                    },
                    ..Spec::default()
                },
                None,
                false
            )
            .is_err()
    );

    runner.fail("id", "no such user");
    let error = provider
        .save_as_root(
            &Spec {
                label: "ok".into(),
                args: shell_args("date"),
                user_name: "nobody-here".into(),
                ..Spec::default()
            },
            None,
            false,
        )
        .expect_err("missing user");
    assert!(
        error.message.contains(r#"no user "nobody-here""#),
        "{error}"
    );

    let error = provider
        .save_as_root(
            &Spec {
                label: "com.example.ok".into(),
                args: shell_args("date"),
                ..Spec::default()
            },
            None,
            false,
        )
        .expect_err("no helper");
    assert_eq!(error.code, ErrorCode::HelperNotInstalled);
    assert!(helper.save_requests().is_empty());
}

#[test]
fn helper_save_requests_are_built_from_the_spec() {
    let (_dir, _runner, helper, provider) = root_fixture();
    helper.install();
    let outcome = provider
        .save_as_root(
            &Spec {
                label: "com.example.d".into(),
                args: shell_args("date"),
                ..Spec::default()
            },
            None,
            true,
        )
        .expect("save");
    let requests = helper.save_requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.path, outcome.path);
    assert_eq!(request.domain, "system");
    assert_eq!(request.target, "system/com.example.d");
    assert_eq!(request.existing_path, None);
    assert!(request.load);
}

#[test]
fn missing_temporary_directory_is_an_error_not_a_panic() {
    let dir = TempDir::new();
    let missing = dir.path().join("gone");
    let runner = FakeRunner::new("");
    let helper = FakeHelper::installed();
    let host = FakeHost::new(dir.path(), &missing);
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    let provider = LaunchdProvider::new(
        Config {
            source: Source::Daemon,
            dir: dir.path().to_path_buf(),
            domain: "system".into(),
            elevated: true,
        },
        ctx,
    );
    assert!(
        provider
            .save_as_root(
                &Spec {
                    label: "a".into(),
                    args: vec!["/bin/date".into()],
                    ..Spec::default()
                },
                None,
                false
            )
            .is_err()
    );
    assert!(helper.save_requests().is_empty());
}

#[test]
fn a_custom_calendar_round_trips_as_an_array() {
    let dir = TempDir::new();
    let path = dir.path().join("x.plist");
    write_file(
        dir.path(),
        "x.plist",
        br#"<?xml version="1.0"?><plist version="1.0"><dict><key>Label</key><string>a</string><key>ProgramArguments</key><array><string>/bin/date</string></array><key>StartCalendarInterval</key><array><dict><key>Minute</key><integer>0</integer></dict><dict><key>Minute</key><integer>30</integer></dict></array></dict></plist>"#,
    );
    let spec = Spec::load(&path).expect("load");
    assert!(!spec.calendar_custom);
    assert_eq!(spec.calendar.minute, "0,30");
}

#[test]
fn save_as_root_checks_the_group_and_the_directory() {
    let dir = TempDir::new();
    let runner = FakeRunner::new("");
    let helper = FakeHelper::installed();
    let host = FakeHost::new(dir.path(), dir.path());
    let ctx = context(Arc::clone(&runner), &helper, Arc::clone(&host));
    let provider = LaunchdProvider::new(
        Config {
            source: Source::Daemon,
            dir: dir.path().to_path_buf(),
            domain: "system".into(),
            elevated: true,
        },
        ctx,
    );
    runner.fail("dscl", "no such group");
    let error = provider
        .save_as_root(
            &Spec {
                label: "com.example.g".into(),
                args: shell_args("date"),
                user_name: "me".into(),
                group_name: "staff".into(),
                ..Spec::default()
            },
            None,
            false,
        )
        .expect_err("missing group");
    assert!(error.message.contains(r#"no group "staff""#), "{error}");
    assert!(helper.save_requests().is_empty());

    let unconfigured = LaunchdProvider::new(
        Config {
            source: Source::Daemon,
            dir: std::path::PathBuf::new(),
            domain: "system".into(),
            elevated: true,
        },
        provider.ctx(),
    );
    let error = unconfigured
        .save_as_root(
            &Spec {
                label: "com.example.u".into(),
                args: shell_args("date"),
                ..Spec::default()
            },
            None,
            false,
        )
        .expect_err("unconfigured directory");
    assert!(
        error
            .message
            .contains("provider directory is not configured"),
        "{error}"
    );
}

#[test]
fn calendar_from_rejects_shapes_the_form_cannot_say() {
    use plist::{Dictionary, Value};

    fn entry(pairs: &[(&str, Value)]) -> Value {
        let mut dict = Dictionary::new();
        for (key, value) in pairs {
            dict.insert((*key).to_string(), value.clone());
        }
        Value::Dictionary(dict)
    }

    fn entries(values: Vec<Value>) -> Value {
        Value::Array(values)
    }

    let minute = |value: i64| Value::Integer(value.into());
    assert!(calendar_from(&entries(Vec::new())).is_none());
    assert!(calendar_from(&entry(&[])).is_none());
    assert!(
        calendar_from(&entries(vec![
            entry(&[("Minute", minute(0))]),
            entry(&[("Hour", minute(9))]),
        ]))
        .is_none(),
        "the entries must name the same fields"
    );
    assert!(
        calendar_from(&entries(vec![
            entry(&[("Minute", minute(0))]),
            entry(&[("Minute", minute(0))]),
        ]))
        .is_none(),
        "duplicates are not a product"
    );
    assert!(
        calendar_from(&entry(&[("Minute", minute(0)), ("Extra", minute(1))])).is_none(),
        "unknown keys are not a calendar"
    );
    assert!(
        calendar_from(&entry(&[("Minute", Value::String("0".into()))])).is_none(),
        "a string is not a plist integer"
    );
    assert!(
        calendar_from(&entry(&[("Minute", minute(60))])).is_none(),
        "outside the field range"
    );

    let product = calendar_from(&entries(vec![
        entry(&[("Minute", minute(0))]),
        entry(&[("Minute", minute(30))]),
    ]))
    .expect("a complete product");
    assert_eq!(product.minute, "0,30");
    let single =
        calendar_from(&entry(&[("Minute", minute(0)), ("Hour", minute(9))])).expect("one entry");
    assert_eq!(single.minute, "0");
    assert_eq!(single.hour, "9");
}

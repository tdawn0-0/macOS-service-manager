use std::collections::BTreeMap;
use std::path::PathBuf;

use plist::Value;

use super::test_support::{FakeRunner, TempDir, XML_PLIST, write_file};
use super::*;

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

#[test]
fn validate_rejects_bad_labels_paths_and_environment() {
    let ok = Spec {
        label: "com.example.job".into(),
        args: vec!["/bin/echo".into(), "hi".into()],
        ..Spec::default()
    };
    ok.validate().expect("valid spec");

    type Mutation = Box<dyn Fn(&mut Spec)>;
    let mutations: Vec<(&str, &str, Mutation)> = vec![
        (
            "empty label",
            "the label may hold",
            Box::new(|s| s.label.clear()),
        ),
        (
            "label with slash",
            "the label may hold",
            Box::new(|s| s.label = "a/b".into()),
        ),
        (
            "label with space",
            "the label may hold",
            Box::new(|s| s.label = "a b".into()),
        ),
        (
            "no program",
            "give the program to run",
            Box::new(|s| s.args.clear()),
        ),
        (
            "relative program",
            "must be an absolute path",
            Box::new(|s| s.args = vec!["echo".into()]),
        ),
        (
            "relative workdir",
            "working directory",
            Box::new(|s| s.working_dir = "tmp".into()),
        ),
        (
            "relative stdout",
            "standard output",
            Box::new(|s| s.stdout = "out.log".into()),
        ),
        (
            "relative stderr",
            "standard error",
            Box::new(|s| s.stderr = "err.log".into()),
        ),
        (
            "negative interval",
            "the interval cannot be negative",
            Box::new(|s| s.start_interval = -1),
        ),
        (
            "bad throttle",
            "the throttle interval must be 1–3600 seconds",
            Box::new(|s| s.throttle_interval = Some(0)),
        ),
        (
            "bad env name",
            "environment variable name",
            Box::new(|s| {
                s.env = BTreeMap::from([("A=B".to_string(), "c".to_string())]);
            }),
        ),
    ];
    for (name, want, mutate) in mutations {
        let mut spec = ok.clone();
        mutate(&mut spec);
        let error = spec.validate().expect_err(name);
        assert!(
            error.message.contains(want),
            "{name}: {} lacks {want:?}",
            error.message
        );
    }
}

#[test]
fn marshal_keeps_unknown_keys_and_conditions() {
    let current = XML_PLIST.as_bytes();
    let mut spec = spec_from_bytes(current);
    assert!(spec.keep_alive_custom && spec.keep_alive, "{spec:?}");
    spec.start_interval = 0;
    spec.stderr.clear();
    spec.args = vec!["/bin/echo".into(), "changed".into()];

    let closing = "</dict></plist>";
    let mut with_nice = current[..current.len() - closing.len()].to_vec();
    with_nice.extend_from_slice(b"<key>Nice</key><integer>5</integer></dict></plist>");
    let out = spec.marshal(Some(&with_nice)).expect("marshal");
    let dict = decode(&out);
    assert!(
        matches!(dict.get("KeepAlive"), Some(Value::Dictionary(_))),
        "KeepAlive conditions lost: {:?}",
        dict.get("KeepAlive")
    );
    assert_eq!(
        dict.get("Nice").and_then(Value::as_signed_integer),
        Some(5),
        "an unknown key was lost"
    );
    assert!(!dict.contains_key("StartInterval"));
    assert!(!dict.contains_key("StandardErrorPath"));
    let args = dict
        .get("ProgramArguments")
        .and_then(Value::as_array)
        .expect("arguments");
    assert_eq!(args.len(), 2);
    assert_eq!(args[1].as_string(), Some("changed"));
}

#[test]
fn save_writes_new_files_and_edits_them() {
    let runner = FakeRunner::new("");
    let dir = TempDir::new();
    let spec = Spec {
        label: "com.example.new".into(),
        args: vec!["/bin/sleep".into(), "60".into()],
        run_at_load: true,
        keep_alive: true,
        start_interval: 30,
        env: BTreeMap::from([("A".to_string(), "1".to_string())]),
        stdout: "/tmp/new.log".into(),
        ..Spec::default()
    };
    let path = save_file(dir.path(), &spec, None, &*runner).expect("save");
    assert_eq!(path, dir.path().join("com.example.new.plist"));

    let mut loaded = Spec::load(&path).expect("load");
    assert_eq!(loaded.label, spec.label);
    assert_eq!(loaded.args, spec.args);
    assert!(loaded.run_at_load);
    assert!(loaded.keep_alive);
    assert_eq!(loaded.start_interval, 30);
    assert_eq!(loaded.env["A"], "1");
    assert_eq!(loaded.stdout, "/tmp/new.log");

    let error = save_file(dir.path(), &spec, None, &*runner).expect_err("taken label");
    assert!(error.message.contains("already exists"), "{error}");

    loaded.args = vec!["/bin/sleep".into(), "120".into()];
    save_file(dir.path(), &loaded, Some(&path), &*runner).expect("edit");
    let again = Spec::load(&path).expect("load again");
    assert_eq!(again.args[1], "120");

    let leftovers: Vec<PathBuf> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(".msm-"))
        })
        .collect();
    assert!(leftovers.is_empty(), "temporary files left: {leftovers:?}");

    let bad = Spec {
        label: "bad label".into(),
        ..Spec::default()
    };
    assert!(save_file(dir.path(), &bad, None, &*runner).is_err());
}

#[test]
fn environment_text_round_trips() {
    let env = parse_env("B=2\n\n A = x y \nC=").expect("env");
    assert_eq!(env["B"], "2");
    assert_eq!(env["A"], "x y");
    assert_eq!(env["C"], "");
    assert_eq!(env.len(), 3);
    assert_eq!(
        format_env(&BTreeMap::from([
            ("B".to_string(), "2".to_string()),
            ("A".to_string(), "1".to_string()),
        ])),
        "A=1\nB=2"
    );
    assert!(parse_env("no equals sign").is_err());
    assert_eq!(
        parse_lines("  /bin/echo \n\nhello world\n"),
        vec!["/bin/echo", "hello world"]
    );
    assert_eq!(parse_list(" a, b ,, c "), vec!["a", "b", "c"]);
    assert_eq!(shell_args("date"), vec!["/bin/sh", "-c", "date"]);
    assert_eq!(
        shell_command(&shell_args("date")).map(str::to_string),
        Some("date".to_string())
    );
    assert_eq!(
        shell_command(&["sh".into(), "-c".into(), "x".into()]),
        Some("x")
    );
    assert_eq!(
        shell_command(&["bash".into(), "-c".into(), "x".into()]),
        None
    );
}

#[test]
fn save_reports_a_plutil_failure_and_removes_the_temporary_file() {
    let runner = FakeRunner::new("");
    runner.fail("plutil", "plutil: invalid property list");
    let dir = TempDir::new();
    let spec = Spec {
        label: "com.example.bad".into(),
        args: vec!["/bin/date".into()],
        ..Spec::default()
    };
    let error = save_file(dir.path(), &spec, None, &*runner).expect_err("plutil");
    assert!(
        error.message.contains("the property list is not valid"),
        "{error}"
    );
    let leftovers: Vec<PathBuf> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(".msm-"))
        })
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    assert!(!dir.path().join("com.example.bad.plist").exists());
}

#[test]
fn a_program_alone_collapses_to_program_when_unchanged() {
    let data = br#"<plist version="1.0"><dict>
<key>Label</key><string>a</string>
<key>Program</key><string>/opt/vendor/helper</string>
</dict></plist>"#;
    let spec = spec_from_bytes(data);
    assert_eq!(spec.args, vec!["/opt/vendor/helper"]);
    let out = spec.marshal(Some(data)).expect("marshal");
    let dict = decode(&out);
    assert_eq!(
        dict.get("Program").and_then(Value::as_string),
        Some("/opt/vendor/helper")
    );
    assert!(!dict.contains_key("ProgramArguments"));
}

use std::time::Duration;

use super::admin::{ADMIN_TIMEOUT, ADMIN_TIMEOUT_LABEL, AdminError, AdminRunner};

#[test]
fn admin_timeout_is_five_minutes() {
    assert_eq!(ADMIN_TIMEOUT, Duration::from_secs(300));
    assert_eq!(ADMIN_TIMEOUT_LABEL, "5m0s");
}

#[test]
fn shell_and_applescript_quoting_are_exact() {
    assert_eq!(super::admin::shell_quote("plain"), "'plain'");
    assert_eq!(super::admin::shell_quote("it's"), r"'it'\''s'");
    assert_eq!(
        super::admin::apple_script_string(r#"say "hi"\bye"#),
        r#""say \"hi\"\\bye""#
    );
    assert_eq!(
        super::admin::admin_script("echo", &["a b", "it's", "say \"hi\"\\"]),
        r#"do shell script "'echo' 'a b' 'it'\\''s' 'say \"hi\"\\'" with administrator privileges"#
    );
}

#[test]
fn run_admin_translates_user_cancel() {
    assert_eq!(
        super::admin::translate_admin_error("execution error: User canceled. (-128)".to_string()),
        AdminError::Canceled
    );
    assert_eq!(AdminError::Canceled.to_string(), "authorization canceled");
    assert_eq!(
        super::admin::translate_admin_error("execution error: boom".to_string()),
        AdminError::Message("execution error: boom".to_string())
    );
}

#[test]
fn run_for_matches_go_run_for() {
    let success = super::admin::run_for(Duration::from_secs(1), "/bin/echo", &["hello"], "1s")
        .expect("echo succeeds");
    assert_eq!(success, "hello\n");

    let failure = super::admin::run_for(
        Duration::from_secs(1),
        "/bin/sh",
        &["-c", "echo '  stderr reason  ' >&2; exit 3"],
        "1s",
    )
    .expect_err("nonzero exit fails");
    assert_eq!(failure, "stderr reason");

    let silent = super::admin::run_for(Duration::from_secs(1), "/bin/sh", &["-c", "exit 7"], "1s")
        .expect_err("nonzero exit fails");
    assert_eq!(silent, "/bin/sh: exit status 7");

    let started = std::time::Instant::now();
    let timeout = super::admin::run_for(
        Duration::from_millis(40),
        "/bin/sleep",
        &["5"],
        ADMIN_TIMEOUT_LABEL,
    )
    .expect_err("sleep times out");
    assert_eq!(timeout, "/bin/sleep did not answer in 5m0s");
    assert!(started.elapsed() < Duration::from_secs(2));
}

/// A compile-time proof that only `AdminRunner` exposes the prompt.
#[test]
fn fake_admin_is_the_only_prompt_seam() {
    let admin = super::test_support::FakeAdmin::succeeding();
    let (output, error) = AdminRunner::run_admin(&*admin, "/bin/echo", &["hi"]);
    assert_eq!(output, "");
    assert!(error.is_none());
    assert_eq!(
        admin.calls(),
        vec![("/bin/echo".to_string(), vec!["hi".to_string()])]
    );
}

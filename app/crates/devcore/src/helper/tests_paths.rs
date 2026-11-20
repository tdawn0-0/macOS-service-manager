use super::paths::{base_name, clean_path, dir_path, is_digits, validate_clean_target_file};

#[test]
fn clean_path_matches_go_filepath_clean() {
    for (input, want) in [
        ("", "."),
        ("/", "/"),
        ("//", "/"),
        ("//x", "/x"),
        ("a/", "a"),
        ("a//b", "a/b"),
        ("./a", "a"),
        ("a/./b", "a/b"),
        ("a/../b", "b"),
        ("a/../../b", "../b"),
        ("/..", "/"),
        ("/a/../b", "/b"),
        ("..", ".."),
        ("a/../..", ".."),
    ] {
        assert_eq!(clean_path(input), want, "clean {input:?}");
    }
}

#[test]
fn dir_and_base_match_go_filepath() {
    for (input, dir, base) in [
        ("", ".", "."),
        ("/", "/", "/"),
        ("a", ".", "a"),
        ("a/b", "a", "b"),
        ("/a/b", "/a", "b"),
        ("a/", "a", "a"),
        ("/a/b/", "/a/b", "b"),
        ("//a", "/", "a"),
        ("a/.", "a", "."),
        ("a/..", "a", ".."),
        ("a/../", ".", ".."),
        ("/a/b//", "/a/b", "b"),
    ] {
        assert_eq!(dir_path(input), dir, "dir {input:?}");
        assert_eq!(base_name(input), base, "base {input:?}");
    }
    assert!(is_digits("501"));
    assert!(!is_digits(""));
    assert!(!is_digits("50a"));
}

#[test]
fn clean_target_file_rejects_the_go_cases() {
    let dir = tempfile::tempdir().expect("temp dir");
    let allowed = vec![dir.path().to_str().expect("utf8").to_string()];
    let job = dir.path().join("com.example.job.plist");
    let job_text = job.to_str().expect("utf8");

    assert_eq!(
        validate_clean_target_file(job_text, &allowed, ".plist"),
        Ok(job_text.to_string())
    );
    assert!(
        validate_clean_target_file("relative.plist", &allowed, ".plist")
            .expect_err("must reject")
            .contains("must be absolute")
    );
    let unnormalized = format!("{}/./com.example.job.plist", dir.path().display());
    assert!(
        validate_clean_target_file(&unnormalized, &allowed, ".plist")
            .expect_err("must reject")
            .contains("not normalized")
    );
    assert!(
        validate_clean_target_file(job_text, &allowed, ".txt")
            .expect_err("must reject")
            .contains("must end with .txt")
    );
    assert!(
        validate_clean_target_file("/etc/com.example.job.plist", &allowed, ".plist")
            .expect_err("must reject")
            .contains("not permitted")
    );
    let with_nul = format!("{job_text}\0");
    assert!(
        validate_clean_target_file(&with_nul, &allowed, ".plist")
            .expect_err("must reject")
            .contains("NUL")
    );

    std::os::unix::fs::symlink(&job, dir.path().join("link.plist")).expect("symlink");
    let link = dir.path().join("link.plist");
    assert!(
        validate_clean_target_file(link.to_str().expect("utf8"), &allowed, ".plist")
            .expect_err("must reject")
            .contains("refusing symlink")
    );
}

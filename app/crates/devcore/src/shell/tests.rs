//! Shell integration tests: temp homes, fake command/HTTP seams, no real
//! shells and no network.

use std::collections::{BTreeMap, VecDeque};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tempfile::TempDir;

use super::block::update_shell_block;
use super::node::node_version_file;
use super::*;
use crate::engine::CommandRunner;
use crate::launchd::CommandError;
use crate::model::Package;
use crate::software::{HttpTransport, Software, SoftwareError};
use crate::state::Store;

type Handler = dyn Fn(&str, &[&str]) -> Result<String, CommandError> + Send + Sync;

struct FakeRunner {
    handler: Box<Handler>,
    calls: Mutex<Vec<(String, Vec<String>)>>,
}

impl FakeRunner {
    fn new(
        handler: impl Fn(&str, &[&str]) -> Result<String, CommandError> + Send + Sync + 'static,
    ) -> Self {
        FakeRunner {
            handler: Box::new(handler),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn default() -> Self {
        FakeRunner::new(|program, args| {
            Err(CommandError::new(
                program,
                args.iter().map(|arg| (*arg).to_string()).collect(),
                "unexpected command",
            ))
        })
    }
}

impl CommandRunner for FakeRunner {
    fn run(
        &self,
        program: &str,
        args: &[&str],
        _timeout: Duration,
    ) -> Result<String, CommandError> {
        self.calls.lock().unwrap().push((
            program.to_string(),
            args.iter().map(|arg| (*arg).to_string()).collect(),
        ));
        (self.handler)(program, args)
    }
}

type ResponseQueue = BTreeMap<String, VecDeque<Result<Vec<u8>, SoftwareError>>>;

#[derive(Default)]
struct FakeHttp {
    responses: Mutex<ResponseQueue>,
    gets: AtomicUsize,
}

impl FakeHttp {
    fn set(&self, url: &str, body: impl Into<Vec<u8>>) {
        self.responses
            .lock()
            .unwrap()
            .entry(url.to_string())
            .or_default()
            .push_back(Ok(body.into()));
    }
}

impl HttpTransport for FakeHttp {
    fn download(&self, _url: &str, _destination: &Path) -> Result<u64, SoftwareError> {
        Err(SoftwareError::Fetch("download is not used".into()))
    }

    fn get(&self, url: &str) -> Result<Vec<u8>, SoftwareError> {
        self.gets.fetch_add(1, Ordering::SeqCst);
        let mut responses = self.responses.lock().unwrap();
        responses
            .get_mut(url)
            .and_then(|queue| queue.pop_front())
            .unwrap_or_else(|| Err(SoftwareError::Fetch(format!("no response for {url}"))))
    }

    fn size(&self, _url: &str) -> Result<Option<i64>, SoftwareError> {
        Ok(None)
    }
}

fn temp_root() -> (TempDir, Store) {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("dev");
    std::fs::create_dir_all(&root).unwrap();
    (temp, Store::new(root))
}

fn shell_with(store: Store, runner: Arc<FakeRunner>, http: Arc<FakeHttp>, home: &Path) -> Shell {
    let software = Software::new(store.clone())
        .with_runner(runner.clone())
        .with_http(http.clone());
    Shell::with_seams(store, software, runner, http).with_home(home)
}

fn executable(path: &Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, b"#!/bin/sh\n").unwrap();
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

fn plain(path: &Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, b"#!/bin/sh\n").unwrap();
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o644);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[test]
fn update_shell_block_is_idempotent() {
    let begin = super::NODE_BLOCK_BEGIN;
    let end = super::NODE_BLOCK_END;
    let block = format!("{begin}\necho node\n{end}\n");
    let original = b"export A=1\n".to_vec();
    let once = update_shell_block(&original, begin, end, &block, true).unwrap();
    let twice = update_shell_block(&once, begin, end, &block, true).unwrap();
    assert_eq!(once, twice);
    let removed = update_shell_block(&once, begin, end, &block, false).unwrap();
    assert_eq!(removed, b"export A=1\n".to_vec());
    assert!(update_shell_block(&original, begin, end, &block, false).unwrap() == original);
}

#[test]
fn update_shell_block_refuses_damaged_markers() {
    let begin = "# BEGIN Devbase java";
    let end = "# END Devbase java";
    let damaged = b"# BEGIN Devbase java\nkeep\n".to_vec();
    assert!(update_shell_block(&damaged, begin, end, "x", true).is_err());
    let duplicated = b"# BEGIN Devbase java\n# END Devbase java\n# BEGIN Devbase java\n".to_vec();
    assert!(update_shell_block(&duplicated, begin, end, "x", true).is_err());
    let joined = b"prefix # BEGIN Devbase java\n# END Devbase java\n".to_vec();
    assert!(update_shell_block(&joined, begin, end, "x", true).is_err());
}

#[test]
fn shell_detection_follows_order_and_refuses_non_executable_files() {
    let (temp, store) = temp_root();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    let name = "devbase-test-shell";
    executable(&second.join(name));
    plain(&first.join(name));

    let runner = Arc::new(FakeRunner::default());
    let http = Arc::new(FakeHttp::default());
    let shell = shell_with(store, runner, http, &home)
        .with_path_dirs(vec![first.clone()])
        .with_shell_env(Some(second.join(name)));
    // A non-executable LookPath hit is skipped, then `$SHELL` is accepted.
    assert!(shell.has_shell(name));

    let only_plain = shell
        .clone()
        .with_path_dirs(vec![first.clone()])
        .with_shell_env(None);
    assert!(!only_plain.has_shell(name));

    let only_fallback = shell
        .clone()
        .with_path_dirs(Vec::new())
        .with_shell_env(Some(second.join(name)));
    assert!(only_fallback.has_shell(name));
    assert!(!only_fallback.has_shell("devbase-other-shell"));
}

#[test]
fn enabling_node_shell_twice_leaves_every_file_unchanged() {
    let (temp, store) = temp_root();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let runner = Arc::new(FakeRunner::default());
    let http = Arc::new(FakeHttp::default());
    let shell = shell_with(store, runner, http, &home)
        .with_shell_available(Arc::new(|_| true))
        .with_program("/Applications/Devbase.app/Contents/MacOS/Devbase");

    shell.enable_node_shell().unwrap();
    assert!(shell.node_shell_enabled());
    let files = shell.shell_startup_files(true).unwrap();
    let mut paths: Vec<PathBuf> = files.iter().map(|file| file.path.clone()).collect();
    paths.push(shell.bash_env_path());
    let before: Vec<Vec<u8>> = paths
        .iter()
        .map(|path| std::fs::read(path).unwrap())
        .collect();
    assert!(before.iter().all(|data| {
        let text = String::from_utf8_lossy(data);
        text.contains(super::NODE_BLOCK_BEGIN) && text.contains(super::NODE_BLOCK_END)
    }));

    shell.enable_node_shell().unwrap();
    let after: Vec<Vec<u8>> = paths
        .iter()
        .map(|path| std::fs::read(path).unwrap())
        .collect();
    assert_eq!(before, after);

    shell.disable_node_shell().unwrap();
    assert!(!shell.node_shell_enabled());
    for path in &paths {
        let bytes = std::fs::read(path).unwrap();
        assert!(
            !bytes
                .windows(super::NODE_BLOCK_BEGIN.len())
                .any(|window| window == super::NODE_BLOCK_BEGIN.as_bytes())
        );
    }
}

#[test]
fn node_release_index_filters_arm64_and_caches_for_an_hour() {
    let (temp, store) = temp_root();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let tag = crate::platform::HostTarget::current().node_dist_tag();
    let http = Arc::new(FakeHttp::default());
    http.set(
        "https://nodejs.example/dist/index.json",
        format!(
            r#"[
          {{"version":"v22.14.0","date":"2025-02-11","files":["{tag}"],"npm":"10.9.2","v8":"12.4","lts":"Jod"}},
          {{"version":"v20.19.0","date":"2025-03-20","files":["win-x64-zip"],"lts":"Iron"}},
          {{"version":"v21.0.0","date":"2024-10-01","files":["{tag}"],"lts":false}}
        ]"#
        ),
    );
    let runner = Arc::new(FakeRunner::default());
    let shell = shell_with(store.clone(), runner, http.clone(), &home)
        .with_node_dist_base("https://nodejs.example/dist");
    let (releases, stale) = shell.node_releases(&|| false, false).unwrap();
    assert!(!stale);
    assert_eq!(
        releases
            .iter()
            .map(|release| release.version.as_str())
            .collect::<Vec<_>>(),
        vec!["22.14.0", "21.0.0"]
    );
    assert_eq!(releases[0].lts, "Jod");
    assert_eq!(http.gets.load(Ordering::SeqCst), 1);
    // The cache satisfies the next read without HTTP.
    let (cached, stale) = shell.node_releases(&|| false, false).unwrap();
    assert_eq!(cached, releases);
    assert!(!stale);
    assert_eq!(http.gets.load(Ordering::SeqCst), 1);
    assert!(node_index_path(&store).exists());
}

#[test]
fn node_index_falls_back_to_cache_when_the_network_fails() {
    let (temp, store) = temp_root();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let tag = crate::platform::HostTarget::current().node_dist_tag();
    let http = Arc::new(FakeHttp::default());
    http.set(
        "https://node/ dist/index.json",
        format!(r#"[{{"version":"v22.14.0","files":["{tag}"]}}]"#),
    );
    let runner = Arc::new(FakeRunner::default());
    let shell =
        shell_with(store, runner, http.clone(), &home).with_node_dist_base("https://node/ dist");
    let (fresh, stale) = shell.node_releases(&|| false, false).unwrap();
    assert_eq!(fresh.len(), 1);
    assert!(!stale);
    let (cached, stale) = shell.node_releases(&|| false, true).unwrap();
    assert_eq!(cached, fresh);
    assert!(stale);
}

#[test]
fn node_scans_and_resolution_cover_the_upstream_shasums_list() {
    let target = crate::platform::HostTarget::current();
    let archive_name = format!("node-v22.14.0-{}.tar.gz", target.node_archive_platform());
    let http = FakeHttp::default();
    http.set(
        "https://nodejs.org/dist/v22.14.0/SHASUMS256.txt",
        format!(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  {archive_name}\n"
        ),
    );
    let package = super::node::parse_semver("22.14.0").unwrap();
    assert_eq!((package.major, package.minor, package.patch), (22, 14, 0));
    let package = crate::software::catalog::resolve_node(&http, "22.14.0").unwrap();
    assert_eq!(package.sha256.len(), 64);
    assert_eq!(
        package.url,
        format!("https://nodejs.org/dist/v22.14.0/{archive_name}")
    );
    assert_eq!(package.binaries["node"], "bin/node");
    assert_eq!(package.binaries["npm"], "bin/npm");
    assert_eq!(package.binaries["npx"], "bin/npx");
    assert!(crate::software::catalog::resolve_node(&http, "22").is_err());
}

#[test]
fn node_semver_ranges_pick_the_newest_matching_release() {
    let releases = vec![
        NodeRelease {
            version: "22.14.0".into(),
            lts: "Jod".into(),
            ..NodeRelease::default()
        },
        NodeRelease {
            version: "20.19.0".into(),
            lts: "Iron".into(),
            ..NodeRelease::default()
        },
        NodeRelease {
            version: "18.20.8".into(),
            ..NodeRelease::default()
        },
    ];
    for (spec, expected) in [
        ("22", "22.14.0"),
        ("22.x", "22.14.0"),
        ("^20.1", "20.19.0"),
        ("~18.20", "18.20.8"),
        (">=20 <23", "22.14.0"),
        ("lts", "22.14.0"),
        ("lts/iron", "20.19.0"),
        ("18.0.0 - 20.5.0", "18.20.8"),
        ("18 || 22", "22.14.0"),
    ] {
        let picked = pick_node(spec, &releases).unwrap().unwrap();
        assert_eq!(picked.version, expected, "spec {spec}");
    }
    assert!(pick_node("lts/argon", &releases).unwrap().is_none());
}

#[test]
fn node_version_file_walks_up_and_honors_overrides() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("project");
    let nested = root.join("src").join("app");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(root.join(".nvmrc"), "# comment\n20.19.0\n").unwrap();
    let (spec, source) = node_version_file(&nested);
    assert_eq!(spec, "20.19.0");
    assert!(source.ends_with(".nvmrc"));

    let (temp, store) = temp_root();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let runner = Arc::new(FakeRunner::default());
    let http = Arc::new(FakeHttp::default());
    let arch = crate::platform::HostTarget::current().arch_name();
    let package = Package {
        name: "node".into(),
        version: "20.19.0".into(),
        arch: arch.into(),
        ..Package::default()
    };
    let mut state = crate::model::State::default();
    state.packages.insert(package.id(), package);
    store.write_state(&state).unwrap();
    let shell = shell_with(store, runner, http, &home);
    let selection = shell.select_node(&nested, None).unwrap();
    assert_eq!(selection.version, "20.19.0");
    assert!(
        selection
            .bin()
            .ends_with(format!("node@20.19.0-{arch}/bin"))
    );
    let override_selection = shell.select_node(&nested, Some("system")).unwrap();
    assert!(override_selection.system);
    assert_eq!(override_selection.path(), "");
}

#[test]
fn java_catalog_pins_install_folders_and_resolves_by_key() {
    let target = crate::platform::HostTarget::current();
    let (temp, _store) = temp_root();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let body = serde_json::json!({
        "jdks": [
            {
                "vendor": "JetBrains",
                "product": "Runtime",
                "flavour": "JCEF",
                "jdk_version": "21.0.2",
                "jdk_vendor_version": "21.0.2+13",
                "filter": true,
                "listed": true,
                "preview": false,
                "packages": [
                    {
                        "os": target.jetbrains_os(),
                        "arch": target.jetbrains_arch(),
                        "version": "21.0.2",
                        "url": "https://download.jetbrains.com/jdk/jbr-21.0.2.tar.gz",
                        "sha256": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                        "package_type": "targz",
                        "package_root_prefix": "",
                        "package_to_java_home_prefix": "Contents/Home",
                        "install_folder_name": "jbr-21.0.2+13",
                        "filter": true
                    },
                    {
                        "os": "windows",
                        "arch": "x64",
                        "version": "21.0.2",
                        "url": "https://download.jetbrains.com/jdk/windows.tar.gz",
                        "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                        "package_type": "targz",
                        "install_folder_name": "jbr-windows"
                    }
                ]
            },
            {
                "vendor": "Ignored",
                "product": "Preview",
                "filter": false,
                "listed": false,
                "preview": true,
                "packages": []
            }
        ]
    })
    .to_string();
    let http = Arc::new(FakeHttp::default());
    http.set(java::JAVA_CATALOG_URL, body.clone());
    http.set(java::JAVA_CATALOG_URL, body);
    let rows = java::java_catalog(http.as_ref()).unwrap();
    assert_eq!(rows.len(), 1);
    let package = &rows[0];
    assert_eq!(package.name, "java");
    assert_eq!(package.version, "jbr-21.0.2_13");
    assert_eq!(package.java_install_name, "jbr-21.0.2+13");
    assert_eq!(package.sha256.len(), 64);
    assert_eq!(package.binaries["java"], "Contents/Home/bin/java");
    assert_eq!(package.binaries["javac"], "Contents/Home/bin/javac");
    assert_eq!(package.arch, target.arch_name());
    let resolved = java::resolve_java(http.as_ref(), "jbr-21.0.2_13").unwrap();
    assert_eq!(resolved.url, package.url);
    assert!(java::resolve_java(http.as_ref(), "missing").is_err());
}

#[test]
fn java_catalog_refuses_ambiguous_pins() {
    let target = crate::platform::HostTarget::current();
    let base = serde_json::json!({
        "os": target.jetbrains_os(),
        "arch": target.jetbrains_arch(),
        "version": "21.0.2",
        "url": "https://download.jetbrains.com/jdk/a.tar.gz",
        "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "package_type": "targz",
        "package_to_java_home_prefix": "Contents/Home",
        "install_folder_name": "jbr-21.0.2+13"
    });
    let mut second = base.clone();
    second["url"] = serde_json::json!("https://download.jetbrains.com/jdk/b.tar.gz");
    second["sha256"] =
        serde_json::json!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
    let body = serde_json::json!({
        "jdks": [{
            "vendor": "JetBrains",
            "product": "Runtime",
            "filter": true,
            "listed": true,
            "packages": [base, second]
        }]
    })
    .to_string();
    let http = FakeHttp::default();
    http.set(java::JAVA_CATALOG_URL, body);
    let error = java::java_catalog(&http).unwrap_err().to_string();
    assert!(error.contains("ambiguous JDK catalog entry"));
}

#[test]
fn uv_resolution_uses_installed_executable_and_selection() {
    let (temp, store) = temp_root();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let uv = home.join(".local/bin/uv");
    executable(&uv);
    let uv_path = uv.clone();
    let runner = Arc::new(FakeRunner::new(move |program, args| {
        if program == uv_path.to_string_lossy() && args.last() == Some(&"--version") {
            return Ok("uv 0.6.1\n".into());
        }
        if program == uv_path.to_string_lossy()
            && args
                == [
                    "--no-progress",
                    "--directory",
                    "/tmp",
                    "python",
                    "list",
                    "--managed-python",
                    "--output-format",
                    "json",
                    "--only-installed",
                ]
        {
            return Ok(
                r#"[{"key":"cpython-3.12.3","version":"3.12.3","path":"/tmp/py","implementation":"cpython","arch":"aarch64","os":"macos"}]"#
                    .into(),
            );
        }
        Err(CommandError::new(
            program,
            args.iter().map(|arg| (*arg).to_string()).collect(),
            "unexpected",
        ))
    }));
    let http = Arc::new(FakeHttp::default());
    let shell =
        shell_with(store.clone(), runner.clone(), http, &home).with_uv_candidates(vec![uv.clone()]);
    assert_eq!(shell.uv_path().unwrap(), uv);
    let rows = shell.uv_pythons(true).unwrap();
    assert_eq!(rows[0].key, "cpython-3.12.3");
    assert!(shell.uv_description().unwrap().starts_with("uv 0.6.1 · "));

    shell.set_uv_path(&uv).unwrap();
    assert_eq!(store.read().unwrap().uv_executable, uv.to_string_lossy());
    assert!(shell.uv_path().is_ok());

    let missing_home = temp.path().join("empty");
    std::fs::create_dir_all(&missing_home).unwrap();
    let empty = shell_with(
        Store::new(temp.path().join("other")),
        runner,
        Arc::new(FakeHttp::default()),
        &missing_home,
    )
    .with_uv_candidates(Vec::new());
    assert!(empty.uv_path().is_err());
}

#[test]
fn uv_refuses_non_executable_selection() {
    let (temp, store) = temp_root();
    let home = temp.path().join("home");
    let uv = home.join(".local/bin/uv");
    plain(&uv);
    let runner = Arc::new(FakeRunner::default());
    let shell = shell_with(store, runner, Arc::new(FakeHttp::default()), &home)
        .with_uv_candidates(vec![uv.clone()]);
    assert!(shell.uv_path().is_err());
}

fn write_generational_ca_cert(root: &Path) -> PathBuf {
    let proxy = root.join("proxy");
    let generation = proxy.join("ca-generations").join("0123456789abcdef");
    std::fs::create_dir_all(&generation).unwrap();
    std::fs::write(
        generation.join("ca.pem"),
        b"-----BEGIN CERTIFICATE-----\nTEST\n-----END CERTIFICATE-----\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(
        Path::new("ca-generations").join("0123456789abcdef"),
        proxy.join("ca-current"),
    )
    .unwrap();
    std::os::unix::fs::symlink(Path::new("ca-current").join("ca.pem"), proxy.join("ca.pem"))
        .unwrap();
    proxy.join("ca.pem")
}

#[test]
fn ca_env_shell_defaults_disabled_and_requires_existing_ca_cert() {
    let (temp, store) = temp_root();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let zshrc = home.join(".zshrc");
    std::fs::write(&zshrc, b"export EXISTING=1\n").unwrap();

    let shell = shell_with(
        store,
        Arc::new(FakeRunner::default()),
        Arc::new(FakeHttp::default()),
        &home,
    )
    .with_shell_available(Arc::new(|_| true));

    assert!(!shell.ca_env_config().ca_env_enabled);
    assert!(!shell.ca_env_shell_enabled());
    assert!(!shell.local_ca_cert_exists());

    // Enabling without a local CA certificate must fail and leave dotfiles untouched.
    let err = shell.enable_ca_env_shell().unwrap_err();
    assert!(matches!(err, ShellError::Missing(_)));
    assert!(!shell.ca_env_config().ca_env_enabled);
    assert_eq!(std::fs::read(&zshrc).unwrap(), b"export EXISTING=1\n");

    // Disabling when never enabled is a clean no-op.
    shell.disable_ca_env_shell().unwrap();
    assert!(!shell.ca_env_config_path().exists());
    assert_eq!(std::fs::read(&zshrc).unwrap(), b"export EXISTING=1\n");
}

#[test]
fn ca_env_shell_writes_zsh_bash_and_fish_idempotently_and_reversibly() {
    let (temp, store) = temp_root();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let cert_path = write_generational_ca_cert(store.root());

    let zshrc = home.join(".zshrc");
    let bashrc = home.join(".bashrc");
    let fish_rc = home.join(".config/fish/conf.d/devbase.fish");
    std::fs::create_dir_all(fish_rc.parent().unwrap()).unwrap();
    let original_zshrc = b"# user zsh config\nexport FOO=bar\n".to_vec();
    let original_bashrc = b"# user bash config\nexport BAZ=1\n".to_vec();
    let original_fish = b"# user fish config\nset -gx FOO bar\n".to_vec();
    std::fs::write(&zshrc, &original_zshrc).unwrap();
    std::fs::write(&bashrc, &original_bashrc).unwrap();
    std::fs::write(&fish_rc, &original_fish).unwrap();

    let shell = shell_with(
        store,
        Arc::new(FakeRunner::default()),
        Arc::new(FakeHttp::default()),
        &home,
    )
    .with_shell_available(Arc::new(|_| true))
    .with_program("/Applications/Devbase.app/Contents/MacOS/Devbase");

    assert!(shell.local_ca_cert_exists());
    assert_eq!(shell.local_ca_cert_path(), cert_path);

    // Coexisting Node.js shell block must survive CA env enable and disable untouched.
    shell.enable_node_shell().unwrap();
    let zshrc_with_node = std::fs::read(&zshrc).unwrap();
    let bashrc_with_node = std::fs::read(&bashrc).unwrap();
    let fish_with_node = std::fs::read(&fish_rc).unwrap();

    shell.enable_ca_env_shell().unwrap();
    assert!(shell.ca_env_config().ca_env_enabled);
    assert!(shell.ca_env_shell_enabled());

    let quoted_posix = shell_quote(&cert_path.to_string_lossy());
    let quoted_fish = fish_quote(&cert_path.to_string_lossy());

    for posix_path in [
        home.join(".zshenv"),
        home.join(".zprofile"),
        zshrc.clone(),
        bashrc.clone(),
        home.join(".bash_profile"),
        shell.bash_env_path(),
    ] {
        let content = std::fs::read_to_string(&posix_path).unwrap();
        assert!(content.contains(CA_ENV_BLOCK_BEGIN), "{posix_path:?}");
        assert!(content.contains(CA_ENV_BLOCK_END), "{posix_path:?}");
        assert!(content.contains(&format!("export NODE_EXTRA_CA_CERTS={quoted_posix}")));
        assert!(content.contains(&format!("export SSL_CERT_FILE={quoted_posix}")));
        assert!(content.contains(&format!("export REQUESTS_CA_BUNDLE={quoted_posix}")));
        assert!(content.contains(&format!("export CURL_CA_BUNDLE={quoted_posix}")));
    }

    let fish_content = std::fs::read_to_string(&fish_rc).unwrap();
    assert!(fish_content.contains(CA_ENV_BLOCK_BEGIN));
    assert!(fish_content.contains(CA_ENV_BLOCK_END));
    assert!(fish_content.contains(&format!("set -gx NODE_EXTRA_CA_CERTS {quoted_fish}")));
    assert!(fish_content.contains(&format!("set -gx SSL_CERT_FILE {quoted_fish}")));
    assert!(fish_content.contains(&format!("set -gx REQUESTS_CA_BUNDLE {quoted_fish}")));
    assert!(fish_content.contains(&format!("set -gx CURL_CA_BUNDLE {quoted_fish}")));

    // Enabling a second time must be byte-for-byte idempotent.
    let files = shell.shell_startup_files(true).unwrap();
    let mut paths: Vec<PathBuf> = files.iter().map(|file| file.path.clone()).collect();
    paths.push(shell.bash_env_path());
    let before: Vec<Vec<u8>> = paths
        .iter()
        .map(|path| std::fs::read(path).unwrap())
        .collect();
    shell.enable_ca_env_shell().unwrap();
    let after: Vec<Vec<u8>> = paths
        .iter()
        .map(|path| std::fs::read(path).unwrap())
        .collect();
    assert_eq!(before, after);

    // Even if the CA cert is removed later, disabling must succeed cleanly and restore prior content.
    std::fs::remove_file(&cert_path).unwrap();
    assert!(!shell.local_ca_cert_exists());
    shell.disable_ca_env_shell().unwrap();
    assert!(!shell.ca_env_config().ca_env_enabled);
    assert!(!shell.ca_env_shell_enabled());

    assert_eq!(std::fs::read(&zshrc).unwrap(), zshrc_with_node);
    assert_eq!(std::fs::read(&bashrc).unwrap(), bashrc_with_node);
    assert_eq!(std::fs::read(&fish_rc).unwrap(), fish_with_node);
    assert!(shell.node_shell_enabled());

    // Removing the Node.js block too returns every user file to its exact original bytes.
    shell.disable_node_shell().unwrap();
    assert_eq!(std::fs::read(&zshrc).unwrap(), original_zshrc);
    assert_eq!(std::fs::read(&bashrc).unwrap(), original_bashrc);
    assert_eq!(std::fs::read(&fish_rc).unwrap(), original_fish);
}

#[test]
fn ca_env_exports_quote_spaces_and_single_quotes() {
    let path = Path::new("/tmp/dev base/o'reilly/proxy/ca.pem");
    let posix = ca_env_block_posix(path);
    assert!(posix.contains("export NODE_EXTRA_CA_CERTS='/tmp/dev base/o'\\''reilly/proxy/ca.pem'"));
    assert!(posix.contains("export CURL_CA_BUNDLE='/tmp/dev base/o'\\''reilly/proxy/ca.pem'"));
    let fish = ca_env_block_fish(path);
    assert!(fish.contains("set -gx NODE_EXTRA_CA_CERTS '/tmp/dev base/o\\'reilly/proxy/ca.pem'"));
    assert!(fish.contains("set -gx CURL_CA_BUNDLE '/tmp/dev base/o\\'reilly/proxy/ca.pem'"));
}

#[test]
fn ca_env_shell_refuses_damaged_markers() {
    let (temp, store) = temp_root();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    write_generational_ca_cert(store.root());
    let zshrc = home.join(".zshrc");
    std::fs::write(&zshrc, format!("{CA_ENV_BLOCK_BEGIN}\nexport A=1\n")).unwrap();

    let shell = shell_with(
        store,
        Arc::new(FakeRunner::default()),
        Arc::new(FakeHttp::default()),
        &home,
    )
    .with_shell_available(Arc::new(|name| name == "zsh"));

    assert!(shell.enable_ca_env_shell().is_err());
    assert!(!shell.ca_env_config().ca_env_enabled);
}

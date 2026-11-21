//! Tests for the software slice. Every test uses a temp root; command and
//! HTTP seams are fakes, so nothing runs `brew`, `launchctl`, `sw_vers` or
//! touches the network.

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tempfile::TempDir;

use super::archive::extract;
use super::cache::DownloadCacheInfo;
use super::catalog;
use super::catalog::available_versions;
use super::integrity::PackageIntegrityResult;
use super::macho::require_apple_silicon;
use super::packages::JAVA_OWNER_FILE;
use super::{HttpTransport, Software, SoftwareError, file_digest, has_digest_file, integrity_path};
use crate::engine::CommandRunner;
use crate::launchd::CommandError;
use crate::model::Package;
use crate::state::{StateError, Store};

// ---------------------------------------------------------------------------
// Fakes
// ---------------------------------------------------------------------------

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
            if program == "/usr/bin/sw_vers" {
                return Ok("26.0\n".into());
            }
            if program == "launchctl" {
                return Err(CommandError::new(
                    program,
                    args.iter().map(|arg| (*arg).to_string()).collect(),
                    "Could not find service",
                ));
            }
            if program == "systemctl" {
                return Ok(
                    "LoadState=not-found\nActiveState=inactive\nSubState=dead\nMainPID=0\nExecMainCode=0\nExecMainStatus=0\nNRestarts=0\n"
                        .into(),
                );
            }
            Err(CommandError::new(
                program,
                args.iter().map(|arg| (*arg).to_string()).collect(),
                "unexpected command",
            ))
        })
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn calls(&self) -> Vec<(String, Vec<String>)> {
        self.calls.lock().unwrap().clone()
    }
}

impl CommandRunner for FakeRunner {
    fn run(
        &self,
        program: &str,
        args: &[&str],
        _timeout: std::time::Duration,
    ) -> Result<String, CommandError> {
        self.calls.lock().unwrap().push((
            program.to_string(),
            args.iter().map(|arg| (*arg).to_string()).collect(),
        ));
        (self.handler)(program, args)
    }
}

type ResponseQueue = BTreeMap<String, VecDeque<Result<Vec<u8>, String>>>;

#[derive(Default)]
struct FakeHttp {
    responses: Mutex<ResponseQueue>,
    sizes: Mutex<BTreeMap<String, i64>>,
}

impl FakeHttp {
    fn set(&self, url: &str, bytes: Vec<u8>) {
        self.responses
            .lock()
            .unwrap()
            .entry(url.to_string())
            .or_default()
            .push_back(Ok(bytes));
    }

    fn push_error(&self, url: &str, message: &str) {
        self.responses
            .lock()
            .unwrap()
            .entry(url.to_string())
            .or_default()
            .push_back(Err(message.into()));
    }

    fn next(&self, url: &str) -> Result<Vec<u8>, SoftwareError> {
        let mut responses = self.responses.lock().unwrap();
        let queue = responses
            .get_mut(url)
            .ok_or_else(|| SoftwareError::Fetch(format!("no response for {url}")))?;
        match queue.pop_front() {
            Some(Ok(bytes)) => Ok(bytes),
            Some(Err(message)) => Err(SoftwareError::Fetch(message)),
            None => Err(SoftwareError::Fetch(format!("no response for {url}"))),
        }
    }
}

impl HttpTransport for FakeHttp {
    fn download(&self, url: &str, destination: &Path) -> Result<u64, SoftwareError> {
        let bytes = self.next(url)?;
        fs::write(destination, &bytes)?;
        Ok(bytes.len() as u64)
    }

    fn get(&self, url: &str) -> Result<Vec<u8>, SoftwareError> {
        self.next(url)
    }

    fn size(&self, url: &str) -> Result<Option<i64>, SoftwareError> {
        Ok(self.sizes.lock().unwrap().get(url).copied())
    }
}

struct Env {
    _dir: TempDir,
    software: Software,
    http: Arc<FakeHttp>,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    runner: Arc<FakeRunner>,
}

fn env() -> Env {
    env_with_runner(FakeRunner::default())
}

fn env_with_runner(runner: FakeRunner) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("dev");
    let home = dir.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let store = Store::new(&root).with_home(&home);
    store.init().unwrap();
    let http = Arc::new(FakeHttp::default());
    let runner = Arc::new(runner);
    let software = Software::new(store)
        .with_runner(runner.clone())
        .with_http(http.clone());
    Env {
        _dir: dir,
        software,
        http,
        runner,
    }
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn zip_archive(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut cursor);
        for (name, bytes, mode) in entries {
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .unix_permissions(*mode);
            writer.start_file(*name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
    }
    cursor.into_inner()
}

fn zip_with_symlink(name: &str, target: &str) -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut cursor);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o755);
        writer.start_file("bin/fixture", options).unwrap();
        writer.write_all(b"#!/bin/sh\nexit 0\n").unwrap();
        writer.start_file("lib/data", options).unwrap();
        writer.write_all(b"trusted library data").unwrap();
        let symlink = zip::write::SimpleFileOptions::default().unix_permissions(0o777);
        writer.add_symlink(name, target, symlink).unwrap();
        writer.finish().unwrap();
    }
    cursor.into_inner()
}

fn tar_archive(entries: &[(&str, u8, &[u8])]) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let encoder = flate2::write::GzEncoder::new(&mut bytes, flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for (name, kind, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o755);
            header.set_size(0);
            header.set_entry_type(if *kind == b'2' {
                tar::EntryType::Symlink
            } else {
                tar::EntryType::Regular
            });
            copy_raw(&mut header, 0, name.as_bytes());
            if *kind == b'2' {
                copy_raw(&mut header, 157, data);
                header.set_cksum();
                builder.append(&header, std::io::empty()).unwrap();
            } else {
                header.set_size(data.len() as u64);
                header.set_cksum();
                builder.append(&header, *data).unwrap();
            }
        }
        builder.finish().unwrap();
    }
    bytes
}

fn copy_raw(header: &mut tar::Header, offset: usize, value: &[u8]) {
    header.as_mut_bytes()[offset..offset + value.len()].copy_from_slice(value);
}

fn archive_package(version: &str, archive: &[u8]) -> Package {
    Package {
        name: "fixture".into(),
        version: version.into(),
        arch: crate::platform::HostTarget::current().arch_name().into(),
        url: "https://unused.invalid/fixture.zip".into(),
        sha256: crate::fs::digest_bytes(archive),
        format: "zip".into(),
        binaries: BTreeMap::from([("fixture".into(), "bin/fixture".into())]),
        license: "test".into(),
        ..Package::default()
    }
}

fn integrity_archive() -> Vec<u8> {
    zip_with_symlink("lib/current", "data")
}

fn fixture_software() -> (Env, Package, Vec<u8>) {
    let env = env();
    let archive = integrity_archive();
    let package = archive_package("1.0.0", &archive);
    let cache = env
        .software
        .store()
        .download_directory()
        .join(&package.sha256);
    fs::create_dir_all(cache.parent().unwrap()).unwrap();
    fs::write(&cache, &archive).unwrap();
    env.software.install(&package, &|| false).unwrap();
    (env, package, archive)
}

fn assert_integrity(
    software: &Software,
    package: &Package,
    status: &str,
) -> PackageIntegrityResult {
    let result = software.verify_package(&package.id(), &|| false).unwrap();
    assert_eq!(result.status, status, "{}", result.text());
    result
}

// ---------------------------------------------------------------------------
// Catalog
// ---------------------------------------------------------------------------

#[test]
fn catalog_pins_exact_releases_and_lists_versions() {
    let target = crate::platform::HostTarget::current();
    let package = catalog::resolve("mysql", "8.4.13").unwrap();
    assert_eq!(package.arch, target.arch_name());
    assert_eq!(package.strip, 1);
    if target.is_macos() {
        assert_eq!(package.format, "tar.gz");
        assert_eq!(package.min_macos, "26.0");
        assert_eq!(
            package.url,
            "https://dev.mysql.com/get/Downloads/MySQL-8.4/mysql-8.4.13-macos15-arm64.tar.gz"
        );
        assert_eq!(
            package.sha256,
            "f324f8950e24d4147470bbc6a6ef0b90ef93b91d3fc74154b3d2ba085b44912a"
        );
    } else {
        assert_eq!(package.format, "tar.xz");
        assert!(package.url.contains("mysql-8.4.13-linux-"));
        assert_eq!(package.sha256.len(), 64);
    }
    assert_eq!(package.binaries["mysqld"], "bin/mysqld");
    assert!(package.validate().is_ok());

    let mongodb = catalog::resolve("mongodb", "v8.0.38").unwrap();
    assert_eq!(mongodb.version, "8.0.38");
    if target.is_macos() {
        assert_eq!(
            mongodb.url,
            "https://fastdl.mongodb.org/osx/mongodb-macos-arm64-8.0.38.tgz"
        );
        assert_eq!(
            available_versions("mysql"),
            vec!["8.4.13", "8.4.4", "8.0.45", "8.0.41"]
        );
    } else {
        assert!(mongodb.url.contains("mongodb-linux-"));
        assert_eq!(available_versions("mysql"), vec!["8.4.13", "8.0.45"]);
    }
    assert_eq!(available_versions("mongodb").len(), 4);
    if target.is_macos() {
        assert_eq!(available_versions("postgresql"), vec!["17", "18", "16"]);
        assert_eq!(available_versions("redis"), vec!["8.2"]);
    } else {
        assert_eq!(
            available_versions("postgresql"),
            vec!["system", "17", "18", "16"]
        );
        assert_eq!(available_versions("redis"), vec!["system", "8.2"]);
    }
    assert_eq!(available_versions("node"), vec!["22.14.0", "20.19.0"]);
    assert!(available_versions("unknown").is_empty());
    assert!(catalog::resolve("mysql", "9.9.9").is_err());
    assert!(catalog::resolve("node", "22.14.0").is_err());
    assert!(catalog::resolve("java", "21").is_err());
}

#[test]
fn node_checksums_are_resolved_from_shasums() {
    let env = env();
    let target = crate::platform::HostTarget::current();
    let sha = "a".repeat(64);
    let base = "https://nodejs.org/dist/v22.14.0/";
    let archive_name = format!("node-v22.14.0-{}.tar.gz", target.node_archive_platform());
    env.http.set(
        &format!("{base}SHASUMS256.txt"),
        format!(
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  node-v22.14.0-darwin-x64.tar.gz\n{sha}  {archive_name}\n"
        )
        .into_bytes(),
    );
    let package = catalog::resolve_node(env.http.as_ref(), "22.14.0").unwrap();
    assert_eq!(package.url, format!("{base}{archive_name}"));
    assert_eq!(package.sha256, sha);
    assert_eq!(package.license, "MIT");
    assert_eq!(package.binaries["node"], "bin/node");
    assert!(package.validate().is_ok());

    assert!(
        catalog::resolve_node(env.http.as_ref(), "22")
            .unwrap_err()
            .to_string()
            .contains("exact Node.js version")
    );
    let missing = catalog::resolve_node(env.http.as_ref(), "20.19.0");
    assert!(missing.is_err());
}

// ---------------------------------------------------------------------------
// Mach-O
// ---------------------------------------------------------------------------

const ARM64: u32 = 0x0100_000c;
const X86_64: u32 = 0x0100_0007;

fn thin_macho(cpu: u32) -> Vec<u8> {
    let mut bytes = vec![0u8; 32];
    bytes[0..4].copy_from_slice(&0xfeed_facfu32.to_be_bytes());
    bytes[4..8].copy_from_slice(&cpu.to_be_bytes());
    bytes[12..16].copy_from_slice(&2u32.to_be_bytes());
    bytes
}

fn fat_macho(cpu: u32) -> Vec<u8> {
    let thin = thin_macho(cpu);
    let mut bytes = Vec::new();
    bytes.extend(0xcafe_babeu32.to_be_bytes());
    bytes.extend(1u32.to_be_bytes());
    bytes.extend(cpu.to_be_bytes());
    bytes.extend(0u32.to_be_bytes());
    bytes.extend(28u32.to_be_bytes());
    bytes.extend(thin.len().to_be_bytes());
    bytes.extend(4u32.to_be_bytes());
    bytes.extend(thin);
    bytes
}

#[test]
fn macho_requires_an_arm64_slice() {
    let dir = tempfile::tempdir().unwrap();
    let cases: [(&str, Vec<u8>, bool); 5] = [
        ("thin-arm64", thin_macho(ARM64), true),
        ("thin-intel", thin_macho(X86_64), false),
        ("fat-arm64", fat_macho(ARM64), true),
        ("fat-intel", fat_macho(X86_64), false),
        ("script", b"#!/bin/sh\nexit 0\n".to_vec(), true),
    ];
    for (name, bytes, accept) in cases {
        let path = dir.path().join(name);
        fs::write(&path, bytes).unwrap();
        let result = require_apple_silicon(&path);
        assert_eq!(result.is_ok(), accept, "{name}: {result:?}");
    }
}

#[cfg(target_os = "macos")]
#[test]
fn install_rejects_an_intel_binary_before_publish() {
    for arch in ["arm64", "universal"] {
        let env = env();
        let archive = zip_archive(&[("bin/intel", &thin_macho(X86_64), 0o755)]);
        let mut package = archive_package("1.0.0", &archive);
        package.name = "incompatible".into();
        package.arch = arch.into();
        package.binaries = BTreeMap::from([("intel".into(), "bin/intel".into())]);
        let cache = env
            .software
            .store()
            .download_directory()
            .join(&package.sha256);
        fs::create_dir_all(cache.parent().unwrap()).unwrap();
        fs::write(&cache, &archive).unwrap();
        let error = env.software.install(&package, &|| false).unwrap_err();
        assert!(error.to_string().contains("Apple Silicon"), "{error}");
        let state = env.software.store().read().unwrap();
        assert!(state.packages.is_empty());
        let entries: Vec<_> = fs::read_dir(env.software.store().root().join("packages"))
            .unwrap()
            .collect();
        assert!(entries.is_empty(), "staged binary left behind");
    }
}

// ---------------------------------------------------------------------------
// Install, publish, rollback
// ---------------------------------------------------------------------------

#[test]
fn install_stages_verifies_publishes_and_registers() {
    let env = env();
    let archive = integrity_archive();
    let package = archive_package("1.0.0", &archive);
    env.http.set(&package.url, archive.clone());
    env.software.install(&package, &|| false).unwrap();

    let state = env.software.store().read().unwrap();
    let installed = &state.packages[&package.id()];
    assert_eq!(installed.sha256, package.sha256);
    assert!(state.package_integrity.contains_key(&package.id()));
    let directory = env.software.store().package_directory(&package);
    assert!(directory.join(".package.json").is_file());
    assert!(directory.join("bin/fixture").is_file());
    assert!(directory.join("lib/current").is_symlink());
    let index = integrity_path(env.software.store(), &package.id());
    assert!(index.is_file());
    let binary = env
        .software
        .binary(&state, &package.id(), "fixture")
        .unwrap();
    assert_eq!(binary, directory.join("bin/fixture"));
    assert!(
        env.software
            .binary(&state, &package.id(), "missing")
            .is_err()
    );

    let report = assert_integrity(&env.software, &package, "Verified");
    assert!(report.files >= 5);
    assert_eq!(report.baseline, state.package_integrity[&package.id()]);
    // The cache used by the fake download was published under its digest.
    assert!(
        env.software
            .store()
            .download_directory()
            .join(&package.sha256)
            .is_file()
    );
}

#[test]
fn install_rolls_back_when_state_cannot_be_written() {
    let (env, first, _) = fixture_software();
    let directory = env.software.store().root().to_path_buf();
    let state_path = directory.join("state.json");
    let saved = fs::read(&state_path).unwrap();
    let archive = integrity_archive();
    let package = archive_package("2.0.0", &archive);
    let cache = env
        .software
        .store()
        .download_directory()
        .join(&package.sha256);
    fs::write(&cache, &archive).unwrap();
    let event_path = state_path.clone();
    let software = env.software.clone().with_event(move |message| {
        if message.starts_with("Committing package files for fixture@2.0.0") {
            let _ = fs::remove_file(&event_path);
            let _ = fs::create_dir(&event_path);
        }
    });
    let error = software.install(&package, &|| false).unwrap_err();
    assert!(matches!(error, SoftwareError::State(_)), "{error}");
    assert!(!env.software.store().package_directory(&package).exists());
    assert!(!integrity_path(env.software.store(), &package.id()).exists());
    // Restore the document and prove the first package is unaffected.
    fs::remove_dir(directory.join("state.json")).unwrap();
    fs::write(&state_path, saved).unwrap();
    let state = env.software.store().read().unwrap();
    assert!(state.packages.contains_key(&first.id()));
    assert!(!state.packages.contains_key(&package.id()));
    assert_integrity(&env.software, &first, "Verified");
}

#[test]
fn install_refuses_an_unregistered_destination_directory() {
    let env = env();
    let archive = integrity_archive();
    let package = archive_package("1.0.0", &archive);
    let cache = env
        .software
        .store()
        .download_directory()
        .join(&package.sha256);
    fs::write(&cache, &archive).unwrap();
    let destination = env.software.store().package_directory(&package);
    fs::create_dir_all(&destination).unwrap();
    fs::write(destination.join(".package.json"), b"{}").unwrap();
    let error = env.software.install(&package, &|| false).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unregistered package directory exists"),
        "{error}"
    );
    assert!(destination.is_dir());
}

// ---------------------------------------------------------------------------
// Archive safety
// ---------------------------------------------------------------------------

#[test]
fn archives_reject_traversal_and_escaping_symlinks() {
    for bad in ["../outside", "/tmp/outside"] {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("bad.tar.gz");
        fs::write(&source, tar_archive(&[(bad, b'0', b"x")])).unwrap();
        let root = dir.path().join("root");
        fs::create_dir_all(&root).unwrap();
        let error = extract(&source, &root, "tar.gz", 0, &|| false).unwrap_err();
        assert!(error.to_string().contains("unsafe archive path"), "{error}");
    }
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("link.tar.gz");
    fs::write(&source, tar_archive(&[("link", b'2', b"../outside")])).unwrap();
    let root = dir.path().join("root");
    fs::create_dir_all(&root).unwrap();
    let error = extract(&source, &root, "tar.gz", 0, &|| false).unwrap_err();
    assert!(
        error.to_string().contains("unsafe archive symlink"),
        "{error}"
    );
    assert!(!root.join("link").exists());

    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("bad.zip");
    fs::write(&source, zip_with_symlink("link", "../../outside")).unwrap();
    let root = dir.path().join("root");
    fs::create_dir_all(&root).unwrap();
    let error = extract(&source, &root, "zip", 0, &|| false).unwrap_err();
    assert!(
        error.to_string().contains("unsafe archive symlink"),
        "{error}"
    );
}

#[test]
fn install_cleans_staging_after_an_unsafe_archive() {
    let env = env();
    let archive = tar_archive(&[("link", b'2', b"../../outside")]);
    let mut package = archive_package("1.0.0", &archive);
    package.format = "tar.gz".into();
    let cache = env
        .software
        .store()
        .download_directory()
        .join(&package.sha256);
    fs::write(&cache, &archive).unwrap();
    let error = env.software.install(&package, &|| false).unwrap_err();
    assert!(
        error.to_string().contains("unsafe archive symlink"),
        "{error}"
    );
    let state = env.software.store().read().unwrap();
    assert!(state.packages.is_empty());
    assert!(
        fs::read_dir(env.software.store().root().join("packages"))
            .unwrap()
            .next()
            .is_none()
    );
}

// ---------------------------------------------------------------------------
// Integrity
// ---------------------------------------------------------------------------

#[test]
fn integrity_detects_content_mode_links_and_index_tampering() {
    let (env, package, archive) = fixture_software();
    let software = &env.software;
    let directory = software.store().package_directory(&package);
    assert!(assert_integrity(software, &package, "Verified").files >= 5);

    fs::write(directory.join("lib/data"), b"changed library data").unwrap();
    let changed = assert_integrity(software, &package, "Needs repair");
    assert!(changed.text().contains("lib/data"), "{}", changed.text());
    let error = software.install(&package, &|| false).unwrap_err();
    assert!(
        error.to_string().contains("incomplete or changed")
            || error.to_string().contains("Needs repair"),
        "{error}"
    );
    software.repair_package(&package.id(), &|| false).unwrap();
    assert_integrity(software, &package, "Verified");

    fs::remove_file(directory.join("lib/data")).unwrap();
    assert!(
        assert_integrity(software, &package, "Needs repair")
            .text()
            .contains("Missing: lib/data")
    );
    software.repair_package(&package.id(), &|| false).unwrap();

    fs::write(directory.join("unexpected"), b"extra").unwrap();
    assert!(
        assert_integrity(software, &package, "Needs repair")
            .text()
            .contains("Unexpected: unexpected")
    );
    software.repair_package(&package.id(), &|| false).unwrap();

    let mut permissions = fs::metadata(directory.join("bin/fixture"))
        .unwrap()
        .permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o600);
    fs::set_permissions(directory.join("bin/fixture"), permissions).unwrap();
    assert_integrity(software, &package, "Needs repair");
    software.repair_package(&package.id(), &|| false).unwrap();

    let outside = env._dir.path().join("secret");
    fs::write(&outside, b"NEVER-READ-THIS").unwrap();
    let link = directory.join("lib/current");
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let escaped = assert_integrity(software, &package, "Needs repair");
    assert!(
        !escaped.text().contains("NEVER-READ-THIS"),
        "{}",
        escaped.text()
    );
    software.repair_package(&package.id(), &|| false).unwrap();

    fs::write(
        integrity_path(software.store(), &package.id()),
        b"{\"schema\":1,\"files\":{}}",
    )
    .unwrap();
    assert_integrity(software, &package, "Needs repair");
    software.repair_package(&package.id(), &|| false).unwrap();
    assert_integrity(software, &package, "Verified");
    assert!(!archive.is_empty());
}

#[test]
fn integrity_reports_not_recorded_and_homebrew_statuses() {
    let (env, package, _) = fixture_software();
    env.software
        .store()
        .with_state(|state| {
            state.package_integrity.remove(&package.id());
            Ok(())
        })
        .unwrap();
    let result = assert_integrity(&env.software, &package, "Not recorded");
    assert!(result.baseline.is_empty());
    assert!(result.text().contains("No trusted installation index"));
}

#[test]
fn integrity_index_special_file_does_not_block() {
    let (env, package, _) = fixture_software();
    let index = integrity_path(env.software.store(), &package.id());
    fs::remove_file(&index).unwrap();
    let c_path = std::ffi::CString::new(index.to_str().unwrap()).unwrap();
    let result = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
    assert_eq!(result, 0);
    let report = assert_integrity(&env.software, &package, "Needs repair");
    assert!(
        report.text().contains("not a regular file") || report.text().contains("could not be read")
    );
}

// ---------------------------------------------------------------------------
// Repair rollback and download failure
// ---------------------------------------------------------------------------

#[test]
fn repair_rolls_back_cancellation_and_state_write_failure() {
    for fail_commit in [false, true] {
        let (env, package, _) = fixture_software();
        let directory = env.software.store().package_directory(&package);
        let file = directory.join("lib/data");
        fs::write(&file, b"old damaged bytes").unwrap();
        let index = integrity_path(env.software.store(), &package.id());
        let index_before = fs::read(&index).unwrap();
        let state_path = env.software.store().root().join("state.json");
        let saved = fs::read(&state_path).unwrap();
        let sentinel = env.software.store().root().join("instances/untouched/data");
        fs::create_dir_all(sentinel.parent().unwrap()).unwrap();
        fs::write(&sentinel, b"database data").unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let software = {
            let cancelled = cancelled.clone();
            let state_path = state_path.clone();
            env.software.clone().with_event(move |message| {
                if !message.starts_with("Committing package files") {
                    return;
                }
                if fail_commit {
                    let _ = fs::remove_file(&state_path);
                    let _ = fs::create_dir(&state_path);
                } else {
                    cancelled.store(true, Ordering::SeqCst);
                }
            })
        };
        let error = software
            .repair_package(&package.id(), &|| cancelled.load(Ordering::SeqCst))
            .unwrap_err();
        if fail_commit {
            assert!(matches!(error, SoftwareError::State(_)), "{error}");
            fs::remove_dir(&state_path).unwrap();
            fs::write(&state_path, &saved).unwrap();
        } else {
            assert!(matches!(error, SoftwareError::State(StateError::Cancelled)));
        }
        assert_eq!(fs::read(&file).unwrap(), b"old damaged bytes");
        assert_eq!(fs::read(&index).unwrap(), index_before);
        assert_eq!(fs::read(&sentinel).unwrap(), b"database data");
        assert_integrity(&software, &package, "Needs repair");
    }
}

#[test]
fn repair_download_failure_keeps_the_original_and_can_retry() {
    let (env, package, archive) = fixture_software();
    let directory = env.software.store().package_directory(&package);
    let file = directory.join("lib/data");
    fs::write(&file, b"old damaged bytes").unwrap();
    let cache = env
        .software
        .store()
        .download_directory()
        .join(&package.sha256);
    fs::write(&cache, b"damaged cache").unwrap();
    assert_eq!(file_digest(&cache, &|| false).unwrap().len(), 64);
    assert!(!has_digest_file(&cache, &package.sha256, &|| false));
    env.http.push_error(&package.url, "503");
    let error = env
        .software
        .repair_package(&package.id(), &|| false)
        .unwrap_err();
    assert!(error.to_string().contains("503"), "{error}");
    env.http.set(&package.url, b"wrong archive bytes".to_vec());
    let error = env
        .software
        .repair_package(&package.id(), &|| false)
        .unwrap_err();
    assert!(error.to_string().contains("checksum mismatch"), "{error}");
    assert_eq!(fs::read(&file).unwrap(), b"old damaged bytes");
    env.http.set(&package.url, archive);
    env.software
        .repair_package(&package.id(), &|| false)
        .unwrap();
    assert_integrity(&env.software, &package, "Verified");
}

// ---------------------------------------------------------------------------
// Homebrew
// ---------------------------------------------------------------------------

fn brew_metadata(full_name: &str) -> String {
    serde_json::json!({
        "formulae": [{
            "name": full_name.rsplit('/').next().unwrap(),
            "full_name": full_name,
            "license": "test license",
            "revision": 1,
            "versions": {"stable": "17.11"},
            "ruby_source_checksum": {"sha256": "a".repeat(64)}
        }]
    })
    .to_string()
}

fn brew_env(cellar: PathBuf, metadata: String) -> Env {
    let cellar_for_install = cellar.clone();
    let runner = FakeRunner::new(move |program, args| {
        if program == "launchctl" {
            return Err(CommandError::new(
                program,
                args.iter().map(|value| (*value).to_string()).collect(),
                "Could not find service",
            ));
        }
        if program != "/usr/bin/env" || args.len() < 5 || args[3] != "/opt/homebrew/bin/brew" {
            return Err(CommandError::new(
                program,
                args.iter().map(|value| (*value).to_string()).collect(),
                "unexpected command",
            ));
        }
        match args[4] {
            "info" => Ok(metadata.clone()),
            "install" => {
                write_brew_keg(
                    &cellar_for_install,
                    "postgresql@17",
                    "17.11_1",
                    "homebrew/core",
                );
                Ok("installed".into())
            }
            _ => Err(CommandError::new(
                program,
                args.iter().map(|value| (*value).to_string()).collect(),
                "unexpected Homebrew command",
            )),
        }
    });
    let mut env = env_with_runner(runner);
    env.software = env.software.with_brew_cellar(cellar);
    env
}

fn write_brew_keg(cellar: &Path, formula: &str, version: &str, tap: &str) {
    let keg = cellar.join(formula).join(version);
    for binary in [
        "postgres",
        "initdb",
        "pg_dump",
        "pg_restore",
        "psql",
        "createdb",
    ] {
        let path = keg.join("bin").join(binary);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"#!/bin/sh\nexit 0\n").unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        fs::set_permissions(&path, permissions).unwrap();
    }
    let receipt = serde_json::json!({
        "source": {"tap": tap, "versions": {"stable": "17.11"}}
    });
    fs::write(
        keg.join("INSTALL_RECEIPT.json"),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn homebrew_install_validates_receipt_and_never_deletes_shared_software() {
    let cellar = tempfile::tempdir().unwrap();
    let env = brew_env(cellar.path().to_path_buf(), brew_metadata("postgresql@17"));
    let package = env.software.resolve_package("postgresql", "17").unwrap();
    assert_eq!(package.version, "17.11_1");
    assert_eq!(package.formula, "postgresql@17");
    assert_eq!(package.format, "homebrew");
    assert_eq!(package.license, "test license");
    assert_eq!(package.binaries["postgres"], "bin/postgres");
    assert!(package.validate().is_ok());

    let details = env.software.package_details(&|| false, &package).unwrap();
    assert!(!details.installed);
    assert_eq!(
        details.location,
        env.software.store().package_directory(&package)
    );
    let information_calls = env
        .runner
        .calls()
        .into_iter()
        .filter(|(_, args)| args.get(4).map(String::as_str) == Some("info"))
        .count();
    assert_eq!(information_calls, 1);
    assert_eq!(
        env.runner
            .calls()
            .iter()
            .filter(|(_, args)| args.get(4).map(String::as_str) == Some("install"))
            .count(),
        0
    );

    env.software.install(&package, &|| false).unwrap();
    let state = env.software.store().read().unwrap();
    let binary = env
        .software
        .binary(&state, &package.id(), "postgres")
        .unwrap();
    assert!(binary.starts_with(cellar.path()));
    assert!(!binary.to_string_lossy().contains("/opt/"));
    let report = assert_integrity(&env.software, &package, "Homebrew managed");
    assert_eq!(report.files, 6);

    env.software.uninstall(&package.id(), &|| false).unwrap();
    assert!(binary.exists(), "shared Homebrew keg was deleted");
    assert!(
        !env.software
            .store()
            .read()
            .unwrap()
            .packages
            .contains_key(&package.id())
    );
    for (_, args) in env.runner.calls() {
        if args.len() >= 5 && args[3] == "/opt/homebrew/bin/brew" {
            assert!(
                args[4] == "info"
                    || args[4..] == ["install", "--formula", "--force-bottle", "postgresql@17"],
                "unexpected Homebrew mutation {args:?}"
            );
        }
    }
}

#[cfg(target_os = "macos")]
#[test]
fn homebrew_rejects_foreign_taps_formula_drift_and_archive_repair() {
    let cellar = tempfile::tempdir().unwrap();
    let env = brew_env(cellar.path().to_path_buf(), brew_metadata("postgresql@17"));
    let package = env.software.resolve_package("postgresql", "17").unwrap();
    write_brew_keg(cellar.path(), "postgresql@17", "17.11_1", "other/tap");
    let error = env.software.install(&package, &|| false).unwrap_err();
    assert!(error.to_string().contains("different tap"), "{error}");
    for (_, args) in env.runner.calls() {
        if args.len() >= 5 && args[3] == "/opt/homebrew/bin/brew" {
            assert_eq!(args[4], "info", "mutated a conflicting keg");
        }
    }
    fs::remove_dir_all(env.software.store().package_directory(&package)).unwrap();
    let mut drifted = package.clone();
    drifted.sha256 = "b".repeat(64);
    let error = env.software.install(&drifted, &|| false).unwrap_err();
    assert!(
        error.to_string().contains("changed since preview"),
        "{error}"
    );
    assert!(env.software.store().read().unwrap().packages.is_empty());

    write_brew_keg(cellar.path(), "postgresql@17", "17.11_1", "homebrew/core");
    env.software.install(&package, &|| false).unwrap();
    fs::rename(
        env.software.store().package_directory(&package),
        env.software
            .store()
            .package_directory(&package)
            .with_extension("new"),
    )
    .unwrap();
    let state = env.software.store().read().unwrap();
    assert!(
        env.software
            .binary(&state, &package.id(), "postgres")
            .is_err()
    );
    let details = env.software.package_details(&|| false, &package).unwrap();
    assert_eq!(details.integrity, "Needs repair");
    let error = env
        .software
        .repair_package(&package.id(), &|| false)
        .unwrap_err();
    assert!(error.to_string().contains("brew reinstall"), "{error}");

    assert!(env.software.resolve_package("postgresql", "17.10").is_err());
    assert!(env.software.resolve_package("postgresql", "18").is_err());
}

#[cfg(target_os = "macos")]
#[test]
fn homebrew_failure_registers_nothing() {
    let cellar = tempfile::tempdir().unwrap();
    let env = brew_env(cellar.path().to_path_buf(), brew_metadata("postgresql@17"));
    let package = env.software.resolve_package("postgresql", "17").unwrap();
    let failing = FakeRunner::new(|_program, _args| {
        Err(CommandError::new("brew", Vec::new(), "brew unavailable"))
    });
    let software = env.software.clone().with_runner(Arc::new(failing));
    let error = software.install(&package, &|| false).unwrap_err();
    assert!(error.to_string().contains("brew unavailable"), "{error}");
    assert!(env.software.store().read().unwrap().packages.is_empty());
}

#[test]
fn homebrew_metadata_must_be_one_formula_from_homebrew_core() {
    let cellar = tempfile::tempdir().unwrap();
    let env = brew_env(
        cellar.path().to_path_buf(),
        brew_metadata("other/tap/postgresql@17"),
    );
    assert!(env.software.resolve_package("postgresql", "17").is_err());
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

#[test]
fn download_cache_info_counts_files_and_clearing_refuses_active_install() {
    let env = env();
    let directory = env.software.store().download_directory();
    fs::create_dir_all(&directory).unwrap();
    let a = directory.join("archive-a");
    let b = directory.join("archive-b");
    fs::write(&a, b"12345").unwrap();
    fs::write(&b, b"67890123").unwrap();
    fs::create_dir_all(directory.join("nested-dir")).unwrap();
    std::os::unix::fs::symlink(&a, directory.join("link-a")).unwrap();

    let shared = env
        .software
        .store()
        .package_lease("mysql@8.4.4-arm64", false)
        .unwrap();
    let info = env.software.download_cache_info().unwrap();
    assert_eq!(info.directory, directory);
    assert_eq!(info.files, 2);
    assert_eq!(info.bytes, 13);
    drop(shared);

    let exclusive = env
        .software
        .store()
        .package_lease("node@22.14.0-arm64", true)
        .unwrap();
    let error = env.software.clear_download_cache(&|| false).unwrap_err();
    match error {
        SoftwareError::State(StateError::PackageBusy(busy)) => {
            assert_eq!(busy.package_id, "node@22.14.0-arm64")
        }
        other => panic!("expected PackageBusyError, got {other}"),
    }
    drop(exclusive);

    let installed = env
        .software
        .store()
        .root()
        .join("packages/node@22.14.0-arm64/bin/node");
    fs::create_dir_all(installed.parent().unwrap()).unwrap();
    fs::write(&installed, b"binary").unwrap();

    let removed = env.software.clear_download_cache(&|| false).unwrap();
    assert_eq!(
        removed,
        DownloadCacheInfo {
            directory: directory.clone(),
            files: 2,
            bytes: 13
        }
    );
    let after = env.software.download_cache_info().unwrap();
    assert_eq!(after.files, 0);
    assert_eq!(after.bytes, 0);
    assert!(installed.is_file());
    assert!(directory.join("nested-dir").is_dir());
    assert!(directory.join("link-a").is_symlink());
}

// ---------------------------------------------------------------------------
// Cancellation
// ---------------------------------------------------------------------------

#[test]
fn cancelled_install_leaves_no_staging_or_registration() {
    let env = env();
    let archive = integrity_archive();
    let package = archive_package("1.0.0", &archive);
    let cache = env
        .software
        .store()
        .download_directory()
        .join(&package.sha256);
    fs::write(&cache, &archive).unwrap();
    let error = env.software.install(&package, &|| true).unwrap_err();
    assert!(matches!(error, SoftwareError::State(StateError::Cancelled)));
    assert!(env.software.store().read().unwrap().packages.is_empty());
    assert!(
        fs::read_dir(env.software.store().root().join("packages"))
            .unwrap()
            .next()
            .is_none()
    );
}

#[test]
fn java_owner_marker_is_required_for_uninstall() {
    let env = env();
    let archive = zip_archive(&[
        ("bin/java", b"#!/bin/sh\nexit 0\n", 0o755),
        ("release", b"JAVA_VERSION=\"21.0.1\"\n", 0o644),
    ]);
    let mut package = archive_package("21.0.1", &archive);
    package.name = "java".into();
    package.license = "GPL-2.0".into();
    package.java_install_name = "temurin-21.0.1".into();
    package.binaries = BTreeMap::from([("java".into(), "bin/java".into())]);
    let cache = env
        .software
        .store()
        .download_directory()
        .join(&package.sha256);
    fs::write(&cache, &archive).unwrap();
    env.software.install(&package, &|| false).unwrap();
    let state = env.software.store().read().unwrap();
    let installed = &state.packages[&package.id()];
    let directory = env.software.store().package_directory(installed);
    assert!(directory.join(JAVA_OWNER_FILE).is_file());
    fs::remove_file(directory.join(JAVA_OWNER_FILE)).unwrap();
    let error = env
        .software
        .uninstall(&package.id(), &|| false)
        .unwrap_err();
    assert!(error.to_string().contains("ownership"), "{error}");
    assert!(directory.is_dir());
}

#[test]
fn package_install_is_busy_while_an_exclusive_lease_is_held() {
    let env = env();
    let archive = integrity_archive();
    let package = archive_package("1.0.0", &archive);
    let exclusive = env
        .software
        .store()
        .package_lease(&package.id(), true)
        .unwrap();
    let error = env.software.install(&package, &|| false).unwrap_err();
    assert!(error.is_busy(), "{error}");
    drop(exclusive);
    let cache = env
        .software
        .store()
        .download_directory()
        .join(&package.sha256);
    fs::write(&cache, &archive).unwrap();
    env.software.install(&package, &|| false).unwrap();
}

#[test]
fn package_state_and_index_are_removed_on_uninstall() {
    let (env, package, _) = fixture_software();
    env.software.uninstall(&package.id(), &|| false).unwrap();
    assert!(!env.software.store().package_directory(&package).exists());
    assert!(!integrity_path(env.software.store(), &package.id()).exists());
    assert!(env.software.store().read().unwrap().packages.is_empty());
    assert!(env.software.uninstall(&package.id(), &|| false).is_err());
}

#[test]
fn uninstall_refuses_live_references() {
    let (env, package, _) = fixture_software();
    let directory = env.software.store().package_directory(&package);
    env.software
        .store()
        .with_state(|state| {
            state.instances.insert(
                "db".into(),
                crate::model::Instance {
                    id: "db".into(),
                    kind: "redis".into(),
                    package: package.id(),
                    port: 6379,
                    ..crate::model::Instance::default()
                },
            );
            Ok(())
        })
        .unwrap();
    let error = env
        .software
        .uninstall(&package.id(), &|| false)
        .unwrap_err();
    assert!(error.to_string().contains("used by instance db"), "{error}");
    env.software
        .store()
        .with_state(|state| {
            state.instances.clear();
            Ok(())
        })
        .unwrap();

    let jobs = env.software.store().root().join("jobs");
    fs::create_dir_all(&jobs).unwrap();
    let label = format!(
        "xyz.jyeu.devbase.dev.{}.orphan",
        crate::engine::label_prefix(env.software.store().root())
    );
    let value = plist::Value::Dictionary(plist::Dictionary::from_iter([
        ("Label".to_string(), plist::Value::String(label)),
        (
            "ProgramArguments".to_string(),
            plist::Value::Array(vec![plist::Value::String(
                directory.join("bin/fixture").to_string_lossy().into_owned(),
            )]),
        ),
    ]));
    value.to_file_xml(jobs.join("orphan.plist")).unwrap();
    let error = env
        .software
        .uninstall(&package.id(), &|| false)
        .unwrap_err();
    assert!(
        error.to_string().contains("referenced by a launchd job")
            || error
                .to_string()
                .contains("referenced by a managed service job"),
        "{error}"
    );
    assert!(directory.is_dir());
}

#[test]
fn idle_check_treats_unknown_launchd_state_as_busy() {
    let loaded = Arc::new(AtomicBool::new(false));
    let unavailable = Arc::new(AtomicBool::new(false));
    let handler_loaded = loaded.clone();
    let handler_unavailable = unavailable.clone();
    let runner = FakeRunner::new(move |program, args| {
        let make_error = |message: &str| {
            CommandError::new(
                program,
                args.iter().map(|value| (*value).to_string()).collect(),
                message,
            )
        };
        if program == "/usr/bin/sw_vers" {
            return Ok("26.0\n".into());
        }
        if program == "launchctl" {
            if handler_unavailable.load(Ordering::SeqCst) {
                return Err(make_error("launchd query unavailable"));
            }
            if handler_loaded.load(Ordering::SeqCst) {
                return Ok("state = running".into());
            }
            return Err(make_error("Could not find service"));
        }
        if program == "systemctl" {
            if handler_unavailable.load(Ordering::SeqCst) {
                return Err(make_error("systemd query unavailable"));
            }
            if handler_loaded.load(Ordering::SeqCst) {
                return Ok(
                    "LoadState=loaded\nActiveState=active\nSubState=running\nMainPID=42\nExecMainCode=0\nExecMainStatus=0\nNRestarts=0\n"
                        .into(),
                );
            }
            return Ok(
                "LoadState=not-found\nActiveState=inactive\nSubState=dead\nMainPID=0\nExecMainCode=0\nExecMainStatus=0\nNRestarts=0\n"
                    .into(),
            );
        }
        Err(make_error("unexpected command"))
    });
    let env = env_with_runner(runner);
    let archive = integrity_archive();
    let package = archive_package("1.0.0", &archive);
    let cache = env
        .software
        .store()
        .download_directory()
        .join(&package.sha256);
    fs::write(&cache, &archive).unwrap();
    env.software.install(&package, &|| false).unwrap();
    let directory = env.software.store().package_directory(&package);
    let jobs = env.software.store().root().join("jobs");
    fs::create_dir_all(&jobs).unwrap();
    let label = format!(
        "xyz.jyeu.devbase.dev.{}.orphan",
        crate::engine::label_prefix(env.software.store().root())
    );
    let value = plist::Value::Dictionary(plist::Dictionary::from_iter([
        ("Label".to_string(), plist::Value::String(label)),
        (
            "ProgramArguments".to_string(),
            plist::Value::Array(vec![plist::Value::String(
                directory.join("bin/fixture").to_string_lossy().into_owned(),
            )]),
        ),
    ]));
    value.to_file_xml(jobs.join("orphan.plist")).unwrap();

    loaded.store(true, Ordering::SeqCst);
    let error = env
        .software
        .repair_package(&package.id(), &|| false)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("stop the launchd job using this package")
            || error
                .to_string()
                .contains("stop the managed service job using this package"),
        "{error}"
    );
    loaded.store(false, Ordering::SeqCst);
    unavailable.store(true, Ordering::SeqCst);
    let error = env
        .software
        .repair_package(&package.id(), &|| false)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("could not verify whether the package is in use"),
        "{error}"
    );
    unavailable.store(false, Ordering::SeqCst);
    env.software
        .repair_package(&package.id(), &|| false)
        .unwrap();
    assert_integrity(&env.software, &package, "Verified");
}

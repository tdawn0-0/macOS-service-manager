use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use plist::{Dictionary, Uid, Value};
use tempfile::TempDir;

use super::database::{APPLE_EPOCH_OFFSET, database_file, format_local_ymd_hm, generated_uid};
use super::*;
use crate::launchd::{Job, LaunchdError};

/// Trimmed output of `sfltool dumpbtm`.
const DUMP: &str = "\
========================
 Records for UID -2 : FFFFEEEE-DDDD-CCCC-BBBB-AAAAFFFFFFFE
========================

 Items:

 #1:
                 UUID: AAAAAAAA-0000-0000-0000-000000000001
                 Name: bash
                 Type: legacy daemon (0x10010)
          Disposition: [enabled, allowed, notified] (0xb)
           Identifier: 16.com.local.regionkext
                  URL: /Library/LaunchDaemons/com.local.regionkext.plist

========================
 Records for UID 501 : A057A343-8A99-493B-8B4A-2E692F9D491A
========================

 Items:

 #1:
                 UUID: 11111111-0000-0000-0000-000000000001
                 Name: CleanShot X
       Developer Name: (null)
      Team Identifier: 8LHDM4TDZ8
                 Type: app (0x2)
                Flags: [  ] (0)
          Disposition: [enabled, allowed, notified] (0xb)
           Identifier: 2.pl.maketheweb.cleanshotx
                  URL: /Applications/CleanShot X.app
    Bundle Identifier: pl.maketheweb.cleanshotx
             Last Use: 2026-11-01 10:00:00+08:00

 #2:
                 UUID: 11111111-0000-0000-0000-000000000002
                 Name: Affinity
                 Type: app (0x2)
          Disposition: [disabled, allowed, notified] (0xa)
           Identifier: 2.com.canva.affinity
                  URL: /Applications/Affinity.app

 #3:
                 UUID: 11111111-0000-0000-0000-000000000003
                 Name: WeatherMenu
       Developer Name: Some Dev
                 Type: login item (0x4)
          Disposition: [disabled, disallowed, notified] (0x8)
           Identifier: 4.com.example.WeatherMenu
                  URL: Contents/Library/LoginItems/WeatherMenu.app
    Parent Identifier: 2.com.example.Weather

 #4:
                 UUID: 11111111-0000-0000-0000-000000000004
                 Name: sh
                 Type: legacy agent (0x10008)
          Disposition: [enabled, allowed, notified] (0xb)
                  URL: file:///Users/x/Library/LaunchAgents/a.plist
";

struct FakeHost {
    uid: u32,
    home: PathBuf,
    temp: PathBuf,
}

impl FakeHost {
    fn new(root: &Path) -> Arc<Self> {
        let home = root.join("home");
        let temp = root.join("tmp");
        let _ = std::fs::create_dir_all(&home);
        let _ = std::fs::create_dir_all(&temp);
        Arc::new(FakeHost {
            uid: 501,
            home,
            temp,
        })
    }
}

impl Host for FakeHost {
    fn uid(&self) -> u32 {
        self.uid
    }

    fn home_dir(&self) -> Result<PathBuf, LaunchdError> {
        Ok(self.home.clone())
    }

    fn temp_dir(&self) -> PathBuf {
        self.temp.clone()
    }

    fn current_exe(&self) -> Result<PathBuf, LaunchdError> {
        Ok(PathBuf::from("/fake/Devbase"))
    }

    fn eval_symlinks(&self, path: &Path) -> Option<PathBuf> {
        Some(path.to_path_buf())
    }

    fn trash(&self, _path: &Path) -> Result<(), LaunchdError> {
        Ok(())
    }
}

type RunnerFn = Box<dyn Fn(&str, &[&str]) -> Result<String, CommandError> + Send + Sync>;

struct FnRunner {
    func: RunnerFn,
    calls: Mutex<Vec<Vec<String>>>,
}

impl FnRunner {
    fn new(
        func: impl Fn(&str, &[&str]) -> Result<String, CommandError> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(FnRunner {
            func: Box::new(func),
            calls: Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().expect("lock calls").clone()
    }
}

impl ProcessRunner for FnRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<String, CommandError> {
        let mut call = Vec::with_capacity(args.len() + 1);
        call.push(program.to_string());
        call.extend(args.iter().map(|arg| (*arg).to_string()));
        self.calls.lock().expect("lock calls").push(call);
        (self.func)(program, args)
    }
}

/// Restores directory permissions on drop so `TempDir` cleanup always succeeds.
struct ChmodGuard {
    path: PathBuf,
}

impl ChmodGuard {
    fn deny(path: &Path) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000))
                .expect("chmod 0");
        }
        ChmodGuard {
            path: path.to_path_buf(),
        }
    }

    fn restore(&self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o755));
        }
    }
}

impl Drop for ChmodGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Builds a binary `NSKeyedArchiver` BTM database plist just like macOS writes.
struct KeyedArchive {
    objects: Vec<Value>,
}

impl KeyedArchive {
    fn new() -> Self {
        let mut archive = KeyedArchive {
            objects: Vec::new(),
        };
        archive.add(Value::String("$null".to_string()));
        archive
    }

    fn add(&mut self, value: Value) -> Uid {
        let uid = Uid::new(self.objects.len() as u64);
        self.objects.push(value);
        uid
    }

    fn class(&mut self, name: &str) -> Uid {
        let mut dict = Dictionary::new();
        dict.insert("$classname".to_string(), Value::String(name.to_string()));
        dict.insert(
            "$classes".to_string(),
            Value::Array(vec![
                Value::String(name.to_string()),
                Value::String("NSObject".to_string()),
            ]),
        );
        self.add(Value::Dictionary(dict))
    }

    #[allow(clippy::too_many_arguments)]
    fn record(
        &mut self,
        kind: i64,
        disposition: i64,
        name: &str,
        identifier: &str,
        url: &str,
        container: &str,
        id: u8,
    ) -> Uid {
        let mut uuid = vec![0u8; 16];
        uuid[15] = id;

        let bundle = identifier
            .split_once('.')
            .map_or(identifier, |(_, rest)| rest);

        let class_item = self.class("ItemRecord");
        let name_uid = self.add(Value::String(name.to_string()));
        let ident_uid = self.add(Value::String(identifier.to_string()));
        let team_uid = self.add(Value::String("TEAM".to_string()));
        let bundle_uid = self.add(Value::String(bundle.to_string()));
        let container_uid = if container.is_empty() {
            Uid::new(0)
        } else {
            self.add(Value::String(container.to_string()))
        };

        let mut uuid_dict = Dictionary::new();
        uuid_dict.insert("$class".to_string(), Value::Uid(self.class("NSUUID")));
        uuid_dict.insert("NS.uuidbytes".to_string(), Value::Data(uuid));
        let uuid_uid = self.add(Value::Dictionary(uuid_dict));

        let url_rel_uid = self.add(Value::String(url.to_string()));
        let mut url_dict = Dictionary::new();
        url_dict.insert("$class".to_string(), Value::Uid(self.class("NSURL")));
        url_dict.insert("NS.base".to_string(), Value::Uid(Uid::new(0)));
        url_dict.insert("NS.relative".to_string(), Value::Uid(url_rel_uid));
        let url_uid = self.add(Value::Dictionary(url_dict));

        let mut date_dict = Dictionary::new();
        date_dict.insert("$class".to_string(), Value::Uid(self.class("NSDate")));
        date_dict.insert("NS.time".to_string(), Value::Real(1000.0));
        let date_uid = self.add(Value::Dictionary(date_dict));

        let mut r = Dictionary::new();
        r.insert("$class".to_string(), Value::Uid(class_item));
        r.insert("type".to_string(), Value::Integer(kind.into()));
        r.insert(
            "disposition".to_string(),
            Value::Integer(disposition.into()),
        );
        r.insert("name".to_string(), Value::Uid(name_uid));
        r.insert("identifier".to_string(), Value::Uid(ident_uid));
        r.insert("developerName".to_string(), Value::Uid(Uid::new(0)));
        r.insert("teamIdentifier".to_string(), Value::Uid(team_uid));
        r.insert("bundleIdentifier".to_string(), Value::Uid(bundle_uid));
        r.insert("container".to_string(), Value::Uid(container_uid));
        r.insert("uuid".to_string(), Value::Uid(uuid_uid));
        r.insert("url".to_string(), Value::Uid(url_uid));
        r.insert("lastUseDate".to_string(), Value::Uid(date_uid));
        self.add(Value::Dictionary(r))
    }

    fn array(&mut self, items: &[Uid]) -> Uid {
        let mut dict = Dictionary::new();
        dict.insert(
            "$class".to_string(),
            Value::Uid(self.class("NSMutableArray")),
        );
        dict.insert(
            "NS.objects".to_string(),
            Value::Array(items.iter().copied().map(Value::Uid).collect()),
        );
        self.add(Value::Dictionary(dict))
    }

    fn encode(self, top: Dictionary) -> Vec<u8> {
        let mut root = Dictionary::new();
        root.insert("$version".to_string(), Value::Integer(100_000.into()));
        root.insert(
            "$archiver".to_string(),
            Value::String("NSKeyedArchiver".to_string()),
        );
        root.insert("$top".to_string(), Value::Dictionary(top));
        root.insert("$objects".to_string(), Value::Array(self.objects));
        let mut out = Vec::new();
        Value::Dictionary(root)
            .to_writer_binary(&mut out)
            .expect("encode binary plist");
        out
    }
}

fn sample_records(archive: &mut KeyedArchive) -> Uid {
    let r1 = archive.record(
        0x2,
        0xb,
        "CleanShot X",
        "2.pl.maketheweb.cleanshotx",
        "file:///Applications/CleanShot%20X.app/",
        "",
        1,
    );
    let r2 = archive.record(
        0x2,
        0x2,
        "Weather",
        "2.com.apple.weather",
        "file:///System/Applications/Weather.app/",
        "",
        2,
    );
    let r3 = archive.record(
        0x4,
        0xb,
        "WeatherMenu",
        "4.com.apple.weather.menu",
        "Contents/Library/LoginItems/Weather%20Menu.app",
        "2.com.apple.weather",
        3,
    );
    let r4 = archive.record(
        0x10008,
        0xb,
        "sh",
        "8.com.example.agent",
        "file:///x/a.plist",
        "",
        4,
    );
    archive.array(&[r1, r2, r3, r4])
}

/// Per-user BTM file of version 18 and later.
fn user_store_file() -> Vec<u8> {
    let mut archive = KeyedArchive::new();
    let records = sample_records(&mut archive);
    let user_uid = archive.add(Value::String("USER".to_string()));
    let mut store_dict = Dictionary::new();
    store_dict.insert(
        "$class".to_string(),
        Value::Uid(archive.class("BTMUserStore")),
    );
    store_dict.insert("records".to_string(), Value::Uid(records));
    store_dict.insert("userIdentifier".to_string(), Value::Uid(user_uid));
    let store = archive.add(Value::Dictionary(store_dict));
    let mut top = Dictionary::new();
    top.insert("userStore".to_string(), Value::Uid(store));
    archive.encode(top)
}

struct FallbackHarness {
    _root: TempDir,
    dir: PathBuf,
    cache_path: PathBuf,
    dumps: Arc<AtomicUsize>,
    runner: Arc<FnRunner>,
    host: Arc<FakeHost>,
    provider: Provider,
}

impl FallbackHarness {
    fn new(dump_error: Option<&str>) -> Self {
        let root = TempDir::new().expect("temp root");
        let dir = root.path().join("btm");
        std::fs::create_dir_all(&dir).expect("create btm dir");
        std::fs::write(dir.join("BackgroundItems-v99-USER.btm"), b"a new format")
            .expect("write unreadable btm");
        let cache_path = root.path().join("cache/login-items.json");

        let dumps = Arc::new(AtomicUsize::new(0));
        let dumps_ref = Arc::clone(&dumps);
        let dump_err = dump_error.map(str::to_string);
        let runner = FnRunner::new(move |program, _args| match program {
            "sfltool" => {
                dumps_ref.fetch_add(1, Ordering::SeqCst);
                if let Some(message) = &dump_err {
                    return Err(CommandError::new(
                        "sfltool",
                        vec!["dumpbtm".to_string()],
                        message,
                    ));
                }
                Ok(DUMP.to_string())
            }
            "osascript" => Ok("/Applications/CleanShot X.app\n/Applications/New.app\n".to_string()),
            _ => Ok(String::new()),
        });
        let host = FakeHost::new(root.path());
        let provider = Provider::new(
            Config {
                dir: dir.clone(),
                cache_path: Some(cache_path.clone()),
                user_id: Some("USER".to_string()),
            },
            Arc::clone(&runner) as Arc<dyn ProcessRunner>,
            Arc::clone(&host) as Arc<dyn Host>,
        );
        FallbackHarness {
            _root: root,
            dir,
            cache_path,
            dumps,
            runner,
            host,
            provider,
        }
    }

    fn next_launch(&self) -> Provider {
        Provider::new(
            Config {
                dir: self.dir.clone(),
                cache_path: Some(self.cache_path.clone()),
                user_id: Some("USER".to_string()),
            },
            Arc::clone(&self.runner) as Arc<dyn ProcessRunner>,
            Arc::clone(&self.host) as Arc<dyn Host>,
        )
    }
}

fn names_of(items: &[Item]) -> Vec<&str> {
    items.iter().map(|item| item.name.as_str()).collect()
}

#[test]
fn parse_btm_keeps_the_users_records() {
    let records = parse_btm(DUMP, 501);
    assert_eq!(records.len(), 4);
    let cleanshot = &records[0];
    assert_eq!(cleanshot.name, "CleanShot X");
    assert_eq!(cleanshot.record_type, "app");
    assert!(cleanshot.enabled);
    assert_eq!(cleanshot.url, "/Applications/CleanShot X.app");
    assert_eq!(cleanshot.developer, "");
    assert_eq!(cleanshot.bundle, "pl.maketheweb.cleanshotx");
    assert_eq!(cleanshot.team, "8LHDM4TDZ8");

    assert!(!records[1].enabled);
    assert!(!records[2].enabled);
    assert_eq!(records[2].record_type, "login item");
    assert_eq!(records[2].parent, "2.com.example.Weather");
    assert!(parse_btm(DUMP, 999).is_empty());
}

#[test]
fn items_resolves_parents_processes_actions_and_confirm() {
    let mut state = DomainState::default();
    state.jobs.insert(
        "application.pl.maketheweb.cleanshotx.1.2".to_string(),
        Job { pid: 42, exit: 0 },
    );
    state.jobs.insert(
        "com.example.WeatherMenu".to_string(),
        Job { pid: 43, exit: 0 },
    );
    let mut records = parse_btm(DUMP, 501);
    records.push(Record {
        record_type: TYPE_APP.to_string(),
        identifier: "2.com.example.Weather".to_string(),
        name: "Weather".to_string(),
        url: "/Applications/Weather.app".to_string(),
        ..Record::default()
    });
    let items = items_of(&records, &state);
    // Enabled apps and every login item; not disabled apps or legacy agents.
    assert_eq!(names_of(&items), vec!["CleanShot X", "WeatherMenu"]);

    let (app, helper) = (&items[0], &items[1]);
    assert_eq!(app.autostart, Autostart::On);
    assert_eq!(helper.autostart, Autostart::Off);
    assert_eq!(app.state, State::Running);
    assert_eq!(app.pid, 42);
    assert_eq!(helper.state, State::Running);
    assert_eq!(helper.pid, 43);
    assert_eq!(
        helper.path,
        PathBuf::from("/Applications/Weather.app/Contents/Library/LoginItems/WeatherMenu.app")
    );
    assert_eq!(helper.owner, "Weather");

    let harness = FallbackHarness::new(None);
    assert_eq!(harness.provider.actions(app), vec![Action::Remove]);
    assert!(harness.provider.actions(helper).is_empty());
    assert!(harness.provider.confirm(app, Action::Remove).is_some());
}

#[test]
fn parse_database_reads_v18_user_store() {
    let records = parse_database(&user_store_file(), "USER").expect("parse userStore");
    assert_eq!(records.len(), 4);
    let (app, weather, helper) = (&records[0], &records[1], &records[2]);
    assert_eq!(app.name, "CleanShot X");
    assert_eq!(app.url, "/Applications/CleanShot X.app");
    assert!(app.enabled);
    assert_eq!(app.record_type, TYPE_APP);
    assert_eq!(app.developer, "");
    assert_eq!(app.team, "TEAM");
    assert_eq!(app.bundle, "pl.maketheweb.cleanshotx");
    assert!(!app.uuid.is_empty());

    assert!(!weather.enabled);
    assert_eq!(helper.record_type, TYPE_LOGIN_ITEM);
    assert_eq!(helper.url, "Contents/Library/LoginItems/Weather Menu.app");
    assert_eq!(helper.parent, "2.com.apple.weather");

    assert_eq!(app.last_use, format_local_ymd_hm(APPLE_EPOCH_OFFSET + 1000));

    let items = items_of(&records, &DomainState::default());
    assert_eq!(items.len(), 2);
    assert_eq!(
        items[1].path,
        PathBuf::from(
            "/System/Applications/Weather.app/Contents/Library/LoginItems/Weather Menu.app"
        )
    );
}

#[test]
fn parse_database_of_several_users_reads_pre_v18_store() {
    let mut archive = KeyedArchive::new();
    let records = sample_records(&mut archive);
    let other = archive.array(&[]);
    let key_other = archive.add(Value::String("OTHER".to_string()));
    let key_user = archive.add(Value::String("USER".to_string()));
    let mut by_user_dict = Dictionary::new();
    by_user_dict.insert(
        "$class".to_string(),
        Value::Uid(archive.class("NSMutableDictionary")),
    );
    by_user_dict.insert(
        "NS.keys".to_string(),
        Value::Array(vec![Value::Uid(key_other), Value::Uid(key_user)]),
    );
    by_user_dict.insert(
        "NS.objects".to_string(),
        Value::Array(vec![Value::Uid(other), Value::Uid(records)]),
    );
    let by_user = archive.add(Value::Dictionary(by_user_dict));

    let mut store_dict = Dictionary::new();
    store_dict.insert("$class".to_string(), Value::Uid(archive.class("Storage")));
    store_dict.insert("itemsByUserIdentifier".to_string(), Value::Uid(by_user));
    let store = archive.add(Value::Dictionary(store_dict));

    let mut top = Dictionary::new();
    top.insert("version".to_string(), Value::Integer(16.into()));
    top.insert("store".to_string(), Value::Uid(store));
    let data = archive.encode(top);

    assert_eq!(parse_database(&data, "USER").expect("USER").len(), 4);
    assert!(parse_database(&data, "OTHER").expect("OTHER").is_empty());
    assert!(parse_database(&data, "NOBODY").expect("NOBODY").is_empty());
}

#[test]
fn parse_database_fails_on_unknown_formats() {
    assert!(parse_database(b"not a plist", "USER").is_err());

    let mut archive = KeyedArchive::new();
    let val = archive.add(Value::String("x".to_string()));
    let mut top = Dictionary::new();
    top.insert("somethingNew".to_string(), Value::Uid(val));
    assert!(parse_database(&archive.encode(top), "USER").is_err());

    let mut archive = KeyedArchive::new();
    let mut renamed_dict = Dictionary::new();
    renamed_dict.insert(
        "$class".to_string(),
        Value::Uid(archive.class("ItemRecord")),
    );
    renamed_dict.insert("kind".to_string(), Value::Integer(2.into()));
    let renamed = archive.add(Value::Dictionary(renamed_dict));
    let records = archive.array(&[renamed]);
    let mut store_dict = Dictionary::new();
    store_dict.insert(
        "$class".to_string(),
        Value::Uid(archive.class("BTMUserStore")),
    );
    store_dict.insert("records".to_string(), Value::Uid(records));
    let store = archive.add(Value::Dictionary(store_dict));
    let mut top = Dictionary::new();
    top.insert("userStore".to_string(), Value::Uid(store));
    assert!(parse_database(&archive.encode(top), "USER").is_err());
}

#[test]
fn database_file_chooses_the_newest_of_the_user() {
    let dir = TempDir::new().expect("temp dir");
    for name in [
        "BackgroundItems-v16.btm",
        "BackgroundItems-v18.btm",
        "BackgroundItems-v18-USER.btm",
        "BackgroundItems-v19-OTHER.btm",
        "BackgroundItems-v9-USER.btm",
        "other.txt",
    ] {
        std::fs::write(dir.path().join(name), b"").expect("write dummy btm");
    }
    let got = database_file(dir.path(), "USER").expect("find newest");
    assert_eq!(
        got.file_name().and_then(|n| n.to_str()),
        Some("BackgroundItems-v18-USER.btm")
    );

    let empty = TempDir::new().expect("empty dir");
    assert!(database_file(empty.path(), "USER").is_err());
}

#[test]
fn readable_database_asks_for_nothing() {
    let harness = FallbackHarness::new(None);
    std::fs::write(
        harness.dir.join("BackgroundItems-v100-USER.btm"),
        user_store_file(),
    )
    .expect("write v100 database");

    let items = harness.provider.list().expect("list from database");
    assert_eq!(items.len(), 2);
    assert_eq!(harness.dumps.load(Ordering::SeqCst), 0);
    assert_eq!(harness.provider.restriction(), None);
}

#[test]
fn fallback_never_asks_for_an_administrator_by_itself() {
    let harness = FallbackHarness::new(None);
    for _ in 0..3 {
        let items = harness.provider.list().expect("fallback list");
        assert_eq!(names_of(&items), vec!["CleanShot X", "New"]);
    }
    let next = harness.next_launch();
    next.list().expect("next launch list");
    assert_eq!(harness.dumps.load(Ordering::SeqCst), 0);
}

#[test]
fn restriction_says_why_and_what_lifts_it() {
    let harness = FallbackHarness::new(None);
    harness.provider.list().expect("list unknown format");
    let restriction = harness
        .provider
        .restriction()
        .expect("unknown format restriction");
    assert!(!restriction.settings);
    assert!(restriction.text.contains("cannot read"));

    // Permission denied -> Full Disk Access setting can lift it.
    let guard = ChmodGuard::deny(&harness.dir);
    harness.provider.list().expect("list permission denied");
    let restriction = harness
        .provider
        .restriction()
        .expect("permission denied restriction");
    assert!(restriction.settings);
    assert!(restriction.text.contains("Full Disk Access"));

    // Allowed and readable: nothing is left out.
    guard.restore();
    std::fs::write(
        harness.dir.join("BackgroundItems-v100-USER.btm"),
        user_store_file(),
    )
    .expect("write readable database");
    harness.provider.list().expect("list readable");
    assert_eq!(harness.provider.restriction(), None);
}

#[test]
fn needs_access_is_about_permission_only() {
    let harness = FallbackHarness::new(None);
    assert!(
        !harness.provider.needs_access(),
        "readable unknown format is not an access error"
    );

    let _guard = ChmodGuard::deny(&harness.dir);
    assert!(
        harness.provider.needs_access(),
        "unreadable directory needs access"
    );
    assert_eq!(harness.dumps.load(Ordering::SeqCst), 0);
}

#[test]
fn read_once_is_kept_across_launches() {
    let harness = FallbackHarness::new(None);
    harness.provider.read_once().expect("read_once");

    let items = harness.provider.list().expect("list after read_once");
    // CleanShot X with dump metadata, WeatherMenu helper, New from System Events.
    assert_eq!(names_of(&items), vec!["CleanShot X", "WeatherMenu", "New"]);
    assert_eq!(
        items[0].id,
        "login-item:11111111-0000-0000-0000-000000000001"
    );

    let next = harness.next_launch();
    let items = next.list().expect("next launch list");
    assert_eq!(names_of(&items), vec!["CleanShot X", "WeatherMenu", "New"]);
    let restriction = next.restriction().expect("restriction after cache");
    assert!(
        restriction.text.contains("are as of"),
        "expected timestamp in restriction: {:?}",
        restriction
    );
    assert_eq!(harness.dumps.load(Ordering::SeqCst), 1);
}

#[test]
fn canceled_read_once_keeps_apps_and_writes_no_cache() {
    let harness = FallbackHarness::new(Some("User canceled"));
    assert!(harness.provider.read_once().is_err());

    let items = harness.provider.list().expect("fallback list still works");
    assert_eq!(items.len(), 2);
    assert_eq!(harness.dumps.load(Ordering::SeqCst), 1);
    assert!(!harness.cache_path.exists());
}

#[test]
fn refused_automation_says_where_to_allow_it() {
    let root = TempDir::new().expect("temp root");
    let dir = root.path().join("btm");
    std::fs::create_dir_all(&dir).expect("create btm dir");
    std::fs::write(dir.join("BackgroundItems-v99-USER.btm"), b"unreadable")
        .expect("write unreadable btm");

    let runner = FnRunner::new(|program, args| {
        if program == "osascript" {
            return Err(CommandError::new(
                program,
                args.iter().map(|s| (*s).to_string()).collect(),
                "execution error: Not authorized to send Apple events to System Events. (-1743)",
            ));
        }
        Ok(String::new())
    });
    let provider = Provider::new(
        Config {
            dir,
            cache_path: None,
            user_id: Some("USER".to_string()),
        },
        runner,
        FakeHost::new(root.path()),
    );

    let error = provider.list().expect_err("list should fail");
    assert!(error.is_automation_denied());
    assert!(error.to_string().contains("Automation"));

    let app = root.path().join("A.app");
    std::fs::create_dir(&app).expect("create fake app");
    let error = provider.add(&app).expect_err("add should fail");
    assert!(error.is_automation_denied());
    assert!(error.to_string().contains("Automation"));

    let item = Item {
        id: "login-item:a".into(),
        source: Source::LoginItem,
        name: "A".into(),
        state: State::None,
        pid: 0,
        autostart: Autostart::On,
        owner: String::new(),
        path: app,
        kind: Some(KIND_APP.into()),
        admin: false,
        keep_alive: false,
    };
    let error = provider
        .do_action(&item, Action::Remove)
        .expect_err("remove should fail");
    assert!(error.is_automation_denied());
    assert!(error.to_string().contains("Automation"));
}

#[test]
fn add_and_remove_generate_expected_applescripts() {
    let root = TempDir::new().expect("temp root");
    let calc_app = root.path().join("Calculator.app");
    std::fs::create_dir(&calc_app).expect("create fake Calculator.app");

    let runner = FnRunner::new(|_program, _args| Ok("1\n".to_string()));
    let provider = Provider::new(
        Config {
            dir: root.path().join("btm"),
            cache_path: None,
            user_id: Some("USER".to_string()),
        },
        Arc::clone(&runner) as Arc<dyn ProcessRunner>,
        FakeHost::new(root.path()),
    );

    provider.add(&calc_app).expect("add Calculator.app");
    let remove_item = Item {
        id: "login-item:my-app".into(),
        source: Source::LoginItem,
        name: "My App".into(),
        state: State::None,
        pid: 0,
        autostart: Autostart::On,
        owner: String::new(),
        path: PathBuf::from(r#"/Applications/My "App".app"#),
        kind: Some(KIND_APP.into()),
        admin: false,
        keep_alive: false,
    };
    provider
        .do_action(&remove_item, Action::Remove)
        .expect("remove My App");

    let escaped_calc = apple_script_string(&calc_app.to_string_lossy());
    let escaped_remove = r#""/Applications/My \"App\".app""#;
    let want = vec![
        vec![
            "osascript".to_string(),
            "-e".to_string(),
            format!(
                "tell application \"System Events\" to make login item at end with properties {{path:{escaped_calc}, hidden:false}}"
            ),
        ],
        vec![
            "osascript".to_string(),
            "-e".to_string(),
            format!(
                "tell application \"System Events\"\n\
                 \tset found to count of (every login item whose path is {escaped_remove})\n\
                 \tif found > 0 then delete (every login item whose path is {escaped_remove})\n\
                 \treturn found\n\
                 end tell"
            ),
        ],
    ];
    assert_eq!(runner.calls(), want);

    // Rejects non-.app paths and missing .app paths without invoking osascript.
    assert!(provider.add(Path::new("/etc/hosts")).is_err());
    assert!(provider.add(&root.path().join("Missing.app")).is_err());
    assert_eq!(runner.calls().len(), 2);
}

#[test]
fn removing_self_registered_app_points_to_system_settings() {
    let root = TempDir::new().expect("temp root");
    let runner = FnRunner::new(|_program, _args| Ok("0\n".to_string()));
    let provider = Provider::new(
        Config {
            dir: root.path().join("btm"),
            cache_path: None,
            user_id: Some("USER".to_string()),
        },
        runner,
        FakeHost::new(root.path()),
    );
    let item = Item {
        id: "login-item:vitals".into(),
        source: Source::LoginItem,
        name: "Vitals".into(),
        state: State::None,
        pid: 0,
        autostart: Autostart::On,
        owner: String::new(),
        path: PathBuf::from("/Applications/Vitals.app"),
        kind: Some(KIND_APP.into()),
        admin: false,
        keep_alive: false,
    };
    let error = provider
        .do_action(&item, Action::Remove)
        .expect_err("self-registered app");
    assert!(error.to_string().contains("System Settings"));
}

#[test]
fn detail_uses_the_last_list_without_running_commands() {
    let root = TempDir::new().expect("temp root");
    let runner = FnRunner::new(|_program, _args| Ok(DUMP.to_string()));
    let provider = Provider::new(
        Config {
            dir: root.path().join("btm"),
            cache_path: None,
            user_id: Some("USER".to_string()),
        },
        Arc::clone(&runner) as Arc<dyn ProcessRunner>,
        FakeHost::new(root.path()),
    );
    let records = parse_btm(DUMP, 501);
    let items = items_of(&records, &DomainState::default());
    provider.state.lock().expect("lock state").records = records;

    let detail = provider.detail(&items[0]).expect("detail");
    assert!(
        runner.calls().is_empty(),
        "detail should use cached records from last list"
    );
    let rows: HashMap<&str, &str> = detail
        .rows
        .iter()
        .map(|row| (row.key.as_str(), row.value.as_str()))
        .collect();
    assert_eq!(rows.get("Path"), Some(&"/Applications/CleanShot X.app"));
    assert_eq!(rows.get("Kind"), Some(&"App that opens at login"));
    assert_eq!(rows.get("Enabled"), Some(&"Yes"));

    let missing = Item {
        id: "login-item:gone".into(),
        source: Source::LoginItem,
        name: "gone".into(),
        state: State::None,
        pid: 0,
        autostart: Autostart::Off,
        owner: String::new(),
        path: PathBuf::new(),
        kind: None,
        admin: false,
        keep_alive: false,
    };
    assert!(provider.detail(&missing).is_err());
}

#[test]
fn open_at_login_reads_paths_with_commas() {
    let root = TempDir::new().expect("temp root");
    let runner = FnRunner::new(|program, args| {
        assert_eq!(program, "osascript");
        assert!(args.join(" ").contains("linefeed"));
        Ok("/Applications/Notes, Tasks.app\n/Applications/Mail.app\n".to_string())
    });
    let provider = Provider::new(
        Config {
            dir: root.path().join("btm"),
            cache_path: None,
            user_id: Some("USER".to_string()),
        },
        runner,
        FakeHost::new(root.path()),
    );
    let records = provider.open_at_login().expect("open_at_login");
    let paths: Vec<&str> = records.iter().map(|r| r.url.as_str()).collect();
    assert_eq!(
        paths,
        vec!["/Applications/Notes, Tasks.app", "/Applications/Mail.app"]
    );
    assert_eq!(records[0].name, "Notes, Tasks");
}

#[test]
fn generated_uid_reads_id_and_dscl() {
    let runner = FnRunner::new(|program, args| match program {
        "id" => {
            assert_eq!(args, &["-un"]);
            Ok("jyeu\n".to_string())
        }
        "dscl" => {
            assert_eq!(args, &[".", "-read", "/Users/jyeu", "GeneratedUID"]);
            Ok("GeneratedUID: A057A343-8A99-493B-8B4A-2E692F9D491A\n".to_string())
        }
        other => panic!("unexpected command {other}"),
    });
    assert_eq!(
        generated_uid(runner.as_ref()).expect("generated_uid"),
        "A057A343-8A99-493B-8B4A-2E692F9D491A"
    );

    let bad_runner = FnRunner::new(|program, _| match program {
        "id" => Ok("jyeu\n".to_string()),
        _ => Ok("unexpected\n".to_string()),
    });
    assert!(generated_uid(bad_runner.as_ref()).is_err());
}

#[test]
fn open_settings_and_trait_objects_work() {
    let root = TempDir::new().expect("temp root");
    let dir = root.path().join("btm");
    std::fs::create_dir_all(&dir).expect("create btm dir");
    std::fs::write(dir.join("BackgroundItems-v18-USER.btm"), user_store_file()).expect("write db");

    let runner = FnRunner::new(|_program, _args| Ok(String::new()));
    let provider = Provider::new(
        Config {
            dir,
            cache_path: None,
            user_id: Some("USER".to_string()),
        },
        Arc::clone(&runner) as Arc<dyn ProcessRunner>,
        FakeHost::new(root.path()),
    );

    let dyn_provider: &dyn service::Provider = &provider;
    assert_eq!(dyn_provider.source(), Source::LoginItem);
    let restricted = dyn_provider
        .as_restricted()
        .expect("login item provider is restricted");
    assert!(!restricted.needs_access());
    restricted.open_settings().expect("open settings");
    assert_eq!(
        runner.calls(),
        vec![vec![
            "open".to_string(),
            FULL_DISK_ACCESS_SETTINGS_URL.to_string(),
        ]]
    );
}

#[test]
fn list_with_partial_returns_cached_dump_even_when_automation_fails() {
    let root = TempDir::new().expect("temp root");
    let dir = root.path().join("btm");
    std::fs::create_dir_all(&dir).expect("create btm dir");
    std::fs::write(dir.join("BackgroundItems-v99-USER.btm"), b"unreadable").expect("write db");

    let runner = FnRunner::new(|program, args| match program {
        "sfltool" => Ok(DUMP.to_string()),
        "osascript" => Err(CommandError::new(
            program,
            args.iter().map(|s| (*s).to_string()).collect(),
            "Not authorized (-1743)",
        )),
        _ => Ok(String::new()),
    });
    let provider = Provider::new(
        Config {
            dir,
            cache_path: None,
            user_id: Some("USER".to_string()),
        },
        runner,
        FakeHost::new(root.path()),
    );
    provider.read_once().expect("read_once");

    let (items, error) = provider.list_with_partial();
    assert_eq!(names_of(&items), vec!["CleanShot X", "WeatherMenu"]);
    assert!(error.expect("automation error").is_automation_denied());
}

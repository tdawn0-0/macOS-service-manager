//! Fakes for the ported tests. Nothing here touches the real launchd, the real
//! LaunchAgents directories, or the real Trash.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use plist::{Dictionary, Value};

use super::error::LaunchdError;
use super::host::{
    CommandError, HelperApi, Host, LaunchdRemoveRequest, LaunchdSaveRequest, LaunchdSaveResponse,
    PrivilegedHelper, ProcessRunner,
};
use super::provider::Context;

pub const PRINT_OUT: &str = "gui/501 = {
\ttype = login
\tservices = {
\t   409      - \tcom.example.running
\t     0      - \tcom.example.idle
\t     0   (pe) \tcom.example.pending
\t     0      1 \tcom.example.failed
\t     0    -15 \tcom.example.signaled
\t}

\tunmanaged processes = {
\t   111      - \tnot.a.service
\t}

\tdisabled services = {
\t\t\"com.example.off\" => disabled
\t\t\"com.example.idle\" => enabled
\t}
}";

pub const XML_PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>com.example.running</string>
  <key>ProgramArguments</key><array><string>/bin/echo</string><string>hi</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>Crashed</key><true/></dict>
  <key>StartInterval</key><integer>60</integer>
  <key>EnvironmentVariables</key><dict><key>B</key><string>2</string><key>A</key><string>1</string></dict>
  <key>StandardOutPath</key><string>/tmp/x.log</string>
  <key>StandardErrorPath</key><string>/tmp/x.log</string>
</dict></plist>"#;

pub const VENDOR_JOB: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>Label</key><string>com.vendor.helper</string>
<key>Program</key><string>/opt/vendor/helper</string>
<key>StartInterval</key><integer>60</integer>
<key>KeepAlive</key><dict><key>Crashed</key><true/><key>NetworkState</key><true/><key>PathState</key><dict><key>/a</key><true/><key>/b</key><false/></dict></dict>
<key>MachServices</key><dict><key>com.vendor.a</key><true/><key>com.vendor.b</key><true/><key>com.vendor.c</key><true/></dict>
<key>Nice</key><integer>-5</integer>
</dict></plist>"#;

/// A temporary directory removed on drop.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "devcore-test-{}-{}-{n}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&path).expect("create a test directory");
        TempDir { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

pub fn write_file(dir: &Path, name: &str, data: &[u8]) -> PathBuf {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create a fixture directory");
    }
    std::fs::write(&path, data).expect("write a fixture");
    path
}

pub fn binary_off_plist() -> Vec<u8> {
    let mut dict = Dictionary::new();
    dict.insert("Label".to_string(), Value::String("com.example.off".into()));
    dict.insert("Program".to_string(), Value::String("/bin/true".into()));
    let mut bytes = Vec::new();
    Value::Dictionary(dict)
        .to_writer_binary(&mut bytes)
        .expect("binary plist");
    bytes
}

pub fn binary_vendor_plist() -> Vec<u8> {
    let mut socket = Dictionary::new();
    socket.insert("SockPathName".to_string(), Value::String("/tmp/s".into()));
    socket.insert("SockPathMode".to_string(), Value::Integer(438.into()));
    let mut sockets = Dictionary::new();
    sockets.insert("Listener".to_string(), Value::Dictionary(socket));

    let mut dict = Dictionary::new();
    dict.insert("Label".to_string(), Value::String("com.vendor.bin".into()));
    dict.insert(
        "Program".to_string(),
        Value::String("/opt/vendor/bin".into()),
    );
    dict.insert("RunAtLoad".to_string(), Value::Boolean(false));
    dict.insert("StartOnMount".to_string(), Value::Boolean(false));
    dict.insert("Nice".to_string(), Value::Integer((-5).into()));
    dict.insert("ExitTimeOut".to_string(), Value::Integer(30.into()));
    dict.insert("Sockets".to_string(), Value::Dictionary(sockets));

    let mut bytes = Vec::new();
    Value::Dictionary(dict)
        .to_writer_binary(&mut bytes)
        .expect("binary plist");
    bytes
}

struct FailRule {
    program: String,
    message: String,
}

/// Records every command and returns canned output, or a canned failure.
#[derive(Default)]
pub struct FakeRunner {
    calls: Mutex<Vec<Vec<String>>>,
    output: Mutex<String>,
    failures: Mutex<Vec<FailRule>>,
}

impl FakeRunner {
    pub fn new(output: &str) -> Arc<Self> {
        Arc::new(FakeRunner {
            output: Mutex::new(output.to_string()),
            ..FakeRunner::default()
        })
    }

    pub fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().unwrap().clone()
    }

    pub fn take_calls(&self) -> Vec<Vec<String>> {
        std::mem::take(&mut *self.calls.lock().unwrap())
    }

    pub fn fail(&self, program: &str, message: &str) {
        self.failures.lock().unwrap().push(FailRule {
            program: program.to_string(),
            message: message.to_string(),
        });
    }
}

impl ProcessRunner for FakeRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<String, CommandError> {
        let mut call = vec![program.to_string()];
        call.extend(args.iter().map(|arg| (*arg).to_string()));
        self.calls.lock().unwrap().push(call.clone());
        for rule in self.failures.lock().unwrap().iter() {
            if rule.program == program {
                return Err(CommandError {
                    program: program.to_string(),
                    args: call[1..].to_vec(),
                    stdout: String::new(),
                    stderr: rule.message.clone(),
                    status: Some(1),
                    timed_out: false,
                });
            }
        }
        Ok(self.output.lock().unwrap().clone())
    }
}

/// A host with a private home and temp directory, and a fake Trash.
pub struct FakeHost {
    pub uid: u32,
    pub home: PathBuf,
    pub temp: PathBuf,
    exe: Mutex<PathBuf>,
    trashed: Mutex<Vec<PathBuf>>,
    trash_error: Mutex<Option<String>>,
    suffix: String,
}

impl FakeHost {
    pub fn new(home: &Path, temp: &Path) -> Arc<Self> {
        Arc::new(FakeHost {
            uid: 501,
            home: home.to_path_buf(),
            temp: temp.to_path_buf(),
            exe: Mutex::new(PathBuf::from(
                "/Applications/Devbase.app/Contents/MacOS/Devbase",
            )),
            trashed: Mutex::new(Vec::new()),
            trash_error: Mutex::new(None),
            suffix: "12.34.56".to_string(),
        })
    }

    pub fn set_exe(&self, exe: &Path) {
        *self.exe.lock().unwrap() = exe.to_path_buf();
    }

    pub fn trashed(&self) -> Vec<PathBuf> {
        self.trashed.lock().unwrap().clone()
    }

    pub fn fail_trash(&self, message: &str) {
        *self.trash_error.lock().unwrap() = Some(message.to_string());
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
        Ok(self.exe.lock().unwrap().clone())
    }

    fn eval_symlinks(&self, path: &Path) -> Option<PathBuf> {
        std::fs::canonicalize(path).ok()
    }

    fn trash(&self, path: &Path) -> Result<(), LaunchdError> {
        if let Some(error) = self.trash_error.lock().unwrap().clone() {
            return Err(LaunchdError::io(error));
        }
        let name = path
            .file_name()
            .ok_or_else(|| LaunchdError::io("the file has no name"))?;
        let mut destination = self.home.join(".Trash").join(name);
        if std::fs::symlink_metadata(&destination).is_ok() {
            let name = destination
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let (stem, extension) = match name.rfind('.') {
                Some(index) => name.split_at(index),
                None => (name.as_str(), ""),
            };
            destination = destination.with_file_name(format!("{stem} {}{extension}", self.suffix));
        }
        std::fs::rename(path, &destination).map_err(|error| LaunchdError::io(error.to_string()))?;
        self.trashed.lock().unwrap().push(destination);
        Ok(())
    }
}

struct HelperState {
    installed: Mutex<bool>,
    control_requests: Mutex<Vec<Vec<Vec<String>>>>,
    calls: Mutex<Vec<Vec<String>>>,
    save_requests: Mutex<Vec<LaunchdSaveRequest>>,
    remove_requests: Mutex<Vec<LaunchdRemoveRequest>>,
    failures: Mutex<HashMap<String, String>>,
    print_succeeds: Mutex<bool>,
}

/// An in-memory privileged helper that emulates the server's file writing and
/// its launchctl ordering.
#[derive(Clone)]
pub struct FakeHelper {
    state: Arc<HelperState>,
}

impl FakeHelper {
    pub fn new(installed: bool) -> Self {
        FakeHelper {
            state: Arc::new(HelperState {
                installed: Mutex::new(installed),
                control_requests: Mutex::new(Vec::new()),
                calls: Mutex::new(Vec::new()),
                save_requests: Mutex::new(Vec::new()),
                remove_requests: Mutex::new(Vec::new()),
                failures: Mutex::new(HashMap::new()),
                print_succeeds: Mutex::new(true),
            }),
        }
    }

    pub fn installed() -> Self {
        Self::new(true)
    }

    pub fn missing() -> Self {
        Self::new(false)
    }

    pub fn install(&self) {
        *self.state.installed.lock().unwrap() = true;
    }

    pub fn calls(&self) -> Vec<Vec<String>> {
        self.state.calls.lock().unwrap().clone()
    }

    pub fn take_calls(&self) -> Vec<Vec<String>> {
        std::mem::take(&mut *self.state.calls.lock().unwrap())
    }

    pub fn control_requests(&self) -> Vec<Vec<Vec<String>>> {
        self.state.control_requests.lock().unwrap().clone()
    }

    pub fn save_requests(&self) -> Vec<LaunchdSaveRequest> {
        self.state.save_requests.lock().unwrap().clone()
    }

    pub fn remove_requests(&self) -> Vec<LaunchdRemoveRequest> {
        self.state.remove_requests.lock().unwrap().clone()
    }

    pub fn fail(&self, subcommand: &str, message: &str) {
        self.state
            .failures
            .lock()
            .unwrap()
            .insert(subcommand.to_string(), message.to_string());
    }

    pub fn set_print_succeeds(&self, succeeds: bool) {
        *self.state.print_succeeds.lock().unwrap() = succeeds;
    }

    fn record(&self, args: &[String]) {
        let mut call = vec!["/bin/launchctl".to_string()];
        call.extend(args.iter().cloned());
        self.state.calls.lock().unwrap().push(call);
    }

    fn failure(&self, subcommand: &str) -> Option<String> {
        self.state.failures.lock().unwrap().get(subcommand).cloned()
    }
}

impl PrivilegedHelper for FakeHelper {
    fn require(&self) -> Result<Arc<dyn HelperApi>, LaunchdError> {
        if *self.state.installed.lock().unwrap() {
            Ok(Arc::new(self.clone()))
        } else {
            Err(LaunchdError::helper_not_installed())
        }
    }
}

impl HelperApi for FakeHelper {
    fn launchd_control(&self, commands: &[Vec<String>]) -> Result<(), LaunchdError> {
        self.state
            .control_requests
            .lock()
            .unwrap()
            .push(commands.to_vec());
        for command in commands {
            self.record(command);
            if let Some(message) = self.failure(command.first().map_or("", String::as_str)) {
                return Err(LaunchdError::helper(message));
            }
        }
        Ok(())
    }

    fn launchd_save(
        &self,
        request: LaunchdSaveRequest,
    ) -> Result<LaunchdSaveResponse, LaunchdError> {
        self.state
            .save_requests
            .lock()
            .unwrap()
            .push(request.clone());
        if let Some(parent) = request.path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| LaunchdError::io(error.to_string()))?;
        }
        std::fs::write(&request.path, &request.plist)
            .map_err(|error| LaunchdError::io(error.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&request.path, std::fs::Permissions::from_mode(0o644));
        }
        if !request.load {
            return Ok(LaunchdSaveResponse {
                saved: true,
                error: None,
            });
        }
        if request.existing_path.is_some() {
            // Editing: unload what launchd has, then load the new file.
            self.record(&["print".to_string(), request.target.clone()]);
            let print_succeeds = *self.state.print_succeeds.lock().unwrap();
            if print_succeeds {
                self.record(&["bootout".to_string(), request.target.clone()]);
                if let Some(message) = self.failure("bootout") {
                    return Ok(LaunchdSaveResponse {
                        saved: true,
                        error: Some(message),
                    });
                }
            }
        }
        self.record(&[
            "bootstrap".to_string(),
            request.domain.clone(),
            request.path.display().to_string(),
        ]);
        if let Some(message) = self.failure("bootstrap") {
            return Ok(LaunchdSaveResponse {
                saved: true,
                error: Some(message),
            });
        }
        Ok(LaunchdSaveResponse {
            saved: true,
            error: None,
        })
    }

    fn launchd_remove(&self, request: LaunchdRemoveRequest) -> Result<(), LaunchdError> {
        self.state
            .remove_requests
            .lock()
            .unwrap()
            .push(request.clone());
        self.record(&["print".to_string(), request.target.clone()]);
        let print_succeeds = *self.state.print_succeeds.lock().unwrap();
        if print_succeeds {
            self.record(&["bootout".to_string(), request.target.clone()]);
            if let Some(message) = self.failure("bootout") {
                return Err(LaunchdError::helper(message));
            }
        }
        let name = request
            .path
            .file_name()
            .ok_or_else(|| LaunchdError::io("the file has no name"))?;
        std::fs::rename(&request.path, request.trash_dir.join(name))
            .map_err(|error| LaunchdError::io(error.to_string()))
    }
}

pub fn context(runner: Arc<FakeRunner>, helper: &FakeHelper, host: Arc<FakeHost>) -> Arc<Context> {
    Arc::new(Context::new(runner, Arc::new(helper.clone()), host))
}

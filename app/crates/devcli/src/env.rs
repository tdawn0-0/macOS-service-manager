//! The injectable CLI environment, mirroring `foundation.go`'s `Env`.
//!
//! The production defaults read `MSM_DEV_HOME` through `devcore`'s path
//! resolution so tests get isolated workspaces. Unlike the Go `Env` this port
//! has no service-provider map or manager factory: the ported commands talk to
//! `devcore`/`devproxy` directly, and the helper client is not part of this
//! slice.

use std::io::Read;
use std::path::PathBuf;

use serde::Serialize;

use crate::wrapper;

/// The privileged-helper status shape. Capitalized keys, Go field order.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[allow(non_snake_case)]
pub struct HelperStatus {
    pub State: String,
    pub Message: String,
}

impl HelperStatus {
    /// The helper client is a later slice; the honest state is not installed.
    pub fn unavailable() -> Self {
        HelperStatus {
            State: "not-installed".to_string(),
            Message: "Privileged helper is unavailable in this build.".to_string(),
        }
    }
}

pub struct CliEnv {
    pub input: Box<dyn Read + Send>,
    pub json: bool,
    pub root: PathBuf,
    pub cli_bin_dir: PathBuf,
    pub app_path: String,
    pub login_program: String,
    pub helper_status: HelperStatus,
    /// Injected runtime states for tests; `None` probes launchd.
    pub runtime_states: Option<std::collections::BTreeMap<String, String>>,
}

impl CliEnv {
    pub fn default_env(input: Box<dyn Read + Send>) -> Self {
        CliEnv {
            input,
            json: false,
            root: devcore::state::default_root(),
            cli_bin_dir: wrapper::default_cli_bin_dir(),
            app_path: String::new(),
            login_program: String::new(),
            helper_status: HelperStatus::unavailable(),
            runtime_states: None,
        }
    }

    pub fn for_test() -> Self {
        CliEnv::default_env(Box::new(std::io::empty()))
    }
}

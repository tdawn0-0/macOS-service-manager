//! Test seams shared by the engine, health and shutdown tests.

use std::sync::Mutex;
use std::time::Duration;

use crate::engine::CommandRunner;
use crate::health::HealthProbe;
use crate::launchd::CommandError;

type Handler = Box<dyn Fn(&str, &[String]) -> Result<String, String> + Send + Sync>;

/// A command runner driven by a closure. Calls are recorded, never executed.
pub(crate) struct FakeCommands {
    calls: Mutex<Vec<Vec<String>>>,
    handler: Handler,
}

impl FakeCommands {
    pub(crate) fn new(
        handler: impl Fn(&str, &[String]) -> Result<String, String> + Send + Sync + 'static,
    ) -> Self {
        FakeCommands {
            calls: Mutex::new(Vec::new()),
            handler: Box::new(handler),
        }
    }

    pub(crate) fn calls(&self) -> Vec<Vec<String>> {
        self.calls.lock().unwrap().clone()
    }
}

impl CommandRunner for FakeCommands {
    fn run(
        &self,
        program: &str,
        args: &[&str],
        _timeout: Duration,
    ) -> Result<String, CommandError> {
        let mut call = vec![program.to_string()];
        call.extend(args.iter().map(|arg| (*arg).to_string()));
        self.calls.lock().unwrap().push(call.clone());
        match (self.handler)(program, &call[1..]) {
            Ok(output) => Ok(output),
            Err(message) => Err(CommandError::new(program, call[1..].to_vec(), message)),
        }
    }
}

/// A readiness probe with a fixed or scripted answer.
pub(crate) struct FakeProbe {
    result: Mutex<Result<(), String>>,
    calls: Mutex<Vec<String>>,
}

impl FakeProbe {
    pub(crate) fn passing() -> Self {
        FakeProbe {
            result: Mutex::new(Ok(())),
            calls: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn failing(message: &str) -> Self {
        FakeProbe {
            result: Mutex::new(Err(message.to_string())),
            calls: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl HealthProbe for FakeProbe {
    fn probe(&self, address: &str, _timeout: Duration) -> Result<(), String> {
        self.calls.lock().unwrap().push(address.to_string());
        self.result.lock().unwrap().clone()
    }
}

//! `status`, `diagnostics`, `tasks`, `logs`, `ports`, and `shutdown`
//! (ported from `cmd_workspace.go`).
//!
//! Everything here reads the workspace store; runtime probes go through
//! `devcore::engine::Engine`, which uses the injected command runner in tests
//! and `launchctl` in production.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use serde::Serialize;

use crate::args::{FlagSpecs, ParseOutcome, parse_flags};
use crate::commands::{maybe_help, require_yes};
use crate::env::CliEnv;
use crate::error::{CliError, CliResult};
use crate::wrapper::CLIInstallStatus;

use devcore::databases::{
    Databases, InstanceAction, PortCategory, PortOwner, SignalOutcome, ports::owner_for_port,
};
use devcore::diagnostics::{DiagnosticCheck, DiagnosticReport, Diagnostics};
use devcore::engine::{Engine, ProcessRunnerAdapter};
use devcore::launchd::SystemRunner;
use devcore::model::State;
use devcore::operations::{OperationJournal, OperationRecord, operation_can_retry};
use devcore::shutdown::{Shutdown as ShutdownRunner, ShutdownOptions, ShutdownPlan};
use devcore::state::Store;

pub use devcore::diagnostics::RuntimeSnapshot;

/// A status payload with State flattened first, then the CLI fields.
#[derive(Serialize)]
struct StatusOutput<'a> {
    #[serde(flatten)]
    state: &'a State,
    runtime: RuntimeSnapshot,
    #[serde(rename = "proxyPublicationState")]
    proxy_publication_state: String,
    helper: crate::env::HelperStatus,
    cli: CLIInstallStatus,
    root: String,
}

fn store(env: &CliEnv) -> Store {
    Store::new(env.root.clone())
}

fn engine(env: &CliEnv) -> Engine {
    Engine::new(
        env.root.clone(),
        std::sync::Arc::new(ProcessRunnerAdapter::new(std::sync::Arc::new(
            SystemRunner::new(),
        ))),
    )
}

fn diagnostics(env: &CliEnv) -> Diagnostics {
    let store = store(env);
    Diagnostics::new(
        store.clone(),
        engine(env),
        devcore::software::Software::new(store),
    )
}

fn snapshot(env: &CliEnv, state: &State) -> RuntimeSnapshot {
    if let Some(states) = &env.runtime_states {
        return RuntimeSnapshot {
            states: states.clone(),
        };
    }
    diagnostics(env).snapshot(state)
}

fn proxy_publication_state(env: &CliEnv) -> String {
    let store = store(env);
    match devproxy::proxy::rules::publication_state(&store) {
        Ok(value) if !value.is_empty() => value.to_string(),
        _ => "Unknown".into(),
    }
}

pub fn run_status_command(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    if maybe_help(env, out, args, "status")? {
        return Ok(());
    }
    if let Some(extra) = args.first() {
        return Err(CliError::usage(format!(
            "unexpected argument {extra:?} for status"
        )));
    }
    let store = store(env);
    let state = store.read()?;
    let snap = snapshot(env, &state);
    let publication = proxy_publication_state(env);
    let cli = crate::commands::cli_status(env);
    let payload = StatusOutput {
        state: &state,
        runtime: snap,
        proxy_publication_state: publication,
        helper: env.helper_status.clone(),
        cli,
        root: env.root.to_string_lossy().into_owned(),
    };
    crate::commands::write_result(env, out, &payload, |w| {
        writeln!(w, "Workspace: {}", payload.root)?;
        writeln!(w, "Packages: {}", payload.state.packages.len())?;
        writeln!(w, "Instances: {}", payload.state.instances.len())?;
        for (id, instance) in &payload.state.instances {
            let runtime = payload
                .runtime
                .states
                .get(&format!("instance:{id}"))
                .map(String::as_str)
                .unwrap_or("");
            writeln!(
                w,
                "  - {} ({}, :{}) [{}]",
                instance.id, instance.kind, instance.port, runtime
            )?;
        }
        writeln!(
            w,
            "HTTPS Proxy: {} ({} rules, listener :{}, {})",
            payload
                .runtime
                .states
                .get("proxy")
                .map(String::as_str)
                .unwrap_or(""),
            payload.state.proxy_rules.len(),
            payload.state.proxy_listener_port,
            payload.proxy_publication_state
        )?;
        let tunnel = payload
            .runtime
            .states
            .get("tunnel")
            .cloned()
            .unwrap_or_default();
        if !tunnel.is_empty() {
            writeln!(w, "Cloudflare Tunnel: {tunnel}")?;
        }
        writeln!(w, "Privileged Helper: {}", payload.helper.State)?;
        writeln!(
            w,
            "CLI Wrapper: installed={} ({})",
            payload.cli.installed, payload.cli.wrapper_path
        )?;
        Ok(())
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CheckView {
    layer: String,
    resource: String,
    status: String,
    summary: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    remedy: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticView {
    schema_version: i64,
    generated_at: String,
    redacted: bool,
    checks: Vec<CheckView>,
}

fn report_view(report: &DiagnosticReport) -> DiagnosticView {
    DiagnosticView {
        schema_version: report.schema_version,
        generated_at: report.generated_at.clone(),
        redacted: report.redacted,
        checks: report
            .checks
            .iter()
            .map(|check: &DiagnosticCheck| CheckView {
                layer: check.layer.clone(),
                resource: check.resource.clone(),
                status: check.status.clone(),
                summary: check.summary.clone(),
                remedy: check.remedy.clone(),
            })
            .collect(),
    }
}

pub fn run_diagnostics_command(
    env: &mut CliEnv,
    out: &mut dyn Write,
    args: &[String],
) -> CliResult<()> {
    if maybe_help(env, out, args, "diagnostics")? {
        return Ok(());
    }
    let mut rest = args;
    let mut sub = "run";
    if let Some(first) = rest.first() {
        sub = first;
        rest = &rest[1..];
    }
    if let Some(extra) = rest.first() {
        return Err(CliError::usage(format!(
            "unexpected argument {extra:?} for diagnostics {sub}"
        )));
    }
    match sub {
        "run" => {
            let diagnostics = diagnostics(env);
            let report = diagnostics
                .diagnose()
                .map_err(|error| CliError::operation(error.to_string()))?;
            let view = report_view(&report);
            crate::commands::write_result(env, out, &view, |w| {
                writeln!(w, "Diagnostics ({} checks):", view.checks.len())?;
                for check in &view.checks {
                    writeln!(
                        w,
                        "  [{}] {} | {}: {}",
                        check.status, check.layer, check.resource, check.summary
                    )?;
                    if !check.remedy.is_empty() {
                        writeln!(w, "         Remedy: {}", check.remedy)?;
                    }
                }
                Ok(())
            })
        }
        "export" => {
            let diagnostics = diagnostics(env);
            let report = diagnostics
                .diagnose()
                .map_err(|error| CliError::operation(error.to_string()))?;
            let path = diagnostics
                .export_diagnostics(&report)
                .map_err(|error| CliError::operation(error.to_string()))?;
            let view = report_view(&report);
            let payload = serde_json::json!({"path": path.to_string_lossy(), "report": view});
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(
                    w,
                    "Redacted diagnostic report exported: {}",
                    payload["path"].as_str().unwrap_or_default()
                )
            })
        }
        other => Err(CliError::usage(format!(
            "unknown diagnostics subcommand {other:?} (expected run or export)"
        ))),
    }
}

/// The capitalized operation view: Go field names plus `canRetry`.
#[derive(Serialize)]
#[allow(non_snake_case)]
struct OperationView {
    #[serde(flatten)]
    record: OperationRecord,
    canRetry: bool,
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct DatabaseWorkView {
    ID: String,
    InstanceID: String,
    Kind: String,
    Database: String,
    Status: String,
    Detail: String,
    Output: String,
    Revision: String,
    CallerActive: bool,
    Running: bool,
    CanResolve: bool,
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct InitializationView {
    ID: String,
    InstanceID: String,
    Kind: String,
    Package: String,
    Path: String,
    Status: String,
    Detail: String,
    Revision: String,
    Port: i64,
    Busy: bool,
    CanResolve: bool,
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct DeletionView {
    ID: String,
    InstanceID: String,
    Kind: String,
    OriginalPath: String,
    StagedPath: String,
    Status: String,
    Detail: String,
    Revision: String,
    Entries: i64,
    Bytes: i64,
    CanErase: bool,
    CanRestore: bool,
    Busy: bool,
}

fn journal(env: &CliEnv) -> OperationJournal {
    OperationJournal::new(store(env))
}

fn databases(env: &CliEnv) -> Databases {
    Databases::new(store(env))
}

pub fn run_tasks_command(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    if maybe_help(env, out, args, "tasks")? {
        return Ok(());
    }
    let mut rest = args;
    let mut sub = "list";
    if let Some(first) = rest.first() {
        sub = first;
        rest = &rest[1..];
    }
    let cancelled: &dyn Fn() -> bool = &|| false;
    match sub {
        "list" => {
            if let Some(extra) = rest.first() {
                return Err(CliError::usage(format!(
                    "unexpected argument {extra:?} for tasks list"
                )));
            }
            let records = journal(env).operations(cancelled)?;
            let views: Vec<OperationView> = records
                .into_iter()
                .map(|record| OperationView {
                    canRetry: operation_can_retry(&record),
                    record,
                })
                .collect();
            #[derive(Serialize)]
            struct Payload<'a> {
                operations: &'a [OperationView],
            }
            crate::commands::write_result(env, out, &Payload { operations: &views }, |w| {
                if views.is_empty() {
                    writeln!(w, "No recorded operations.")?;
                    return Ok(());
                }
                for view in &views {
                    writeln!(
                        w,
                        "{} | {} | {} | {} | rev={} | retry={}",
                        view.record.id,
                        view.record.request.kind,
                        view.record.request.resource,
                        view.record.status,
                        view.record.revision,
                        view.canRetry
                    )?;
                    if !view.record.result.is_empty() {
                        writeln!(w, "  {}", view.record.result)?;
                    }
                }
                Ok(())
            })
        }
        "dismiss" | "retry" | "resolve-work" | "resolve-init" | "restore-deletion" => {
            if rest.len() != 2 {
                return Err(CliError::usage(format!(
                    "usage: devbase tasks {sub} <id> <revision>"
                )));
            }
            let id = &rest[0];
            let revision = &rest[1];
            match sub {
                "dismiss" => {
                    journal(env).dismiss_operation(id, revision, cancelled)?;
                    let mut payload = BTreeMap::new();
                    payload.insert("dismissed", id.clone());
                    payload.insert("revision", revision.clone());
                    crate::commands::write_result(env, out, &payload, |w| {
                        writeln!(w, "Dismissed operation {id}.")
                    })
                }
                "retry" => {
                    journal(env).retry_operation(id, revision, cancelled)?;
                    let mut payload = BTreeMap::new();
                    payload.insert("retried", id.clone());
                    payload.insert("revision", revision.clone());
                    crate::commands::write_result(env, out, &payload, |w| {
                        writeln!(w, "Retried operation {id}.")
                    })
                }
                "resolve-work" => {
                    databases(env).resolve_database_work(id, revision)?;
                    let mut payload = BTreeMap::new();
                    payload.insert("resolved", id.clone());
                    payload.insert("revision", revision.clone());
                    crate::commands::write_result(env, out, &payload, |w| {
                        writeln!(w, "Resolved database work {id}.")
                    })
                }
                "resolve-init" => {
                    let archive =
                        databases(env).resolve_instance_initialization(id, revision, cancelled)?;
                    let mut payload = BTreeMap::new();
                    payload.insert("resolved", id.clone());
                    payload.insert("revision", revision.clone());
                    payload.insert("archive", archive.clone());
                    crate::commands::write_result(env, out, &payload, |w| {
                        writeln!(w, "Resolved initialization {id} (archive: {archive}).")
                    })
                }
                "restore-deletion" => {
                    databases(env).restore_instance_deletion(id, revision, cancelled)?;
                    let mut payload = BTreeMap::new();
                    payload.insert("restored", id.clone());
                    payload.insert("revision", revision.clone());
                    crate::commands::write_result(env, out, &payload, |w| {
                        writeln!(w, "Restored pending deletion {id}.")
                    })
                }
                _ => unreachable!(),
            }
        }
        "work" => {
            if let Some(extra) = rest.first() {
                return Err(CliError::usage(format!(
                    "unexpected argument {extra:?} for tasks work"
                )));
            }
            let items = databases(env).pending_database_work(cancelled)?;
            let views: Vec<DatabaseWorkView> = items
                .into_iter()
                .map(|item| DatabaseWorkView {
                    ID: item.id,
                    InstanceID: item.instance_id,
                    Kind: item.kind,
                    Database: item.database,
                    Status: item.status,
                    Detail: item.detail,
                    Output: item.output,
                    Revision: item.revision,
                    CallerActive: item.caller_active,
                    Running: item.running,
                    CanResolve: item.can_resolve,
                })
                .collect();
            #[derive(Serialize)]
            struct Payload<'a> {
                #[serde(rename = "databaseWork")]
                database_work: &'a [DatabaseWorkView],
            }
            crate::commands::write_result(
                env,
                out,
                &Payload {
                    database_work: &views,
                },
                |w| {
                    if views.is_empty() {
                        writeln!(w, "No unfinished database tasks.")?;
                        return Ok(());
                    }
                    for view in &views {
                        writeln!(
                            w,
                            "{} | {} | {} | {} | rev={}",
                            view.ID, view.Kind, view.InstanceID, view.Status, view.Revision
                        )?;
                        if !view.Detail.is_empty() {
                            writeln!(w, "  {}", view.Detail)?;
                        }
                    }
                    Ok(())
                },
            )
        }
        "initializations" => {
            if let Some(extra) = rest.first() {
                return Err(CliError::usage(format!(
                    "unexpected argument {extra:?} for tasks initializations"
                )));
            }
            let items = databases(env).pending_instance_initializations(cancelled)?;
            let views: Vec<InitializationView> = items
                .into_iter()
                .map(|item| InitializationView {
                    ID: item.id,
                    InstanceID: item.instance_id,
                    Kind: item.kind,
                    Package: item.package,
                    Path: item.path.to_string_lossy().into_owned(),
                    Status: item.status,
                    Detail: item.detail,
                    Revision: item.revision,
                    Port: item.port,
                    Busy: item.busy,
                    CanResolve: item.can_resolve,
                })
                .collect();
            #[derive(Serialize)]
            struct Payload<'a> {
                initializations: &'a [InitializationView],
            }
            crate::commands::write_result(
                env,
                out,
                &Payload {
                    initializations: &views,
                },
                |w| {
                    if views.is_empty() {
                        writeln!(w, "No unfinished initializations.")?;
                        return Ok(());
                    }
                    for view in &views {
                        writeln!(
                            w,
                            "{} | {} | {} | {} | rev={}",
                            view.ID, view.InstanceID, view.Kind, view.Status, view.Revision
                        )?;
                        if !view.Detail.is_empty() {
                            writeln!(w, "  {}", view.Detail)?;
                        }
                    }
                    Ok(())
                },
            )
        }
        "deletions" => {
            if let Some(extra) = rest.first() {
                return Err(CliError::usage(format!(
                    "unexpected argument {extra:?} for tasks deletions"
                )));
            }
            let items = databases(env).pending_instance_deletions(cancelled)?;
            let views: Vec<DeletionView> = items
                .into_iter()
                .map(|item| DeletionView {
                    ID: item.id,
                    InstanceID: item.instance_id,
                    Kind: item.kind,
                    OriginalPath: item.original_path,
                    StagedPath: item.staged_path,
                    Status: item.status,
                    Detail: item.detail,
                    Revision: item.revision,
                    Entries: item.entries,
                    Bytes: item.bytes,
                    CanErase: item.can_erase,
                    CanRestore: item.can_restore,
                    Busy: item.busy,
                })
                .collect();
            #[derive(Serialize)]
            struct Payload<'a> {
                deletions: &'a [DeletionView],
            }
            crate::commands::write_result(env, out, &Payload { deletions: &views }, |w| {
                if views.is_empty() {
                    writeln!(w, "No pending deletions.")?;
                    return Ok(());
                }
                for view in &views {
                    writeln!(
                        w,
                        "{} | {} | {} | {} | rev={}",
                        view.ID, view.InstanceID, view.Kind, view.Status, view.Revision
                    )?;
                    if !view.Detail.is_empty() {
                        writeln!(w, "  {}", view.Detail)?;
                    }
                }
                Ok(())
            })
        }
        "finish-deletion" => {
            let specs = FlagSpecs::new().value("confirm");
            let ParseOutcome::Parsed(parsed) = parse_flags(rest, &specs)? else {
                return Ok(());
            };
            let confirm = parsed.string("confirm");
            if parsed.positionals.len() != 2 || confirm.trim().is_empty() {
                return Err(CliError::usage(
                    "usage: devbase tasks finish-deletion <id> <revision> --confirm <instance-id>",
                ));
            }
            let id = &parsed.positionals[0];
            let revision = &parsed.positionals[1];
            databases(env).finish_instance_deletion(id, revision, &confirm, cancelled)?;
            let mut payload = BTreeMap::new();
            payload.insert("finished", id.clone());
            payload.insert("revision", revision.clone());
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(w, "Finished pending deletion {id}.")
            })
        }
        other => Err(CliError::usage(format!(
            "unknown tasks subcommand {other:?}"
        ))),
    }
}

/// The `internal/logs/logs.go` snapshot.
struct LogSnapshot {
    lines: Vec<String>,
    truncated: bool,
}

fn tail_log(path: &Path, max_bytes: u64, max_lines: usize) -> std::io::Result<LogSnapshot> {
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    let offset = size.saturating_sub(max_bytes);
    let truncated = size > max_bytes;
    file.seek(SeekFrom::Start(offset))?;
    let mut buffer = Vec::with_capacity((size - offset) as usize);
    file.read_to_end(&mut buffer)?;
    if offset > 0
        && let Some(index) = buffer.iter().position(|byte| *byte == b'\n')
    {
        buffer.drain(..=index);
    }
    let text = String::from_utf8_lossy(&buffer)
        .replace("\r\n", "\n")
        .trim_end_matches('\n')
        .to_string();
    let mut lines: Vec<String> = if text.is_empty() {
        Vec::new()
    } else {
        text.split('\n').map(str::to_string).collect()
    };
    let mut truncated = truncated;
    if lines.len() > max_lines {
        lines = lines.split_off(lines.len() - max_lines);
        truncated = true;
    }
    Ok(LogSnapshot { lines, truncated })
}

fn valid_log_target(target: &str) -> bool {
    let bytes = target.as_bytes();
    if bytes.is_empty() || bytes.len() > 240 {
        return false;
    }
    let first = bytes[0];
    if !(first.is_ascii_alphanumeric()) {
        return false;
    }
    bytes[1..]
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

pub fn run_logs_command(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    if maybe_help(env, out, args, "logs")? {
        return Ok(());
    }
    let specs = FlagSpecs::new()
        .value("stream")
        .value("lines")
        .value("filter");
    let ParseOutcome::Parsed(parsed) = parse_flags(args, &specs)? else {
        return Ok(());
    };
    if parsed.positionals.len() != 1 {
        return Err(CliError::usage(
            "usage: devbase logs <proxy|tunnel|db-<id>|<instance-id>> [--stream stderr|stdout] [--lines N] [--filter <text>]",
        ));
    }
    let mut lines = parsed.integer("lines")?.unwrap_or(200);
    if lines <= 0 {
        lines = 200;
    }
    if lines > 5000 {
        lines = 5000;
    }
    let mut target = parsed.positionals[0].trim().to_string();
    if target != "proxy" && target != "tunnel" && !target.starts_with("db-") {
        let store = store(env);
        if let Ok(state) = store.read()
            && state.instances.contains_key(&target)
        {
            target = format!("db-{target}");
        }
    }
    if !valid_log_target(&target) {
        return Err(CliError::usage(format!(
            "invalid log target {:?}",
            parsed.positionals[0]
        )));
    }
    let mut stream = parsed.string("stream").trim().to_lowercase();
    if !stream.is_empty() && stream != "stderr" && stream != "stdout" {
        return Err(CliError::usage("--stream must be stderr or stdout"));
    }
    let err_path = env.root.join("logs").join(format!("{target}.err.log"));
    let out_path = env.root.join("logs").join(format!("{target}.log"));
    let mut path = err_path.clone();
    if stream == "stdout" {
        path = out_path.clone();
    } else if stream.is_empty() {
        stream = "stderr".into();
        let err_empty = std::fs::metadata(&err_path)
            .map(|info| info.len() == 0)
            .unwrap_or(true);
        if err_empty
            && std::fs::metadata(&out_path)
                .map(|info| info.len() > 0)
                .unwrap_or(false)
        {
            path = out_path.clone();
            stream = "stdout".into();
        }
    }
    let (snap, exists) = match tail_log(&path, 256 << 10, lines as usize) {
        Ok(snapshot) => (snapshot, true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (
            LogSnapshot {
                lines: Vec::new(),
                truncated: false,
            },
            false,
        ),
        Err(error) => return Err(CliError::operation(error.to_string())),
    };
    let needle = parsed.string("filter").trim().to_lowercase();
    let matched: Vec<String> = snap
        .lines
        .into_iter()
        .filter(|line| needle.is_empty() || line.to_lowercase().contains(&needle))
        .collect();
    let mut payload = BTreeMap::new();
    payload.insert("jobId", serde_json::Value::String(target.clone()));
    payload.insert("stream", serde_json::Value::String(stream.clone()));
    payload.insert(
        "path",
        serde_json::Value::String(path.to_string_lossy().into_owned()),
    );
    payload.insert("exists", serde_json::Value::Bool(exists));
    payload.insert("truncated", serde_json::Value::Bool(snap.truncated));
    payload.insert("lines", serde_json::to_value(&matched).unwrap());
    crate::commands::write_result(env, out, &payload, |w| {
        if !exists {
            writeln!(w, "No logs yet ({}).", path.display())?;
            return Ok(());
        }
        if matched.is_empty() {
            writeln!(w, "No matching log lines.")?;
            return Ok(());
        }
        for line in &matched {
            writeln!(w, "{line}")?;
        }
        Ok(())
    })
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct PortListenerView {
    PID: i64,
    UID: i64,
    Name: String,
    Address: String,
    Own: bool,
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct PortInspectionView {
    Port: i64,
    Status: String,
    CheckedAt: String,
    Listeners: Vec<PortListenerView>,
    Truncated: bool,
    Detail: String,
}

fn parse_cli_port(raw: &str) -> CliResult<i64> {
    let port: i64 = raw
        .parse()
        .map_err(|_| CliError::usage("port must be 1–65535"))?;
    if !(1..=65535).contains(&port) {
        return Err(CliError::usage("port must be 1–65535"));
    }
    Ok(port)
}

fn format_owner_label(owner: &PortOwner) -> String {
    match owner {
        PortOwner::DevbaseDatabase { id, engine } => format!("devbase-db:{id} ({engine})"),
        PortOwner::DevbaseProxy => "devbase-proxy".to_string(),
        PortOwner::DevbaseRule { domain } => format!("https://{domain}"),
        PortOwner::External => String::new(),
    }
}

pub fn run_ports_command(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    if maybe_help(env, out, args, "ports")? {
        return Ok(());
    }
    let mut rest = args;
    let mut sub = "active";
    if let Some(first) = rest.first()
        && !first.starts_with('-')
    {
        sub = first;
        rest = &rest[1..];
    }
    let databases = databases(env);
    let cancelled: &dyn Fn() -> bool = &|| false;
    match sub {
        "active" => {
            let specs = FlagSpecs::new().value("category").bool("starred");
            let parsed = match parse_flags(rest, &specs)? {
                ParseOutcome::Help => {
                    crate::schema::print_topic_help(out, "ports")?;
                    return Ok(());
                }
                ParseOutcome::Parsed(parsed) => parsed,
            };
            if let Some(extra) = parsed.positionals.first() {
                return Err(CliError::usage(format!(
                    "unexpected argument {extra:?} for ports active"
                )));
            }
            let category_filter = match parsed.value("category") {
                Some(raw) if !raw.trim().is_empty() => {
                    Some(PortCategory::parse(raw).ok_or_else(|| {
                        CliError::usage(format!(
                            "unknown category {raw:?} (expected dev, database, web, or system)"
                        ))
                    })?)
                }
                _ => None,
            };
            let starred_only = parsed.boolean("starred")?;

            let mut ports = databases.listening_ports(cancelled)?;
            if let Some(category) = category_filter {
                ports.retain(|entry| entry.category == category);
            }
            if starred_only {
                ports.retain(|entry| entry.starred);
            }
            // Pin starred ports first, then sort by port number.
            ports.sort_by(|a, b| b.starred.cmp(&a.starred).then(a.port.cmp(&b.port)));

            #[derive(Serialize)]
            struct PortView {
                #[serde(rename = "Port")]
                port: i64,
                #[serde(rename = "Process")]
                process: String,
                #[serde(skip_serializing_if = "Option::is_none")]
                pid: Option<i64>,
                #[serde(skip_serializing_if = "String::is_empty")]
                address: String,
                #[serde(skip_serializing_if = "String::is_empty")]
                command: String,
                category: PortCategory,
                owner: PortOwner,
                starred: bool,
                watched: bool,
            }
            let views: Vec<PortView> = ports
                .into_iter()
                .map(|port| PortView {
                    port: port.port,
                    process: port.process,
                    pid: port.pid,
                    address: port.address,
                    command: port.command,
                    category: port.category,
                    owner: port.owner,
                    starred: port.starred,
                    watched: port.watched,
                })
                .collect();
            #[derive(Serialize)]
            struct Payload<'a> {
                ports: &'a [PortView],
            }
            crate::commands::write_result(env, out, &Payload { ports: &views }, |w| {
                if views.is_empty() {
                    writeln!(w, "No active listening local ports found.")?;
                    return Ok(());
                }
                for view in &views {
                    let pid_str = view
                        .pid
                        .map(|p| format!("pid={p}"))
                        .unwrap_or_else(|| "-".to_string());
                    let addr_str = if view.address.is_empty() {
                        "-".to_string()
                    } else {
                        view.address.clone()
                    };
                    let owner_str = format_owner_label(&view.owner);
                    let mut marks = String::new();
                    if view.starred {
                        marks.push('*');
                    }
                    if view.watched {
                        marks.push('W');
                    }
                    if owner_str.is_empty() && marks.is_empty() {
                        writeln!(
                            w,
                            "{}\t{}\t{}\t{}\t{}",
                            view.port,
                            view.process,
                            pid_str,
                            addr_str,
                            view.category.as_str()
                        )?;
                    } else {
                        writeln!(
                            w,
                            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                            view.port,
                            view.process,
                            pid_str,
                            addr_str,
                            view.category.as_str(),
                            owner_str,
                            marks
                        )?;
                    }
                }
                Ok(())
            })
        }
        "free" => {
            if let Some(extra) = rest.first() {
                return Err(CliError::usage(format!(
                    "unexpected argument {extra:?} for ports free"
                )));
            }
            let port = databases.available_local_port(cancelled)?;
            let mut payload = BTreeMap::new();
            payload.insert("port", serde_json::Value::from(port));
            crate::commands::write_result(env, out, &payload, |w| writeln!(w, "{port}"))
        }
        "check" => {
            if rest.len() != 1 {
                return Err(CliError::usage("usage: devbase ports check <port>"));
            }
            let port = parse_cli_port(&rest[0])?;
            let state = store(env).read()?;
            let owner = owner_for_port(&state, port);
            let starred = state.favorite_ports.contains(&port);
            let watched = state.watched_ports.contains(&port);
            let inspection = databases.inspect_port(port, cancelled)?;
            let view = PortInspectionView {
                Port: inspection.port,
                Status: inspection.status,
                CheckedAt: inspection.checked_at,
                Listeners: inspection
                    .listeners
                    .into_iter()
                    .map(|listener| PortListenerView {
                        PID: listener.pid,
                        UID: listener.uid,
                        Name: listener.name,
                        Address: listener.address,
                        Own: listener.own,
                    })
                    .collect(),
                Truncated: inspection.truncated,
                Detail: inspection.detail,
            };
            #[derive(Serialize)]
            struct CheckPayload {
                #[serde(flatten)]
                inspection: PortInspectionView,
                owner: PortOwner,
                starred: bool,
                watched: bool,
            }
            let payload = CheckPayload {
                inspection: view,
                owner,
                starred,
                watched,
            };
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(
                    w,
                    "Port {}: {}",
                    payload.inspection.Port, payload.inspection.Status
                )?;
                let owner_str = format_owner_label(&payload.owner);
                if !owner_str.is_empty() {
                    writeln!(w, "Owner: {owner_str}")?;
                }
                if !payload.inspection.Detail.is_empty() {
                    writeln!(w, "{}", payload.inspection.Detail)?;
                }
                for listener in &payload.inspection.Listeners {
                    writeln!(
                        w,
                        "  PID {} (UID {}) {} {}",
                        listener.PID, listener.UID, listener.Name, listener.Address
                    )?;
                }
                Ok(())
            })
        }
        "kill" => {
            let specs = FlagSpecs::new().bool("yes").bool("force").bool("pid");
            let parsed = match parse_flags(rest, &specs)? {
                ParseOutcome::Help => {
                    crate::schema::print_topic_help(out, "ports")?;
                    return Ok(());
                }
                ParseOutcome::Parsed(parsed) => parsed,
            };
            if parsed.positionals.len() != 1 {
                return Err(CliError::usage(
                    "usage: devbase ports kill <port|pid> --yes [--force] [--pid]",
                ));
            }
            let target_raw = &parsed.positionals[0];
            let by_pid = parsed.boolean("pid")?;
            let force = parsed.boolean("force")?;
            let yes = parsed.boolean("yes")?;

            if by_pid {
                let pid: i64 = target_raw
                    .parse()
                    .map_err(|_| CliError::usage("PID must be a positive integer"))?;
                if pid <= 1 {
                    return Err(CliError::usage("refusing to stop protected system PID"));
                }
                require_yes(yes, &format!("stopping PID {pid} requires --yes"))?;
                let outcome = databases.terminate_port_process(pid, force)?;
                let signal_name = if force { "SIGKILL" } else { "SIGTERM" };
                let payload = serde_json::json!({
                    "action": "signal-process",
                    "pid": pid,
                    "signal": signal_name,
                    "outcome": outcome,
                });
                return crate::commands::write_result(env, out, &payload, |w| match outcome {
                    SignalOutcome::Stopped => writeln!(w, "Stopped PID {pid} ({signal_name})."),
                    SignalOutcome::AlreadyExited => writeln!(w, "PID {pid} had already exited."),
                    SignalOutcome::StillRunning => writeln!(
                        w,
                        "PID {pid} did not exit after {signal_name}; re-run with --force to send SIGKILL."
                    ),
                });
            }

            let port = parse_cli_port(target_raw)?;
            require_yes(
                yes,
                &format!("stopping listener on port {port} requires --yes"),
            )?;
            let state = store(env).read()?;
            match owner_for_port(&state, port) {
                PortOwner::DevbaseDatabase { id, engine } => {
                    databases.instance_action(&id, InstanceAction::Stop, cancelled)?;
                    let payload = serde_json::json!({
                        "action": "stop-database",
                        "port": port,
                        "instance": id,
                        "engine": engine,
                        "stopped": true,
                    });
                    crate::commands::write_result(env, out, &payload, |w| {
                        writeln!(
                            w,
                            "Stopped Devbase database instance {id} ({engine}) on port {port}."
                        )
                    })
                }
                PortOwner::DevbaseProxy => {
                    crate::commands::https::proxy_service(env).stop()?;
                    let payload = serde_json::json!({
                        "action": "stop-proxy",
                        "port": port,
                        "stopped": true,
                    });
                    crate::commands::write_result(env, out, &payload, |w| {
                        writeln!(w, "Stopped Devbase HTTPS proxy on port {port}.")
                    })
                }
                PortOwner::DevbaseRule { .. } | PortOwner::External => {
                    let active = databases.listening_ports(cancelled)?;
                    let pid = active
                        .into_iter()
                        .find(|entry| entry.port == port)
                        .and_then(|entry| entry.pid)
                        .ok_or_else(|| {
                            CliError::operation(format!(
                                "no listening process PID found for port {port}"
                            ))
                        })?;
                    let outcome = databases.terminate_port_process(pid, force)?;
                    let signal_name = if force { "SIGKILL" } else { "SIGTERM" };
                    let payload = serde_json::json!({
                        "action": "signal-process",
                        "port": port,
                        "pid": pid,
                        "signal": signal_name,
                        "outcome": outcome,
                    });
                    crate::commands::write_result(env, out, &payload, |w| match outcome {
                        SignalOutcome::Stopped => {
                            writeln!(w, "Stopped PID {pid} on port {port} ({signal_name}).")
                        }
                        SignalOutcome::AlreadyExited => {
                            writeln!(w, "PID {pid} on port {port} had already exited.")
                        }
                        SignalOutcome::StillRunning => writeln!(
                            w,
                            "PID {pid} on port {port} did not exit after {signal_name}; re-run with --force to send SIGKILL."
                        ),
                    })
                }
            }
        }
        "star" | "unstar" => {
            if rest.len() != 1 {
                return Err(CliError::usage(format!(
                    "usage: devbase ports {sub} <port>"
                )));
            }
            let port = parse_cli_port(&rest[0])?;
            let starred = sub == "star";
            let favorite_ports = databases.set_port_starred(port, starred)?;
            let payload = serde_json::json!({
                "port": port,
                "starred": starred,
                "favoritePorts": favorite_ports,
            });
            crate::commands::write_result(env, out, &payload, |w| {
                if starred {
                    writeln!(w, "Starred port {port}.")
                } else {
                    writeln!(w, "Unstarred port {port}.")
                }
            })
        }
        "watch" | "unwatch" => {
            if rest.len() != 1 {
                return Err(CliError::usage(format!(
                    "usage: devbase ports {sub} <port>"
                )));
            }
            let port = parse_cli_port(&rest[0])?;
            let watched = sub == "watch";
            let watched_ports = databases.set_port_watched(port, watched)?;
            let payload = serde_json::json!({
                "port": port,
                "watched": watched,
                "watchedPorts": watched_ports,
            });
            crate::commands::write_result(env, out, &payload, |w| {
                if watched {
                    writeln!(w, "Watching port {port}.")
                } else {
                    writeln!(w, "Stopped watching port {port}.")
                }
            })
        }
        other => Err(CliError::usage(format!(
            "unknown ports subcommand {other:?} (expected active, free, check, kill, star, unstar, watch, or unwatch)"
        ))),
    }
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct ShutdownInstanceView {
    ID: String,
    Name: String,
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct ShutdownPlanView {
    Instances: Vec<ShutdownInstanceView>,
    Fingerprint: String,
}

fn plan_view(plan: &ShutdownPlan) -> ShutdownPlanView {
    ShutdownPlanView {
        Instances: plan
            .instances
            .iter()
            .map(|instance| ShutdownInstanceView {
                ID: instance.id.clone(),
                Name: instance.name.clone(),
            })
            .collect(),
        Fingerprint: plan.fingerprint.clone(),
    }
}

pub fn run_shutdown_command(
    env: &mut CliEnv,
    out: &mut dyn Write,
    args: &[String],
) -> CliResult<()> {
    if maybe_help(env, out, args, "shutdown")? {
        return Ok(());
    }
    let mut rest = args;
    let mut sub = "preview";
    if let Some(first) = rest.first() {
        sub = first;
        rest = &rest[1..];
    }
    let store = store(env);
    let engine = engine(env);
    let runner = ShutdownRunner::new(&store, &engine);
    let cancelled: &dyn Fn() -> bool = &|| false;
    match sub {
        "preview" => {
            if let Some(extra) = rest.first() {
                return Err(CliError::usage(format!(
                    "unexpected argument {extra:?} for shutdown preview"
                )));
            }
            let plan = runner.preview(cancelled)?;
            let view = plan_view(&plan);
            crate::commands::write_result(env, out, &view, |w| {
                writeln!(
                    w,
                    "Managed database instances in stop scope: {} (fingerprint: {})",
                    view.Instances.len(),
                    view.Fingerprint
                )?;
                for instance in &view.Instances {
                    writeln!(w, "  - {} ({})", instance.ID, instance.Name)?;
                }
                Ok(())
            })
        }
        "stop" => {
            let specs = FlagSpecs::new().bool("yes");
            let ParseOutcome::Parsed(parsed) = parse_flags(rest, &specs)? else {
                return Ok(());
            };
            if let Some(extra) = parsed.positionals.first() {
                return Err(CliError::usage(format!(
                    "unexpected argument {extra:?} for shutdown stop"
                )));
            }
            require_yes(
                parsed.flag("yes"),
                "stopping all managed database instances requires --yes",
            )?;
            let plan = runner.preview(cancelled)?;
            runner.execute(&plan, &ShutdownOptions {}, cancelled)?;
            let view = plan_view(&plan);
            let payload = serde_json::json!({
                "stopped": view.Instances.len(),
                "instances": view.Instances,
            });
            crate::commands::write_result(env, out, &payload, |w| {
                writeln!(
                    w,
                    "Stopped {} managed database instances (HTTPS proxy and tunnel remain running).",
                    payload["stopped"]
                )
            })
        }
        other => Err(CliError::usage(format!(
            "unknown shutdown subcommand {other:?} (expected preview or stop)"
        ))),
    }
}

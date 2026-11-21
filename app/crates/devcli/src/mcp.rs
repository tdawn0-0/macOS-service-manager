//! Read-only stdio Model Context Protocol (MCP) server (`devbase mcp`).
//!
//! Speaks newline-delimited JSON-RPC 2.0 over stdin/stdout with zero external
//! dependencies. Every exposed tool is strictly read-only, never writes to
//! disk or mutates system state, and never emits plaintext secrets.

use std::io::{BufRead, BufReader, Read, Write};

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::args::has_help_flag;
use crate::commands;
use crate::env::CliEnv;
use crate::error::{CliError, CliResult};
use crate::schema::CLI_VERSION;

/// Supported MCP protocol versions (newest first).
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-03-26", "2024-11-05"];
const DEFAULT_PROTOCOL_VERSION: &str = "2024-11-05";

/// Maximum accepted JSON-RPC line length (1 MiB) to keep memory bounded.
const MAX_LINE_BYTES: usize = 1024 * 1024;

// JSON-RPC 2.0 standard error codes.
const ERR_PARSE_ERROR: i64 = -32700;
const ERR_INVALID_REQUEST: i64 = -32600;
const ERR_METHOD_NOT_FOUND: i64 = -32601;
const ERR_INVALID_PARAMS: i64 = -32602;

#[derive(Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Serialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

impl JsonRpcResponse {
    fn ok(id: Value, result: Value) -> Self {
        JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    fn err(id: Value, code: i64, message: impl Into<String>) -> Self {
        JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
            }),
        }
    }
}

/// Entry point from `run_cli_with_env`: takes ownership of `env.input` so the
/// JSON-RPC stream is read line-by-line while tools receive `&mut CliEnv`.
pub fn run_mcp_command(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    if has_help_flag(args) {
        crate::schema::print_topic_help(out, "mcp")?;
        return Ok(());
    }
    if let Some(extra) = args.first() {
        return Err(CliError::usage(format!(
            "unexpected argument {extra:?} for mcp (usage: devbase mcp)"
        )));
    }
    let input = std::mem::replace(&mut env.input, Box::new(std::io::empty()));
    let mut reader = BufReader::new(input);
    serve(env, &mut reader, out)
}

/// Entry point from `run_cli`: reads directly from `input` without calling
/// `read_to_end`, so live stdio sessions stay interactive.
pub fn run_mcp_with_reader(
    env: &mut CliEnv,
    input: &mut dyn Read,
    out: &mut dyn Write,
    args: &[String],
) -> CliResult<()> {
    if has_help_flag(args) {
        crate::schema::print_topic_help(out, "mcp")?;
        return Ok(());
    }
    if let Some(extra) = args.first() {
        return Err(CliError::usage(format!(
            "unexpected argument {extra:?} for mcp (usage: devbase mcp)"
        )));
    }
    let mut reader = BufReader::new(input);
    serve(env, &mut reader, out)
}

/// Serve newline-delimited JSON-RPC 2.0 MCP messages until `reader` hits EOF.
pub fn serve(env: &mut CliEnv, reader: &mut dyn BufRead, out: &mut dyn Write) -> CliResult<()> {
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let read = read_bounded_line(reader, &mut buf, MAX_LINE_BYTES)
            .map_err(|error| CliError::operation(error.to_string()))?;
        match read {
            LineRead::Eof => break,
            LineRead::TooLong => {
                write_response(
                    out,
                    &JsonRpcResponse::err(
                        Value::Null,
                        ERR_INVALID_REQUEST,
                        "request line exceeds maximum size",
                    ),
                )?;
                continue;
            }
            LineRead::Line => {}
        }

        let line = match std::str::from_utf8(&buf) {
            Ok(text) => text.trim(),
            Err(_) => {
                write_response(
                    out,
                    &JsonRpcResponse::err(Value::Null, ERR_PARSE_ERROR, "invalid UTF-8"),
                )?;
                continue;
            }
        };
        if line.is_empty() {
            continue;
        }

        if let Some(response) = handle_line(env, line) {
            write_response(out, &response)?;
        }
    }
    Ok(())
}

enum LineRead {
    Line,
    Eof,
    TooLong,
}

fn read_bounded_line(
    reader: &mut dyn BufRead,
    out: &mut Vec<u8>,
    max_bytes: usize,
) -> std::io::Result<LineRead> {
    let mut exceeded = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if out.is_empty() && !exceeded {
                return Ok(LineRead::Eof);
            }
            return Ok(if exceeded {
                LineRead::TooLong
            } else {
                LineRead::Line
            });
        }
        let (consume, done) = match available.iter().position(|byte| *byte == b'\n') {
            Some(index) => (index + 1, true),
            None => (available.len(), false),
        };
        if !exceeded {
            if out.len() + consume > max_bytes {
                exceeded = true;
                out.clear();
            } else {
                out.extend_from_slice(&available[..consume]);
            }
        }
        reader.consume(consume);
        if done {
            return Ok(if exceeded {
                LineRead::TooLong
            } else {
                LineRead::Line
            });
        }
    }
}

fn write_response(out: &mut dyn Write, response: &JsonRpcResponse) -> CliResult<()> {
    let encoded =
        serde_json::to_string(response).map_err(|error| CliError::operation(error.to_string()))?;
    writeln!(out, "{encoded}")?;
    out.flush()?;
    Ok(())
}

fn handle_line(env: &mut CliEnv, line: &str) -> Option<JsonRpcResponse> {
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => {
            return Some(JsonRpcResponse::err(
                Value::Null,
                ERR_PARSE_ERROR,
                format!("parse error: {error}"),
            ));
        }
    };

    let Some(obj) = value.as_object() else {
        return Some(JsonRpcResponse::err(
            Value::Null,
            ERR_INVALID_REQUEST,
            "request must be a JSON object",
        ));
    };

    let id = obj.get("id").cloned();
    let is_notification = id.is_none();

    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        if is_notification {
            return None;
        }
        return Some(JsonRpcResponse::err(
            id.unwrap_or(Value::Null),
            ERR_INVALID_REQUEST,
            "jsonrpc must be \"2.0\"",
        ));
    }

    let Some(method) = obj.get("method").and_then(Value::as_str) else {
        if is_notification {
            return None;
        }
        return Some(JsonRpcResponse::err(
            id.unwrap_or(Value::Null),
            ERR_INVALID_REQUEST,
            "missing string method",
        ));
    };

    // Notifications (no id) never receive a response per JSON-RPC 2.0 & MCP spec.
    if is_notification {
        return None;
    }

    let id = id.unwrap_or(Value::Null);
    let params = obj.get("params");

    match method {
        "initialize" => Some(handle_initialize(id, params)),
        "ping" => Some(JsonRpcResponse::ok(id, json!({}))),
        "tools/list" => Some(JsonRpcResponse::ok(
            id,
            json!({ "tools": tool_definitions() }),
        )),
        "tools/call" => Some(handle_tools_call(env, id, params)),
        "notifications/initialized" => Some(JsonRpcResponse::ok(id, json!({}))),
        other => Some(JsonRpcResponse::err(
            id,
            ERR_METHOD_NOT_FOUND,
            format!("method not found: {other}"),
        )),
    }
}

fn handle_initialize(id: Value, params: Option<&Value>) -> JsonRpcResponse {
    let requested = params
        .and_then(|value| value.get("protocolVersion"))
        .and_then(Value::as_str);
    let protocol_version = requested
        .filter(|version| SUPPORTED_PROTOCOL_VERSIONS.contains(version))
        .unwrap_or(DEFAULT_PROTOCOL_VERSION);

    JsonRpcResponse::ok(
        id,
        json!({
            "protocolVersion": protocol_version,
            "capabilities": {
                "tools": {
                    "listChanged": false
                }
            },
            "serverInfo": {
                "name": "devbase",
                "version": CLI_VERSION
            },
            "instructions": "Read-only Devbase workspace inspection server. All tools are read-only, never mutate system state, and never return plaintext secrets."
        }),
    )
}

fn read_only_annotations() -> Value {
    json!({
        "readOnlyHint": true,
        "destructiveHint": false,
        "idempotentHint": true,
        "openWorldHint": false
    })
}

/// Static catalog of read-only MCP tools exposed by `devbase mcp`.
pub fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "devbase_status",
            "description": "Inspect Devbase workspace state: installed packages, database instances, HTTPS proxy rules, runtime statuses, privileged helper status, and CLI wrapper status.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            },
            "annotations": read_only_annotations()
        }),
        json!({
            "name": "devbase_db_list",
            "description": "List managed database instances, ports, storage modes, runtime statuses, and compatible upgrade targets.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            },
            "annotations": read_only_annotations()
        }),
        json!({
            "name": "devbase_db_connection",
            "description": "Return password-free connection details (URI, environment assignment, JDBC URL, CLI command, username, and credential file path — never plaintext passwords) for a managed database instance.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Managed database instance ID."
                    },
                    "profile": {
                        "type": "string",
                        "description": "Optional saved connection profile ID."
                    },
                    "database": {
                        "type": "string",
                        "description": "Optional database name override."
                    },
                    "format": {
                        "type": "string",
                        "enum": ["uri", "env", "jdbc", "cli"],
                        "description": "Optional connection format check."
                    }
                },
                "required": ["id"],
                "additionalProperties": false
            },
            "annotations": read_only_annotations()
        }),
        json!({
            "name": "devbase_logs",
            "description": "Read a bounded tail of Devbase logs for proxy, tunnel, or db-<id> (or a configured database instance ID).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "Log target: \"proxy\", \"tunnel\", \"db-<id>\", or a configured database instance ID."
                    },
                    "stream": {
                        "type": "string",
                        "enum": ["stderr", "stdout"],
                        "description": "Optional log stream (defaults to stderr, falling back to stdout if stderr is empty)."
                    },
                    "lines": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 5000,
                        "description": "Maximum number of trailing lines to return (default 200, max 5000)."
                    },
                    "filter": {
                        "type": "string",
                        "description": "Optional case-insensitive substring filter."
                    }
                },
                "required": ["target"],
                "additionalProperties": false
            },
            "annotations": read_only_annotations()
        }),
        json!({
            "name": "devbase_ports",
            "description": "Inspect active listening local user ports, suggest a free local port, or inspect listeners on a specific port.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["active", "free", "check"],
                        "description": "Port operation: \"active\" (default), \"free\", or \"check\"."
                    },
                    "port": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 65535,
                        "description": "Port number (1-65535), required when action is \"check\"."
                    }
                },
                "additionalProperties": false
            },
            "annotations": read_only_annotations()
        }),
        json!({
            "name": "devbase_diagnostics",
            "description": "Run Devbase layered health diagnostics across runtimes, databases, HTTPS proxy, tunnel, and local CA and return the redacted report.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            },
            "annotations": read_only_annotations()
        }),
        json!({
            "name": "devbase_services_list",
            "description": "List macOS services across launchd (user-agent, system-agent, daemon), brew, and login-item with optional filtering.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "source": {
                        "type": "string",
                        "enum": ["brew", "login-item", "user-agent", "system-agent", "daemon"],
                        "description": "Optional service source filter."
                    },
                    "state": {
                        "type": "string",
                        "description": "Optional state filter (running, stopped, idle, scheduled, error, up, active)."
                    },
                    "query": {
                        "type": "string",
                        "description": "Optional case-insensitive substring query."
                    }
                },
                "additionalProperties": false
            },
            "annotations": read_only_annotations()
        }),
    ]
}

fn handle_tools_call(env: &mut CliEnv, id: Value, params: Option<&Value>) -> JsonRpcResponse {
    let Some(params) = params.and_then(Value::as_object) else {
        return JsonRpcResponse::err(id, ERR_INVALID_PARAMS, "params must be an object");
    };
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return JsonRpcResponse::err(id, ERR_INVALID_PARAMS, "params.name must be a string");
    };
    let empty_args = Map::new();
    let arguments = match params.get("arguments") {
        None | Some(Value::Null) => &empty_args,
        Some(Value::Object(map)) => map,
        Some(_) => {
            return JsonRpcResponse::err(
                id,
                ERR_INVALID_PARAMS,
                "params.arguments must be an object",
            );
        }
    };

    let result = match name {
        "devbase_status" => call_status(env, arguments),
        "devbase_db_list" => call_db_list(env, arguments),
        "devbase_db_connection" => call_db_connection(env, arguments),
        "devbase_logs" => call_logs(env, arguments),
        "devbase_ports" => call_ports(env, arguments),
        "devbase_diagnostics" => call_diagnostics(env, arguments),
        "devbase_services_list" => call_services_list(env, arguments),
        other => {
            return JsonRpcResponse::err(id, ERR_INVALID_PARAMS, format!("unknown tool {other:?}"));
        }
    };

    match result {
        Ok(text) => JsonRpcResponse::ok(
            id,
            json!({
                "content": [
                    {
                        "type": "text",
                        "text": text
                    }
                ],
                "isError": false
            }),
        ),
        Err(message) => JsonRpcResponse::ok(
            id,
            json!({
                "content": [
                    {
                        "type": "text",
                        "text": message
                    }
                ],
                "isError": true
            }),
        ),
    }
}

fn capture_json(
    env: &mut CliEnv,
    run: impl FnOnce(&mut CliEnv, &mut dyn Write) -> CliResult<()>,
) -> Result<String, String> {
    let previous = env.json;
    env.json = true;
    let mut out = Vec::new();
    let outcome = run(env, &mut out);
    env.json = previous;
    outcome.map_err(|error| error.message().to_string())?;
    let text = String::from_utf8(out).map_err(|error| error.to_string())?;
    Ok(text.trim_end().to_string())
}

fn reject_unknown_keys(args: &Map<String, Value>, allowed: &[&str]) -> Result<(), String> {
    for key in args.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!("unknown argument {key:?}"));
        }
    }
    Ok(())
}

fn optional_string<'a>(args: &'a Map<String, Value>, key: &str) -> Result<Option<&'a str>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.as_str())),
        Some(_) => Err(format!("argument {key:?} must be a string")),
    }
}

fn required_string<'a>(args: &'a Map<String, Value>, key: &str) -> Result<&'a str, String> {
    match optional_string(args, key)? {
        Some(value) if !value.trim().is_empty() => Ok(value.trim()),
        _ => Err(format!("missing required string argument {key:?}")),
    }
}

fn optional_i64(args: &Map<String, Value>, key: &str) -> Result<Option<i64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(num)) => num
            .as_i64()
            .map(Some)
            .ok_or_else(|| format!("argument {key:?} must be an integer")),
        Some(_) => Err(format!("argument {key:?} must be an integer")),
    }
}

fn call_status(env: &mut CliEnv, args: &Map<String, Value>) -> Result<String, String> {
    reject_unknown_keys(args, &[])?;
    capture_json(env, |env, out| {
        commands::workspace::run_status_command(env, out, &[])
    })
}

fn call_db_list(env: &mut CliEnv, args: &Map<String, Value>) -> Result<String, String> {
    reject_unknown_keys(args, &[])?;
    let cli_args = vec!["list".to_string()];
    capture_json(env, |env, out| {
        commands::databases::run_database_command(env, out, &cli_args)
    })
}

fn call_db_connection(env: &mut CliEnv, args: &Map<String, Value>) -> Result<String, String> {
    reject_unknown_keys(args, &["id", "profile", "database", "format"])?;
    let id = required_string(args, "id")?;
    if !devcore::model::valid_id(id) {
        return Err(format!("invalid database instance ID {id:?}"));
    }
    let mut cli_args = vec!["connection".to_string()];
    if let Some(profile) = optional_string(args, "profile")?
        && !profile.trim().is_empty()
    {
        cli_args.push(format!("--profile={}", profile.trim()));
    }
    if let Some(database) = optional_string(args, "database")?
        && !database.trim().is_empty()
    {
        cli_args.push(format!("--database={}", database.trim()));
    }
    if let Some(format) = optional_string(args, "format")?
        && !format.trim().is_empty()
    {
        let normalized = format.trim().to_lowercase();
        if !matches!(normalized.as_str(), "uri" | "env" | "jdbc" | "cli") {
            return Err("format must be one of uri, env, jdbc, cli".to_string());
        }
        cli_args.push(format!("--format={normalized}"));
    }
    cli_args.push("--".to_string());
    cli_args.push(id.to_string());
    capture_json(env, |env, out| {
        commands::databases::run_database_command(env, out, &cli_args)
    })
}

fn validate_mcp_log_target(env: &CliEnv, raw: &str) -> Result<String, String> {
    let target = raw.trim();
    if target.is_empty()
        || target.contains('/')
        || target.contains('\\')
        || target.contains("..")
        || target.starts_with('-')
    {
        return Err(format!(
            "invalid log target {raw:?}: expected \"proxy\", \"tunnel\", \"db-<id>\", or a configured instance ID"
        ));
    }
    if target == "proxy" || target == "tunnel" {
        return Ok(target.to_string());
    }
    if let Some(instance_id) = target.strip_prefix("db-") {
        if devcore::model::valid_id(instance_id) {
            return Ok(target.to_string());
        }
        return Err(format!("invalid database log target {raw:?}"));
    }
    if devcore::model::valid_id(target) {
        let store = devcore::state::Store::new(env.root.clone());
        if let Ok(state) = store.read()
            && state.instances.contains_key(target)
        {
            return Ok(format!("db-{target}"));
        }
    }
    Err(format!(
        "invalid log target {raw:?}: expected \"proxy\", \"tunnel\", \"db-<id>\", or a configured instance ID"
    ))
}

fn call_logs(env: &mut CliEnv, args: &Map<String, Value>) -> Result<String, String> {
    reject_unknown_keys(args, &["target", "stream", "lines", "filter"])?;
    let raw_target = required_string(args, "target")?;
    let target = validate_mcp_log_target(env, raw_target)?;

    let mut cli_args = Vec::new();
    if let Some(stream) = optional_string(args, "stream")?
        && !stream.trim().is_empty()
    {
        let normalized = stream.trim().to_lowercase();
        if normalized != "stderr" && normalized != "stdout" {
            return Err("stream must be stderr or stdout".to_string());
        }
        cli_args.push(format!("--stream={normalized}"));
    }
    if let Some(lines) = optional_i64(args, "lines")? {
        if !(1..=5000).contains(&lines) {
            return Err("lines must be between 1 and 5000".to_string());
        }
        cli_args.push(format!("--lines={lines}"));
    }
    if let Some(filter) = optional_string(args, "filter")?
        && !filter.is_empty()
    {
        cli_args.push(format!("--filter={filter}"));
    }
    cli_args.push("--".to_string());
    cli_args.push(target);

    capture_json(env, |env, out| {
        commands::workspace::run_logs_command(env, out, &cli_args)
    })
}

fn call_ports(env: &mut CliEnv, args: &Map<String, Value>) -> Result<String, String> {
    reject_unknown_keys(args, &["action", "port"])?;
    let action = optional_string(args, "action")?
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("active")
        .to_lowercase();
    let cli_args = match action.as_str() {
        "active" => vec!["active".to_string()],
        "free" => vec!["free".to_string()],
        "check" => {
            let port = optional_i64(args, "port")?
                .ok_or_else(|| "port (1-65535) is required when action is \"check\"".to_string())?;
            if !(1..=65535).contains(&port) {
                return Err("port must be 1–65535".to_string());
            }
            vec!["check".to_string(), port.to_string()]
        }
        other => {
            return Err(format!(
                "unknown ports action {other:?} (expected active, free, or check)"
            ));
        }
    };
    capture_json(env, |env, out| {
        commands::workspace::run_ports_command(env, out, &cli_args)
    })
}

fn call_diagnostics(env: &mut CliEnv, args: &Map<String, Value>) -> Result<String, String> {
    reject_unknown_keys(args, &[])?;
    let store = devcore::state::Store::new(env.root.clone());
    let engine = devcore::engine::Engine::new(
        env.root.clone(),
        std::sync::Arc::new(devcore::engine::ProcessRunnerAdapter::new(
            std::sync::Arc::new(devcore::launchd::SystemRunner::new()),
        )),
    );
    let diagnostics = devcore::diagnostics::Diagnostics::new(
        store.clone(),
        engine,
        devcore::software::Software::new(store),
    );
    let report = diagnostics.diagnose().map_err(|error| error.to_string())?;
    let bytes = report.redacted_json().map_err(|error| error.to_string())?;
    let text = String::from_utf8(bytes).map_err(|error| error.to_string())?;
    Ok(text.trim_end().to_string())
}

fn call_services_list(env: &mut CliEnv, args: &Map<String, Value>) -> Result<String, String> {
    reject_unknown_keys(args, &["source", "state", "query"])?;
    let mut cli_args = vec!["list".to_string()];
    if let Some(source) = optional_string(args, "source")?
        && !source.trim().is_empty()
    {
        cli_args.push(format!("--source={}", source.trim()));
    }
    if let Some(state) = optional_string(args, "state")?
        && !state.trim().is_empty()
    {
        cli_args.push(format!("--state={}", state.trim()));
    }
    if let Some(query) = optional_string(args, "query")?
        && !query.is_empty()
    {
        cli_args.push(format!("--query={query}"));
    }
    capture_json(env, |env, out| {
        commands::services::run_services_command(env, out, &cli_args)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::io::Cursor;

    fn test_env(dir: &tempfile::TempDir) -> CliEnv {
        let mut env = CliEnv::for_test();
        env.root = dir.path().to_path_buf();
        env.cli_bin_dir = dir.path().join("bin");
        env.runtime_states = Some(BTreeMap::from([
            ("proxy".to_string(), "Stopped".to_string()),
            ("instance:pg-app".to_string(), "Running".to_string()),
        ]));
        env
    }

    fn seed_postgres_instance(dir: &tempfile::TempDir) {
        let store = devcore::state::Store::new(dir.path().to_path_buf());
        store
            .with_state(|state| {
                state.instances.insert(
                    "pg-app".to_string(),
                    devcore::model::Instance {
                        id: "pg-app".to_string(),
                        kind: "postgresql".to_string(),
                        package: "postgresql@17-arm64".to_string(),
                        port: 5432,
                        persistent: true,
                        ..Default::default()
                    },
                );
                Ok(())
            })
            .unwrap();
        // Write a secret password file to prove MCP never leaks it.
        let pw_dir = dir.path().join("instances").join("pg-app");
        std::fs::create_dir_all(&pw_dir).unwrap();
        std::fs::write(
            pw_dir.join("password"),
            "SUPER_SECRET_PASSWORD_DO_NOT_LEAK\n",
        )
        .unwrap();
    }

    fn exchange(env: &mut CliEnv, requests: &[Value]) -> Vec<Value> {
        let mut stdin_text = String::new();
        for req in requests {
            stdin_text.push_str(&serde_json::to_string(req).unwrap());
            stdin_text.push('\n');
        }
        let mut reader = Cursor::new(stdin_text.into_bytes());
        let mut out = Vec::new();
        serve(env, &mut reader, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        text.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn handshake_ping_and_tools_list() {
        let dir = tempfile::tempdir().unwrap();
        let mut env = test_env(&dir);
        let responses = exchange(
            &mut env,
            &[
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": "2025-03-26",
                        "capabilities": {},
                        "clientInfo": { "name": "test", "version": "1.0" }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/initialized"
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "ping"
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "tools/list"
                }),
            ],
        );

        // Notification produces no response: 3 requests -> 3 responses.
        assert_eq!(responses.len(), 3);
        assert_eq!(responses[0]["id"], 1);
        assert_eq!(responses[0]["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(responses[0]["result"]["serverInfo"]["name"], "devbase");
        assert_eq!(responses[0]["result"]["serverInfo"]["version"], CLI_VERSION);

        assert_eq!(responses[1]["id"], 2);
        assert_eq!(responses[1]["result"], json!({}));

        assert_eq!(responses[2]["id"], 3);
        let tools = responses[2]["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            vec![
                "devbase_status",
                "devbase_db_list",
                "devbase_db_connection",
                "devbase_logs",
                "devbase_ports",
                "devbase_diagnostics",
                "devbase_services_list",
            ]
        );
        for tool in tools {
            assert_eq!(tool["annotations"]["readOnlyHint"], true);
            assert_eq!(tool["annotations"]["destructiveHint"], false);
        }
    }

    #[test]
    fn status_db_list_and_password_free_connection() {
        let dir = tempfile::tempdir().unwrap();
        seed_postgres_instance(&dir);
        let mut env = test_env(&dir);

        let responses = exchange(
            &mut env,
            &[
                json!({
                    "jsonrpc": "2.0",
                    "id": "s1",
                    "method": "tools/call",
                    "params": { "name": "devbase_status", "arguments": {} }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": "d1",
                    "method": "tools/call",
                    "params": { "name": "devbase_db_list" }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": "c1",
                    "method": "tools/call",
                    "params": {
                        "name": "devbase_db_connection",
                        "arguments": { "id": "pg-app", "database": "myapp" }
                    }
                }),
            ],
        );

        assert_eq!(responses.len(), 3);
        for resp in &responses {
            assert_eq!(resp["result"]["isError"], false, "{resp}");
            let text = resp["result"]["content"][0]["text"].as_str().unwrap();
            assert!(
                !text.contains("SUPER_SECRET_PASSWORD_DO_NOT_LEAK"),
                "secret leaked in MCP tool response: {text}"
            );
        }

        let status: Value = serde_json::from_str(
            responses[0]["result"]["content"][0]["text"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(status["instances"]["pg-app"]["kind"], "postgresql");
        assert_eq!(status["runtime"]["states"]["instance:pg-app"], "Running");

        let db_list: Value = serde_json::from_str(
            responses[1]["result"]["content"][0]["text"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(db_list["instances"][0]["id"], "pg-app");

        let conn: Value = serde_json::from_str(
            responses[2]["result"]["content"][0]["text"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(conn["instanceId"], "pg-app");
        assert_eq!(conn["uri"], "postgresql://devmanager@127.0.0.1:5432/myapp");
        assert_eq!(
            conn["jdbc"],
            "jdbc:postgresql://127.0.0.1:5432/myapp?user=devmanager"
        );
    }

    #[test]
    fn logs_validates_job_ids_and_tails_safely() {
        let dir = tempfile::tempdir().unwrap();
        seed_postgres_instance(&dir);
        let logs_dir = dir.path().join("logs");
        std::fs::create_dir_all(&logs_dir).unwrap();
        std::fs::write(
            logs_dir.join("proxy.err.log"),
            "boot ok\nwarn: slow upstream\nready\n",
        )
        .unwrap();
        std::fs::write(logs_dir.join("db-pg-app.log"), "pg started\n").unwrap();

        let mut env = test_env(&dir);
        let responses = exchange(
            &mut env,
            &[
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": {
                        "name": "devbase_logs",
                        "arguments": { "target": "proxy", "stream": "stderr", "filter": "warn", "lines": 50 }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {
                        "name": "devbase_logs",
                        "arguments": { "target": "pg-app" }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "tools/call",
                    "params": {
                        "name": "devbase_logs",
                        "arguments": { "target": "../state.json" }
                    }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 4,
                    "method": "tools/call",
                    "params": {
                        "name": "devbase_logs",
                        "arguments": { "target": "arbitrary-name" }
                    }
                }),
            ],
        );

        assert_eq!(responses[0]["result"]["isError"], false);
        let proxy_log: Value = serde_json::from_str(
            responses[0]["result"]["content"][0]["text"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(proxy_log["jobId"], "proxy");
        assert_eq!(proxy_log["lines"], json!(["warn: slow upstream"]));

        assert_eq!(responses[1]["result"]["isError"], false);
        let db_log: Value = serde_json::from_str(
            responses[1]["result"]["content"][0]["text"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(db_log["jobId"], "db-pg-app");
        assert_eq!(db_log["stream"], "stdout");
        assert_eq!(db_log["lines"], json!(["pg started"]));

        assert_eq!(responses[2]["result"]["isError"], true);
        assert!(
            responses[2]["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("invalid log target")
        );

        assert_eq!(responses[3]["result"]["isError"], true);
    }

    #[test]
    fn ports_diagnostics_and_services_list_work() {
        let dir = tempfile::tempdir().unwrap();
        let mut env = test_env(&dir);
        let responses = exchange(
            &mut env,
            &[
                json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "tools/call",
                    "params": { "name": "devbase_ports", "arguments": { "action": "free" } }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": { "name": "devbase_ports", "arguments": { "action": "check" } }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "tools/call",
                    "params": { "name": "devbase_diagnostics", "arguments": {} }
                }),
                json!({
                    "jsonrpc": "2.0",
                    "id": 4,
                    "method": "tools/call",
                    "params": { "name": "devbase_services_list", "arguments": { "source": "brew" } }
                }),
            ],
        );

        assert_eq!(responses[0]["result"]["isError"], false);
        let free_port: Value = serde_json::from_str(
            responses[0]["result"]["content"][0]["text"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert!(free_port["port"].as_i64().unwrap() > 0);

        // Missing port for action=check returns tool error cleanly.
        assert_eq!(responses[1]["result"]["isError"], true);

        // Diagnostics returns redacted report.
        assert_eq!(responses[2]["result"]["isError"], false);
        let diag: Value = serde_json::from_str(
            responses[2]["result"]["content"][0]["text"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(diag["redacted"], true);

        // Services list with brew source returns structured JSON (with honest restriction warning).
        assert_eq!(responses[3]["result"]["isError"], false);
        let services: Value = serde_json::from_str(
            responses[3]["result"]["content"][0]["text"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert!(services["items"].is_array());
    }

    #[test]
    fn jsonrpc_errors_for_malformed_and_unknown_methods() {
        let dir = tempfile::tempdir().unwrap();
        let mut env = test_env(&dir);
        let raw = concat!(
            "{not valid json}\n",
            "{\"jsonrpc\":\"1.0\",\"id\":1,\"method\":\"ping\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"unknown/method\"}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"devbase_delete_everything\"}}\n",
        );
        let mut reader = Cursor::new(raw.as_bytes().to_vec());
        let mut out = Vec::new();
        serve(&mut env, &mut reader, &mut out).unwrap();
        let lines: Vec<Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();

        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0]["error"]["code"], ERR_PARSE_ERROR);
        assert_eq!(lines[1]["error"]["code"], ERR_INVALID_REQUEST);
        assert_eq!(lines[2]["error"]["code"], ERR_METHOD_NOT_FOUND);
        assert_eq!(lines[3]["error"]["code"], ERR_INVALID_PARAMS);
    }
}

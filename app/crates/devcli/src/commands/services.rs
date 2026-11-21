//! `services` (ported from `cmd_services.go`) over `devcore::launchd` on macOS
//! and `devcore::service::ServiceRegistry` (`systemd-user`, `systemd-system`,
//! `autostart`) on Linux.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::args::{FlagSpecs, ParseOutcome, has_help_flag, parse_flags, read_json_stdin};
use crate::env::CliEnv;
use crate::error::{CliError, CliResult};

use devcore::launchd::{
    Action, Autostart, Item, LaunchdProvider, ProcessRunner, Source, State, SystemRunner,
    shell_args,
};
#[cfg(target_os = "macos")]
use devcore::launchd::{
    Context, Launchd, Spec, SystemHost, UnavailableHelper, load_spec, save_file,
};
use devcore::service::{Detail, LogSource};
#[cfg(any(test, not(target_os = "macos")))]
use devcore::service::{Provider, ServiceRegistry};
use devcore::systemd::{DEFAULT_JOURNAL_LINES, read_journal_logs};
#[cfg(any(test, not(target_os = "macos")))]
use devcore::systemd::{SystemdError, SystemdProvider, UnitSpec, parse_unit};

#[derive(Debug, Clone, Serialize)]
pub struct ServiceItemView {
    pub id: String,
    pub source: String,
    pub name: String,
    pub state: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub pid: i32,
    pub autostart: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub owner: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub path: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
    #[serde(skip_serializing_if = "is_false")]
    pub admin: bool,
    #[serde(rename = "keepAlive", skip_serializing_if = "is_false")]
    pub keep_alive: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<String>,
    #[serde(rename = "primaryAction", skip_serializing_if = "String::is_empty")]
    pub primary_action: String,
    #[serde(rename = "toggleAction", skip_serializing_if = "String::is_empty")]
    pub toggle_action: String,
}

fn is_zero(value: &i32) -> bool {
    *value == 0
}
fn is_false(value: &bool) -> bool {
    !*value
}

fn state_text(state: State) -> &'static str {
    match state {
        State::None => "None",
        State::Stopped => "Stopped",
        State::Loaded => "Idle",
        State::Running => "Running",
        State::Scheduled => "Scheduled",
        State::Failed => "Error",
        State::Invalid => "Invalid",
    }
}

fn state_key(state: State) -> &'static str {
    match state {
        State::None => "none",
        State::Stopped => "stopped",
        State::Loaded => "idle",
        State::Running => "running",
        State::Scheduled => "scheduled",
        State::Failed => "error",
        State::Invalid => "invalid",
    }
}

fn autostart_text(autostart: Autostart) -> &'static str {
    match autostart {
        Autostart::On => "on",
        Autostart::Off => "off",
        Autostart::Unknown => "unknown",
    }
}

fn view_from_actions(item: &Item, actions: &[Action]) -> ServiceItemView {
    let names: Vec<String> = actions
        .iter()
        .map(|action| action.as_str().to_string())
        .collect();
    let primary = devcore::service::primary(actions)
        .map(|action| action.as_str().to_string())
        .unwrap_or_default();
    let toggle = devcore::service::toggle(actions)
        .map(|action| action.as_str().to_string())
        .unwrap_or_default();
    ServiceItemView {
        id: item.id.clone(),
        source: item.source.as_str().to_string(),
        name: item.name.clone(),
        state: state_text(item.state).to_string(),
        pid: item.pid,
        autostart: autostart_text(item.autostart).to_string(),
        owner: item.owner.clone(),
        path: item.path.to_string_lossy().into_owned(),
        kind: item.kind.clone().unwrap_or_default(),
        admin: item.admin,
        keep_alive: item.keep_alive,
        actions: names,
        primary_action: primary,
        toggle_action: toggle,
    }
}

fn view_for(provider: &LaunchdProvider, item: &Item) -> ServiceItemView {
    let actions = provider.actions(item);
    view_from_actions(item, &actions)
}

#[cfg(any(test, not(target_os = "macos")))]
fn view_for_provider(provider: &dyn Provider, item: &Item) -> ServiceItemView {
    let actions = provider.actions(item);
    view_from_actions(item, &actions)
}

#[cfg(target_os = "macos")]
fn context() -> Arc<Context> {
    Arc::new(Context::new(
        Arc::new(SystemRunner::new()),
        Arc::new(UnavailableHelper),
        Arc::new(SystemHost::new()),
    ))
}

#[cfg(target_os = "macos")]
fn launchd() -> Launchd {
    Launchd::new(context())
}

fn parse_source(raw: &str) -> CliResult<Source> {
    let normalized = raw.trim().to_lowercase();
    Source::parse(&normalized).ok_or_else(|| {
        #[cfg(target_os = "macos")]
        {
            CliError::usage(format!(
                "unknown service source {raw:?} (expected brew, login-item, user-agent, system-agent, or daemon)"
            ))
        }
        #[cfg(not(target_os = "macos"))]
        {
            CliError::usage(format!(
                "unknown service source {raw:?} (expected systemd-user, systemd-system, or autostart)"
            ))
        }
    })
}

fn matches_state_filter(item: &Item, want_state: &str) -> bool {
    if want_state.is_empty() {
        return true;
    }
    match want_state {
        "up" => item.up(),
        "active" => item.active(),
        _ => {
            state_key(item.state) == want_state
                || state_text(item.state).to_lowercase() == want_state
        }
    }
}

fn resolve_item_from<'a>(
    items: &'a [(Source, Item)],
    target: &str,
) -> CliResult<(Source, &'a Item)> {
    let target = target.trim();
    if target.is_empty() {
        return Err(CliError::usage("service identifier is required"));
    }
    let mut exact = Vec::new();
    let mut by_name = Vec::new();
    for (source, item) in items {
        if item.id == target || item.path.to_string_lossy() == target {
            exact.push((*source, item));
        } else if item.name.eq_ignore_ascii_case(target) {
            by_name.push((*source, item));
        }
    }
    let matches = if exact.is_empty() { by_name } else { exact };
    match matches.len() {
        0 => Err(CliError::operation(format!("service {target:?} not found"))),
        1 => Ok(matches[0]),
        _ => {
            let ids: Vec<String> = matches.iter().map(|(_, item)| item.id.clone()).collect();
            Err(CliError::operation(format!(
                "multiple services match {target:?}; pass exact ID or --source: {}",
                ids.join(", ")
            )))
        }
    }
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct DetailView {
    Rows: Vec<RowView>,
    Logs: Vec<LogView>,
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct RowView {
    Group: String,
    Key: String,
    Value: String,
}

#[derive(Serialize)]
#[allow(non_snake_case)]
struct LogView {
    Label: String,
    Path: String,
}

impl From<Detail> for DetailView {
    fn from(detail: Detail) -> Self {
        DetailView {
            Rows: detail
                .rows
                .into_iter()
                .map(|row| RowView {
                    Group: row.group,
                    Key: row.key,
                    Value: row.value,
                })
                .collect(),
            Logs: detail
                .logs
                .into_iter()
                .map(|log| LogView {
                    Label: log.label,
                    Path: log.path.to_string_lossy().into_owned(),
                })
                .collect(),
        }
    }
}

fn write_list_result(
    env: &CliEnv,
    out: &mut dyn Write,
    views: &[ServiceItemView],
    warnings: &[BTreeMap<String, String>],
) -> CliResult<()> {
    #[derive(Serialize)]
    struct Payload<'a> {
        items: &'a [ServiceItemView],
        #[serde(skip_serializing_if = "is_empty_slice")]
        warnings: &'a [BTreeMap<String, String>],
    }
    fn is_empty_slice<T>(value: &&[T]) -> bool {
        value.is_empty()
    }
    let payload = Payload {
        items: views,
        warnings,
    };
    crate::commands::write_result(env, out, &payload, |w| {
        for warning in warnings {
            if let Some(message) = warning.get("error") {
                writeln!(w, "Warning ({}): {message}", warning["source"])?;
            }
            if let Some(message) = warning.get("restriction") {
                writeln!(w, "Note ({}): {message}", warning["source"])?;
            }
        }
        if views.is_empty() {
            writeln!(w, "No matching services found.")?;
            return Ok(());
        }
        for view in views {
            let pid = if view.pid > 0 {
                format!("pid={}", view.pid)
            } else {
                String::new()
            };
            writeln!(
                w,
                "{}\t[{}]\t{}\t{}\t{}\tactions={}",
                view.id,
                view.source,
                view.name,
                view.state,
                pid,
                view.actions.join(",")
            )?;
        }
        Ok(())
    })
}

fn write_inspect_human(
    w: &mut dyn Write,
    view: &ServiceItemView,
    detail: &DetailView,
) -> std::io::Result<()> {
    writeln!(
        w,
        "Service: {} ({}, {}, source={})",
        view.name, view.id, view.state, view.source
    )?;
    if !view.path.is_empty() {
        writeln!(w, "Path: {}", view.path)?;
    }
    writeln!(w, "Actions: {}", view.actions.join(", "))?;
    for row in &detail.Rows {
        writeln!(w, "  [{}] {}: {}", row.Group, row.Key, row.Value)?;
    }
    for log in &detail.Logs {
        writeln!(w, "  Log ({}): {}", log.Label, log.Path)?;
    }
    Ok(())
}

fn parse_action(raw: &str) -> Action {
    match raw {
        "start" => Action::Start,
        "stop" => Action::Stop,
        "restart" => Action::Restart,
        "load" => Action::Load,
        "unload" => Action::Unload,
        "enable" => Action::Enable,
        "disable" => Action::Disable,
        _ => Action::Remove,
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct LogEntryView {
    label: String,
    source: String,
    content: String,
}

fn tail_log_file(path: &Path, lines: usize) -> CliResult<String> {
    use std::io::{Read, Seek, SeekFrom};

    const MAX_READ_BYTES: u64 = 2 << 20;
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(error) => return Err(CliError::operation(error.to_string())),
    };
    let len = file.metadata()?.len();
    if len > MAX_READ_BYTES {
        file.seek(SeekFrom::End(-(MAX_READ_BYTES as i64)))?;
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let raw = String::from_utf8_lossy(&bytes);
    let max_lines = if lines == 0 {
        DEFAULT_JOURNAL_LINES
    } else {
        lines
    };
    let all: Vec<&str> = raw.lines().collect();
    let start = all.len().saturating_sub(max_lines);
    let mut out = all[start..].join("\n");
    if !out.is_empty() && raw.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

fn read_service_logs(
    item: &Item,
    detail: &Detail,
    runner: &dyn ProcessRunner,
    lines: usize,
) -> CliResult<Vec<LogEntryView>> {
    let mut entries = Vec::new();
    for log in &detail.logs {
        let content = match log.source() {
            LogSource::Journal { unit, system } => read_journal_logs(runner, unit, system, lines)?,
            LogSource::File(path) => tail_log_file(path, lines)?,
        };
        entries.push(LogEntryView {
            label: log.label.clone(),
            source: log.path.to_string_lossy().into_owned(),
            content,
        });
    }
    if entries.is_empty()
        && let Some((unit, system)) = item.journal_unit()
    {
        let content = read_journal_logs(runner, unit, system, lines)?;
        let prefix = if system {
            "journal://system/"
        } else {
            "journal://user/"
        };
        entries.push(LogEntryView {
            label: "Journal".to_string(),
            source: format!("{prefix}{unit}"),
            content,
        });
    }
    Ok(entries)
}

fn write_logs_result(
    env: &CliEnv,
    out: &mut dyn Write,
    item: &Item,
    lines: usize,
    entries: &[LogEntryView],
) -> CliResult<()> {
    #[derive(Serialize)]
    struct Payload<'a> {
        id: &'a str,
        name: &'a str,
        source: &'a str,
        lines: usize,
        logs: &'a [LogEntryView],
    }
    let payload = Payload {
        id: &item.id,
        name: &item.name,
        source: item.source.as_str(),
        lines,
        logs: entries,
    };
    crate::commands::write_result(env, out, &payload, |w| {
        if entries.is_empty() {
            writeln!(w, "No logs available for {}.", item.name)?;
            return Ok(());
        }
        for entry in entries {
            if entries.len() > 1 {
                writeln!(w, "=== {} ({}) ===", entry.label, entry.source)?;
            }
            write!(w, "{}", entry.content)?;
            if !entry.content.is_empty() && !entry.content.ends_with('\n') {
                writeln!(w)?;
            }
        }
        Ok(())
    })
}

fn parse_logs_flags(args: &[String]) -> CliResult<Option<(String, String, usize)>> {
    let specs = FlagSpecs::new().value("source").value("lines");
    let ParseOutcome::Parsed(parsed) = parse_flags(args, &specs)? else {
        return Ok(None);
    };
    if parsed.positionals.len() != 1 {
        return Err(CliError::usage(
            "usage: devbase services logs <id-or-name> [--source <source>] [--lines <n>]",
        ));
    }
    let lines = if parsed.is_set("lines") {
        let value = parsed.integer("lines")?.unwrap_or(0);
        if value < 0 {
            return Err(CliError::usage("--lines must be a non-negative integer"));
        }
        if value == 0 {
            DEFAULT_JOURNAL_LINES
        } else {
            value as usize
        }
    } else {
        DEFAULT_JOURNAL_LINES
    };
    Ok(Some((
        parsed.positionals[0].clone(),
        parsed.string("source"),
        lines,
    )))
}

pub fn run_services_command(
    env: &mut CliEnv,
    out: &mut dyn Write,
    args: &[String],
) -> CliResult<()> {
    if has_help_flag(args) && args.len() == 1 {
        crate::schema::print_topic_help(out, "services")?;
        return Ok(());
    }
    let mut rest = args;
    let mut sub = "list";
    if let Some(first) = rest.first() {
        sub = first;
        rest = &rest[1..];
    }
    if has_help_flag(rest) {
        crate::schema::print_topic_help(out, "services")?;
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        match sub {
            "list" => run_list(env, out, rest),
            "inspect" => run_inspect(env, out, rest),
            "start" | "stop" | "restart" | "load" | "unload" | "enable" | "disable" | "remove" => {
                run_action(env, out, sub, rest)
            }
            "logs" => run_logs_macos(env, out, rest),
            "create" => Err(CliError::operation(
                "creating systemd units is only supported on Linux; use `devbase services job-save` on macOS",
            )),
            "add-login-item" => run_add_login_item(env, out, rest),
            "job-save" => run_job_save(env, out, rest),
            other => Err(CliError::usage(format!(
                "unknown services subcommand {other:?}"
            ))),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        match sub {
            "list" => run_list_with_registry(env, out, rest, ServiceRegistry::platform_default()),
            "inspect" => {
                run_inspect_with_registry(env, out, rest, ServiceRegistry::platform_default())
            }
            "start" | "stop" | "restart" | "load" | "unload" | "enable" | "disable" | "remove" => {
                run_action_with_registry(env, out, sub, rest, ServiceRegistry::platform_default())
            }
            "logs" => run_logs_with_registry(
                env,
                out,
                rest,
                ServiceRegistry::platform_default(),
                &SystemRunner::new(),
            ),
            "create" => run_create_linux(env, out, rest),
            "add-login-item" => Err(CliError::operation(
                "macOS Login Items are only supported on macOS",
            )),
            "job-save" => Err(CliError::operation(
                "launchd job-save is only supported on macOS; use `devbase services create` on Linux",
            )),
            other => Err(CliError::usage(format!(
                "unknown services subcommand {other:?}"
            ))),
        }
    }
}

// =============================================================================
// macOS launchd implementation (preserved 100% intact)
// =============================================================================

#[cfg(target_os = "macos")]
struct Collected {
    items: Vec<(Source, Item)>,
    warnings: Vec<BTreeMap<String, String>>,
    launchd: Launchd,
}

#[cfg(target_os = "macos")]
fn collect(env: &CliEnv, source_filter: &str) -> CliResult<Collected> {
    let launchd = launchd();
    let sources: Vec<Source> = if source_filter.trim().is_empty() {
        Source::ALL.to_vec()
    } else {
        vec![parse_source(source_filter)?]
    };
    let mut items = Vec::new();
    let mut warnings = Vec::new();
    for source in sources {
        match source {
            Source::Brew | Source::LoginItem => {
                let mut warning = BTreeMap::new();
                warning.insert("source".into(), source.as_str().to_string());
                warning.insert(
                    "restriction".into(),
                    format!(
                        "The {} provider is not part of this build yet; only launchd jobs are listed.",
                        source.title()
                    ),
                );
                warnings.push(warning);
            }
            Source::SystemdUser | Source::SystemdSystem | Source::Autostart => {
                let mut warning = BTreeMap::new();
                warning.insert("source".into(), source.as_str().to_string());
                warning.insert(
                    "restriction".into(),
                    format!(
                        "The {} provider is only available on Linux.",
                        source.title()
                    ),
                );
                warnings.push(warning);
            }
            _ => {
                let provider = launchd.provider(source).expect("launchd provider");
                match provider.list() {
                    Ok(found) => {
                        items.extend(found.into_iter().map(|item| (source, item)));
                    }
                    Err(error) => {
                        let mut warning = BTreeMap::new();
                        warning.insert("source".into(), source.as_str().to_string());
                        warning.insert("error".into(), error.to_string());
                        warnings.push(warning);
                    }
                }
            }
        }
    }
    let _ = env;
    Ok(Collected {
        items,
        warnings,
        launchd,
    })
}

#[cfg(target_os = "macos")]
fn run_list(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    let specs = FlagSpecs::new()
        .value("source")
        .value("state")
        .value("query");
    let ParseOutcome::Parsed(parsed) = parse_flags(args, &specs)? else {
        return Ok(());
    };
    if let Some(extra) = parsed.positionals.first() {
        return Err(CliError::usage(format!(
            "unexpected argument {extra:?} for services list"
        )));
    }
    let collected = collect(env, &parsed.string("source"))?;
    let query = parsed.string("query");
    let want_state = parsed.string("state").trim().to_lowercase();
    let mut views = Vec::new();
    for (source, item) in &collected.items {
        if !query.is_empty() && !item.matches(&query) {
            continue;
        }
        if !matches_state_filter(item, &want_state) {
            continue;
        }
        let provider = collected.launchd.provider(*source).expect("provider");
        views.push(view_for(provider, item));
    }
    write_list_result(env, out, &views, &collected.warnings)
}

/// The capitalized `launchd.Spec` view; field order follows `edit.go:22-56`.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Serialize)]
#[allow(non_snake_case)]
struct SpecView {
    Label: String,
    Existing: bool,
    Args: Vec<String>,
    WorkingDir: String,
    RunAtLoad: bool,
    KeepAlive: bool,
    KeepAliveCustom: bool,
    RestartOnCrash: bool,
    StartInterval: i32,
    ThrottleInterval: Option<i32>,
    Calendar: CalendarView,
    CalendarCustom: bool,
    StartOnMount: bool,
    QueueDirs: Vec<String>,
    UserName: String,
    GroupName: String,
    RootDir: String,
    Env: BTreeMap<String, String>,
    StdOut: String,
    StdErr: String,
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Serialize)]
#[allow(non_snake_case)]
struct CalendarView {
    Minute: String,
    Hour: String,
    Day: String,
    Month: String,
    Weekday: String,
}

#[cfg(target_os = "macos")]
impl From<&Spec> for SpecView {
    fn from(spec: &Spec) -> Self {
        SpecView {
            Label: spec.label.clone(),
            Existing: spec.existing,
            Args: spec.args.clone(),
            WorkingDir: spec.working_dir.clone(),
            RunAtLoad: spec.run_at_load,
            KeepAlive: spec.keep_alive,
            KeepAliveCustom: spec.keep_alive_custom,
            RestartOnCrash: spec.restart_on_crash,
            StartInterval: spec.start_interval,
            ThrottleInterval: spec.throttle_interval,
            Calendar: CalendarView {
                Minute: spec.calendar.minute.clone(),
                Hour: spec.calendar.hour.clone(),
                Day: spec.calendar.day.clone(),
                Month: spec.calendar.month.clone(),
                Weekday: spec.calendar.weekday.clone(),
            },
            CalendarCustom: spec.calendar_custom,
            StartOnMount: spec.start_on_mount,
            QueueDirs: spec.queue_dirs.clone(),
            UserName: spec.user_name.clone(),
            GroupName: spec.group_name.clone(),
            RootDir: spec.root_dir.clone(),
            Env: spec.env.clone(),
            StdOut: spec.stdout.clone(),
            StdErr: spec.stderr.clone(),
        }
    }
}

#[cfg(target_os = "macos")]
fn run_inspect(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    let specs = FlagSpecs::new().value("source");
    let ParseOutcome::Parsed(parsed) = parse_flags(args, &specs)? else {
        return Ok(());
    };
    if parsed.positionals.len() != 1 {
        return Err(CliError::usage(
            "usage: devbase services inspect <id-or-name> [--source <source>]",
        ));
    }
    let collected = collect(env, &parsed.string("source"))?;
    let (source, item) = resolve_item_from(&collected.items, &parsed.positionals[0])?;
    let provider = collected.launchd.provider(source).expect("provider");
    let detail = DetailView::from(provider.detail(item)?);
    let view = view_for(provider, item);
    let spec = if matches!(
        source,
        Source::UserAgent | Source::SystemAgent | Source::Daemon
    ) && !item.path.as_os_str().is_empty()
    {
        load_spec(&item.path).ok()
    } else {
        None
    };
    #[derive(Serialize)]
    struct Payload<'a> {
        item: &'a ServiceItemView,
        detail: &'a DetailView,
        #[serde(skip_serializing_if = "Option::is_none")]
        spec: &'a Option<SpecView>,
    }
    let spec_view = spec.as_ref().map(SpecView::from);
    let payload = Payload {
        item: &view,
        detail: &detail,
        spec: &spec_view,
    };
    crate::commands::write_result(env, out, &payload, |w| {
        write_inspect_human(w, &view, &detail)
    })
}

#[cfg(target_os = "macos")]
fn run_action(
    env: &mut CliEnv,
    out: &mut dyn Write,
    action_name: &str,
    args: &[String],
) -> CliResult<()> {
    let action = parse_action(action_name);
    let specs = FlagSpecs::new().value("source").bool("yes");
    let ParseOutcome::Parsed(parsed) = parse_flags(args, &specs)? else {
        return Ok(());
    };
    if parsed.positionals.len() != 1 {
        return Err(CliError::usage(format!(
            "usage: devbase services {action_name} <id-or-name> [--source <source>] [--yes]"
        )));
    }
    let collected = collect(env, &parsed.string("source"))?;
    let (source, item) = resolve_item_from(&collected.items, &parsed.positionals[0])?;
    let provider = collected.launchd.provider(source).expect("provider");
    let allowed = provider.actions(item);
    if !allowed.contains(&action) {
        let names: Vec<&str> = allowed.iter().map(|value| value.as_str()).collect();
        return Err(CliError::operation(format!(
            "action {:?} is not available for {} (available: {})",
            action_name,
            item.id,
            names.join(", ")
        )));
    }
    let confirm = provider.confirm(item, action);
    if (action == Action::Remove || confirm.is_some()) && !parsed.flag("yes") {
        if let Some(message) = confirm {
            return Err(CliError::usage(format!(
                "{action_name} requires --yes ({message})"
            )));
        }
        return Err(CliError::usage(format!("{action_name} requires --yes")));
    }
    provider.do_action(item, action)?;
    // Keep the Go map's sorted keys by serializing an ordered BTreeMap.
    let mut ordered = BTreeMap::new();
    ordered.insert("id", item.id.clone());
    ordered.insert("source", source.as_str().to_string());
    ordered.insert("name", item.name.clone());
    ordered.insert("action", action.as_str().to_string());
    ordered.insert("status", action.past().to_string());
    crate::commands::write_result(env, out, &ordered, |w| {
        writeln!(w, "{} {} ({}).", item.name, action.past(), item.id)
    })
}

#[cfg(target_os = "macos")]
fn run_logs_macos(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    let Some((target, source_filter, lines)) = parse_logs_flags(args)? else {
        return Ok(());
    };
    let collected = collect(env, &source_filter)?;
    let (source, item) = resolve_item_from(&collected.items, &target)?;
    let provider = collected.launchd.provider(source).expect("provider");
    let detail = provider.detail(item)?;
    let entries = read_service_logs(item, &detail, &SystemRunner::new(), lines)?;
    write_logs_result(env, out, item, lines, &entries)
}

#[cfg(target_os = "macos")]
fn run_add_login_item(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    let _ = (env, out);
    if args.len() != 1 || args[0].trim().is_empty() {
        return Err(CliError::usage(
            "usage: devbase services add-login-item <app-path>",
        ));
    }
    Err(CliError::operation(
        "login item adder is unavailable in this build",
    ))
}

/// The Go `launchd.Spec` JSON (capitalized fields) accepted on stdin.
#[cfg(target_os = "macos")]
#[derive(Debug, Default, Deserialize)]
#[allow(non_snake_case)]
struct JobSpecInput {
    #[serde(default)]
    Label: String,
    #[serde(default)]
    Existing: bool,
    #[serde(default)]
    Args: Vec<String>,
    #[serde(default)]
    WorkingDir: String,
    #[serde(default)]
    RunAtLoad: bool,
    #[serde(default)]
    KeepAlive: bool,
    #[serde(default)]
    KeepAliveCustom: bool,
    #[serde(default)]
    RestartOnCrash: bool,
    #[serde(default)]
    StartInterval: i32,
    #[serde(default)]
    ThrottleInterval: Option<i32>,
    #[serde(default)]
    Calendar: Option<CalendarInput>,
    #[serde(default)]
    CalendarCustom: bool,
    #[serde(default)]
    StartOnMount: bool,
    #[serde(default)]
    QueueDirs: Vec<String>,
    #[serde(default)]
    UserName: String,
    #[serde(default)]
    GroupName: String,
    #[serde(default)]
    RootDir: String,
    #[serde(default)]
    Env: BTreeMap<String, String>,
    #[serde(default)]
    StdOut: String,
    #[serde(default)]
    StdErr: String,
}

#[cfg(target_os = "macos")]
#[derive(Debug, Default, Deserialize)]
#[allow(non_snake_case)]
struct CalendarInput {
    #[serde(default)]
    Minute: String,
    #[serde(default)]
    Hour: String,
    #[serde(default)]
    Day: String,
    #[serde(default)]
    Month: String,
    #[serde(default)]
    Weekday: String,
}

#[cfg(target_os = "macos")]
impl JobSpecInput {
    fn into_spec(self) -> Spec {
        Spec {
            label: self.Label,
            existing: self.Existing,
            args: self.Args,
            working_dir: self.WorkingDir,
            run_at_load: self.RunAtLoad,
            keep_alive: self.KeepAlive,
            keep_alive_custom: self.KeepAliveCustom,
            restart_on_crash: self.RestartOnCrash,
            start_interval: self.StartInterval,
            throttle_interval: self.ThrottleInterval,
            calendar: self.Calendar.map(Into::into).unwrap_or_default(),
            calendar_custom: self.CalendarCustom,
            start_on_mount: self.StartOnMount,
            queue_dirs: self.QueueDirs,
            user_name: self.UserName,
            group_name: self.GroupName,
            root_dir: self.RootDir,
            env: self.Env,
            stdout: self.StdOut,
            stderr: self.StdErr,
        }
    }
}

#[cfg(target_os = "macos")]
impl From<CalendarInput> for devcore::launchd::Calendar {
    fn from(value: CalendarInput) -> Self {
        devcore::launchd::Calendar {
            minute: value.Minute,
            hour: value.Hour,
            day: value.Day,
            month: value.Month,
            weekday: value.Weekday,
        }
    }
}

#[cfg(target_os = "macos")]
fn run_job_save(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    let specs = FlagSpecs::new()
        .value("source")
        .value("existing")
        .bool("load")
        .bool("json-stdin")
        .value("label")
        .value("command")
        .repeatable("arg")
        .value("working-dir")
        .bool("run-at-load")
        .bool("keep-alive")
        .bool("restart-on-crash")
        .value("start-interval")
        .bool("start-on-mount")
        .repeatable("env")
        .value("stdout")
        .value("stderr")
        .value("user")
        .value("group")
        .value("root-dir");
    let ParseOutcome::Parsed(parsed) = parse_flags(args, &specs)? else {
        return Ok(());
    };
    if let Some(extra) = parsed.positionals.first() {
        return Err(CliError::usage(format!(
            "unexpected argument {extra:?} for services job-save"
        )));
    }
    let source_raw = if parsed.is_set("source") {
        parsed.string("source")
    } else {
        "user-agent".to_string()
    };
    let source = parse_source(&source_raw)?;
    if !matches!(
        source,
        Source::UserAgent | Source::SystemAgent | Source::Daemon
    ) {
        return Err(CliError::usage(
            "--source for job-save must be user-agent, system-agent, or daemon",
        ));
    }
    let existing = if parsed.is_set("existing") {
        parsed.string("existing")
    } else {
        String::new()
    };
    let mut spec = if parsed.flag("json-stdin") {
        let raw = read_json_stdin(&mut *env.input, 512 << 10)?;
        let input: JobSpecInput = serde_json::from_slice(&raw)
            .map_err(|error| CliError::usage(format!("invalid launchd spec JSON: {error}")))?;
        input.into_spec()
    } else {
        let mut spec = if !existing.is_empty() {
            load_spec(Path::new(&existing)).unwrap_or_default()
        } else {
            Spec::default()
        };
        let command = parsed.string("command");
        let arg_list = parsed.all("arg");
        if !command.is_empty() && !arg_list.is_empty() {
            return Err(CliError::usage(
                "--command and --arg cannot be used together",
            ));
        }
        if parsed.is_set("label") {
            spec.label = parsed.string("label").trim().to_string();
        }
        if !command.is_empty() {
            spec.args = shell_args(&command);
        } else if !arg_list.is_empty() {
            spec.args = arg_list.to_vec();
        }
        if parsed.is_set("working-dir") {
            spec.working_dir = parsed.string("working-dir");
        }
        if parsed.is_set("run-at-load") {
            spec.run_at_load = parsed.boolean("run-at-load")?;
        }
        if parsed.is_set("keep-alive") {
            spec.keep_alive = parsed.boolean("keep-alive")?;
            spec.keep_alive_custom = false;
        }
        if parsed.is_set("restart-on-crash") {
            spec.restart_on_crash = parsed.boolean("restart-on-crash")?;
        }
        if parsed.is_set("start-interval") {
            spec.start_interval = parsed.integer("start-interval")?.unwrap_or_default() as i32;
        }
        if parsed.is_set("start-on-mount") {
            spec.start_on_mount = parsed.boolean("start-on-mount")?;
        }
        if parsed.is_set("stdout") {
            spec.stdout = parsed.string("stdout");
        }
        if parsed.is_set("stderr") {
            spec.stderr = parsed.string("stderr");
        }
        if parsed.is_set("user") {
            spec.user_name = parsed.string("user");
        }
        if parsed.is_set("group") {
            spec.group_name = parsed.string("group");
        }
        if parsed.is_set("root-dir") {
            spec.root_dir = parsed.string("root-dir");
        }
        let env_pairs = parsed.all("env");
        if !env_pairs.is_empty() {
            let joined = env_pairs.join("\n");
            spec.env = devcore::launchd::parse_env(&joined)
                .map_err(|error| CliError::usage(format!("invalid --env: {error}")))?;
        }
        spec
    };
    spec.existing = !existing.is_empty();
    spec.validate()?;

    let load = parsed.flag("load");
    let launchd = launchd();
    let provider = launchd.provider(source).expect("source checked");
    let saved_path = if source == Source::UserAgent {
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_default();
        let dir = match std::env::var("MSM_DEV_HOME") {
            Ok(isolated) if !isolated.is_empty() => {
                std::path::PathBuf::from(isolated).join("user-agents")
            }
            _ => home.join("Library/LaunchAgents"),
        };
        let ctx = context();
        let path = save_file(
            &dir,
            &spec,
            (!existing.is_empty()).then(|| Path::new(&existing)),
            &*ctx.runner,
        )?;
        if load {
            let item = Item {
                id: String::new(),
                source,
                name: spec.label.clone(),
                state: State::Stopped,
                pid: 0,
                autostart: Autostart::On,
                owner: String::new(),
                path: path.clone(),
                kind: None,
                admin: false,
                keep_alive: false,
            };
            if !existing.is_empty() {
                let _ = provider.do_action(&item, Action::Unload);
            }
            provider.do_action(&item, Action::Load)?;
        }
        path
    } else {
        provider
            .save_as_root(
                &spec,
                (!existing.is_empty()).then(|| Path::new(&existing)),
                load,
            )?
            .path
    };
    let payload = serde_json::json!({
        "source": source.as_str(),
        "label": spec.label,
        "path": saved_path.to_string_lossy(),
        "loaded": load,
    });
    crate::commands::write_result(env, out, &payload, |w| {
        writeln!(
            w,
            "Saved {} job {} at {} (loaded={}).",
            payload["source"].as_str().unwrap_or_default(),
            payload["label"].as_str().unwrap_or_default(),
            payload["path"].as_str().unwrap_or_default(),
            load
        )
    })
}

// =============================================================================
// Linux ServiceRegistry + systemd + autostart implementation
// (compiled under `#[cfg(any(test, not(target_os = "macos")))]` so unit tests
// verify all Linux CLI paths on both macOS and Linux)
// =============================================================================

#[cfg(any(test, not(target_os = "macos")))]
struct RegistryCollected {
    items: Vec<(Source, Item)>,
    warnings: Vec<BTreeMap<String, String>>,
    registry: ServiceRegistry,
}

#[cfg(any(test, not(target_os = "macos")))]
fn collect_with_registry(
    registry: ServiceRegistry,
    source_filter: &str,
) -> CliResult<RegistryCollected> {
    let sources: Vec<Source> = if source_filter.trim().is_empty() {
        registry.sources()
    } else {
        vec![parse_source(source_filter)?]
    };
    let mut items = Vec::new();
    let mut warnings = Vec::new();
    for source in sources {
        match registry.provider(source) {
            Some(provider) => match provider.list() {
                Ok(found) => {
                    items.extend(found.into_iter().map(|item| (source, item)));
                }
                Err(error) => {
                    let mut warning = BTreeMap::new();
                    warning.insert("source".into(), source.as_str().to_string());
                    warning.insert("error".into(), error.to_string());
                    warnings.push(warning);
                }
            },
            None => {
                let mut warning = BTreeMap::new();
                warning.insert("source".into(), source.as_str().to_string());
                let message = match source {
                    Source::Brew
                    | Source::LoginItem
                    | Source::UserAgent
                    | Source::SystemAgent
                    | Source::Daemon => {
                        format!(
                            "The {} provider is only available on macOS.",
                            source.title()
                        )
                    }
                    _ => format!("The {} provider is not registered.", source.title()),
                };
                warning.insert("restriction".into(), message);
                warnings.push(warning);
            }
        }
    }
    Ok(RegistryCollected {
        items,
        warnings,
        registry,
    })
}

#[cfg(any(test, not(target_os = "macos")))]
fn run_list_with_registry(
    env: &mut CliEnv,
    out: &mut dyn Write,
    args: &[String],
    registry: ServiceRegistry,
) -> CliResult<()> {
    let specs = FlagSpecs::new()
        .value("source")
        .value("state")
        .value("query");
    let ParseOutcome::Parsed(parsed) = parse_flags(args, &specs)? else {
        return Ok(());
    };
    if let Some(extra) = parsed.positionals.first() {
        return Err(CliError::usage(format!(
            "unexpected argument {extra:?} for services list"
        )));
    }
    let collected = collect_with_registry(registry, &parsed.string("source"))?;
    let query = parsed.string("query");
    let want_state = parsed.string("state").trim().to_lowercase();
    let mut views = Vec::new();
    for (source, item) in &collected.items {
        if !query.is_empty() && !item.matches(&query) {
            continue;
        }
        if !matches_state_filter(item, &want_state) {
            continue;
        }
        let provider = collected.registry.provider(*source).expect("provider");
        views.push(view_for_provider(provider.as_ref(), item));
    }
    write_list_result(env, out, &views, &collected.warnings)
}

#[cfg(any(test, not(target_os = "macos")))]
fn run_inspect_with_registry(
    env: &mut CliEnv,
    out: &mut dyn Write,
    args: &[String],
    registry: ServiceRegistry,
) -> CliResult<()> {
    let specs = FlagSpecs::new().value("source");
    let ParseOutcome::Parsed(parsed) = parse_flags(args, &specs)? else {
        return Ok(());
    };
    if parsed.positionals.len() != 1 {
        return Err(CliError::usage(
            "usage: devbase services inspect <id-or-name> [--source <source>]",
        ));
    }
    let collected = collect_with_registry(registry, &parsed.string("source"))?;
    let (source, item) = resolve_item_from(&collected.items, &parsed.positionals[0])?;
    let provider = collected.registry.provider(source).expect("provider");
    let raw_detail = provider
        .detail(item)
        .map_err(|error| CliError::operation(error.to_string()))?;
    let detail = DetailView::from(raw_detail);
    let view = view_for_provider(provider.as_ref(), item);
    let unit_spec = if matches!(source, Source::SystemdUser | Source::SystemdSystem)
        && !item.path.as_os_str().is_empty()
    {
        std::fs::read_to_string(&item.path)
            .ok()
            .and_then(|raw| parse_unit(&item.name, &raw).ok())
    } else {
        None
    };
    #[derive(Serialize)]
    struct Payload<'a> {
        item: &'a ServiceItemView,
        detail: &'a DetailView,
        #[serde(rename = "unitSpec", skip_serializing_if = "Option::is_none")]
        unit_spec: &'a Option<UnitSpec>,
    }
    let payload = Payload {
        item: &view,
        detail: &detail,
        unit_spec: &unit_spec,
    };
    crate::commands::write_result(env, out, &payload, |w| {
        write_inspect_human(w, &view, &detail)
    })
}

#[cfg(any(test, not(target_os = "macos")))]
fn run_action_with_registry(
    env: &mut CliEnv,
    out: &mut dyn Write,
    action_name: &str,
    args: &[String],
    registry: ServiceRegistry,
) -> CliResult<()> {
    let action = parse_action(action_name);
    let specs = FlagSpecs::new().value("source").bool("yes");
    let ParseOutcome::Parsed(parsed) = parse_flags(args, &specs)? else {
        return Ok(());
    };
    if parsed.positionals.len() != 1 {
        return Err(CliError::usage(format!(
            "usage: devbase services {action_name} <id-or-name> [--source <source>] [--yes]"
        )));
    }
    let collected = collect_with_registry(registry, &parsed.string("source"))?;
    let (source, item) = resolve_item_from(&collected.items, &parsed.positionals[0])?;
    let provider = collected.registry.provider(source).expect("provider");
    let allowed = provider.actions(item);
    if !allowed.contains(&action) {
        let names: Vec<&str> = allowed.iter().map(|value| value.as_str()).collect();
        return Err(CliError::operation(format!(
            "action {:?} is not available for {} (available: {})",
            action_name,
            item.id,
            names.join(", ")
        )));
    }
    let confirm = provider.confirm(item, action);
    if (action == Action::Remove || confirm.is_some()) && !parsed.flag("yes") {
        if let Some(message) = confirm {
            return Err(CliError::usage(format!(
                "{action_name} requires --yes ({message})"
            )));
        }
        return Err(CliError::usage(format!("{action_name} requires --yes")));
    }
    provider
        .do_action(item, action)
        .map_err(|error| CliError::operation(error.to_string()))?;
    let mut ordered = BTreeMap::new();
    ordered.insert("id", item.id.clone());
    ordered.insert("source", source.as_str().to_string());
    ordered.insert("name", item.name.clone());
    ordered.insert("action", action.as_str().to_string());
    ordered.insert("status", action.past().to_string());
    crate::commands::write_result(env, out, &ordered, |w| {
        writeln!(w, "{} {} ({}).", item.name, action.past(), item.id)
    })
}

#[cfg(any(test, not(target_os = "macos")))]
fn run_logs_with_registry(
    env: &mut CliEnv,
    out: &mut dyn Write,
    args: &[String],
    registry: ServiceRegistry,
    runner: &dyn ProcessRunner,
) -> CliResult<()> {
    let Some((target, source_filter, lines)) = parse_logs_flags(args)? else {
        return Ok(());
    };
    let collected = collect_with_registry(registry, &source_filter)?;
    let (source, item) = resolve_item_from(&collected.items, &target)?;
    let provider = collected.registry.provider(source).expect("provider");
    let detail = provider
        .detail(item)
        .map_err(|error| CliError::operation(error.to_string()))?;
    let entries = read_service_logs(item, &detail, runner, lines)?;
    write_logs_result(env, out, item, lines, &entries)
}

#[cfg(not(target_os = "macos"))]
fn run_create_linux(env: &mut CliEnv, out: &mut dyn Write, args: &[String]) -> CliResult<()> {
    let runner: Arc<dyn ProcessRunner> = Arc::new(SystemRunner::new());
    let host: Arc<dyn devcore::launchd::Host> = Arc::new(devcore::launchd::SystemHost::new());
    let user_provider = match std::env::var("MSM_DEV_HOME") {
        Ok(isolated) if !isolated.is_empty() => {
            let dir = std::path::PathBuf::from(isolated).join("systemd-user");
            SystemdProvider::new(
                devcore::systemd::SystemdConfig {
                    source: Source::SystemdUser,
                    scan_dirs: vec![dir.clone()],
                    write_dir: dir,
                    system: false,
                    elevated: false,
                },
                runner.clone(),
                host.clone(),
                Arc::new(devcore::systemd::UnavailableSystemdHelper),
            )
        }
        _ => SystemdProvider::user(runner.clone(), host.clone()),
    };
    let system_provider = SystemdProvider::system(
        runner,
        host,
        Arc::new(devcore::systemd::DefaultSystemdHelper),
    );
    run_create_with(env, out, args, &user_provider, &system_provider)
}

#[cfg(any(test, not(target_os = "macos")))]
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct UnitSpecInput {
    name: String,
    description: String,
    exec_start: Vec<String>,
    working_directory: String,
    environment: BTreeMap<String, String>,
    restart: String,
    restart_sec: Option<u32>,
    wanted_by: Vec<String>,
    user: String,
    group: String,
    stdout_path: String,
    stderr_path: String,
    on_calendar: String,
    on_unit_active_sec: Option<u64>,
}

#[cfg(any(test, not(target_os = "macos")))]
impl From<UnitSpecInput> for UnitSpec {
    fn from(input: UnitSpecInput) -> Self {
        UnitSpec {
            name: input.name,
            description: input.description,
            exec_start: input.exec_start,
            working_directory: input.working_directory,
            environment: input.environment,
            restart: input.restart,
            restart_sec: input.restart_sec,
            wanted_by: input.wanted_by,
            user: input.user,
            group: input.group,
            stdout_path: input.stdout_path,
            stderr_path: input.stderr_path,
            on_calendar: input.on_calendar,
            on_unit_active_sec: input.on_unit_active_sec,
        }
    }
}

#[cfg(any(test, not(target_os = "macos")))]
fn run_create_with(
    env: &mut CliEnv,
    out: &mut dyn Write,
    args: &[String],
    user_provider: &SystemdProvider,
    system_provider: &SystemdProvider,
) -> CliResult<()> {
    let specs = FlagSpecs::new()
        .value("source")
        .bool("json-stdin")
        .bool("enable")
        .bool("load")
        .value("name")
        .value("description")
        .value("command")
        .repeatable("arg")
        .value("working-dir")
        .value("restart")
        .value("restart-sec")
        .repeatable("wanted-by")
        .repeatable("env")
        .value("stdout")
        .value("stderr")
        .value("user")
        .value("group")
        .value("on-calendar")
        .value("on-unit-active-sec");
    let ParseOutcome::Parsed(parsed) = parse_flags(args, &specs)? else {
        return Ok(());
    };
    if let Some(extra) = parsed.positionals.first() {
        return Err(CliError::usage(format!(
            "unexpected argument {extra:?} for services create"
        )));
    }
    let source_raw = if parsed.is_set("source") {
        parsed.string("source")
    } else {
        "systemd-user".to_string()
    };
    let source = parse_source(&source_raw)?;
    if !matches!(source, Source::SystemdUser | Source::SystemdSystem) {
        return Err(CliError::usage(
            "--source for services create must be systemd-user or systemd-system",
        ));
    }

    let mut spec = if parsed.flag("json-stdin") {
        let raw = read_json_stdin(&mut *env.input, 512 << 10)?;
        serde_json::from_slice::<UnitSpecInput>(&raw)
            .map(UnitSpec::from)
            .map_err(|error| CliError::usage(format!("invalid systemd UnitSpec JSON: {error}")))?
    } else {
        let mut spec = UnitSpec::default();
        let command = parsed.string("command");
        let arg_list = parsed.all("arg");
        if !command.is_empty() && !arg_list.is_empty() {
            return Err(CliError::usage(
                "--command and --arg cannot be used together",
            ));
        }
        if parsed.is_set("name") {
            spec.name = parsed.string("name").trim().to_string();
        }
        if parsed.is_set("description") {
            spec.description = parsed.string("description");
        }
        if !command.is_empty() {
            spec.exec_start = shell_args(&command);
        } else if !arg_list.is_empty() {
            spec.exec_start = arg_list.to_vec();
        }
        if parsed.is_set("working-dir") {
            spec.working_directory = parsed.string("working-dir");
        }
        if parsed.is_set("restart") {
            spec.restart = parsed.string("restart");
        }
        if parsed.is_set("restart-sec") {
            let secs = parsed.integer("restart-sec")?.unwrap_or_default();
            if secs < 0 {
                return Err(CliError::usage("--restart-sec must be non-negative"));
            }
            spec.restart_sec = Some(secs as u32);
        }
        let wanted = parsed.all("wanted-by");
        if !wanted.is_empty() {
            spec.wanted_by = wanted.to_vec();
        } else {
            spec.wanted_by = vec![if source == Source::SystemdSystem {
                "multi-user.target".to_string()
            } else {
                "default.target".to_string()
            }];
        }
        if parsed.is_set("stdout") {
            spec.stdout_path = parsed.string("stdout");
        }
        if parsed.is_set("stderr") {
            spec.stderr_path = parsed.string("stderr");
        }
        if parsed.is_set("user") {
            spec.user = parsed.string("user");
        }
        if parsed.is_set("group") {
            spec.group = parsed.string("group");
        }
        if parsed.is_set("on-calendar") {
            spec.on_calendar = parsed.string("on-calendar");
        }
        if parsed.is_set("on-unit-active-sec") {
            let secs = parsed.integer("on-unit-active-sec")?.unwrap_or_default();
            if secs < 0 {
                return Err(CliError::usage("--on-unit-active-sec must be non-negative"));
            }
            spec.on_unit_active_sec = Some(secs as u64);
        }
        let env_pairs = parsed.all("env");
        if !env_pairs.is_empty() {
            let joined = env_pairs.join("\n");
            spec.environment = devcore::launchd::parse_env(&joined)
                .map_err(|error| CliError::usage(format!("invalid --env: {error}")))?;
        }
        spec
    };

    spec.validate().map_err(|error| match error {
        SystemdError::Invalid(message) => CliError::usage(message),
        other => CliError::from(other),
    })?;
    let unit_name = spec.unit_filename()?;
    spec.name = unit_name.clone();

    let enable = parsed.flag("enable") || parsed.flag("load");
    let provider = match source {
        Source::SystemdUser => user_provider,
        Source::SystemdSystem => system_provider,
        _ => unreachable!("checked above"),
    };
    let saved_path = provider.save_unit(&spec, enable)?;
    let payload = serde_json::json!({
        "source": source.as_str(),
        "unit": unit_name,
        "path": saved_path.to_string_lossy(),
        "enabled": enable,
    });
    crate::commands::write_result(env, out, &payload, |w| {
        writeln!(
            w,
            "Created {} unit {} at {} (enabled={}).",
            payload["source"].as_str().unwrap_or_default(),
            payload["unit"].as_str().unwrap_or_default(),
            payload["path"].as_str().unwrap_or_default(),
            enable
        )
    })
}

// Unused but exported for the CLI view tests.
pub fn item_view(provider: &LaunchdProvider, item: &Item) -> ServiceItemView {
    view_for(provider, item)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Mutex;

    use devcore::autostart::{AutostartConfig, AutostartProvider};
    use devcore::launchd::{CommandError, Host, LaunchdError};
    use devcore::systemd::{SystemdConfig, UnavailableSystemdHelper};

    use super::*;

    struct StubHost(PathBuf);

    impl Host for StubHost {
        fn uid(&self) -> u32 {
            1000
        }
        fn home_dir(&self) -> Result<PathBuf, LaunchdError> {
            Ok(self.0.clone())
        }
        fn temp_dir(&self) -> PathBuf {
            self.0.clone()
        }
        fn current_exe(&self) -> Result<PathBuf, LaunchdError> {
            Ok(PathBuf::from("/usr/bin/devbase"))
        }
        fn eval_symlinks(&self, path: &Path) -> Option<PathBuf> {
            Some(path.to_path_buf())
        }
        fn trash(&self, path: &Path) -> Result<(), LaunchdError> {
            std::fs::remove_file(path).map_err(|e| LaunchdError::io(e.to_string()))
        }
    }

    #[derive(Default)]
    struct FakeRunner {
        calls: Mutex<Vec<(String, Vec<String>)>>,
        journal_output: String,
        unit_files_output: String,
        live_units_output: String,
        show_output: String,
    }

    impl ProcessRunner for FakeRunner {
        fn run(&self, program: &str, args: &[&str]) -> Result<String, CommandError> {
            self.calls.lock().unwrap().push((
                program.to_string(),
                args.iter().map(|a| (*a).to_string()).collect(),
            ));
            if program == "journalctl" {
                return Ok(self.journal_output.clone());
            }
            if program == "systemctl" {
                if args.contains(&"list-unit-files") {
                    return Ok(self.unit_files_output.clone());
                }
                if args.contains(&"list-units") {
                    return Ok(self.live_units_output.clone());
                }
                if args.contains(&"show") {
                    return Ok(self.show_output.clone());
                }
            }
            Ok(String::new())
        }
    }

    fn make_linux_registry(
        dir: &Path,
        runner: Arc<FakeRunner>,
    ) -> (ServiceRegistry, PathBuf, PathBuf) {
        let user_dir = dir.join("systemd-user");
        let autostart_dir = dir.join("autostart");
        std::fs::create_dir_all(&user_dir).unwrap();
        std::fs::create_dir_all(&autostart_dir).unwrap();

        let host: Arc<dyn Host> = Arc::new(StubHost(dir.to_path_buf()));
        let systemd_user = Arc::new(SystemdProvider::new(
            SystemdConfig {
                source: Source::SystemdUser,
                scan_dirs: vec![user_dir.clone()],
                write_dir: user_dir.clone(),
                system: false,
                elevated: false,
            },
            runner.clone(),
            host.clone(),
            Arc::new(UnavailableSystemdHelper),
        ));
        let autostart = Arc::new(AutostartProvider::new(
            AutostartConfig {
                user_dir: autostart_dir.clone(),
                system_dir: dir.join("etc-autostart"),
            },
            host,
        ));
        let mut registry = ServiceRegistry::new();
        registry.register(systemd_user);
        registry.register(autostart);
        (registry, user_dir, autostart_dir)
    }

    #[test]
    fn parse_source_accepts_linux_and_macos_slugs() {
        assert_eq!(parse_source("systemd-user").unwrap(), Source::SystemdUser);
        assert_eq!(
            parse_source("systemd-system").unwrap(),
            Source::SystemdSystem
        );
        assert_eq!(parse_source("autostart").unwrap(), Source::Autostart);
        assert_eq!(parse_source("user-agent").unwrap(), Source::UserAgent);
        assert_eq!(parse_source("brew").unwrap(), Source::Brew);
        assert!(parse_source("invalid-source").is_err());
    }

    #[test]
    fn linux_registry_supports_list_inspect_actions_and_logs() {
        let dir = tempfile::tempdir().unwrap();
        let log_file = dir.path().join("api.log");
        std::fs::write(&log_file, "line1\nline2\nline3\n").unwrap();

        let runner = Arc::new(FakeRunner {
            calls: Mutex::new(Vec::new()),
            journal_output: "Nov 22 01:00:00 host api[42]: listening\n".into(),
            unit_files_output: "my-api.service enabled enabled\n".into(),
            live_units_output: "my-api.service loaded active running My API\n".into(),
            show_output: format!(
                "Id=my-api.service\nLoadState=loaded\nActiveState=active\nSubState=running\nMainPID=42\nFragmentPath={}/systemd-user/my-api.service\n",
                dir.path().display()
            ),
        });
        let (registry, user_dir, autostart_dir) = make_linux_registry(dir.path(), runner.clone());

        std::fs::write(
            user_dir.join("my-api.service"),
            format!(
                "[Unit]\nDescription=My API\n\n[Service]\nExecStart=/usr/bin/api\nRestart=on-failure\nStandardOutput=append:{}\n\n[Install]\nWantedBy=default.target\n",
                log_file.display()
            ),
        )
        .unwrap();
        std::fs::write(
            autostart_dir.join("demo.desktop"),
            "[Desktop Entry]\nType=Application\nName=Demo App\nExec=/usr/bin/demo\nHidden=false\n",
        )
        .unwrap();

        // List all Linux sources
        let mut env = CliEnv::for_test();
        env.json = true;
        let mut out = Vec::new();
        run_list_with_registry(&mut env, &mut out, &[], registry).unwrap();
        let listed: serde_json::Value = serde_json::from_slice(&out).unwrap();
        let items = listed["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert!(
            items
                .iter()
                .any(|item| item["source"] == "systemd-user" && item["name"] == "my-api.service")
        );
        assert!(
            items
                .iter()
                .any(|item| item["source"] == "autostart" && item["name"] == "Demo App")
        );

        // Inspect systemd unit
        let (registry, _, _) = make_linux_registry(dir.path(), runner.clone());
        let mut out = Vec::new();
        run_inspect_with_registry(
            &mut env,
            &mut out,
            &[
                "my-api.service".into(),
                "--source".into(),
                "systemd-user".into(),
            ],
            registry,
        )
        .unwrap();
        let inspected: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(inspected["item"]["name"], "my-api.service");
        assert_eq!(inspected["unitSpec"]["description"], "My API");
        assert!(
            inspected["detail"]["Logs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|l| l["Path"] == "journal://user/my-api.service")
        );

        // Restart action dispatches `systemctl --user restart my-api.service`
        let (registry, _, _) = make_linux_registry(dir.path(), runner.clone());
        let mut out = Vec::new();
        run_action_with_registry(
            &mut env,
            &mut out,
            "restart",
            &[
                "my-api.service".into(),
                "--source".into(),
                "systemd-user".into(),
            ],
            registry,
        )
        .unwrap();
        let restarted: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(restarted["action"], "restart");
        assert_eq!(restarted["status"], "restarted");
        assert!(runner.calls.lock().unwrap().iter().any(|(prog, args)| {
            prog == "systemctl" && args == &["--user", "restart", "my-api.service"]
        }));

        // Logs reads both journalctl and file logs
        let (registry, _, _) = make_linux_registry(dir.path(), runner.clone());
        let mut out = Vec::new();
        run_logs_with_registry(
            &mut env,
            &mut out,
            &[
                "my-api.service".into(),
                "--source".into(),
                "systemd-user".into(),
                "--lines".into(),
                "2".into(),
            ],
            registry,
            runner.as_ref(),
        )
        .unwrap();
        let logs: serde_json::Value = serde_json::from_slice(&out).unwrap();
        let log_entries = logs["logs"].as_array().unwrap();
        assert!(
            log_entries
                .iter()
                .any(|entry| entry["source"] == "journal://user/my-api.service"
                    && entry["content"].as_str().unwrap().contains("listening"))
        );
        assert!(log_entries.iter().any(|entry| entry["source"]
            == log_file.to_string_lossy().as_ref()
            && entry["content"] == "line2\nline3\n"));
    }

    #[test]
    fn linux_create_writes_validated_systemd_unit_and_rejects_bad_names() {
        let dir = tempfile::tempdir().unwrap();
        let user_dir = dir.path().join("systemd-user");
        let runner = Arc::new(FakeRunner::default());
        let host: Arc<dyn Host> = Arc::new(StubHost(dir.path().to_path_buf()));
        let user_provider = SystemdProvider::new(
            SystemdConfig {
                source: Source::SystemdUser,
                scan_dirs: vec![user_dir.clone()],
                write_dir: user_dir.clone(),
                system: false,
                elevated: false,
            },
            runner.clone(),
            host.clone(),
            Arc::new(UnavailableSystemdHelper),
        );
        let system_provider =
            SystemdProvider::system(runner.clone(), host, Arc::new(UnavailableSystemdHelper));

        let mut env = CliEnv::for_test();
        env.json = true;
        let mut out = Vec::new();
        run_create_with(
            &mut env,
            &mut out,
            &[
                "--name".into(),
                "my-worker".into(),
                "--description".into(),
                "Background worker".into(),
                "--arg".into(),
                "/usr/bin/python3".into(),
                "--arg".into(),
                "worker.py".into(),
                "--working-dir".into(),
                "/srv/app".into(),
                "--restart".into(),
                "on-failure".into(),
                "--enable".into(),
            ],
            &user_provider,
            &system_provider,
        )
        .unwrap();

        let created: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(created["source"], "systemd-user");
        assert_eq!(created["unit"], "my-worker.service");
        assert_eq!(created["enabled"], true);
        let written = std::fs::read_to_string(user_dir.join("my-worker.service")).unwrap();
        assert!(written.contains("Description=Background worker"));
        assert!(written.contains("ExecStart=/usr/bin/python3 worker.py"));
        assert!(written.contains("WorkingDirectory=/srv/app"));
        assert!(written.contains("Restart=on-failure"));
        assert!(written.contains("WantedBy=default.target"));
        assert!(
            runner.calls.lock().unwrap().iter().any(|(prog, args)| {
                prog == "systemctl" && args == &["--user", "daemon-reload"]
            })
        );
        assert!(runner.calls.lock().unwrap().iter().any(|(prog, args)| {
            prog == "systemctl" && args == &["--user", "enable", "--now", "my-worker.service"]
        }));

        // JSON stdin creation parses UnitSpec directly
        let mut json_env = CliEnv::default_env(Box::new(std::io::Cursor::new(
            br#"{
                "name": "scheduled-job",
                "description": "Nightly cleanup",
                "execStart": ["/usr/bin/env", "sh", "-c", "echo ok"],
                "onCalendar": "daily"
            }"#
            .to_vec(),
        )));
        json_env.json = true;
        let mut out = Vec::new();
        run_create_with(
            &mut json_env,
            &mut out,
            &["--json-stdin".into()],
            &user_provider,
            &system_provider,
        )
        .unwrap();
        let created_json: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(created_json["unit"], "scheduled-job.service");
        assert!(user_dir.join("scheduled-job.service").exists());
        assert!(user_dir.join("scheduled-job.timer").exists());

        // Invalid unit name exits 2 (usage error)
        let mut out = Vec::new();
        let err = run_create_with(
            &mut env,
            &mut out,
            &[
                "--name".into(),
                "../escape".into(),
                "--arg".into(),
                "/bin/true".into(),
            ],
            &user_provider,
            &system_provider,
        )
        .unwrap_err();
        assert_eq!(err.exit_code(), 2);
    }
}

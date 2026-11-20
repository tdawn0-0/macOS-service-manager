//! `services` (ported from `cmd_services.go`) over `devcore::launchd`.
//!
//! The three launchd directories are the ported providers. Homebrew and Login
//! Items are a later slice, so selecting them reports an explicit restriction
//! instead of pretending the source is empty.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::args::{FlagSpecs, ParseOutcome, has_help_flag, parse_flags, read_json_stdin};
use crate::env::CliEnv;
use crate::error::{CliError, CliResult};

use devcore::launchd::{
    Action, Autostart, Context, Item, Launchd, LaunchdProvider, Source, Spec, State, SystemHost,
    SystemRunner, UnavailableHelper, load_spec, save_file, shell_args,
};

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

fn view_for(provider: &LaunchdProvider, item: &Item) -> ServiceItemView {
    let actions = provider.actions(item);
    let names: Vec<String> = actions
        .iter()
        .map(|action| action.as_str().to_string())
        .collect();
    let primary = devcore::service::primary(&actions)
        .map(|action| action.as_str().to_string())
        .unwrap_or_default();
    let toggle = devcore::service::toggle(&actions)
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

fn context() -> Arc<Context> {
    Arc::new(Context::new(
        Arc::new(SystemRunner::new()),
        Arc::new(UnavailableHelper),
        Arc::new(SystemHost::new()),
    ))
}

fn launchd() -> Launchd {
    Launchd::new(context())
}

fn parse_source(raw: &str) -> CliResult<Source> {
    match raw.trim().to_lowercase().as_str() {
        "brew" => Ok(Source::Brew),
        "login-item" => Ok(Source::LoginItem),
        "user-agent" => Ok(Source::UserAgent),
        "system-agent" => Ok(Source::SystemAgent),
        "daemon" => Ok(Source::Daemon),
        _ => Err(CliError::usage(format!(
            "unknown service source {raw:?} (expected brew, login-item, user-agent, system-agent, or daemon)"
        ))),
    }
}

struct Collected {
    items: Vec<(Source, Item)>,
    warnings: Vec<BTreeMap<String, String>>,
    launchd: Launchd,
}

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
    match sub {
        "list" => run_list(env, out, rest),
        "inspect" => run_inspect(env, out, rest),
        "start" | "stop" | "restart" | "load" | "unload" | "enable" | "disable" | "remove" => {
            run_action(env, out, sub, rest)
        }
        "add-login-item" => run_add_login_item(env, out, rest),
        "job-save" => run_job_save(env, out, rest),
        other => Err(CliError::usage(format!(
            "unknown services subcommand {other:?}"
        ))),
    }
}

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
        if !want_state.is_empty() {
            match want_state.as_str() {
                "up" => {
                    if !item.up() {
                        continue;
                    }
                }
                "active" => {
                    if !item.active() {
                        continue;
                    }
                }
                _ => {
                    if state_key(item.state) != want_state
                        && state_text(item.state).to_lowercase() != want_state
                    {
                        continue;
                    }
                }
            }
        }
        let provider = collected.launchd.provider(*source).expect("provider");
        views.push(view_for(provider, item));
    }
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
        items: &views,
        warnings: &collected.warnings,
    };
    crate::commands::write_result(env, out, &payload, |w| {
        for warning in &collected.warnings {
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
        for view in &views {
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

/// The capitalized `launchd.Spec` view; field order follows `edit.go:22-56`.
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

#[derive(Debug, Clone, Serialize)]
#[allow(non_snake_case)]
struct CalendarView {
    Minute: String,
    Hour: String,
    Day: String,
    Month: String,
    Weekday: String,
}

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

fn resolve_item<'a>(collected: &'a Collected, target: &str) -> CliResult<(Source, &'a Item)> {
    let target = target.trim();
    if target.is_empty() {
        return Err(CliError::usage("service identifier is required"));
    }
    let mut exact = Vec::new();
    let mut by_name = Vec::new();
    for (source, item) in &collected.items {
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
    let (source, item) = resolve_item(&collected, &parsed.positionals[0])?;
    let provider = collected.launchd.provider(source).expect("provider");
    let detail = provider.detail(item)?;
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
    let detail = DetailView {
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
    })
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
    let (source, item) = resolve_item(&collected, &parsed.positionals[0])?;
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

// Unused but exported for the CLI view tests.
pub fn item_view(provider: &LaunchdProvider, item: &Item) -> ServiceItemView {
    view_for(provider, item)
}

//! The static CLI schema, copied verbatim from `schema.go:9-360`.
//!
//! Deriving this from argument definitions would change both the bytes and
//! the meaning: `gui` and `open` are in the schema but have no `RunCLI` case,
//! and some usage strings mention flags that the parser does not declare.

use serde::Serialize;

use crate::error::{CliError, CliResult};

pub const CLI_VERSION: &str = match option_env!("DEVBASE_VERSION") {
    Some(version) if !version.is_empty() => version,
    _ => env!("CARGO_PKG_VERSION"),
};
pub const CLI_ARCH: &str = devcore::platform::HostTarget::current().cli_arch();

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FlagSpec {
    pub name: &'static str,
    #[serde(rename = "type")]
    pub kind: &'static str,
    #[serde(skip_serializing_if = "is_false")]
    pub required: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub repeatable: bool,
    pub description: &'static str,
}

const fn flag(name: &'static str, kind: &'static str, description: &'static str) -> FlagSpec {
    FlagSpec {
        name,
        kind,
        required: false,
        repeatable: false,
        description,
    }
}

const fn required_flag(
    name: &'static str,
    kind: &'static str,
    description: &'static str,
) -> FlagSpec {
    FlagSpec {
        name,
        kind,
        required: true,
        repeatable: false,
        description,
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SubcommandSpec {
    pub name: &'static str,
    pub summary: &'static str,
    pub usage: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<FlagSpec>,
    #[serde(skip_serializing_if = "str::is_empty")]
    pub stdin: &'static str,
    #[serde(skip_serializing_if = "is_false")]
    pub destructive: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub privileged: bool,
}

const fn sub(name: &'static str, summary: &'static str, usage: &'static str) -> SubcommandSpec {
    SubcommandSpec {
        name,
        summary,
        usage,
        flags: Vec::new(),
        stdin: "",
        destructive: false,
        privileged: false,
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CommandSpec {
    pub name: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<&'static str>,
    pub summary: &'static str,
    pub usage: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub subcommands: Vec<SubcommandSpec>,
}

const fn command(name: &'static str, summary: &'static str, usage: &'static str) -> CommandSpec {
    CommandSpec {
        name,
        aliases: Vec::new(),
        summary,
        usage,
        subcommands: Vec::new(),
    }
}

const fn command_with(
    name: &'static str,
    aliases: Vec<&'static str>,
    summary: &'static str,
    usage: &'static str,
    subcommands: Vec<SubcommandSpec>,
) -> CommandSpec {
    CommandSpec {
        name,
        aliases,
        summary,
        usage,
        subcommands,
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CliSchema {
    #[serde(rename = "schemaVersion")]
    pub schema_version: i64,
    pub name: &'static str,
    pub version: &'static str,
    #[serde(rename = "globalFlags")]
    pub global_flags: Vec<FlagSpec>,
    #[serde(rename = "exitCodes")]
    pub exit_codes: std::collections::BTreeMap<&'static str, &'static str>,
    #[serde(rename = "safetyInvariants")]
    pub safety_invariants: Vec<&'static str>,
    pub commands: Vec<CommandSpec>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

macro_rules! sub_flags {
    ($name:literal, $summary:literal, $usage:literal, [$($flag:expr),* $(,)?]) => {{
        let mut item = sub($name, $summary, $usage);
        item.flags = vec![$($flag),*];
        item
    }};
}

macro_rules! sub_full {
    ($name:literal, $summary:literal, $usage:literal, [$($flag:expr),* $(,)?], stdin = $stdin:literal) => {{
        let mut item = sub_flags!($name, $summary, $usage, [$($flag),*]);
        item.stdin = $stdin;
        item
    }};
}

macro_rules! destructive {
    ($name:literal, $summary:literal, $usage:literal, [$($flag:expr),* $(,)?]) => {{
        let mut item = sub_flags!($name, $summary, $usage, [$($flag),*]);
        item.destructive = true;
        item
    }};
}

macro_rules! privileged {
    ($name:literal, $summary:literal, $usage:literal) => {{
        let mut item = sub($name, $summary, $usage);
        item.privileged = true;
        item
    }};
}

pub fn build_cli_schema() -> CliSchema {
    let mut exit_codes = std::collections::BTreeMap::new();
    exit_codes.insert("0", "success");
    exit_codes.insert("1", "runtime_or_operation_error");
    exit_codes.insert("2", "usage_or_safety_guard_error");

    CliSchema {
        schema_version: 1,
        name: "devbase",
        version: CLI_VERSION,
        global_flags: vec![
            flag(
                "--json",
                "bool",
                "Emit structured JSON output on stdout and structured JSON errors on stderr",
            ),
            flag(
                "--help, -h",
                "bool",
                "Print help for top-level or topic commands",
            ),
        ],
        exit_codes,
        safety_invariants: vec![
            "Plaintext secrets are forbidden in argv (--password, --token, --api-token exit 2); always pipe via stdin using --password-stdin or --token-stdin.",
            "Destructive operations require explicit non-interactive confirmation flags (--yes, or --confirm <id> --fingerprint <fp> for app-managed permanent database deletion).",
            "External database directories (dataDirectory != \"\") can never be permanently deleted.",
            "Shared Homebrew kegs are never removed when uninstalling a Devbase package registration.",
            "Devbase never starts or supervises project web/app servers; developers run their own upstream apps.",
        ],
        commands: vec![
            command(
                "status",
                "Inspect workspace state, installed packages, database instances, proxy/tunnel status, helper, and CLI wrapper",
                "devbase status [--json]",
            ),
            command_with(
                "diagnostics",
                Vec::new(),
                "Run Devbase environment diagnostics or export a redacted report",
                "devbase diagnostics [run|export] [--json]",
                vec![
                    sub(
                        "run",
                        "Run full layered health checks across runtimes, databases, proxy, tunnel, and CA",
                        "devbase diagnostics [run]",
                    ),
                    sub(
                        "export",
                        "Write a redacted diagnostic report bundle to disk",
                        "devbase diagnostics export",
                    ),
                ],
            ),
            command_with(
                "tasks",
                Vec::new(),
                "Inspect and resolve recorded background operations, database work, initializations, and staged deletions",
                "devbase tasks <list|dismiss|retry|work|resolve-work|initializations|resolve-init|deletions|finish-deletion|restore-deletion>",
                vec![
                    sub("list", "List recorded operations", "devbase tasks list"),
                    sub(
                        "dismiss",
                        "Dismiss a recorded operation",
                        "devbase tasks dismiss <id> <revision>",
                    ),
                    sub(
                        "retry",
                        "Retry a failed eligible operation",
                        "devbase tasks retry <id> <revision>",
                    ),
                    sub(
                        "work",
                        "List unfinished database tasks",
                        "devbase tasks work",
                    ),
                    sub(
                        "resolve-work",
                        "Mark an unfinished database task resolved",
                        "devbase tasks resolve-work <id> <revision>",
                    ),
                    sub(
                        "initializations",
                        "List interrupted database initializations",
                        "devbase tasks initializations",
                    ),
                    sub(
                        "resolve-init",
                        "Archive and clear an unfinished initialization",
                        "devbase tasks resolve-init <id> <revision>",
                    ),
                    sub(
                        "deletions",
                        "List staged database deletions",
                        "devbase tasks deletions",
                    ),
                    destructive!(
                        "finish-deletion",
                        "Finish erasing a staged deletion",
                        "devbase tasks finish-deletion <id> <revision> --confirm <instance-id>",
                        [required_flag(
                            "--confirm",
                            "string",
                            "Exact original instance ID"
                        )]
                    ),
                    sub(
                        "restore-deletion",
                        "Restore a staged deletion",
                        "devbase tasks restore-deletion <id> <revision>",
                    ),
                ],
            ),
            command_with(
                "logs",
                Vec::new(),
                "Read bounded trailing log lines for proxy, tunnel, or database instances",
                "devbase logs <proxy|tunnel|db-<id>|<instance-id>> [--stream stderr|stdout] [--lines 200] [--filter <text>]",
                vec![sub_flags!(
                    "tail",
                    "Tail logs from proxy, tunnel, or a database job",
                    "devbase logs <target> [--stream stderr|stdout] [--lines N] [--filter <text>]",
                    [
                        flag("--stream", "string", "Log stream: stderr or stdout"),
                        flag(
                            "--lines",
                            "int",
                            "Maximum number of trailing lines (default 200, max 5000)"
                        ),
                        flag("--filter", "string", "Case-insensitive substring filter"),
                    ]
                )],
            ),
            command_with(
                "ports",
                Vec::new(),
                "Inspect active local listening user ports, find a free port, or check a port",
                "devbase ports <active|free|check <port>>",
                vec![
                    sub(
                        "active",
                        "List listening local user ports",
                        "devbase ports [active]",
                    ),
                    sub(
                        "free",
                        "Allocate/suggest an unused local TCP port",
                        "devbase ports free",
                    ),
                    privileged!(
                        "check",
                        "Inspect listeners on a specific port",
                        "devbase ports check <port>"
                    ),
                ],
            ),
            command_with(
                "shutdown",
                Vec::new(),
                "Preview or stop all managed database instances (keeping proxy and tunnel running)",
                "devbase shutdown <preview|stop --yes>",
                vec![
                    sub(
                        "preview",
                        "Preview managed database instances in stop scope",
                        "devbase shutdown [preview]",
                    ),
                    destructive!(
                        "stop",
                        "Stop all managed database instances",
                        "devbase shutdown stop --yes",
                        [required_flag(
                            "--yes",
                            "bool",
                            "Confirm stopping all managed database instances"
                        )]
                    ),
                ],
            ),
            command_with(
                "software",
                Vec::new(),
                "Manage runtimes and database engine packages (node, java, python, mysql, postgresql, redis, mongodb)",
                "devbase software <list|versions|review|install|uninstall|verify|repair|default|shell|which|uv-path>",
                vec![
                    sub(
                        "list",
                        "List installed packages, JDKs, and uv Python runtimes",
                        "devbase software list",
                    ),
                    sub_flags!(
                        "versions",
                        "List available versions from catalog or live index",
                        "devbase software versions <node|java|python|mysql|postgresql|redis|mongodb> [--refresh]",
                        [flag("--refresh", "bool", "Force refresh remote index")]
                    ),
                    sub(
                        "review",
                        "Inspect URL, SHA-256, license, compatibility, and integrity before installing",
                        "devbase software review <software> <version>",
                    ),
                    sub(
                        "install",
                        "Install a runtime or database engine package",
                        "devbase software install <software> <version>",
                    ),
                    destructive!(
                        "uninstall",
                        "Uninstall a managed runtime, JDK, Python interpreter, or Devbase package registration",
                        "devbase software uninstall (<package-id> | <software> <version-or-id>) --yes",
                        [required_flag("--yes", "bool", "Confirm uninstall")]
                    ),
                    sub(
                        "verify",
                        "Verify integrity of an installed package",
                        "devbase software verify (<package-id> | <software> <version>)",
                    ),
                    sub(
                        "repair",
                        "Reinstall/repair an installed package cleanly",
                        "devbase software repair (<package-id> | <software> <version>)",
                    ),
                    sub(
                        "default",
                        "Set default version for node, java, or python",
                        "devbase software default <node|java|python> <version-or-id>",
                    ),
                    sub(
                        "shell",
                        "Manage node/java shell integration",
                        "devbase software shell <node|java> [status|enable|disable|bash-command]",
                    ),
                    sub_flags!(
                        "which",
                        "Resolve effective runtime or database binary path",
                        "devbase software which <software> [--dir <path>] [--package <id>]",
                        [
                            flag(
                                "--dir",
                                "string",
                                "Directory for Node.js project file detection"
                            ),
                            flag("--package", "string", "Explicit installed package ID"),
                        ]
                    ),
                    sub(
                        "uv-path",
                        "Show or set the uv executable path",
                        "devbase software uv-path [<absolute-path>]",
                    ),
                ],
            ),
            command(
                "install",
                "Shortcut for `devbase software install <software> <version>`",
                "devbase install <software> <version>",
            ),
            command_with(
                "db",
                vec!["database"],
                "Create, operate, inspect, connect, back up, restore, and upgrade database instances",
                "devbase db <list|create|start|stop|restart|stop-all|remove|delete-data|port|upgrade|inspect|create-db|drop-db|create-user|drop-user|connection|password|open-cli|profile|backup>",
                vec![
                    sub(
                        "list",
                        "List configured database instances and runtime statuses",
                        "devbase db list",
                    ),
                    sub_flags!(
                        "create",
                        "Create a new managed database instance",
                        "devbase db create <id> --kind <mysql|postgresql|redis|mongodb> [--package <id> | --version <ver>] [--port <port>] [--persistent] [--data-dir <path>]",
                        [
                            required_flag(
                                "--kind",
                                "string",
                                "Engine: mysql, postgresql, redis, mongodb"
                            ),
                            flag("--package", "string", "Installed package ID"),
                            flag("--version", "string", "Version to resolve"),
                            flag("--port", "int", "TCP port (0 auto-picks a free port)"),
                            flag("--persistent", "bool", "Run automatically in background"),
                            flag("--data-dir", "string", "Optional external data directory"),
                        ]
                    ),
                    sub("start", "Start an instance", "devbase db start <id>"),
                    sub("stop", "Stop an instance", "devbase db stop <id>"),
                    sub("restart", "Restart an instance", "devbase db restart <id>"),
                    destructive!(
                        "stop-all",
                        "Stop all managed database instances",
                        "devbase db stop-all --yes",
                        [required_flag(
                            "--yes",
                            "bool",
                            "Confirm stopping all instances"
                        )]
                    ),
                    destructive!(
                        "remove",
                        "Remove an instance configuration while retaining data files",
                        "devbase db remove <id> --yes",
                        [required_flag("--yes", "bool", "Confirm removal")]
                    ),
                    destructive!(
                        "delete-data",
                        "Preview or permanently delete a stopped instance and its app-managed data (external data refused)",
                        "devbase db delete-data <id> (--preview | --confirm <id> --fingerprint <fp>)",
                        [
                            flag("--preview", "bool", "Preview deletion scope"),
                            flag("--confirm", "string", "Exact instance ID"),
                            flag("--fingerprint", "string", "Fingerprint from --preview"),
                        ]
                    ),
                    sub_flags!(
                        "port",
                        "Change or repair an instance listening port",
                        "devbase db port <id> (<new-port> | --repair)",
                        [flag(
                            "--repair",
                            "bool",
                            "Rewrite config files for current port"
                        )]
                    ),
                    sub_flags!(
                        "upgrade",
                        "Inspect upgrade targets or upgrade an instance package",
                        "devbase db upgrade <id> [<target>] [--targets]",
                        [flag("--targets", "bool", "List compatible upgrade targets")]
                    ),
                    sub(
                        "inspect",
                        "Inspect databases and users in a running instance",
                        "devbase db inspect <id>",
                    ),
                    sub(
                        "create-db",
                        "Create a database inside an instance",
                        "devbase db create-db <id> <db-name>",
                    ),
                    destructive!(
                        "drop-db",
                        "Drop a database inside an instance",
                        "devbase db drop-db <id> <db-name> --yes",
                        [required_flag("--yes", "bool", "Confirm drop")]
                    ),
                    sub_full!(
                        "create-user",
                        "Create a database user reading password from stdin",
                        "devbase db create-user <id> <username> [--database <db>] --password-stdin",
                        [
                            flag("--database", "string", "Target database"),
                            required_flag(
                                "--password-stdin",
                                "bool",
                                "Read user password from stdin"
                            )
                        ],
                        stdin = "plaintext password line"
                    ),
                    destructive!(
                        "drop-user",
                        "Drop a database user",
                        "devbase db drop-user <id> <username> [--database <db>] --yes",
                        [
                            flag("--database", "string", "MongoDB auth DB"),
                            required_flag("--yes", "bool", "Confirm drop")
                        ]
                    ),
                    sub_flags!(
                        "connection",
                        "Print password-free URI, .env assignment, JDBC URL, and CLI command",
                        "devbase db connection <id> [--profile <id>] [--database <db>] [--format uri|env|jdbc|cli]",
                        [
                            flag("--profile", "string", "Saved profile ID"),
                            flag("--database", "string", "Override DB"),
                            flag("--format", "string", "uri, env, jdbc, or cli")
                        ]
                    ),
                    sub(
                        "password",
                        "Print generated instance password on stdout",
                        "devbase db password <id>",
                    ),
                    sub_flags!(
                        "open-cli",
                        "Launch database CLI in Terminal or print shell command",
                        "devbase db open-cli <id> [--profile <id>] [--database <db>] [--print]",
                        [
                            flag("--profile", "string", "Saved profile ID"),
                            flag("--database", "string", "Override DB"),
                            flag("--print", "bool", "Only print CLI command")
                        ]
                    ),
                    sub(
                        "profile list",
                        "List saved connection profiles",
                        "devbase db profile list [<instance-id>]",
                    ),
                    sub(
                        "profile save",
                        "Save a non-secret connection profile",
                        "devbase db profile save --instance <id> --name <label> [--id <id>] [--database <db>] [--username <user>] [--auth-source <db>] [--redis-db <0-15>]",
                    ),
                    destructive!(
                        "profile delete",
                        "Delete a saved connection profile",
                        "devbase db profile delete <instance-id> <profile-id> --yes",
                        [required_flag("--yes", "bool", "Confirm profile delete")]
                    ),
                    sub(
                        "backup list",
                        "List backup files for an instance",
                        "devbase db backup list <id>",
                    ),
                    sub(
                        "backup create",
                        "Back up an instance or database",
                        "devbase db backup create <id> [--database <db>]",
                    ),
                    destructive!(
                        "backup restore",
                        "Restore an instance or database from a backup",
                        "devbase db backup restore <id> <backup-name-or-path> [--database <db>] --yes",
                        [
                            flag("--database", "string", "Target database"),
                            required_flag("--yes", "bool", "Confirm restore")
                        ]
                    ),
                    destructive!(
                        "backup delete",
                        "Delete a backup file",
                        "devbase db backup delete <id> <backup-name> --yes",
                        [required_flag("--yes", "bool", "Confirm delete")]
                    ),
                ],
            ),
            command_with(
                "https",
                vec!["proxy"],
                "Manage local *.devbase.localhost HTTPS proxy rules, listener port, activation, and live checks",
                "devbase https <status|rules|add|remove|set-port|replace|apply|stop|restart|check>",
                vec![
                    sub(
                        "status",
                        "Show proxy listener state and configured rules",
                        "devbase https [status|rules]",
                    ),
                    sub(
                        "add",
                        "Add or update an HTTPS proxy rule",
                        "devbase https add <name-or-domain> <local-port> [--upstream-host <host>] [--tunnel] [--tunnel-auth] [--tunnel-worker <name>] [--tunnel-domain <host>] [--listener-port <port>] [--apply]",
                    ),
                    sub(
                        "remove",
                        "Remove an HTTPS proxy rule",
                        "devbase https remove <name-or-domain> [--apply]",
                    ),
                    sub(
                        "set-port",
                        "Set HTTPS proxy listener port",
                        "devbase https set-port <port> [--apply]",
                    ),
                    sub_full!(
                        "replace",
                        "Replace all proxy rules from JSON on stdin",
                        "devbase https replace --json-stdin [--listener-port <port>] [--apply]",
                        [required_flag(
                            "--json-stdin",
                            "bool",
                            "Read JSON array or {listenerPort, rules}"
                        )],
                        stdin = "JSON rule list or object"
                    ),
                    privileged!(
                        "apply",
                        "Apply proxy configuration and start listener",
                        "devbase https apply"
                    ),
                    privileged!("stop", "Stop the HTTPS proxy", "devbase https stop"),
                    privileged!(
                        "restart",
                        "Restart the HTTPS proxy",
                        "devbase https restart"
                    ),
                    sub(
                        "check",
                        "Run live TLS+HTTP health check against a rule",
                        "devbase https check <name-or-domain>",
                    ),
                ],
            ),
            command_with(
                "tunnel",
                Vec::new(),
                "Manage Cloudflare Workers+Durable Object tunnels and per-rule access tokens",
                "devbase tunnel <status|configure|remove-config|discover|sync|apply|stop|restart|check|access-token>",
                vec![
                    sub(
                        "status",
                        "Show tunnel status, account metadata, workers, and rule states (never prints API token)",
                        "devbase tunnel [status]",
                    ),
                    sub_full!(
                        "configure",
                        "Save Cloudflare API token via stdin and optional account/client ID",
                        "devbase tunnel configure [--token-stdin] [--account-id <id>] [--client-id <id>]",
                        [flag(
                            "--token-stdin",
                            "bool",
                            "Read Cloudflare API token from stdin"
                        )],
                        stdin = "Cloudflare API token"
                    ),
                    destructive!(
                        "remove-config",
                        "Remove Cloudflare API token and metadata (optionally delete deployed Workers)",
                        "devbase tunnel remove-config --yes [--delete-workers]",
                        [
                            required_flag("--yes", "bool", "Confirm removal"),
                            flag(
                                "--delete-workers",
                                "bool",
                                "Also delete devbase-* Workers on Cloudflare"
                            )
                        ]
                    ),
                    sub(
                        "discover",
                        "Discover Cloudflare account ID, workers.dev subdomain, zones, and DNS records",
                        "devbase tunnel discover",
                    ),
                    sub(
                        "sync",
                        "Deploy/update Workers and Custom Domains for tunnel-enabled rules",
                        "devbase tunnel sync",
                    ),
                    sub(
                        "apply",
                        "Start the local tunnel agent",
                        "devbase tunnel apply",
                    ),
                    sub("stop", "Stop the local tunnel agent", "devbase tunnel stop"),
                    sub(
                        "restart",
                        "Restart the local tunnel agent",
                        "devbase tunnel restart",
                    ),
                    sub(
                        "check",
                        "Verify live reachability through Cloudflare tunnel",
                        "devbase tunnel check <name-or-domain>",
                    ),
                    sub(
                        "access-token",
                        "Read or rotate the Devbase access token for a protected tunnel rule",
                        "devbase tunnel access-token <name-or-domain> [--rotate]",
                    ),
                ],
            ),
            command_with(
                "ca",
                Vec::new(),
                "Manage the generational Local Development CA and login keychain trust",
                "devbase ca <status|trust|untrust|prepare|trust-prepared|activate|discard-prepared|restore|saved-trust|remove-archived-trust>",
                vec![
                    sub(
                        "status",
                        "Show active, prepared, previous, and archived CA generations",
                        "devbase ca [status]",
                    ),
                    sub(
                        "trust",
                        "Trust the active local CA in login keychain",
                        "devbase ca trust",
                    ),
                    destructive!(
                        "untrust",
                        "Remove active local CA trust from login keychain",
                        "devbase ca untrust --yes",
                        [required_flag("--yes", "bool", "Confirm untrust")]
                    ),
                    sub(
                        "prepare",
                        "Generate a replacement prepared CA generation",
                        "devbase ca prepare",
                    ),
                    sub(
                        "trust-prepared",
                        "Trust prepared CA generation before activating",
                        "devbase ca trust-prepared [--revision <rev>]",
                    ),
                    sub(
                        "activate",
                        "Switch active CA to prepared generation",
                        "devbase ca activate [--revision <rev>]",
                    ),
                    sub(
                        "discard-prepared",
                        "Discard prepared CA generation",
                        "devbase ca discard-prepared [--revision <rev>]",
                    ),
                    sub(
                        "restore",
                        "Restore a previous/archived CA generation",
                        "devbase ca restore <generation-id> --fingerprint <fp> [--revision <rev>]",
                    ),
                    sub(
                        "saved-trust",
                        "Trust or untrust a saved CA generation",
                        "devbase ca saved-trust <generation-id> --fingerprint <fp> [--remove]",
                    ),
                    destructive!(
                        "remove-archived-trust",
                        "Remove keychain trust for an archived CA",
                        "devbase ca remove-archived-trust <archive-id> --fingerprint <fp> --yes",
                        [required_flag("--yes", "bool", "Confirm removal")]
                    ),
                ],
            ),
            command_with(
                "services",
                vec!["service"],
                #[cfg(target_os = "macos")]
                "Inspect and manage Homebrew services, Login Items, User Agents, System Agents, and Daemons",
                #[cfg(not(target_os = "macos"))]
                "Inspect and manage systemd user/system services and XDG autostart entries",
                #[cfg(target_os = "macos")]
                "devbase services <list|inspect|start|stop|restart|load|unload|enable|disable|remove|add-login-item|job-save>",
                #[cfg(not(target_os = "macos"))]
                "devbase services <list|inspect|start|stop|restart|enable|disable|remove|logs|create>",
                {
                    #[cfg_attr(target_os = "macos", allow(unused_mut))]
                    let mut subs = vec![
                        sub(
                            "list",
                            "List services filtered by source, state, and query",
                            "devbase services list [--source <src>] [--state <state>] [--query <text>]",
                        ),
                        sub(
                            "inspect",
                            "Show details, logs, actions, and parsed launchd Spec",
                            "devbase services inspect <id-or-name> [--source <src>]",
                        ),
                        sub(
                            "start|stop|restart|load|unload|enable|disable",
                            "Run lifecycle action on a service",
                            "devbase services <action> <id-or-name> [--source <src>] [--yes]",
                        ),
                        destructive!(
                            "remove",
                            "Remove a service or launchd job",
                            "devbase services remove <id-or-name> [--source <src>] --yes",
                            [required_flag("--yes", "bool", "Confirm removal")]
                        ),
                        sub(
                            "add-login-item",
                            "Add an .app bundle to macOS Login Items",
                            "devbase services add-login-item <app-path>",
                        ),
                        sub_full!(
                            "job-save",
                            "Create or edit a launchd property list job from flags or JSON stdin",
                            "devbase services job-save [--source user-agent|system-agent|daemon] [--existing <path>] [--load] (--json-stdin | --label <id> (--command <sh> | --arg <bin>))",
                            [],
                            stdin = "optional launchd.Spec JSON"
                        ),
                    ];
                    #[cfg(not(target_os = "macos"))]
                    {
                        subs.push(sub(
                            "logs",
                            "Read systemd journal or file logs for a service",
                            "devbase services logs <id-or-name> [--source <src>] [--lines <n>]",
                        ));
                        subs.push(sub_full!(
                            "create",
                            "Create a validated systemd .service unit from flags or JSON stdin",
                            "devbase services create [--source systemd-user|systemd-system] [--enable] (--json-stdin | --name <unit> (--command <sh> | --arg <bin>))",
                            [],
                            stdin = "optional systemd UnitSpec JSON"
                        ));
                    }
                    subs
                },
            ),
            command_with(
                "settings",
                Vec::new(),
                "Manage privileged helper, download cache, log rotation, and login menu bar item",
                "devbase settings <status|helper|cache|log-rotation|login-item>",
                vec![
                    sub(
                        "status",
                        "Show settings summary",
                        "devbase settings [status]",
                    ),
                    privileged!(
                        "helper",
                        "Check, install, or remove the privileged helper",
                        "devbase settings helper [status|install|uninstall --yes]"
                    ),
                    sub(
                        "cache",
                        "Inspect or clear the download archive cache",
                        "devbase settings cache [status|clear --yes]",
                    ),
                    sub(
                        "log-rotation",
                        "Inspect or toggle scheduled 60s log rotation",
                        "devbase settings log-rotation [status|on|off]",
                    ),
                    sub(
                        "login-item",
                        "Inspect or toggle starting Devbase in menu bar at login",
                        "devbase settings login-item [status|on|off]",
                    ),
                ],
            ),
            command(
                "log-rotation",
                "Alias for `devbase settings log-rotation`",
                "devbase log-rotation [status|on|off]",
            ),
            command_with(
                "cli",
                Vec::new(),
                "Install, inspect, or remove the managed `devbase` wrapper script in PATH",
                "devbase cli <status|install|uninstall> [--dir <bin-dir>]",
                vec![
                    sub(
                        "status",
                        "Inspect CLI wrapper status",
                        "devbase cli [status] [--dir <bin-dir>]",
                    ),
                    sub(
                        "install",
                        "Install managed devbase wrapper script into bin directory",
                        "devbase cli install [--dir <bin-dir>]",
                    ),
                    sub(
                        "uninstall",
                        "Remove managed devbase wrapper script",
                        "devbase cli uninstall [--dir <bin-dir>]",
                    ),
                ],
            ),
            command(
                "schema",
                "Emit complete machine-readable JSON CLI contract for AI agents and scripts",
                "devbase schema [--command <name>]",
            ),
            command(
                "mcp",
                "Run the read-only stdio Model Context Protocol (MCP) server for AI agents",
                "devbase mcp",
            ),
            command_with(
                "version",
                vec!["--version", "-v"],
                "Show Devbase CLI version and runtime info",
                "devbase version [--json]",
                Vec::new(),
            ),
            command_with(
                "help",
                vec!["--help", "-h"],
                "Show general or topic-specific CLI help",
                "devbase help [<command>]",
                Vec::new(),
            ),
            command_with(
                "gui",
                vec!["open"],
                "Launch the Devbase macOS desktop window (or pass --background for menu bar only)",
                "devbase gui [--background]",
                Vec::new(),
            ),
        ],
    }
}

pub fn find_schema_command(topic: &str) -> Option<CommandSpec> {
    let topic = topic.trim().to_lowercase();
    build_cli_schema().commands.into_iter().find(|command| {
        command.name == topic || command.aliases.iter().any(|alias| *alias == topic)
    })
}

pub fn has_topic_in_schema(topic: &str) -> bool {
    find_schema_command(topic).is_some()
}

pub fn print_topic_help(w: &mut dyn std::io::Write, topic: &str) -> std::io::Result<()> {
    if !topic.is_empty()
        && let Some(command) = find_schema_command(topic)
    {
        writeln!(
            w,
            "{} - {}\n\nUsage:\n  {}",
            command.name, command.summary, command.usage
        )?;
        if !command.aliases.is_empty() {
            writeln!(w, "\nAliases: {}", command.aliases.join(", "))?;
        }
        if !command.subcommands.is_empty() {
            writeln!(w, "\nSubcommands:")?;
            for item in &command.subcommands {
                writeln!(
                    w,
                    "  {:<22} {}\n    {}",
                    item.name, item.summary, item.usage
                )?;
            }
        }
        writeln!(
            w,
            "\nGlobal flags:\n  --json                 Emit JSON output (stdout) and JSON errors (stderr)"
        )?;
        return Ok(());
    }
    let schema = build_cli_schema();
    writeln!(
        w,
        "Devbase CLI ({}) - Local runtimes, databases, HTTPS proxy, Cloudflare tunnels, and macOS services\n",
        schema.version
    )?;
    writeln!(w, "Usage:")?;
    writeln!(w, "  devbase <command> [subcommand] [args] [--json]")?;
    writeln!(
        w,
        "  devbase                  (from Finder/Dock) launches the desktop window"
    )?;
    writeln!(w, "\nCommands:")?;
    for command in &schema.commands {
        let mut name = command.name.to_string();
        if !command.aliases.is_empty() {
            name.push_str(&format!(" ({})", command.aliases.join(",")));
        }
        writeln!(w, "  {:<22} {}", name, command.summary)?;
    }
    writeln!(w, "\nGlobal flags & AI usage:")?;
    writeln!(
        w,
        "  --json                 Structured JSON stdout and structured JSON stderr errors"
    )?;
    writeln!(
        w,
        "  devbase schema         Full machine-readable JSON specification of commands, flags, exit codes"
    )?;
    writeln!(w, "  devbase help <cmd>     Detailed usage for one command")?;
    writeln!(w, "\nSecurity rules:")?;
    writeln!(
        w,
        "  Never pass secrets in argv (--password/--token/--api-token fail with exit code 2);"
    )?;
    writeln!(w, "  use --password-stdin or --token-stdin over stdin.")?;
    Ok(())
}

/// `schema [--command X] [X]`: always JSON, regardless of `--json`.
pub fn run_schema_command(out: &mut dyn std::io::Write, args: &[String]) -> CliResult<()> {
    use crate::args::{FlagSpecs, ParseOutcome, has_help_flag, parse_flags};

    if has_help_flag(args) {
        print_topic_help(out, "schema")?;
        return Ok(());
    }
    let specs = FlagSpecs::new().value("command");
    let ParseOutcome::Parsed(parsed) = parse_flags(args, &specs)? else {
        print_topic_help(out, "schema")?;
        return Ok(());
    };
    if parsed.positionals.len() > 1 {
        return Err(CliError::usage("usage: devbase schema [--command <name>]"));
    }
    let mut filter = parsed.string("command");
    if parsed.positionals.len() == 1 && filter.is_empty() {
        filter = parsed.positionals[0].clone();
    }
    let mut schema = build_cli_schema();
    let filter = filter.trim().to_lowercase();
    if !filter.is_empty() {
        let matched: Vec<CommandSpec> = schema
            .commands
            .iter()
            .filter(|command| {
                command.name == filter || command.aliases.iter().any(|alias| *alias == filter)
            })
            .cloned()
            .collect();
        if matched.is_empty() {
            return Err(CliError::usage(format!(
                "unknown command {filter:?} in schema"
            )));
        }
        schema.commands = matched;
    }
    crate::output::write_json(out, &schema)?;
    Ok(())
}

/// `version` with the map keys in Go's sorted order.
pub fn run_version_command(
    out: &mut dyn std::io::Write,
    json: bool,
    args: &[String],
) -> CliResult<()> {
    use crate::args::has_help_flag;
    use crate::output::write_json;

    if has_help_flag(args) {
        print_topic_help(out, "version")?;
        return Ok(());
    }
    if let Some(extra) = args.first() {
        return Err(CliError::usage(format!(
            "unexpected argument {extra:?} for version"
        )));
    }
    let mut payload = std::collections::BTreeMap::new();
    payload.insert("arch", CLI_ARCH);
    payload.insert("name", "devbase");
    payload.insert("version", CLI_VERSION);
    if json {
        write_json(out, &payload)?;
    } else {
        writeln!(out, "devbase {} ({})", CLI_VERSION, CLI_ARCH)?;
    }
    Ok(())
}

pub fn run_help_command(out: &mut dyn std::io::Write, args: &[String]) -> CliResult<()> {
    if args.len() > 1 {
        return Err(CliError::usage("usage: devbase help [<command>]"));
    }
    if let Some(topic) = args.first() {
        let topic = topic.trim().to_lowercase();
        if !has_topic_in_schema(&topic) {
            return Err(CliError::usage(format!("unknown help topic {:?}", args[0])));
        }
        print_topic_help(out, &topic)?;
        return Ok(());
    }
    print_topic_help(out, "")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_has_the_static_shape() {
        let schema = build_cli_schema();
        assert_eq!(schema.schema_version, 1);
        assert_eq!(schema.name, "devbase");
        assert_eq!(schema.version, "0.1.0");
        assert_eq!(schema.commands.len(), 21);
        assert_eq!(schema.commands[0].name, "status");
        assert_eq!(schema.commands.last().unwrap().name, "gui");
        assert_eq!(
            schema.exit_codes.get("2"),
            Some(&"usage_or_safety_guard_error")
        );
    }

    #[test]
    fn filtered_schema_matches_alias() {
        let schema = build_cli_schema();
        let db = schema.commands.iter().find(|c| c.name == "db").unwrap();
        assert!(db.aliases.contains(&"database"));
        let command = find_schema_command("database").unwrap();
        assert_eq!(command.name, "db");
        assert!(find_schema_command("nope").is_none());
    }

    #[test]
    fn schema_json_is_two_space_indented_and_unescaped() {
        let schema = build_cli_schema();
        let text = crate::output::to_go_json(&schema).unwrap();
        assert!(text.starts_with("{\n  \"schemaVersion\": 1,"));
        assert!(text.contains("  \"commands\": ["));
        assert!(!text.contains("\\u003c"));
    }

    #[test]
    fn cli_arch_matches_current_host_target() {
        assert_eq!(
            CLI_ARCH,
            devcore::platform::HostTarget::current().cli_arch()
        );
        #[cfg(target_os = "macos")]
        assert_eq!(CLI_ARCH, "darwin/arm64");
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        assert_eq!(CLI_ARCH, "linux/amd64");
        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        assert_eq!(CLI_ARCH, "linux/arm64");
    }
}

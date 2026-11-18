package devcli

import (
	"fmt"
	"io"
	"strings"
)

// FlagSpec describes a CLI flag in the machine-readable schema output.
type FlagSpec struct {
	Name        string `json:"name"`
	Type        string `json:"type"`
	Required    bool   `json:"required,omitempty"`
	Repeatable  bool   `json:"repeatable,omitempty"`
	Description string `json:"description"`
}

// SubcommandSpec describes a single CLI subcommand operation.
type SubcommandSpec struct {
	Name        string     `json:"name"`
	Summary     string     `json:"summary"`
	Usage       string     `json:"usage"`
	Flags       []FlagSpec `json:"flags,omitempty"`
	Stdin       string     `json:"stdin,omitempty"`
	Destructive bool       `json:"destructive,omitempty"`
	Privileged  bool       `json:"privileged,omitempty"`
}

// CommandSpec describes a top-level Devbase CLI command group.
type CommandSpec struct {
	Name        string           `json:"name"`
	Aliases     []string         `json:"aliases,omitempty"`
	Summary     string           `json:"summary"`
	Usage       string           `json:"usage"`
	Subcommands []SubcommandSpec `json:"subcommands,omitempty"`
}

// CLISchema is the self-describing machine contract returned by `devbase schema`.
type CLISchema struct {
	SchemaVersion    int               `json:"schemaVersion"`
	Name             string            `json:"name"`
	Version          string            `json:"version"`
	GlobalFlags      []FlagSpec        `json:"globalFlags"`
	ExitCodes        map[string]string `json:"exitCodes"`
	SafetyInvariants []string          `json:"safetyInvariants"`
	Commands         []CommandSpec     `json:"commands"`
}

func buildCLISchema() CLISchema {
	return CLISchema{
		SchemaVersion: 1,
		Name:          "devbase",
		Version:       CLIVersion,
		GlobalFlags: []FlagSpec{
			{Name: "--json", Type: "bool", Description: "Emit structured JSON output on stdout and structured JSON errors on stderr"},
			{Name: "--help, -h", Type: "bool", Description: "Print help for top-level or topic commands"},
		},
		ExitCodes: map[string]string{
			"0": "success",
			"1": "runtime_or_operation_error",
			"2": "usage_or_safety_guard_error",
		},
		SafetyInvariants: []string{
			"Plaintext secrets are forbidden in argv (--password, --token, --api-token exit 2); always pipe via stdin using --password-stdin or --token-stdin.",
			"Destructive operations require explicit non-interactive confirmation flags (--yes, or --confirm <id> --fingerprint <fp> for app-managed permanent database deletion).",
			"External database directories (dataDirectory != \"\") can never be permanently deleted.",
			"Shared Homebrew kegs are never removed when uninstalling a Devbase package registration.",
			"Devbase never starts or supervises project web/app servers; developers run their own upstream apps.",
		},
		Commands: []CommandSpec{
			{
				Name:    "status",
				Summary: "Inspect workspace state, installed packages, database instances, proxy/tunnel status, helper, and CLI wrapper",
				Usage:   "devbase status [--json]",
			},
			{
				Name:    "diagnostics",
				Summary: "Run Devbase environment diagnostics or export a redacted report",
				Usage:   "devbase diagnostics [run|export] [--json]",
				Subcommands: []SubcommandSpec{
					{Name: "run", Summary: "Run full layered health checks across runtimes, databases, proxy, tunnel, and CA", Usage: "devbase diagnostics [run]"},
					{Name: "export", Summary: "Write a redacted diagnostic report bundle to disk", Usage: "devbase diagnostics export"},
				},
			},
			{
				Name:    "tasks",
				Summary: "Inspect and resolve recorded background operations, database work, initializations, and staged deletions",
				Usage:   "devbase tasks <list|dismiss|retry|work|resolve-work|initializations|resolve-init|deletions|finish-deletion|restore-deletion>",
				Subcommands: []SubcommandSpec{
					{Name: "list", Summary: "List recorded operations", Usage: "devbase tasks list"},
					{Name: "dismiss", Summary: "Dismiss a recorded operation", Usage: "devbase tasks dismiss <id> <revision>"},
					{Name: "retry", Summary: "Retry a failed eligible operation", Usage: "devbase tasks retry <id> <revision>"},
					{Name: "work", Summary: "List unfinished database tasks", Usage: "devbase tasks work"},
					{Name: "resolve-work", Summary: "Mark an unfinished database task resolved", Usage: "devbase tasks resolve-work <id> <revision>"},
					{Name: "initializations", Summary: "List interrupted database initializations", Usage: "devbase tasks initializations"},
					{Name: "resolve-init", Summary: "Archive and clear an unfinished initialization", Usage: "devbase tasks resolve-init <id> <revision>"},
					{Name: "deletions", Summary: "List staged database deletions", Usage: "devbase tasks deletions"},
					{
						Name:        "finish-deletion",
						Summary:     "Finish erasing a staged deletion",
						Usage:       "devbase tasks finish-deletion <id> <revision> --confirm <instance-id>",
						Flags:       []FlagSpec{{Name: "--confirm", Type: "string", Required: true, Description: "Exact original instance ID"}},
						Destructive: true,
					},
					{Name: "restore-deletion", Summary: "Restore a staged deletion", Usage: "devbase tasks restore-deletion <id> <revision>"},
				},
			},
			{
				Name:    "logs",
				Summary: "Read bounded trailing log lines for proxy, tunnel, or database instances",
				Usage:   "devbase logs <proxy|tunnel|db-<id>|<instance-id>> [--stream stderr|stdout] [--lines 200] [--filter <text>]",
				Subcommands: []SubcommandSpec{
					{
						Name:    "tail",
						Summary: "Tail logs from proxy, tunnel, or a database job",
						Usage:   "devbase logs <target> [--stream stderr|stdout] [--lines N] [--filter <text>]",
						Flags: []FlagSpec{
							{Name: "--stream", Type: "string", Description: "Log stream: stderr or stdout"},
							{Name: "--lines", Type: "int", Description: "Maximum number of trailing lines (default 200, max 5000)"},
							{Name: "--filter", Type: "string", Description: "Case-insensitive substring filter"},
						},
					},
				},
			},
			{
				Name:    "ports",
				Summary: "Inspect active local listening user ports, find a free port, or check a port",
				Usage:   "devbase ports <active|free|check <port>>",
				Subcommands: []SubcommandSpec{
					{Name: "active", Summary: "List listening local user ports", Usage: "devbase ports [active]"},
					{Name: "free", Summary: "Allocate/suggest an unused local TCP port", Usage: "devbase ports free"},
					{Name: "check", Summary: "Inspect listeners on a specific port", Usage: "devbase ports check <port>", Privileged: true},
				},
			},
			{
				Name:    "shutdown",
				Summary: "Preview or stop all managed database instances (keeping proxy and tunnel running)",
				Usage:   "devbase shutdown <preview|stop --yes>",
				Subcommands: []SubcommandSpec{
					{Name: "preview", Summary: "Preview managed database instances in stop scope", Usage: "devbase shutdown [preview]"},
					{
						Name:        "stop",
						Summary:     "Stop all managed database instances",
						Usage:       "devbase shutdown stop --yes",
						Flags:       []FlagSpec{{Name: "--yes", Type: "bool", Required: true, Description: "Confirm stopping all managed database instances"}},
						Destructive: true,
					},
				},
			},
			{
				Name:    "software",
				Summary: "Manage runtimes and database engine packages (node, java, python, mysql, postgresql, redis, mongodb)",
				Usage:   "devbase software <list|versions|review|install|uninstall|verify|repair|default|shell|which|uv-path>",
				Subcommands: []SubcommandSpec{
					{Name: "list", Summary: "List installed packages, JDKs, and uv Python runtimes", Usage: "devbase software list"},
					{
						Name:    "versions",
						Summary: "List available versions from catalog or live index",
						Usage:   "devbase software versions <node|java|python|mysql|postgresql|redis|mongodb> [--refresh]",
						Flags:   []FlagSpec{{Name: "--refresh", Type: "bool", Description: "Force refresh remote index"}},
					},
					{Name: "review", Summary: "Inspect URL, SHA-256, license, compatibility, and integrity before installing", Usage: "devbase software review <software> <version>"},
					{Name: "install", Summary: "Install a runtime or database engine package", Usage: "devbase software install <software> <version>"},
					{
						Name:        "uninstall",
						Summary:     "Uninstall a managed runtime, JDK, Python interpreter, or Devbase package registration",
						Usage:       "devbase software uninstall (<package-id> | <software> <version-or-id>) --yes",
						Flags:       []FlagSpec{{Name: "--yes", Type: "bool", Required: true, Description: "Confirm uninstall"}},
						Destructive: true,
					},
					{Name: "verify", Summary: "Verify integrity of an installed package", Usage: "devbase software verify (<package-id> | <software> <version>)"},
					{Name: "repair", Summary: "Reinstall/repair an installed package cleanly", Usage: "devbase software repair (<package-id> | <software> <version>)"},
					{Name: "default", Summary: "Set default version for node, java, or python", Usage: "devbase software default <node|java|python> <version-or-id>"},
					{Name: "shell", Summary: "Manage node/java shell integration", Usage: "devbase software shell <node|java> [status|enable|disable|bash-command]"},
					{
						Name:    "which",
						Summary: "Resolve effective runtime or database binary path",
						Usage:   "devbase software which <software> [--dir <path>] [--package <id>]",
						Flags: []FlagSpec{
							{Name: "--dir", Type: "string", Description: "Directory for Node.js project file detection"},
							{Name: "--package", Type: "string", Description: "Explicit installed package ID"},
						},
					},
					{Name: "uv-path", Summary: "Show or set the uv executable path", Usage: "devbase software uv-path [<absolute-path>]"},
				},
			},
			{
				Name:    "install",
				Summary: "Shortcut for `devbase software install <software> <version>`",
				Usage:   "devbase install <software> <version>",
			},
			{
				Name:    "db",
				Aliases: []string{"database"},
				Summary: "Create, operate, inspect, connect, back up, restore, and upgrade database instances",
				Usage:   "devbase db <list|create|start|stop|restart|stop-all|remove|delete-data|port|upgrade|inspect|create-db|drop-db|create-user|drop-user|connection|password|open-cli|profile|backup>",
				Subcommands: []SubcommandSpec{
					{Name: "list", Summary: "List configured database instances and runtime statuses", Usage: "devbase db list"},
					{
						Name:    "create",
						Summary: "Create a new managed database instance",
						Usage:   "devbase db create <id> --kind <mysql|postgresql|redis|mongodb> [--package <id> | --version <ver>] [--port <port>] [--persistent] [--data-dir <path>]",
						Flags: []FlagSpec{
							{Name: "--kind", Type: "string", Required: true, Description: "Engine: mysql, postgresql, redis, mongodb"},
							{Name: "--package", Type: "string", Description: "Installed package ID"},
							{Name: "--version", Type: "string", Description: "Version to resolve"},
							{Name: "--port", Type: "int", Description: "TCP port (0 auto-picks a free port)"},
							{Name: "--persistent", Type: "bool", Description: "Run automatically in background"},
							{Name: "--data-dir", Type: "string", Description: "Optional external data directory"},
						},
					},
					{Name: "start", Summary: "Start an instance", Usage: "devbase db start <id>"},
					{Name: "stop", Summary: "Stop an instance", Usage: "devbase db stop <id>"},
					{Name: "restart", Summary: "Restart an instance", Usage: "devbase db restart <id>"},
					{Name: "stop-all", Summary: "Stop all managed database instances", Usage: "devbase db stop-all --yes", Flags: []FlagSpec{{Name: "--yes", Type: "bool", Required: true, Description: "Confirm stopping all instances"}}, Destructive: true},
					{Name: "remove", Summary: "Remove an instance configuration while retaining data files", Usage: "devbase db remove <id> --yes", Flags: []FlagSpec{{Name: "--yes", Type: "bool", Required: true, Description: "Confirm removal"}}, Destructive: true},
					{
						Name:        "delete-data",
						Summary:     "Preview or permanently delete a stopped instance and its app-managed data (external data refused)",
						Usage:       "devbase db delete-data <id> (--preview | --confirm <id> --fingerprint <fp>)",
						Flags:       []FlagSpec{{Name: "--preview", Type: "bool", Description: "Preview deletion scope"}, {Name: "--confirm", Type: "string", Description: "Exact instance ID"}, {Name: "--fingerprint", Type: "string", Description: "Fingerprint from --preview"}},
						Destructive: true,
					},
					{Name: "port", Summary: "Change or repair an instance listening port", Usage: "devbase db port <id> (<new-port> | --repair)", Flags: []FlagSpec{{Name: "--repair", Type: "bool", Description: "Rewrite config files for current port"}}},
					{Name: "upgrade", Summary: "Inspect upgrade targets or upgrade an instance package", Usage: "devbase db upgrade <id> [<target>] [--targets]", Flags: []FlagSpec{{Name: "--targets", Type: "bool", Description: "List compatible upgrade targets"}}},
					{Name: "inspect", Summary: "Inspect databases and users in a running instance", Usage: "devbase db inspect <id>"},
					{Name: "create-db", Summary: "Create a database inside an instance", Usage: "devbase db create-db <id> <db-name>"},
					{Name: "drop-db", Summary: "Drop a database inside an instance", Usage: "devbase db drop-db <id> <db-name> --yes", Flags: []FlagSpec{{Name: "--yes", Type: "bool", Required: true, Description: "Confirm drop"}}, Destructive: true},
					{Name: "create-user", Summary: "Create a database user reading password from stdin", Usage: "devbase db create-user <id> <username> [--database <db>] --password-stdin", Flags: []FlagSpec{{Name: "--database", Type: "string", Description: "Target database"}, {Name: "--password-stdin", Type: "bool", Required: true, Description: "Read user password from stdin"}}, Stdin: "plaintext password line"},
					{Name: "drop-user", Summary: "Drop a database user", Usage: "devbase db drop-user <id> <username> [--database <db>] --yes", Flags: []FlagSpec{{Name: "--database", Type: "string", Description: "MongoDB auth DB"}, {Name: "--yes", Type: "bool", Required: true, Description: "Confirm drop"}}, Destructive: true},
					{Name: "connection", Summary: "Print password-free URI, .env assignment, JDBC URL, and CLI command", Usage: "devbase db connection <id> [--profile <id>] [--database <db>] [--format uri|env|jdbc|cli]", Flags: []FlagSpec{{Name: "--profile", Type: "string", Description: "Saved profile ID"}, {Name: "--database", Type: "string", Description: "Override DB"}, {Name: "--format", Type: "string", Description: "uri, env, jdbc, or cli"}}},
					{Name: "password", Summary: "Print generated instance password on stdout", Usage: "devbase db password <id>"},
					{Name: "open-cli", Summary: "Launch database CLI in Terminal or print shell command", Usage: "devbase db open-cli <id> [--profile <id>] [--database <db>] [--print]", Flags: []FlagSpec{{Name: "--profile", Type: "string", Description: "Saved profile ID"}, {Name: "--database", Type: "string", Description: "Override DB"}, {Name: "--print", Type: "bool", Description: "Only print CLI command"}}},
					{Name: "profile list", Summary: "List saved connection profiles", Usage: "devbase db profile list [<instance-id>]"},
					{Name: "profile save", Summary: "Save a non-secret connection profile", Usage: "devbase db profile save --instance <id> --name <label> [--id <id>] [--database <db>] [--username <user>] [--auth-source <db>] [--redis-db <0-15>]"},
					{Name: "profile delete", Summary: "Delete a saved connection profile", Usage: "devbase db profile delete <instance-id> <profile-id> --yes", Flags: []FlagSpec{{Name: "--yes", Type: "bool", Required: true, Description: "Confirm profile delete"}}, Destructive: true},
					{Name: "backup list", Summary: "List backup files for an instance", Usage: "devbase db backup list <id>"},
					{Name: "backup create", Summary: "Back up an instance or database", Usage: "devbase db backup create <id> [--database <db>]"},
					{Name: "backup restore", Summary: "Restore an instance or database from a backup", Usage: "devbase db backup restore <id> <backup-name-or-path> [--database <db>] --yes", Flags: []FlagSpec{{Name: "--database", Type: "string", Description: "Target database"}, {Name: "--yes", Type: "bool", Required: true, Description: "Confirm restore"}}, Destructive: true},
					{Name: "backup delete", Summary: "Delete a backup file", Usage: "devbase db backup delete <id> <backup-name> --yes", Flags: []FlagSpec{{Name: "--yes", Type: "bool", Required: true, Description: "Confirm delete"}}, Destructive: true},
				},
			},
			{
				Name:    "https",
				Aliases: []string{"proxy"},
				Summary: "Manage local *.devbase.localhost HTTPS proxy rules, listener port, activation, and live checks",
				Usage:   "devbase https <status|rules|add|remove|set-port|replace|apply|stop|restart|check>",
				Subcommands: []SubcommandSpec{
					{Name: "status", Summary: "Show proxy listener state and configured rules", Usage: "devbase https [status|rules]"},
					{Name: "add", Summary: "Add or update an HTTPS proxy rule", Usage: "devbase https add <name-or-domain> <local-port> [--upstream-host <host>] [--tunnel] [--tunnel-auth] [--tunnel-worker <name>] [--tunnel-domain <host>] [--listener-port <port>] [--apply]"},
					{Name: "remove", Summary: "Remove an HTTPS proxy rule", Usage: "devbase https remove <name-or-domain> [--apply]"},
					{Name: "set-port", Summary: "Set HTTPS proxy listener port", Usage: "devbase https set-port <port> [--apply]"},
					{Name: "replace", Summary: "Replace all proxy rules from JSON on stdin", Usage: "devbase https replace --json-stdin [--listener-port <port>] [--apply]", Flags: []FlagSpec{{Name: "--json-stdin", Type: "bool", Required: true, Description: "Read JSON array or {listenerPort, rules}"}}, Stdin: "JSON rule list or object"},
					{Name: "apply", Summary: "Apply proxy configuration and start listener", Usage: "devbase https apply", Privileged: true},
					{Name: "stop", Summary: "Stop the HTTPS proxy", Usage: "devbase https stop", Privileged: true},
					{Name: "restart", Summary: "Restart the HTTPS proxy", Usage: "devbase https restart", Privileged: true},
					{Name: "check", Summary: "Run live TLS+HTTP health check against a rule", Usage: "devbase https check <name-or-domain>"},
				},
			},
			{
				Name:    "tunnel",
				Summary: "Manage Cloudflare Workers+Durable Object tunnels and per-rule access tokens",
				Usage:   "devbase tunnel <status|configure|remove-config|discover|sync|apply|stop|restart|check|access-token>",
				Subcommands: []SubcommandSpec{
					{Name: "status", Summary: "Show tunnel status, account metadata, workers, and rule states (never prints API token)", Usage: "devbase tunnel [status]"},
					{Name: "configure", Summary: "Save Cloudflare API token via stdin and optional account/client ID", Usage: "devbase tunnel configure [--token-stdin] [--account-id <id>] [--client-id <id>]", Flags: []FlagSpec{{Name: "--token-stdin", Type: "bool", Description: "Read Cloudflare API token from stdin"}}, Stdin: "Cloudflare API token"},
					{Name: "remove-config", Summary: "Remove Cloudflare API token and metadata (optionally delete deployed Workers)", Usage: "devbase tunnel remove-config --yes [--delete-workers]", Flags: []FlagSpec{{Name: "--yes", Type: "bool", Required: true, Description: "Confirm removal"}, {Name: "--delete-workers", Type: "bool", Description: "Also delete devbase-* Workers on Cloudflare"}}, Destructive: true},
					{Name: "discover", Summary: "Discover Cloudflare account ID, workers.dev subdomain, zones, and DNS records", Usage: "devbase tunnel discover"},
					{Name: "sync", Summary: "Deploy/update Workers and Custom Domains for tunnel-enabled rules", Usage: "devbase tunnel sync"},
					{Name: "apply", Summary: "Start the local tunnel agent", Usage: "devbase tunnel apply"},
					{Name: "stop", Summary: "Stop the local tunnel agent", Usage: "devbase tunnel stop"},
					{Name: "restart", Summary: "Restart the local tunnel agent", Usage: "devbase tunnel restart"},
					{Name: "check", Summary: "Verify live reachability through Cloudflare tunnel", Usage: "devbase tunnel check <name-or-domain>"},
					{Name: "access-token", Summary: "Read or rotate the Devbase access token for a protected tunnel rule", Usage: "devbase tunnel access-token <name-or-domain> [--rotate]"},
				},
			},
			{
				Name:    "ca",
				Summary: "Manage the generational Local Development CA and login keychain trust",
				Usage:   "devbase ca <status|trust|untrust|prepare|trust-prepared|activate|discard-prepared|restore|saved-trust|remove-archived-trust>",
				Subcommands: []SubcommandSpec{
					{Name: "status", Summary: "Show active, prepared, previous, and archived CA generations", Usage: "devbase ca [status]"},
					{Name: "trust", Summary: "Trust the active local CA in login keychain", Usage: "devbase ca trust"},
					{Name: "untrust", Summary: "Remove active local CA trust from login keychain", Usage: "devbase ca untrust --yes", Flags: []FlagSpec{{Name: "--yes", Type: "bool", Required: true, Description: "Confirm untrust"}}, Destructive: true},
					{Name: "prepare", Summary: "Generate a replacement prepared CA generation", Usage: "devbase ca prepare"},
					{Name: "trust-prepared", Summary: "Trust prepared CA generation before activating", Usage: "devbase ca trust-prepared [--revision <rev>]"},
					{Name: "activate", Summary: "Switch active CA to prepared generation", Usage: "devbase ca activate [--revision <rev>]"},
					{Name: "discard-prepared", Summary: "Discard prepared CA generation", Usage: "devbase ca discard-prepared [--revision <rev>]"},
					{Name: "restore", Summary: "Restore a previous/archived CA generation", Usage: "devbase ca restore <generation-id> --fingerprint <fp> [--revision <rev>]"},
					{Name: "saved-trust", Summary: "Trust or untrust a saved CA generation", Usage: "devbase ca saved-trust <generation-id> --fingerprint <fp> [--remove]"},
					{Name: "remove-archived-trust", Summary: "Remove keychain trust for an archived CA", Usage: "devbase ca remove-archived-trust <archive-id> --fingerprint <fp> --yes", Flags: []FlagSpec{{Name: "--yes", Type: "bool", Required: true, Description: "Confirm removal"}}, Destructive: true},
				},
			},
			{
				Name:    "services",
				Aliases: []string{"service"},
				Summary: "Inspect and manage Homebrew services, Login Items, User Agents, System Agents, and Daemons",
				Usage:   "devbase services <list|inspect|start|stop|restart|load|unload|enable|disable|remove|add-login-item|job-save>",
				Subcommands: []SubcommandSpec{
					{Name: "list", Summary: "List services filtered by source, state, and query", Usage: "devbase services list [--source <src>] [--state <state>] [--query <text>]"},
					{Name: "inspect", Summary: "Show details, logs, actions, and parsed launchd Spec", Usage: "devbase services inspect <id-or-name> [--source <src>]"},
					{Name: "start|stop|restart|load|unload|enable|disable", Summary: "Run lifecycle action on a service", Usage: "devbase services <action> <id-or-name> [--source <src>] [--yes]"},
					{Name: "remove", Summary: "Remove a service or launchd job", Usage: "devbase services remove <id-or-name> [--source <src>] --yes", Flags: []FlagSpec{{Name: "--yes", Type: "bool", Required: true, Description: "Confirm removal"}}, Destructive: true},
					{Name: "add-login-item", Summary: "Add an .app bundle to macOS Login Items", Usage: "devbase services add-login-item <app-path>"},
					{Name: "job-save", Summary: "Create or edit a launchd property list job from flags or JSON stdin", Usage: "devbase services job-save [--source user-agent|system-agent|daemon] [--existing <path>] [--load] (--json-stdin | --label <id> (--command <sh> | --arg <bin>))", Stdin: "optional launchd.Spec JSON"},
				},
			},
			{
				Name:    "settings",
				Summary: "Manage privileged helper, download cache, log rotation, and login menu bar item",
				Usage:   "devbase settings <status|helper|cache|log-rotation|login-item>",
				Subcommands: []SubcommandSpec{
					{Name: "status", Summary: "Show settings summary", Usage: "devbase settings [status]"},
					{Name: "helper", Summary: "Check, install, or remove the privileged helper", Usage: "devbase settings helper [status|install|uninstall --yes]", Privileged: true},
					{Name: "cache", Summary: "Inspect or clear the download archive cache", Usage: "devbase settings cache [status|clear --yes]"},
					{Name: "log-rotation", Summary: "Inspect or toggle scheduled 60s log rotation", Usage: "devbase settings log-rotation [status|on|off]"},
					{Name: "login-item", Summary: "Inspect or toggle starting Devbase in menu bar at login", Usage: "devbase settings login-item [status|on|off]"},
				},
			},
			{
				Name:    "log-rotation",
				Summary: "Alias for `devbase settings log-rotation`",
				Usage:   "devbase log-rotation [status|on|off]",
			},
			{
				Name:    "cli",
				Summary: "Install, inspect, or remove the managed `devbase` wrapper script in PATH",
				Usage:   "devbase cli <status|install|uninstall> [--dir <bin-dir>]",
				Subcommands: []SubcommandSpec{
					{Name: "status", Summary: "Inspect CLI wrapper status", Usage: "devbase cli [status] [--dir <bin-dir>]"},
					{Name: "install", Summary: "Install managed devbase wrapper script into bin directory", Usage: "devbase cli install [--dir <bin-dir>]"},
					{Name: "uninstall", Summary: "Remove managed devbase wrapper script", Usage: "devbase cli uninstall [--dir <bin-dir>]"},
				},
			},
			{
				Name:    "schema",
				Summary: "Emit complete machine-readable JSON CLI contract for AI agents and scripts",
				Usage:   "devbase schema [--command <name>]",
			},
			{
				Name:    "version",
				Aliases: []string{"--version", "-v"},
				Summary: "Show Devbase CLI version and runtime info",
				Usage:   "devbase version [--json]",
			},
			{
				Name:    "help",
				Aliases: []string{"--help", "-h"},
				Summary: "Show general or topic-specific CLI help",
				Usage:   "devbase help [<command>]",
			},
			{
				Name:    "gui",
				Aliases: []string{"open"},
				Summary: "Launch the Devbase macOS desktop window (or pass --background for menu bar only)",
				Usage:   "devbase gui [--background]",
			},
		},
	}
}

func runSchemaCommand(env *Env, args []string) error {
	if hasHelpFlag(args) {
		printTopicHelp(env.Out, "schema")
		return nil
	}
	fs := newFlagSet("schema")
	var cmdFilter string
	fs.StringVar(&cmdFilter, "command", "", "Filter schema output to one command name or alias")
	pos, err := parseFlags(fs, args)
	if err != nil {
		return err
	}
	if len(pos) > 1 {
		return usageErrorf("usage: devbase schema [--command <name>]")
	}
	if len(pos) == 1 && cmdFilter == "" {
		cmdFilter = pos[0]
	}
	schema := buildCLISchema()
	cmdFilter = strings.ToLower(strings.TrimSpace(cmdFilter))
	if cmdFilter != "" {
		var matched []CommandSpec
		for _, c := range schema.Commands {
			if c.Name == cmdFilter {
				matched = append(matched, c)
				continue
			}
			for _, a := range c.Aliases {
				if a == cmdFilter {
					matched = append(matched, c)
					break
				}
			}
		}
		if len(matched) == 0 {
			return usageErrorf("unknown command %q in schema", cmdFilter)
		}
		schema.Commands = matched
	}
	return writeJSONValue(env.Out, schema)
}

func runVersionCommand(env *Env, args []string) error {
	if hasHelpFlag(args) {
		printTopicHelp(env.Out, "version")
		return nil
	}
	if len(args) > 0 {
		return usageErrorf("unexpected argument %q for version", args[0])
	}
	payload := map[string]any{
		"name":    "devbase",
		"version": CLIVersion,
		"arch":    "darwin/arm64",
	}
	return writeResult(env, payload, func(w io.Writer) {
		fmt.Fprintf(w, "devbase %s (darwin/arm64)\n", CLIVersion)
	})
}

func runHelpCommand(env *Env, args []string) error {
	if len(args) > 1 {
		return usageErrorf("usage: devbase help [<command>]")
	}
	if len(args) == 1 {
		topic := strings.ToLower(strings.TrimSpace(args[0]))
		if !hasTopicInSchema(topic) {
			return usageErrorf("unknown help topic %q", args[0])
		}
		printTopicHelp(env.Out, topic)
		return nil
	}
	printTopicHelp(env.Out, "")
	return nil
}

func findSchemaCommand(topic string) (CommandSpec, bool) {
	topic = strings.ToLower(strings.TrimSpace(topic))
	schema := buildCLISchema()
	for _, c := range schema.Commands {
		if c.Name == topic {
			return c, true
		}
		for _, a := range c.Aliases {
			if a == topic {
				return c, true
			}
		}
	}
	return CommandSpec{}, false
}

func hasTopicInSchema(topic string) bool {
	_, ok := findSchemaCommand(topic)
	return ok
}

func printTopicHelp(w io.Writer, topic string) {
	if topic != "" {
		if cmd, ok := findSchemaCommand(topic); ok {
			fmt.Fprintf(w, "%s - %s\n\nUsage:\n  %s\n", cmd.Name, cmd.Summary, cmd.Usage)
			if len(cmd.Aliases) > 0 {
				fmt.Fprintf(w, "\nAliases: %s\n", strings.Join(cmd.Aliases, ", "))
			}
			if len(cmd.Subcommands) > 0 {
				fmt.Fprintln(w, "\nSubcommands:")
				for _, sc := range cmd.Subcommands {
					fmt.Fprintf(w, "  %-22s %s\n    %s\n", sc.Name, sc.Summary, sc.Usage)
				}
			}
			fmt.Fprintln(w, "\nGlobal flags:\n  --json                 Emit JSON output (stdout) and JSON errors (stderr)")
			return
		}
	}
	schema := buildCLISchema()
	fmt.Fprintf(w, "Devbase CLI (%s) - Local runtimes, databases, HTTPS proxy, Cloudflare tunnels, and macOS services\n\n", schema.Version)
	fmt.Fprintln(w, "Usage:")
	fmt.Fprintln(w, "  devbase <command> [subcommand] [args] [--json]")
	fmt.Fprintln(w, "  devbase                  (from Finder/Dock) launches the desktop window")
	fmt.Fprintln(w, "\nCommands:")
	for _, c := range schema.Commands {
		name := c.Name
		if len(c.Aliases) > 0 {
			name += " (" + strings.Join(c.Aliases, ",") + ")"
		}
		fmt.Fprintf(w, "  %-22s %s\n", name, c.Summary)
	}
	fmt.Fprintln(w, "\nGlobal flags & AI usage:")
	fmt.Fprintln(w, "  --json                 Structured JSON stdout and structured JSON stderr errors")
	fmt.Fprintln(w, "  devbase schema         Full machine-readable JSON specification of commands, flags, exit codes")
	fmt.Fprintln(w, "  devbase help <cmd>     Detailed usage for one command")
	fmt.Fprintln(w, "\nSecurity rules:")
	fmt.Fprintln(w, "  Never pass secrets in argv (--password/--token/--api-token fail with exit code 2);")
	fmt.Fprintln(w, "  use --password-stdin or --token-stdin over stdin.")
}

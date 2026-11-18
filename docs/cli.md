# Devbase CLI Reference

Devbase exposes its workspace, software runtimes, databases, local HTTPS proxy, optional Cloudflare tunnels, local CA trust, macOS services inspector, and settings via a non-interactive, AI-friendly CLI (`devbase`).

## Installation in `PATH`

1. In Devbase Settings (**Settings → Command-line interface**), click **Install devbase CLI**, or run directly from the app bundle:

```sh
/Applications/Devbase.app/Contents/MacOS/Devbase cli install
```

2. This installs a managed shell wrapper at `~/.local/bin/devbase` (or `$MSM_DEV_HOME/bin/devbase` during isolated testing) pointing directly to `Devbase.app/Contents/MacOS/Devbase`, preserving bundle identity for the privileged helper. Devbase refuses to overwrite or remove foreign files not created by Devbase.
3. Ensure `~/.local/bin` is in your `PATH`:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

4. Running `devbase gui` or `devbase open` (or opening `Devbase.app` from Finder/Dock with no CLI arguments or `--background`) launches the desktop window / menu bar. Running any CLI command or `--json` executes headlessly without starting the GUI window.

> Note: `Devbase --dev <internal-cmd>` remains strictly reserved for Devbase's own internal `launchd` jobs, shell shim, and privileged helper (`serve`, `tunnel`, `maintain`, `database-worker`, `node`, `helper`).

## AI Self-Discovery, `--json`, and Exit Codes

AI agents and automation scripts can inspect the entire command surface programmatically:

```sh
devbase schema
devbase schema --command db
devbase help db
```

- **Global `--json` flag**: Passing `--json` anywhere before `--` emits structured JSON on `stdout` and structured JSON errors (`{"error": "...", "exitCode": N}`) on `stderr`.
- **Exit codes**:
  - `0`: success
  - `1`: operational or runtime error
  - `2`: invalid usage, unknown command, missing confirmation flag (`--yes` / `--confirm` + `--fingerprint`), or forbidden plaintext secret flag

## Safety Rules

- **No plaintext secrets in `argv`**: Passing `--password`, `--token`, or `--api-token` fails immediately with exit code `2`. Always pipe secrets over `stdin` using `--password-stdin` or `--token-stdin`.
- **Explicit non-interactive confirmations**: Destructive operations never prompt interactively; pass `--yes` where required, or follow `--preview` -> `--confirm <id> --fingerprint <fp>` for `db delete-data`.
- **External database protection**: Instances with an external `dataDirectory` retain their data on `db remove --yes` and reject `db delete-data`.
- **Shared Homebrew keg safety**: Uninstalling a Homebrew-backed package removes only Devbase's workspace registration and leaves the shared Homebrew Cellar keg untouched.
- **No app-server supervision**: Devbase manages foundational runtimes, databases, local HTTPS forwarding, and tunnels; developers start their own upstream web/app servers.

## Command Reference

### AI Discovery & Workspace
- `devbase schema [--command <name>]` — Full JSON specification of commands, flags, stdin contracts, and exit codes.
- `devbase version [--json]` / `devbase help [<command>]`
- `devbase status [--json]` — Workspace state (`packages`, `instances`, `proxyRules`), runtime statuses, helper state, and CLI wrapper state.
- `devbase diagnostics [run|export] [--json]` — Layered health checks or redacted diagnostic export.
- `devbase tasks <list|dismiss|retry|work|resolve-work|initializations|resolve-init|deletions|finish-deletion|restore-deletion> [--json]`
- `devbase logs <proxy|tunnel|db-<id>|<instance-id>> [--stream stderr|stdout] [--lines 200] [--filter <text>] [--json]`
- `devbase ports <active|free|check <port>> [--json]`
- `devbase shutdown <preview|stop --yes> [--json]`

### Software & Runtimes
- `devbase software list [--json]`
- `devbase software versions <node|java|python|mysql|postgresql|redis|mongodb> [--refresh] [--json]`
- `devbase software review <software> <version> [--json]`
- `devbase software install <software> <version> [--json]` (alias: `devbase install <software> <version>`)
- `devbase software uninstall (<package-id> | <software> <version-or-id>) --yes [--json]`
- `devbase software verify (<package-id> | <software> <version>) [--json]`
- `devbase software repair (<package-id> | <software> <version>) [--json]`
- `devbase software default <node|java|python> <version-or-id> [--json]`
- `devbase software shell <node|java> [status|enable|disable|bash-command] [--json]`
- `devbase software which <software> [--dir <path>] [--package <id>] [--json]`
- `devbase software uv-path [<absolute-path>] [--json]`

### Databases
- `devbase db list [--json]` (alias: `devbase database ...`)
- `devbase db create <id> --kind <mysql|postgresql|redis|mongodb> [--package <pkg-id> | --version <ver>] [--port <port>] [--persistent] [--data-dir <path>] [--json]`
- `devbase db <start|stop|restart> <id> [--json]`
- `devbase db stop-all --yes [--json]`
- `devbase db remove <id> --yes [--json]`
- `devbase db delete-data <id> (--preview | --confirm <id> --fingerprint <fp>) [--json]`
- `devbase db port <id> (<new-port> | --repair) [--json]`
- `devbase db upgrade <id> [<target-pkg-or-ver>] [--targets] [--json]`
- `devbase db inspect <id> [--json]`
- `devbase db create-db <id> <db-name> [--json]` / `devbase db drop-db <id> <db-name> --yes [--json]`
- `printf '%s\n' "$PW" | devbase db create-user <id> <username> [--database <db>] --password-stdin [--json]`
- `devbase db drop-user <id> <username> [--database <db>] --yes [--json]`
- `devbase db connection <id> [--profile <profile-id>] [--database <db>] [--format uri|env|jdbc|cli] [--json]`
- `devbase db password <id> [--json]`
- `devbase db open-cli <id> [--profile <profile-id>] [--database <db>] [--print] [--json]`
- `devbase db profile <list [<id>] | save --instance <id> --name <label> [...] | delete <id> <profile-id> --yes> [--json]`
- `devbase db backup <list <id> | create <id> [--database <db>] | restore <id> <name-or-path> [--database <db>] --yes | delete <id> <name> --yes> [--json]`

### Local HTTPS Proxy, Cloudflare Tunnel & Local CA
- `devbase https <status|rules|list> [--json]` (alias: `devbase proxy ...`)
- `devbase https add <name-or-domain> <local-port> [--upstream-host <host>] [--tunnel] [--tunnel-auth] [--tunnel-worker <name>] [--tunnel-domain <host>] [--listener-port <port>] [--apply] [--json]`
- `devbase https remove <name-or-domain> [--apply] [--json]`
- `devbase https set-port <port> [--apply] [--json]`
- `devbase https replace --json-stdin [--listener-port <port>] [--apply] [--json]`
- `devbase https <apply|stop|restart> [--json]` / `devbase https check <name-or-domain> [--json]`
- `devbase tunnel <status|discover|sync|apply|stop|restart> [--json]`
- `printf '%s\n' "$CF_TOKEN" | devbase tunnel configure --token-stdin [--account-id <id>] [--client-id <id>] [--json]`
- `devbase tunnel remove-config --yes [--delete-workers] [--json]`
- `devbase tunnel check <name-or-domain> [--json]` / `devbase tunnel access-token <name-or-domain> [--rotate] [--json]`
- `devbase ca <status|trust|untrust --yes|prepare|trust-prepared|activate|discard-prepared|restore|saved-trust|remove-archived-trust> [--json]`

### macOS System Services Inspector (`services`)
- `devbase services list [--source brew|login-item|user-agent|system-agent|daemon] [--state <state>] [--query <text>] [--json]` (alias: `devbase service ...`)
- `devbase services inspect <id-or-name> [--source <source>] [--json]`
- `devbase services <start|stop|restart|load|unload|enable|disable|remove> <id-or-name> [--source <source>] [--yes] [--json]`
- `devbase services add-login-item <app-path> [--json]`
- `devbase services job-save [--source user-agent|system-agent|daemon] [--existing <path>] [--load] (--json-stdin | --label <id> (--command <sh> | --arg <bin> ...)) [--json]`

### Settings & CLI Management
- `devbase settings [status] [--json]`
- `devbase settings helper [status|install|uninstall --yes] [--json]`
- `devbase settings cache [status|clear --yes] [--json]`
- `devbase settings log-rotation [status|on|off] [--json]` (alias: `devbase log-rotation ...`)
- `devbase settings login-item [status|on|off] [--json]`
- `devbase cli <status|install|uninstall> [--dir <bin-dir>] [--json]`

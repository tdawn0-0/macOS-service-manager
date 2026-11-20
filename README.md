# Devbase

A native macOS app for verified development software, independent database instances, and local HTTPS reverse proxy rules with optional Cloudflare tunnel exposure. It also retains a separate inspector for system services.

## Development

The three main entries are **Software**, **Databases**, and **HTTPS**. Start your application in a terminal or IDE; this app does not start it.

![HTTPS rule table in the 900×520 test viewport](docs/images/development/https-rules-900x520.png)

- **Software:** install and manage Node.js, Java, Python, MySQL, PostgreSQL, Redis, and MongoDB. Java installs macOS arm64 JDK distributions from the JetBrains catalog in `~/Library/Java/JavaVirtualMachines`, uses IDEA-style names, and manages existing JDKs in that directory, Python uses the existing uv installation store (with a native file picker for custom `uv` paths), MySQL (`8.4`/`8.0`) and MongoDB (`8.0`/`7.0`) use verified upstream archives, and PostgreSQL and Redis use Homebrew core. Java and Python (uv) automatically load available versions when opened (with manual refresh kept), and picking a version in Software previews package details in the background. Archive downloads are checked against upstream hashes and cached under `~/Library/Caches/Devbase/Development` (inspectable and clearable from Settings). See the [storage layout](docs/development-runtimes-and-data.md#storage-layout) for installed packages and custom roots. Node.js and Java surface newer same-series updates for quick review. Node.js can use `.node-version`, `.nvmrc`, or `package.json` `engines.node` and Java can use `.java-version` or `.sdkmanrc` in any directory without registering it. Set a default and explicitly enable zsh, Bash, or fish integration in a new terminal.
- **Databases:** create independent MySQL, PostgreSQL, Redis, and MongoDB instances with an exact installed version, their own port, and an optional new absolute data location (or pick a parent folder). Different installed PostgreSQL major versions can coexist, and stopped MySQL, PostgreSQL, Redis, and MongoDB instances can upgrade to a newer installed same-series patch or revision release. External data is retained on Remove; permanent deletion is available only for app-managed data. Inspect live databases and users on running instances, create or drop databases and users where supported (system databases and built-in/admin users are protected), back up (with a target database selector) and restore into a prefilled or custom target database, view logs in the workspace live log drawer or in Console, open `psql`/`mysql`/`redis-cli` via **Open CLI in Terminal** using private credential files, and copy password-free URIs, engine-specific `.env` lines (`DATABASE_URL`, `REDIS_URL`, `MONGODB_URI`), JDBC URLs, or saved connection profiles for external clients. Redis uses numbered databases.
- **HTTPS:** edit one table of `Domain → Local port` rules (with a **Pick active port** menu of listening local user ports on each row). Each rule forwards to `http://127.0.0.1:<port>`, or to another host you name, through a local HTTPS proxy. Copy or open saved local URLs directly from each row, save the full table with **Save HTTPS rules**, or use **Save & apply** to save, sync Workers, and publish in one step. Domains are always `<name>.devbase.localhost`, which macOS resolves to this Mac without a hosts entry; **Local certificate trust** shows an inline CA trust badge (`Trusted`, `Not trusted`, or `Not created`) and warns if the proxy runs while untrusted, while trusting the local CA stays a separate explicit action. The upstream application stays under your control.
- **Cloudflare tunnel (optional):** turn on Tunnel for a rule to expose it publicly. Paste a masked **Cloudflare API token** (with a tooltip listing required permissions) and **Save Cloudflare settings** to auto-discover the account ID, `*.workers.dev` subdomain, and zones; **Remove Cloudflare token** stops the tunnel and clears saved Cloudflare metadata after confirmation. Saving deploys a dedicated `devbase-<name>` Worker + Durable Object for each tunneled rule on `*.workers.dev` or a Custom Domain in one of your zones, and removes Workers for rules that no longer tunnel. Rules can also enable edge access-token protection (`Protect`) stored privately in `proxy/tunnel-access.json`, show live worker status tags (**Tunnel connected**, **Tunnel connecting**, **Tunnel conflict**) while the tunnel runs, and be probed end-to-end via **Check tunnel response**. A background client on this Mac carries requests from the Worker to the rule's upstream. The API token lives in a private file (`proxy/cloudflare.key`), not in app state.

Java has separate defaults and explicit terminal integration for installed zsh, Bash, and fish (resolving `.java-version` and `.sdkmanrc` on directory change). Python requires an installed uv executable; the app can discover it or use an absolute path you select. Its global uv pin does not replace the shell’s `python` command or override a virtual environment. See [runtime and data-location usage](docs/development-runtimes-and-data.md).

The main window groups **Software**, **Databases**, and **HTTPS** under **Workspace** (accessible with `Cmd+1`–`Cmd+3`), separates **All Items** under **Inspector**, includes a live workspace log drawer (**Logs** / **Live logs**), and opens **Settings** via `Cmd+,`. Settings includes **Start at login in menu bar** (`--background` via `xyz.jyeu.devbase.login.plist`) and **Command-line interface** (installing `~/.local/bin/devbase` + copying `devbase schema`). The menu bar shows managed database instances (with **Stop all** when running), a failed-resource count in the header/tooltip when any workspace resource fails, one HTTPS proxy switch, and a Cloudflare tunnel switch once a tunnel is configured, with hover actions to copy or open single or multiple rule URLs. Closing the window leaves background jobs running and hides the Dock icon until the window opens again. Quit offers a reviewed keep/stop choice for app-owned resources. System service controls remain under their own source list.

## Installation and permissions

Requires an Apple Silicon Mac running macOS 26 or 27. Only the two latest macOS releases are tested and supported. App release artifacts are described by the build workflow; this source checkout is not itself an installed release. Work that needs root (an HTTPS listener below port 1024 such as 443, system daemons, and Homebrew services registered as root) runs through a privileged helper that the app asks you to install the first time it is needed, with one administrator password; it can also be installed from Settings. Trusting the local CA is a separate step that macOS confirms itself. It never edits the hosts file. Deleting a database instance has a separate data review and confirmation. No product operation should remove an unrelated application process or shared Homebrew keg. The helper only accepts requests from the Devbase build it was installed from, running as the installing user.

## Source and verification

The current product scope is [documented here](docs/product-scope.md). [Usage](docs/development-usage.md), [UI guidance](docs/development-ui.md), and the [acceptance matrix](docs/resources-acceptance.md) describe the new behavior. Verification evidence and missing real-machine checks are in [development-verification](docs/development-verification.md).

Building requires Rust stable and [nsh](https://nsh.dev) 0.6.1 (`brew install longbridge/tap/nsh`).

```sh
cd app && cargo test
cd app/crates/devcore && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check
cd app/crates/devproxy && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check
cd app/crates/devcli && cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check
cd app && cargo clippy --all-targets -- -D warnings && cargo fmt --check && nsh build
cd app && nsh package --target macos
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_*.py'
python3 scripts/collect-third-party-notices.py --check
```

`nsh package --target macos` writes an ad-hoc signed `.app` under `app/.nsh/out/` for local verification. A release build is signed with the Developer ID and notarized by the release workflow. Real system trust and Chrome acceptance require a separately signed App on a disposable Mac, as specified in the [acceptance plan](docs/resources-acceptance.md). Neither an ad-hoc build nor a private test CA proves that check.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). The Rust/nsh app, database workers, HTTPS proxy, and macOS service integrations are tested separately. Tests that need real launchd or database binaries must run in an isolated, clean environment.

## Command-line interface (CLI)

Every Devbase capability (Workspace status/diagnostics/logs/ports/shutdown, Software, Databases, HTTPS, Cloudflare Tunnels, Local CA, System Services, and Settings) is also available headlessly via the `devbase` CLI for scripts and AI agents (`--json` output, `devbase schema` self-discovery, `--password-stdin`/`--token-stdin`, and explicit `--yes` / `--confirm` guards). Install `~/.local/bin/devbase` from **Settings → Command-line interface** or `/Applications/Devbase.app/Contents/MacOS/Devbase cli install`, and see [CLI reference](docs/cli.md).

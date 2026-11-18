# AGENTS.md

## 1. Overview & Platform Constraints

- **Application**: Devbase is a native Apple Silicon macOS desktop application.
- **Go Module & Identity**: `module devbase`, bundle identifier `xyz.jyeu.devbase`.
- **Toolchain**: Go `1.27+` using `egoist/mygo` and `lobehub/lobe-mygo` for native macOS rendering.
- **Architecture**: Apple Silicon (`darwin/arm64`) only. Any other target fails to compile (`internal/devplatform/platform_unsupported.go`), so code never checks the host architecture at run time; it only checks that downloaded executables contain an arm64 slice.
- **OS Support**: only the two latest macOS releases, currently macOS 26 and 27. The bundle declares `minimumSystemVersion` 26.0 (`mygo.json`). Older releases are neither tested nor supported; don't add compatibility code or test paths for them. When a new macOS ships, raise the minimum to the older of the two latest releases.

## 2. Package & File Map

- **Root Desktop App**:
  - `main.go`, `app.go`: Application lifecycle, window creation, menu bar initialization, public CLI headless dispatch (`shouldRunPublicCLI` -> `devcli.RunCLI`), and main loop.
  - `editor.go`: The launchd job form's state, validation and saving (drawn by `view_editor.go`).
  - `settings.go`: Privileged helper install, reinstall and removal, public `devbase` CLI wrapper installer, and the Settings page's actions (drawn by `view_settings.go`).
  - `menubar.go`: Native menu bar panel: database instance rows, the HTTPS proxy switch, the Cloudflare tunnel switch (shown once a tunnel is configured), and status readouts.
  - `ui_kit.go`: Immediate-mode UI layout components, typography helpers, and color primitives.
  - `view_*.go`: Immediate-mode views. Development workspace: `view_development.go` (shell), `view_software.go`, `view_java.go`, `view_additional_runtimes.go`, `view_runtime_terminal.go`, `view_package_*.go`, `view_databases.go`, `view_database_*.go`, `view_instance_deletion.go`, `view_https_proxy.go` (rules, proxy and Cloudflare tunnel cards), `view_proxy_activation.go`, `view_authority.go` (local CA), `view_diagnostics.go`, `view_ports.go`, `view_operations.go`, `view_quit.go`, `view_settings.go`. System-service inspector: `view.go`, `view_detail.go`, `view_editor.go`, `view_status.go`.
  - `guide.go`, `theme.go`, `icons.go`: First-run guide, theme tokens, and embedded Lucide icons.
  - `*_test.go`: End-to-end desktop and UI lifecycle test suites, one file per feature. `main_test.go` holds the inspector harness (`newHarness`, `fake`) and the list and detail tests; `editor_test.go`, `guide_test.go`, `menubar_test.go` and `settings_test.go` cover those features.
- **`internal/devplatform/`**:
  - Software catalog, installation, health checks, and repairs for supported runtimes: Node.js, JetBrains JDKs, `uv` Python, MySQL, Homebrew PostgreSQL (16/17/18), Redis, and MongoDB.
  - Database instance orchestration, port allocation, isolated runtimes, connection profiles, backups, and restores.
  - Local HTTPS reverse proxy with SNI matching, host routing, loopback interface bindings, and generational local CA certificate management. Rule domains are always `<name>.devbase.localhost`; the hosts file is never edited.
  - Optional Cloudflare tunnel (`tunnel_*.go`, `cloudflare_api.go`, `ws.go`): per-rule `devbase-<slug>` Worker + Durable Object deployed via the Cloudflare API, on `*.workers.dev` or a Custom Domain, with a local launchd client that streams requests over WebSocket. The API token lives in `proxy/cloudflare.key` (0600), never in `state.json`.
  - Shell startup integration scripts (zsh, Bash, fish) and PATH management.
  - Diagnostics, status gathering, and file-locked transactional state persistence (`Manager.withState`/`withStateTransaction` over `state.json` + `state.lock`). The UI's own `developmentState` lives in the root package.
- **`internal/devcli/` & `cmd/devmgr/`**:
  - `internal/devcli/`: Public AI-friendly `devbase` CLI (`RunCLI`, `RunCLIWithEnv`, `schema.go`, `install_path.go`, `cmd_workspace.go`, `cmd_software.go`, `cmd_databases.go`, `cmd_https.go`, `cmd_services.go`, `cmd_settings.go`) plus internal `--dev` subcommands via `RunInternal` (`serve`, `tunnel`, `maintain`, `database-worker`, `node`, `helper` — `--dev` refuses any non-internal command). All public CLI operations support `--json` and `devbase schema`, forbid plaintext secrets in `argv`, and require explicit `--yes` or `--confirm`/`--fingerprint` flags for destructive actions. See [docs/cli.md](docs/cli.md).
  - `cmd/devmgr/`: Unshipped developer test harness for headless verification (`scripts/verify-native-release.py`), fixtures, and diagnostics via `devcli.Run`.
- **Inspector & System Packages**:
  - `internal/launchd/`: Native macOS launchd job discovery, property list parsing, and lifecycle control.
  - `internal/service/`: High-level service abstractions across background daemons.
  - `internal/brew/`: Homebrew formula/keg detection and inventory helpers.
  - `internal/loginitem/`: macOS Login Items inspection and user preference queries.
  - `internal/logs/`: Bounded tail reads of log files (`Stat`, `Tail`), used by the log viewer.
  - `internal/system/`: OS utilities and command execution (`system.Run`, replaceable in tests).
  - `internal/privhelper/`: Privileged helper that every root operation goes through (privileged proxy listeners, system daemons, root Homebrew services); there is no fallback to a per-action password prompt. Only installing or removing the helper asks for the administrator password, through its unexported `runAdmin` (tests set `ClientConfig.RunAdmin`), so no other package can reach a password prompt. The app offers to install it when first needed, or from Settings. It serves only the Devbase build it was installed from, running as the installing user.

## 3. Verification Gates (Run Before Finishing Code Changes)

Run the full suite of verification commands before marking any feature or fix complete:

```sh
go test ./...
go test -race ./...
go vet ./...
go mod verify
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_*.py'
python3 scripts/collect-third-party-notices.py --check
go tool mygo build -sign - -skip-notarize
```

For fast local iteration on specific packages during active development:
```sh
go test -race ./internal/devplatform -run '^Test...' -count=1
go test ./internal/system ./internal/logs
```

## 4. Commands NEVER to Run on a Developer Machine

- **`python3 scripts/verify-native-release.py --run`**:
  - **NEVER** run this command on a personal developer machine.
  - It performs host-level mutations and is reserved strictly for disposable GitHub-hosted macOS runners (`README.md:38`).
- **Host System Mutations in Automated Tests**:
  - **NEVER** mutate host `/etc/hosts` directly in tests.
  - **NEVER** modify the user login keychain or install real local CA trust anchors during tests.
  - **NEVER** write into `~/Library/Java/JavaVirtualMachines` or real user `~/Library/LaunchAgents`.
  - Always isolate tests using `t.TempDir()`, configure `MSM_DEV_HOME`, and mock `system.Run` or `Manager.Run`.
- **Documentation Screenshot Environment Variable**:
  - **NEVER** set `MSM_SHOTS` during standard test runs.
  - Set it only when deliberately regenerating screenshots, to a directory under the ignored `build/` directory; each test writes to `$MSM_SHOTS/<test name>/` via `saveShot` (`shots_test.go`). Commit an image only when a document links to it.

## 5. Non-Negotiable Architecture & Safety Invariants

- **UI Threading Model**:
  - All `app` and `developmentState` reads and mutations must occur on the main UI thread.
  - Background asynchronous tasks must run in `a.spawn(...)`.
  - Background tasks must deliver state updates back to the UI thread exclusively via `a.post(...)`.
  - Never execute synchronous disk I/O, network requests, or child processes directly inside `view_*.go` render functions.
- **Data & Process Safety**:
  - Removing a database instance must always retain external data directories.
  - Permanent deletion of external data (`Instance.DataDirectory != ""`) is deliberately unsupported to safeguard developer assets (`CONTRIBUTING.md:11`).
  - Never delete shared Homebrew kegs or terminate unowned process groups.
- **Product Scope Boundaries**:
  - Upstream web and application servers are started by the developer in their own terminal or IDE, never spawned or supervised by Devbase.
  - Devbase manages foundational runtimes, databases, the local proxy, the optional Cloudflare tunnel, and development CAs.
  - Go runtime management was intentionally removed (`a86aac5`); PHP, automatic project registration, and legacy state migrations are out of scope (`docs/product-scope.md:19-21`).
- **Credentials & License Notices**:
  - Never log, export, or pass plaintext passwords via process command-line arguments (`argv`).
  - Never store plaintext credentials inside exported `ConnectionProfile` structs.
  - Whenever dependencies change in `go.mod`, execute `python3 scripts/collect-third-party-notices.py` to update notices and review diffs in `licenses/`.

## 6. Commit Style

Follow Conventional Commits conventions for clear changelogs:
- `fix(ui): ...`
- `fix(proxy): ...`
- `feat(menubar): ...`
- `feat(database): ...`
- `test(devplatform): ...`
- `chore(licenses): ...`
- `docs: ...`

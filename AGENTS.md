# AGENTS.md

## 1. Overview & Platform Constraints

- **Application**: Devbase is a native Apple Silicon macOS desktop application.
- **Go Module & Identity**: `module devbase`, bundle identifier `xyz.jyeu.devbase`.
- **Toolchain**: Go `1.27+` using `egoist/mygo` and `lobehub/lobe-mygo` for native macOS rendering.
- **Architecture**: Apple Silicon (`darwin/arm64`) only. Intel (`darwin/amd64`) builds and packages are rejected by design.
- **OS Requirement**: macOS 13.0 (Ventura) or newer.

## 2. Package & File Map

- **Root Desktop App**:
  - `main.go`, `app.go`: Application lifecycle, window creation, menu bar initialization, and main loop.
  - `menubar.go`: Native menu bar panel interface, quick service toggles, and status readouts.
  - `ui_kit.go`: Immediate-mode UI layout components, typography helpers, and color primitives.
  - `view_*.go`: Immediate-mode views for development status, services, databases, proxy, logs, and authority.
  - `*_test.go`: End-to-end desktop and UI lifecycle test suites.
- **`internal/devplatform/`**:
  - Software catalog, installation, health checks, and repairs for supported runtimes: Node.js, JetBrains JDKs, `uv` Python, MySQL, Homebrew PostgreSQL (16/17/18), Redis, and MongoDB.
  - Database instance orchestration, port allocation, isolated runtimes, connection profiles, backups, and restores.
  - Local HTTPS reverse proxy with SNI matching, host routing, loopback interface bindings, and generational local CA certificate management.
  - Shell startup integration scripts (zsh, Bash, fish) and PATH management.
  - Diagnostics, status gathering, and file-locked transactional state persistence (`Manager.withState`/`withStateTransaction` over `state.json` + `state.lock`). The UI's own `developmentState` lives in the root package.
- **`internal/devcli/` & `cmd/devmgr/`**:
  - `internal/devcli/`: Internal `--dev` subcommands (`serve`, `maintain`, `database-worker`, `node`). The shipped `.app` has no public user-facing CLI.
  - `cmd/devmgr/`: Unshipped developer test harness for headless verification, fixtures, and diagnostics.
- **Inspector & System Packages**:
  - `internal/launchd/`: Native macOS launchd job discovery, property list parsing, and lifecycle control.
  - `internal/service/`: High-level service abstractions across background daemons.
  - `internal/brew/`: Homebrew formula/keg detection and inventory helpers.
  - `internal/loginitem/`: macOS Login Items inspection and user preference queries.
  - `internal/logs/`: Bounded tail reads of log files (`Stat`, `Tail`), used by the log viewer.
  - `internal/system/`: OS utilities, command execution, and privileged `osascript` authorization wrappers.

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
  - It performs host-level mutations and is reserved strictly for disposable GitHub-hosted macOS runners (`README.md:37`).
- **Host System Mutations in Automated Tests**:
  - **NEVER** mutate host `/etc/hosts` directly in tests.
  - **NEVER** modify the user login keychain or install real local CA trust anchors during tests.
  - **NEVER** write into `~/Library/Java/JavaVirtualMachines` or real user `~/Library/LaunchAgents`.
  - Always isolate tests using `t.TempDir()`, configure `MSM_DEV_HOME`, and mock `system.RunAdmin` or `Manager.Run`.
- **Documentation Screenshot Environment Variables**:
  - **NEVER** set `MSM_README_SHOTS`, `MSM_DEV_SHOTS`, or `MSM_ADDITIONAL_SHOTS` during standard test runs.
  - Use these variables only when deliberately regenerating documentation assets.

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
  - Devbase manages foundational runtimes, databases, the local proxy, and development CAs.
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

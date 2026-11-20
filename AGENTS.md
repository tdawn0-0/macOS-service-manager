# AGENTS.md

## 1. Overview & Platform Constraints

- **Application**: Devbase is a native Apple Silicon macOS desktop application.
- **Crate & Identity**: workspace `nsh-app-devbase`, bundle identifier `xyz.jyeu.devbase`, packaged from `app/`.
- **Toolchain**: Rust stable (edition 2024) with [Native Shell](https://nsh.dev) `nsh` 0.6.1 (`brew install longbridge/tap/nsh`); the `nsh-*` crates are pinned to 0.6.1 in `app/Cargo.toml`.
- **Architecture**: Apple Silicon (`darwin/arm64`) only. The macOS host is the only target built, packaged and tested, so code never checks the host architecture at run time; it only checks that downloaded executables contain an arm64 slice.
- **OS Support**: only the two latest macOS releases, currently macOS 26 and 27. The bundle declares `LSMinimumSystemVersion` 26.0 (`app/nsh.json`). Older releases are neither tested nor supported; don't add compatibility code or test paths for them. When a new macOS ships, raise the minimum to the older of the two latest releases.

## 2. Package & File Map

- **`app/` — the nsh app** (`nsh.json` `"root": "App"`; pure Rust, no TSX and no QuickJS):
  - `nsh.json`, `theme.json`: Bundle name, identifier, version, nsh and Rust roots, window size, macOS minimum 26.0 and the Apple Events entitlement; theme tokens (shadcn `cssVars`) ported from the old Go theme.
  - `Cargo.toml`, `src/lib.rs`, `src/main.rs`: The app crate, the root `App` component, and the `cargo run` entry.
  - `src/shell.rs`, `src/components/`: The window shell and navigation rail, shared components, and the inline Lucide icon set.
  - `src/pages/`: One module per screen — system-service inspector, development workspace, software, databases, HTTPS proxy/activation/authority, diagnostics, settings.
  - `src/store/`: Signal-backed state stores, one per feature (services, software, databases, development, proxy, system/settings).
  - `src/menubar.rs`, `src/guide.rs`, `src/launch.rs`, `src/meta.rs`, `src/theme.rs`, `src/logs.rs`: Menu-bar/tray panel, first-run guide, same-binary public CLI and `--dev` dispatch, build metadata, typed theme bindings, and the log drawer.
  - `tests/`: Headless shell tests driven through `nsh-testing`.
- **`app/crates/devcore/`**: System core — launchd job discovery/control, plist parsing, file-locked transactional state (`state.json` + `state.lock`), operations/engine/health/journal, software catalog/install/repair, database instances/backups/restores, shell integration, diagnostics, and the privileged helper.
- **`app/crates/devproxy/`**: Local HTTPS reverse proxy (SNI/host routing, loopback bindings), generational local CA management, and the optional Cloudflare tunnel with its Worker + Durable Object and WebSocket client. Rule domains are always `<name>.devbase.localhost`; the hosts file is never edited.
- **`app/crates/devcli/`**: The public AI-friendly `devbase` CLI (`--json`, `schema`, `--yes`/`--confirm` gates, no plaintext secrets in `argv`) and the internal `--dev` commands (`serve`, `tunnel`, `maintain`, `database-worker`, `node`, `helper`). See [docs/cli.md](docs/cli.md).
- **Repository**: `docs/` product and verification documentation; `scripts/` packaging/release/licence tooling; `licenses/` generated third-party notices; `assets/` icon sources plus asset licences; `resources/` the packaged icon PNGs.

## 3. Verification Gates (Run Before Finishing Code Changes)

Run the full suite of verification commands before marking any feature or fix complete. The app and each crate under `app/crates/` are standalone Cargo workspaces with their own `Cargo.lock`.

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

The notice check reads the fetched crates.io sources: run `cargo fetch --locked` in `app/` and in each crate first. `nsh package` writes the `.app` under `app/.nsh/out/`; the gates never install or launch it.

For fast local iteration on specific crates during active development:
```sh
cd app/crates/devcore && cargo test <filter>
cd app/crates/devproxy && cargo test <filter>
```

## 4. Commands NEVER to Run on a Developer Machine

- **Host System Mutations in Automated Tests**:
  - **NEVER** mutate host `/etc/hosts` directly in tests.
  - **NEVER** modify the user login keychain or install real local CA trust anchors during tests.
  - **NEVER** write into `~/Library/Java/JavaVirtualMachines` or real user `~/Library/LaunchAgents`.
  - Always isolate tests using `tempfile`/`TempDir`, configure `MSM_DEV_HOME`, and install fake providers or command runners instead of touching launchd, Homebrew or the keychain.
- **Packaging and Launch**:
  - **NEVER** run `nsh package --target macos --run`, install the packaged app, or register launchd jobs from an automated test. Packaging and launch stay a manual gate.

## 5. Non-Negotiable Architecture & Safety Invariants

- **UI Threading Model**:
  - All UI state reads and mutations must occur on the main UI thread.
  - Background asynchronous tasks must run through nsh tasks (`spawn(...)`), never inline in a binding or render.
  - Background tasks must deliver state updates back to the UI thread exclusively through signals (`Signal::set`/`update`; nsh tasks set signals).
  - Never execute blocking disk I/O, network requests, or child processes directly inside a render function.
- **Data & Process Safety**:
  - Removing a database instance must always retain external data directories.
  - Permanent deletion of external data (`Instance.data_directory != ""`) is deliberately unsupported to safeguard developer assets (`CONTRIBUTING.md:11`).
  - Never delete shared Homebrew kegs or terminate unowned process groups.
- **Product Scope Boundaries**:
  - Upstream web and application servers are started by the developer in their own terminal or IDE, never spawned or supervised by Devbase.
  - Devbase manages foundational runtimes, databases, the local proxy, the optional Cloudflare tunnel, and development CAs.
  - Go runtime management was intentionally removed (`a86aac5`); PHP, automatic project registration, and legacy state migrations are out of scope (`docs/product-scope.md:19-21`).
- **Credentials & License Notices**:
  - Never log, export, or pass plaintext passwords via process command-line arguments (`argv`).
  - Never store plaintext credentials inside exported `ConnectionProfile` structs.
  - Whenever dependencies change in a `Cargo.lock`, run `cargo fetch --locked` and execute `python3 scripts/collect-third-party-notices.py` to update notices and review diffs in `licenses/`.

## 6. Commit Style

Follow Conventional Commits conventions for clear changelogs:
- `fix(ui): ...`
- `fix(proxy): ...`
- `feat(menubar): ...`
- `feat(database): ...`
- `test(devplatform): ...`
- `chore(licenses): ...`
- `docs: ...`

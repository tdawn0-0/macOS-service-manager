# Devbase → Native Shell (nsh) migration plan

Decision (owner-approved): **full rewrite of the Devbase UI stack from Go/mygo to
[longbridge/nsh](https://github.com/longbridge/nsh), written entirely in Rust** — the
`--template rust` app form: typed components and styles on the native reactive engine,
no TSX and no QuickJS anywhere. Go is removed at cutover. The Go app stays buildable
until Phase 5 cutover, then is deleted.

Migration surface (to be ported, then deleted):

| Go package | LOC | nsh target |
|---|---|---|
| root app (`app.go`, `view_*.go`, `editor.go`, `settings.go`, `menubar.go`, `ui_kit.go`, `theme.go`, `guide.go`, `icons.go`) | 19,134 | `app/` Rust components + `nsh/desktop` (menu bar, tray, app menu) |
| `internal/launchd/` | 2,909 | Rust module `devcore::launchd` |
| `internal/devplatform/` | 32,726 | Rust modules `devcore::*` (software, databases, proxy, CA, tunnel, state) |
| `internal/privhelper/` | 2,637 | Rust privileged helper (Phase 4 design) |
| `internal/devcli/` + `cmd/devmgr/` | 6,064 | Rust CLI crate (Phase 4) |
| `internal/{brew,loginitem,logs,service,system}` | 2,591 | Rust modules of `devcore` |

Total: 66,261 LOC across 241 Go files.

## Target architecture

- **Rust crates** hold all system interaction: launchd, brew, process control, state
  persistence (`state.json` + lock), the local CA, the proxy, the tunnel client. Rust code
  runs on the engine thread pool as plain Rust — no JS in the core.
- **Rust components** hold all UI, following `examples/tasks-rs`/`gallery-rs` conventions:
  typed style methods, `signal`/`computed`/`effect` for state, a store module per feature,
  components in their own modules. No TSX, no QuickJS.
- **Desktop chrome** via `nsh/desktop` (Rust API): app menu, tray (menu bar panel),
  global shortcuts — replaces `menubar.go`.
- **Packaging**: `nsh package --target macos`, identifier `xyz.jyeu.devbase`,
  `minimumSystemVersion 26.0` (AGENTS.md: two latest macOS releases only).

## Directory layout (created by Phase 0)

```
app/                 # nsh app, --template rust (a Cargo project)
  nsh.json           # name Devbase, identifier xyz.jyeu.devbase, macOS >= 26, "root" names the root component
  Cargo.toml         # the app crate; devcore as a path dependency or workspace member
  src/lib.rs         # root component: window shell + nav sidebar + theme
  src/screens/       # one module per screen (port of view_*.go)
  src/store/         # signal-backed state stores
  src/theme.rs       # tokens ported from theme.go
  crates/devcore/    # system core (launchd, brew, state, ...)
```

## Phases

### Phase 0 — Scaffold (this session)
- `nsh init` `--template rust`; `nsh.json` per repo identity.
- App shell: native window, sidebar navigation with sections (Services, Development,
  Databases, HTTPS Proxy, Diagnostics, Settings) as placeholders, theme tokens ported from
  `theme.go` into typed Rust styles.
- Verify: `nsh build --target macos` passes; `nsh run --target macos` opens a window.
- No Go edits.

### Phase 1 — System-service inspector end-to-end (this session)
- Rust: port `internal/launchd` (job discovery, plist parsing, lifecycle control,
  status) to `devcore::launchd`, behind the `internal/service` model (Item/Source/State/
  Action/Detail) so later sources (brew, loginitem) drop in.
- UI: port `view.go` (list pane, rows with state icon + hover action), `view_detail.go`
  (state chip, action row, autostart card, info tab with copy buttons, log tab),
  `view_status.go` as Rust components wired to a `store/services.rs` signal store.
- Scope note: Phase 1 serves the three launchd sources (user agents, system agents,
  daemons). Brew and login-item sources are Phase 2 additions to the same provider
  model — no stubs: sections absent until their providers land.
- Verify: `cargo test` on devcore with the Go package's plist fixtures;
  `nsh run --target macos` lists real launchd jobs and start/stop works.

### Phase 2 — devplatform core (largest; several passes)
- `devcore::state`: file-locked transactional persistence (`state.json` + `state.lock`).
- `devcore::software`: catalog, install, health, repairs (Node, JDKs, uv, MySQL, Postgres,
  Redis, MongoDB) via brew + downloads; arm64-slice checks. Includes the brew/login-item
  service sources completing the inspector.
- `devcore::databases`: instances, ports, isolated runtimes, connection profiles,
  backups/restores. Keep invariant: never delete external data directories.
- Screens: development workspace, software, databases + editor form.

### Phase 3 — Proxy, CA, tunnel
- `devcore::proxy`: local HTTPS reverse proxy, SNI/host routing, loopback bindings,
  generational local CA; `<name>.devbase.localhost` rules; hosts file untouched.
- `devcore::tunnel`: Cloudflare Worker + DO deployment, WebSocket client; token in
  `proxy/cloudflare.key` (0600), never in state.
- Screens: proxy rules, activation, authority, tunnel cards.

### Phase 4 — Privileged operations + CLI
- Rust privileged helper replacing `internal/privhelper`: same launchd-based install, one
  admin-password prompt at install/remove only, serves only the Devbase build it came from.
- Rust CLI keeping the public `devbase` contract: `--json`, schema, `--yes`/`--confirm`
  gates, argv never carries secrets.
- Settings screen + helper install flow.

### Phase 5 — Cutover
- Delete all Go (`*.go`, `go.mod`, `go.sum`, `mygo.json`, `cmd/`, `internal/`), the Rust
  app becomes the repo's product.
- Replace verification gates: `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --check`, `nsh build`, `nsh package --target macos`, Python notice script
  retargeted to the Cargo.lock files.
- Rewrite AGENTS.md sections 1–4 for the Rust stack; keep safety invariants verbatim.

## Invariants carried over unchanged

- Removing a database instance never deletes external data directories; permanent deletion
  of external data stays unsupported.
- No plaintext passwords in argv; no plaintext credentials in exported profiles; Cloudflare
  token never in `state.json`.
- Privileged ops only through the helper; password prompt only at helper install/remove.
- Never delete shared brew kegs or kill unowned process groups.
- Upstream app servers stay developer-managed; no Go-runtime/PHP/legacy migration scope.

## Risks

- nsh v0.6 released 2026-11-18; API churn possible — pin the nsh version in CI, read
  nsh-compat before upgrading.
- Menu-bar/tray and login-item fidelity depend on `nsh/desktop` coverage; gaps found in
  Phase 1 are fed back into Phase 4 design instead of blocking.
- `devbase` CLI must keep its public contract byte-for-byte on `--json` output or callers
  (AI agents) break; Phase 4 ports against `docs/cli.md` as spec.
- The Rust-only form (no QuickJS) is nsh's preferred mode and removes the JS engine risk
  entirely, at the cost of hot TSX updates during development (cargo rebuilds only).

## Progress ledger (2026-11-20; cut over 2026-11-21)

Every slice below is `cargo test`/`clippy -D warnings`/`fmt --check` green in its crate.
The Rust app is now the repository's product; the Go tree was deleted at cutover.

| Slice | Where | Tests | Notes |
|---|---|---|---|
| Scaffold (nsh `--template rust`, theme, sidebar, packaging) | `app/` | 2 shell + 1 store | `nsh build` clean; `nsh run --target macos` opens the window (CGWindowList verified) |
| launchd core (`internal/launchd` port) | `app/crates/devcore/src/{service,launchd}` | 69 | exact launchctl argv, plist/calendar/edit/root behaviors pinned |
| Inspector UI (Services page) | `app/src/pages/services/`, `app/src/store/services.rs` | 8 + logs | real run listed 6 user agents through the store; privileged failures surface the helper-not-installed message |
| State persistence (schema 2, flock, atomic write) | `app/crates/devcore/src/{model,fs,state}` | +21 | old-schema byte preservation, 2-writer concurrency, symlink/size refusal |
| Operations/engine/health/shutdown | `app/crates/devcore/src/{operations,engine,health,shutdown}` | +34 | journal leases, `OperationActions` seam |
| Software layer (§2.2) | `app/crates/devcore/src/software/` | +25 | staged install with undo, arm64 fat/thin fixtures, brew receipt drift |
| Databases foundation (§9 item 8) | `app/crates/devcore/src/databases/` | +24 | external-dir retention, rollback leaves no partial instance, argv has no passwords |
| DB operations (§9 items 9–11) | `app/crates/devcore/src/databases/` | +10 | backup/restore per engine, DB-work guard, journal executor, recovery/upgrade |
| Proxy/CA/tunnel (Phase 3 core) | `app/crates/devproxy/` | 97 | hand-written reverse proxy (101 tunneling, SSE flush verified), generational CA, full tunnel stack; Phase-4 socket seam returns `Unsupported` |
| Software screen (UI) | `app/src/pages/software/`, `app/src/store/software.rs` | 7 | install/uninstall/review/verify/repair flows with confirm dialogs |

## Cutover record (2026-11-21)

Phase 5 is complete. Deleted at the repository root: all 77 root `*.go` files (the app, the
UI/e2e suites and `shots_test.go`), `internal/` and `cmd/` (241 tracked Go files in total),
`go.mod`, `go.sum`, `mygo.json`, and `entitlements.plist`. `scripts/verify-native-release.py`
and its tests were deleted with the Go release harness.

The Rust app under `app/` is the product. `app/crates/devcore`, `app/crates/devproxy` and
`app/crates/devcli` remain standalone workspaces; the app depends on them by path. The app
crate was made `clippy -D warnings`/`rustfmt` clean at cutover with mechanical fixes only
(redundant closures, three type aliases, formatting). The verification gates are now:

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

`scripts/collect-third-party-notices.py` now derives its inventory from `app/Cargo.lock` and
`app/crates/*/Cargo.lock`, reading the fetched crates.io registry offline (run
`cargo fetch --locked` in each workspace first). `licenses/` records every production crate's
declared licence and copied notice files, replacing the Go module and Go toolchain notices.
CI runs the cargo gates, `nsh build`, `nsh package --target macos`, the Python tests and the
notice check; the release workflow signs and notarizes the packaged app.

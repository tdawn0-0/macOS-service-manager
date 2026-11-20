# devcore

Devbase's system core in pure Rust. The first module is the port of the Go
`internal/service` + `internal/launchd` packages; later phases add brew, state,
databases, proxy and tunnel to the same crate.

The crate is standalone (its own `[workspace]`) and is not yet a dependency of
the nsh app; the app integration lands separately.

## Module map

| Path | Contents |
|---|---|
| `src/service.rs` | Shared model: `Source`, `State`, `Autostart`, `Item`, `Action`, `Row`, `LogFile`, `Detail`, the object-safe `Provider`/`Batcher` traits, and `has`/`toggle`/`primary`. |
| `src/launchd/provider.rs` | `Context`, `Config`, `LaunchdProvider` (`list`, `detail`, `actions`, `confirm`, `do_action`, `do_all`, `save_as_root`), `SaveOutcome`, and the `Launchd` facade for the three launchd sources. |
| `src/launchd/parse.rs` | `parse_print`, `DomainState`, `Job`; exact tab-based parsing of `launchctl print`. |
| `src/launchd/plist_file.rs` | Internal `FileModel`: strict decoding of the keys the app reads, `scheduled`/`keep_alive`/`disabled_in`/`runs`, `problem`. |
| `src/launchd/spec.rs` | `Spec`, `load_spec`, `validate`, `marshal`, `changes`, `save_file`, managed-path checks, shell/env/list helpers. |
| `src/launchd/calendar.rs` | `Calendar`, `entries`, `calendar_from`, the 1000-entry limit. |
| `src/launchd/login_agent.rs` | Login-agent label/path/target, executable resolution, enable/disable. |
| `src/launchd/host.rs` | Injected `ProcessRunner`, `Host`, `PrivilegedHelper`/`HelperApi`, the `SystemRunner`/`SystemHost` implementations, and the helper request types. |
| `src/launchd/error.rs` | `LaunchdError` and `ErrorCode`, preserving the Go message fragments. |

Tests live beside the code (`tests_provider.rs`, `tests_edit.rs`,
`tests_calendar.rs`, `tests_root.rs`, `tests_login.rs`) and use the fakes in
`test_support.rs`. They never spawn `launchctl`, `plutil`, `id` or `dscl`, and
never write to a real LaunchAgents directory or `~/.Trash`.

## Public API the UI consumes

```rust
use std::sync::Arc;
use devcore::launchd::{
    Action, Context, Detail, Item, Launchd, LaunchdProvider, PrivilegedHelper,
    SystemHost, SystemRunner, UnavailableHelper,
};
use devcore::service::Provider;
```

- `Launchd::new(ctx)` registers `UserAgent`, `SystemAgent` and `Daemon`; iterate
  `Source::ALL` for display order and use
  `launchd.provider(source)` / `launchd.list_all()`.
- Per-provider calls: `list()`, `detail(&item)`, `actions(&item)` and
  `confirm(&item, action)` are cheap/pure enough to run while rendering;
  `do_action(&item, action)` and `do_all(&items, action)` touch the system.
- `actions` rows follow the Go matrix; the main action is
  `service::primary(&actions)`, the autostart toggle is `service::toggle`.
- `Item`/`Detail`/`Row`/`LogFile` derive serde for diagnostics and persistence.
- `LaunchdProvider::save_as_root(&spec, existing, load)` returns `SaveOutcome`:
  `load_error: Some(..)` means the file is in place but launchd did not load or
  unload it. User-agent edits use the free `save_file`.
- `UnavailableHelper` reports `ErrorCode::HelperNotInstalled`; the UI should
  offer helper installation, then retry. There is no password-prompt fallback.

## Verify

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```

## State persistence (Phase 2)

The port of the Go `internal/devplatform` persistence layer. The state document
is still `state.json` schema 2 beside `state.lock`, so the GUI and CLI can share
one workspace during the phased cutover.

| Path | Contents |
|---|---|
| `src/model.rs` | `Package`, `Instance`, `ConnectionProfile`, `ProxyRule`, `CloudflareZone`, `CloudflareDnsRecord`, `TunnelWorkerState`, `State`; strict serde contract, `Package::validate`, `normalize_proxy_rules`, `validate_tunnel_state`, profile validation, `local_domain`/`default_tunnel_worker_name`. |
| `src/fs.rs` | `read_regular_file` (`O_NOFOLLOW`, regular-file and size checks), `sync_directory`, `digest_bytes`/`valid_digest` (SHA-256), `atomic_write`, `clean_path`/`safe_relative`/`inside`. |
| `src/state/mod.rs` | `Store` (`read`, `write_state`, `with_state`, `with_state_transaction`, `init`), `Tx` undo/commit hooks, the `state.lock` exclusive flock, and the schema-version gate. |
| `src/state/paths.rs` | `MSM_DEV_HOME`/`default_root`, user-storage detection, cache/download/Java directories, package directories. |
| `src/state/package_locks.rs` | `PackageLease`: per-package `LOCK_SH`/`LOCK_EX` on `package-locks/<id>`; exclusive acquisition is try-only and returns `PackageBusyError`; shared acquisition waits. |
| `src/state/resource_locks.rs` | `ResourceKey`/`ResourceLease`: try-only `LOCK_EX` on `resource-locks/<sha256("kind:id")>`, sorted and deduplicated, `ResourceBusyError`. |
| `src/state/error.rs` | `StateError`, `PackageBusyError`, `ResourceBusyError`, preserving the Go message fragments. |
| `src/state/tests.rs` | Schema rejection, unknown-field/secrets rejection, atomic concurrent writers, lease contention, symlink/size refusal, paths. |

Guarantees pinned by the tests:

- A document whose `schemaVersion` is not 2 is rejected before any field is
  decoded and is never rewritten (there is no migrator in Phase 2).
- Unknown fields are rejected at every nesting level; credentials/tokens
  therefore cannot enter the document. Reads are lock-free; writes take the
  exclusive `state.lock` flock and publish with a same-directory rename.
- Leases are real `flock(2)` locks shared across processes, never in-process
  mutexes. A state-lock holder must use try-only exclusive package leases, as
  in the Go code.
- Every test uses a `tempfile::TempDir` root and an explicit home; the only
  environment mutation sets `MSM_DEV_HOME`/`HOME` to temp paths under a mutex.

```rust
use devcore::model::State;
use devcore::state::{ResourceKey, Store, default_root};

let store = Store::new(default_root());
let state: State = store.read()?;
store.with_state(|state| {
    state.proxy_rules_revision += 1;
    Ok(())
})?;
let _lease = store.package_lease("node@22.14.0-arm64", false)?;
let _reserved = store.resource_leases(&[ResourceKey::new("TCP port", "5432")])?;
# Ok::<(), devcore::state::StateError>(())
```

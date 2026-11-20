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

## Operations, engine, health and shutdown (Phase 2)

The execution half of the Go `internal/devplatform` layer. The journal is
cross-process (a real `flock(2)` owner lease per operation), the launchd engine
takes an injected [`CommandRunner`], and the health/shutdown loops preserve the
Go timing and failure rules.

| Path | Contents |
|---|---|
| `src/operations.rs` | `OperationRequest`/`OperationRecord` (Go field names, schema 1), `OperationJournal` (`run_operation`, `install`, `repair_package`, `create_instance`, `backup`, `restore`, `instance_action`, `create_database`, `create_user`, `operations`, `dismiss_operation`, `retry_operation`), the `OperationActions` executor seam, `OperationScope`, `operation_phase`/`operation_work_phase`, `operation_can_retry`, and `operations/<16hex>/{record.json,owner.lock}`. |
| `src/engine.rs` | `CommandRunner`/`SystemCommandRunner` (five-minute deadline), `Engine` (`domain`, `label`, `target`, `start_job`/`start_job_policy`, `stop_job`, `run_command`), `JobSpec`, `JobSavedHook`, `ProcessRunnerAdapter`, and the `gui/<uid>` label derivation from the cleaned root's SHA-256. |
| `src/health.rs` | `JobState`/`parse_job_state`, `validate_health`, `HealthProbe`/`NetworkProbe`, `probe_health`, `wait_health`, `read_job_state`, `ServiceProbe` and `wait_service` (stable PID for 200 ms, 100 ms observations, default 30 s readiness). |
| `src/shutdown.rs` | `Shutdown` (`preview`, `execute`, `verify_target`), `ShutdownPlan`/`ShutdownResource`/`ShutdownOptions`, `shutdown_fingerprint`; instance leases held while only `db-<id>` jobs stop. |

Guarantees pinned by the new tests:

- `record.json` is `0600` and never contains progress text, command output,
  raw errors, or a `create_user` password; fixed result strings are the only
  outcome text.
- A live operation is `Busy` through its owner flock; another journal cannot
  dismiss or retry it. A released owner with a still-`Running` record is
  reported as `Interrupted`, and retry lineage links parent `Followup` to the
  child record.
- Jobs are labelled `xyz.jyeu.devbase.dev.<root-hash-prefix>.<id>` in
  `gui/<uid>`; stop waits for both the launchd record and the owned PID.
- Shutdown rejects a stale state fingerprint, takes try-only instance resource
  leases, stops only managed `db-<id>` jobs, and never treats a failed launchctl
  read as proof of absence.
- All tests use temp roots and fake command runners/probes; nothing touches real
  `launchctl`, launchd directories, or network endpoints.

## Software catalog, install, integrity and Homebrew (Phase 2)

The port of the Go `catalog.go`, `packages.go`, `package_details.go`,
`package_integrity.go`, `package_repair.go`, `package_swap_darwin.go`,
`homebrew.go`, `cache.go` and `architecture.go`. The install pipeline is
unchanged: download to `downloads/<sha256>`, stage beside the destination,
verify (SHA-256, traversal, symlink escape, arm64 slice), publish with
`renamex_np(RENAME_SWAP|RENAME_EXCL)`, and register inside a state transaction
whose undo restores the previous tree and index.

| Path | Contents |
|---|---|
| `src/software/mod.rs` | `Software` facade over `Store` + injected `CommandRunner`/`HttpTransport`, `SoftwareError`, `CurlTransport` (HTTPS-only redirects, 2 GiB cap), `version_less`, `canonical_inside`, digest helpers. |
| `src/software/catalog.rs` | Exact MySQL/MongoDB release pins, `available_versions` (MySQL, MongoDB, PostgreSQL, Redis, Node), Node `SHASUMS256.txt` resolution through the HTTP seam, manifest validation. |
| `src/software/archive.rs` | tar.gz/zip extraction with the Go safety rules: backslash/absolute/`..` rejection, symlink escape and chain rejection, expanded-size cap, symlinks created last, entry-size mismatch detection. |
| `src/software/macho.rs` | Thin/fat Mach-O header reader; `require_apple_silicon` accepts scripts and arm64 slices, rejects Intel-only binaries before publish. |
| `src/software/packages.rs` | `stage_package`, `install`, `uninstall`, `binary`, Java ownership marker/next folder, launchd/database idle checks, publication with transactional rollback, Node default link. |
| `src/software/integrity.rs` | `PackageTree`/`PackageFile` schema 1, `scan_package_tree` (kinds/modes/sizes/digests/symlink targets, 100k entry and 8 GiB caps), `encode_package_tree`, `verify_package`/`verify_package_in_state` with `Verified`/`Needs repair`/`Not recorded`/`Homebrew managed`. |
| `src/software/repair.rs` | `repair_package`: exclusive lease, refuses Homebrew and live references, re-stages the exact manifest, publishes transactionally. |
| `src/software/swap.rs` | `renamex_np` FFI: `RENAME_SWAP` exchange and `RENAME_EXCL` exclusive publish; no non-atomic fallback. |
| `src/software/homebrew.rs` | `resolve_homebrew` (`brew info --json=v2`), exact stable version/revision and `homebrew/core` tap checks, `INSTALL_RECEIPT.json` validation, `brew install --formula --force-bottle`, registration. |
| `src/software/details.rs` | `PackageDetails` preview, installed/cached checksums, integrity hints, references, `resolve_installed_or_catalog`. |
| `src/software/cache.rs` | `DownloadCacheInfo`/`clear_download_cache`; counts only regular files, refuses while any package lease is exclusively held. |
| `src/software/tests.rs` | Fake runner/HTTP, zip/tar/Mach-O fixtures, rollback/cancellation, traversal+link rejection, integrity statuses, Homebrew receipt/tap tests, cache lock refusal. |

Guarantees pinned by the tests:

- Staging is beside the destination; a failure during extraction, verification
  or the state write leaves no package directory, index or registration.
- Publication uses `renamex_np` exclusively; repair rollback restores both the
  old tree and the old index, and the old tree is never erased when rollback
  itself fails.
- Every declared binary must resolve inside the tree, be regular/executable,
  and contain an arm64 slice for thin or fat Mach-O.
- The integrity index records no timestamps/xattrs; its digest lives only in
  `state.json`, so index tampering is detected.
- Cache clearing refuses with `PackageBusyError` while an exclusive package
  lease is held; shared leases (running databases) do not block it.

All tests use temp roots and fake command/HTTP seams; nothing runs `brew`,
`launchctl`, `sw_vers`, or touches the network.

## Database instances: locations, ports, lifecycle and initialization (Phase 2)

The first database slice, ported from the Go `database_locations.go`,
`ports.go`, `database.go`, `instance_initialization.go`,
`instance_deletion.go`, `connection_profiles.go` and `connections.go`. It is
the foundation the remaining database screens build on; backup/restore and
background database work land separately.

| Path | Contents |
|---|---|
| `src/databases/mod.rs` | `Databases` facade over `Store` + `Engine` + `Software`, `DatabaseError`, injected `PortBinder`/`HealthProbe`, and the staging-area initialization. |
| `src/databases/locations.rs` | `database_data_directory`/`config_directory`, external-location validation (`plain_path`, overlap and ancestor checks), the `dev:ino` identity, `.service-manager-owner` preparation and verification. |
| `src/databases/ports.rs` | `PortBinder`/`SystemPortBinder`, `free_instance_port`/`free_database_port` preflight, `inspect_port`, `listening_user_ports`, `available_local_port`, and the bounded `lsof`/`netstat` parsers. |
| `src/databases/init.rs` | `InitializationRecord`, staged creation under `initializations/<random>/data`, per-job plist digests, PostgreSQL/MySQL/MongoDB/Redis initialization, and the transactional `rename` publication with undo. |
| `src/databases/lifecycle.rs` | `InstanceAction` (`start`/`stop`/`restart`/`remove`), start arguments per engine, readiness waits, shutdown-target verification, and removal that unregisters without deleting data. |
| `src/databases/deletion.rs` | Deletion preview with tree identity/digest and fingerprint, `deletions/<random>/data` staging inside the state transaction, identity-checked erasure, and the external-data refusal. |
| `src/databases/mongo.rs` | `MongoAdmin` seam and `WireMongoAdmin`: a minimal OP_MSG/BSON `createUser` command over loopback (the MongoDB package ships no shell), with a loopback-server test for the exact bytes. |
| `src/databases/profiles.rs` | Credential-free profiles and URIs, `with_database`/`EnvAssignment`/`JDBCURL`, password-file reads with the 4096-byte limit, and the Terminal launcher script that reads the secret inside the shell. |
| `src/databases/tests.rs` | Fake launchd runner, port binder and probe; creation/start/stop/restart/remove, rollback, lease contention, deletion staging and credential assertions. |

Guarantees pinned by the tests:

- Creation holds the `database instance:<id>` and `TCP port:<port>` resource
  leases (plus `database locations:creation` for external data) and the
  shared package lease. Data is built beside the destination and published
  with `rename` inside the state transaction; the undo restores the staging
  path, so a failed write leaves no registered or live instance.
- Every initialization and lifecycle job is recorded with its plist SHA-256;
  only jobs this operation wrote are stopped. A changed plist fails the
  operation instead of being booted out.
- `remove` deletes only the generated plist and registrations. Internal data
  stays under `instances/<id>`; an external `DataDirectory` and its ownership
  marker are never touched. Permanent erasure is a separate, typed-ID and
  fingerprint-confirmed operation, internal data only.
- Port preflight runs before every create and start and reports
  `PortUnavailableError`; selection skips configured instance/proxy ports and
  returns a kernel-assigned free port.
- Passwords are 32 hex characters in `instances/<id>/password` (0600);
  MySQL's `client.cnf` is also 0600. State, profiles and job arguments never
  contain a password: the tests scan every recorded argv, the generated
  plists and the MongoDB wire command seam for the generated secret. MySQL
  SQL and MongoDB user creation read the secret from a 0600 file/client
  configuration or send it inside the protocol, never in `argv`.
- Profiles are credential-free and engine-validated (`PostgreSQL`/`MySQL`
  reject Mongo/Redis fields, Mongo requires `authSource`, Redis accepts only
  the `default` user and databases 0–15).

All tests use a `tempfile::TempDir` root and fake command/bind/probe seams;
nothing runs `launchctl`, `brew`, `initdb`, `mysqld`, `mongod`, `redis-server`
or touches the network.

## Database data operations, recovery and upgrades (Phase 2)

The second database slice ports `connections.go`, `backups.go`,
`database_inventory.go`, `database_drop.go`, `database_work.go`,
`database_other.go`, `mongodb_backup.go`, `redis.go`, the recovery paths of
`instance_initialization.go`/`instance_deletion.go` and `instance_upgrade.go`.

| Path | Contents |
|---|---|
| `src/databases/backups.rs` | Finished-backup listing and deletion. Names must match `<instance>-<database>-<16 hex>.dump` and survive the longer-instance-ID check. |
| `src/databases/inventory.rs` | `InstanceCatalog`, protected database/user lists, `inspect_instance_catalog`, `drop_database`, `drop_user` (PostgreSQL, MySQL, MongoDB; Redis refuses drops). |
| `src/databases/data.rs` | `create_database`, `create_user`, staged `backup`/`restore` for PostgreSQL/MySQL/MongoDB/Redis, the shared `with_database_operation` lease/snapshot wrapper, and `database_command`/`mysql_sql`. |
| `src/databases/database_work.rs` | Durable `database-work/<id>/record.json` reservations, the pending-task listing, and `install_database_work_guard`, which installs the cross-process idle check on `Store`'s `database instance` leases. |
| `src/databases/actions.rs` | `DatabaseActions`, the `OperationActions` executor for the journal; the journal records the backup artifact and never output or credentials. |
| `src/databases/redis.rs` | In-process RESP client behind the `RedisAdmin` seam, logical JSON backups, empty-target restore with TTL replay and `FLUSHDB` rollback. |
| `src/databases/mongo/mongo_data.rs` | Authenticated OP_MSG (SCRAM-SHA-256) inventory, drops, create user/database and logical backup/restore behind `MongoAdmin`. |
| `src/databases/upgrade.rs` | `patch_series`, `upgrade_targets`, `newer_listed_patch`, and `upgrade_instance_package` (same series, same arch, higher patch/revision; stopped instance; MySQL clean-shutdown check). |

Guarantees pinned by the tests:

- Backups stage as 0600 `<final>.partial`, fsync, rename into `backups/`, then
  become the operation's artifact. A failure removes the partial and never
  registers a path.
- PostgreSQL tools receive the admin password through a single-use 0600
  `pgpass` file whose path is in `PGPASSFILE`; MySQL uses the instance's
  `client.cnf`; a new role's password is SQL on stdin. Recorded argv never
  contains a secret, and the journal record never contains one either.
- A `database-work` record refuses every later `database instance:<id>` lease
  from any process until it is resolved; the refusal is typed busy.
- Initialization recovery stops only digest-recorded jobs and archives the
  prepared data under `retained-initializations/<id>`; it never deletes it.
  Deletion recovery restores staged data when removal did not commit, and
  `finish_instance_deletion` erases only the reviewed revision after the typed
  confirmation.
- Upgrades never touch data, port or credentials; they change the saved
  package version and remove the generated plist so the next start rewrites it.

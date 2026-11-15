# Spike: Keeping the Database Catalog Current and Moving an Instance to a Newer Patch Release

**Context & Status**: Design spike; no code changes. Baseline: `b7cf66b`. Upstream facts were fetched on **2026-11-15** (access date for every URL in section 3). Respects `docs/product-scope.md` ("verified versions", no in-place **major** upgrade) and `catalog.go:32-34`. Where a fact could not be confirmed from a fetched page it is marked **not confirmed**; nothing in section 3 comes from memory.

## 1. Problem & Evidence

Devbase installs exactly one version each of MySQL, Redis and MongoDB, and the version, URL, SHA-256 and minimum macOS are source literals:

- `internal/devplatform/catalog.go:49-61` MySQL `8.4.4` (`cdn.mysql.com/archives/...macos15-arm64.tar.gz`, `MinMacOS 15.0`); `catalog.go:63-75` Redis `7.4.7` taken from the Redis Stack Server `7.4.0-v8` zip (`MinMacOS 14.0`); `catalog.go:76-87` MongoDB `8.0.5` (`MinMacOS 13.0`); `catalog.go:143-157` `AvailableVersions` repeats the same literals for the picker (`view_software.go:123`; `view_software.go:209` also defaults to the first entry).
- A new upstream patch reaches users only through a new app release. The fetched Redis 7.4 release notes show how fast that falls behind: 7.4.7 shipped November 2025; 7.4.8 (Feb 2026), 7.4.9 (May 2026, three CVEs incl. remote code execution), 7.4.10 (July 2026) and 7.4.11 (Aug 2026, "Update urgency: SECURITY") followed (source S-R4). MySQL's download page offers 8.4.13 LTS against the shipped 8.4.4 (S-M1). MongoDB's current server version is shown as 9.0.8 (S-G1); the 8.0 series is what Devbase pins, and its latest 8.0.x patch is **not confirmed** (only the first 100,000 characters of the release JSON were read).
- An instance is bound to the exact package ID: `Instance.Package` (`model.go:110`, `Package.ID()` = `name@version-arch`, `model.go:45`). No `Manager` method changes it (the port API is `ChangeInstancePort` / `RepairInstancePort`, `database_port_change.go:60,66`). A grep of non-test code for `.Package = ` finds no assignment to an existing instance's package.
- Node.js is already dynamic (`catalog.go:101-137`, `node_releases.go`), PostgreSQL resolves through Homebrew (`homebrew.go`), Java through `java_catalog.go`. The three archive databases are the outliers.

### 1.1 How a package and an instance are coupled (code map at `b7cf66b`)

Every reader of `Instance.Package`:

| Where | What it does |
|---|---|
| `database.go:91-122` `instanceStartArguments` | Resolves `postgres` / `mysqld` / `mongod` / `redis-server` / `valkey-server` via `m.Binary(s, i.Package, ...)`; MySQL also passes `--basedir=` `m.packageDir(s.Packages[i.Package])` (`database.go:105`). These are the launchd job arguments. |
| `database.go:61-88` `startInstance` | Calls `instanceStartArguments` then `startJob` (`engine.go:38-78`). `startJobPolicy` first runs `launchctl print`; if the job is loaded it only `kickstart`s and **does not rewrite the plist** (`engine.go:45-49`). Otherwise it rewrites `jobs/<label>.plist` via `launchd.Save` (`engine.go:62`). `stopJob` boots the job out (`engine.go:80-100`) but leaves the plist file. So after a stop, the next start regenerates arguments from the instance's current package; a still-loaded (not running) job would keep old arguments. |
| `database.go:29,45-55` | Initial creation: `initdb` for PostgreSQL; for Redis/Valkey only checks the binary exists and writes `instances/<id>/valkey.conf` (no package path inside: `bind`, `port`, `dir`, `requirepass`, persistence lines). |
| `database_other.go:33-54,80,129,178` | MongoDB/MySQL creation (`mysqld --initialize-insecure --basedir=<package dir> --datadir=...`), `mysql` client for SQL, `mysqldump` for backup. |
| `database.go:250,371` | Tool lookup for other operations (`psql`, etc.). |
| `database_operations.go:38`, `database.go:159`, `database_work.go:256`, `instance_initialization.go:214` | `packageLease(ctx, i.Package, false)` shared lease while the instance is started/worked on; repair (`package_repair.go:110`) and uninstall (`packages.go:320`) take it exclusively (`package_locks.go:25`). |
| `database_work.go:138` `requirePackageDatabaseWorkIdle` | Refuses package changes while an unfinished database task references the package. |
| `instance_initialization.go:226,313` | Initialization captures the package and re-checks `reflect.DeepEqual(latest.Packages[i.Package], pkg)` inside the state transaction. |
| `packages.go:325` `Uninstall` | Refuses with "package is used by instance X"; also scans `jobs/*.plist` for any argument under the package directory (`packages.go:328-340`) and refuses ("package is referenced by a launchd job"). |
| `package_repair.go:23` `requirePackageIdle` | Refuses repair while any instance using the package has its job loaded. |
| `package_details.go:112`, `instance_deletion.go:164`, `view_databases.go:130` | Display only (version shown in "Endpoint & storage"). |

Files inside an instance directory (`instances/<id>/`) generated from the package: `valkey.conf` (Redis/Valkey; contains no package path), `client.cnf` (MySQL client config; host/port/credentials, no package path, `database_other.go:72-74`), `password`. MongoDB keeps no config file; its arguments are only in the launchd plist. The **launchd plist** in `jobs/` is the one artifact that embeds the package path, and it is regenerated on the next non-loaded start. Backups are logical (`mysqldump`, Mongo and Redis tool paths: `database_other.go:124-150`, `mongodb_backup.go:29`, `redis.go:164`), written to a caller-chosen backup path, not copies of the data directory.

## 2. Scope Guardrails

- `docs/product-scope.md`: "Software installs and manages **verified** versions"; "There is no automatic adoption, relocation, or in-place **major** upgrade." Nothing there excludes patch-level upgrades, and "Explicitly outside scope" lists cloud sync and PHP only for related topics.
- `catalog.go:32-34`: "Returned manifests contain exact hashes and are persisted with the installed package; no floating versions are stored."
- `CONTRIBUTING.md`: "Keep software source checks exact: Node.js, Java ..., MySQL, Redis, and MongoDB use verified archives".
- `AGENTS.md`: removing an instance always retains external data; never delete shared Homebrew kegs; no legacy state migrations; `state.json` is decoded strictly (a new field makes the file unreadable by older builds); UI work off the main thread goes through `a.spawn` / `a.post`; no I/O or network in `view_*.go` render functions.
- `docs/third-party-software.md:15`: Redis instances use the distribution's `redis-server`, not Valkey, and do not auto-enable Stack modules.

## 3. Version Metadata Sources

All fetched 2026-11-15. "Not confirmed" means the fetched page did not state it.

| Engine | Machine-readable macOS arm64 index | SHA-256 published | Minimum macOS | Same-series data-directory reuse (vendor statement) |
|---|---|---|---|---|
| MySQL (8.4 LTS) | **Not confirmed.** The download page (S-M1) is an HTML form; its macOS arm64 listing for 8.4.13 LTS was not rendered in the fetched content (only 26.10.0 was shown). No JSON/API index found. | **No.** Page lists MD5 and a GnuPG signature link only (S-M1). | Archive name is `...macos15-arm64` (S-M1, shown for 26.10.0). Supported-platforms page: "macOS (Latest 2 major releases)", arm64 only, no version numbers (S-M3). Catalog's `15.0` for 8.4.4 is therefore **consistent with the archive name only**. | **Supported.** Upgrade-paths table row "Within an LTS or Bugfix series: 8.0.37 to 8.0.41 or 8.4.0 to 8.4.4" lists in-place upgrade (S-M2). In-place procedure: set `innodb_fast_shutdown` to 0 or 1, shut down the old server, start the new binary on the same datadir; the server upgrades the data dictionary at startup (S-M4). No separate upgrade tool is required by that page; it recommends MySQL Shell's Upgrade Checker. Downgrade row "Within an LTS series: 8.4.y LTS to 8.4.x LTS" lists in-place downgrade with no notes (S-M5). |
| Redis (Redis Stack Server zip, `7.4.0-v8`) | **Not confirmed.** `packages.redis.io/redis-stack/` returned HTTP 403 (S-R3). The Redis Stack macOS docs describe only Homebrew casks (S-R1). The current Redis install page lists Homebrew and Docker for macOS and no archive (S-R5); the downloads page names no macOS archive (S-R2). | **Not confirmed** for the Stack zip. The downloads page shows a `.SHA256SUM` file only for the stable *source* tarball (S-R2). | Redis Open Source "latest version" tested on macOS 14.8.4, 15.7.4, 26.3, Intel and ARM (S-R5). Nothing stated for the Stack Server zip; catalog's `14.0` is **not confirmed** by a fetched page. | **Not confirmed.** The upgrade page (S-R6) is a stub about upgrading to 8.x. Persistence docs (S-R7) state nothing about version compatibility except that "Since Redis 7.0.0, Redis uses a multi part AOF mechanism". The 7.4 release notes list 7.4.1 to 7.4.11 as patch releases with security/bug fixes (S-R4); the only mention of a file-format change is "Hash - expiration of individual fields: RDB file format changes" between 7.4 RC2 and GA. Downgrade: **not confirmed**. |
| MongoDB (8.0) | **Yes.** `https://downloads.mongodb.org/current.json`: object with `versions[]`; each has `version`, `downloads[]` with `arch`, `target` (`macos`), `edition`, and an `archive` object (`url`, `sha1`, `sha256`) (S-G1). Only the first version (9.0.8) was read; whether older 8.0.x entries are present in this file or only in a full-release index is **not confirmed**. | **Yes** (`archive.sha256`, S-G1; e.g. the 9.0.8 enterprise arm64 entry carries a 64-hex value). The Community download page itself shows no checksum (S-G2). | Community platform support: "macOS 15+ / arm64" for 9.x, 8.x and 7.x (S-G3), applying to the OS version at installation. Catalog `MinMacOS 13.0` for 8.0.5 **disagrees** with this page. | **Not confirmed as a stated patch rule.** The 7.0-to-8.0 standalone page contains no 8.0.x-to-8.0.y section (S-G4). It states: FCV must be managed (`setFeatureCompatibilityVersion`), and downgrade is single-version only and, once FCV is changed, "you cannot downgrade the binary version without support assistance". Binary-only patch moves leave FCV unchanged by construction of that text, but no page fetched says so for patches. Treat as **not confirmed** until the maintainer checks MongoDB's 8.0 revision-upgrade guidance. |

Sources (all accessed 2026-11-15):

- S-M1 https://dev.mysql.com/downloads/mysql/ and https://dev.mysql.com/downloads/mysql/?os=33
- S-M2 https://dev.mysql.com/doc/refman/8.4/en/upgrade-paths.html
- S-M3 https://www.mysql.com/support/supportedplatforms/database.html
- S-M4 https://dev.mysql.com/doc/refman/8.4/en/upgrade-binary-package.html
- S-M5 https://dev.mysql.com/doc/refman/8.4/en/downgrading.html
- S-R1 https://redis.io/docs/latest/operate/oss_and_stack/install/archive/install-stack/mac-os/
- S-R2 https://redis.io/downloads/
- S-R3 https://packages.redis.io/redis-stack/ (HTTP 403)
- S-R4 https://redis.io/docs/latest/operate/oss_and_stack/stack-with-enterprise/release-notes/redisce/redisce-7.4-release-notes/
- S-R5 https://redis.io/docs/latest/operate/oss_and_stack/install/install-stack/
- S-R6 https://redis.io/docs/latest/operate/oss_and_stack/install/upgrade/
- S-R7 https://redis.io/docs/latest/operate/oss_and_stack/management/persistence/
- S-G1 https://downloads.mongodb.org/current.json
- S-G2 https://www.mongodb.com/try/download/community
- S-G3 https://www.mongodb.com/docs/community-platform-support/
- S-G4 https://www.mongodb.com/docs/manual/release-notes/8.0-upgrade-standalone/

Consequences: only MongoDB has a documented machine-readable index with SHA-256; MySQL publishes MD5 + GPG only, so a SHA-256 would have to be computed by Devbase maintainers and pinned; Redis has no confirmed index or SHA-256 for the archive format Devbase uses. Two catalog `MinMacOS` values (Redis 14.0, MongoDB 13.0) are not supported by what was fetched and should be re-verified regardless of this spike.

Reviewer note: S-G4 and S-M2 were read through a summarising fetch tool; check the quoted lines against the vendor pages before relying on them.

## 4. Catalog Options and Recommendation

**A. Keep constants, add tooling.** Versions stay in source, several patch versions per series may be listed. A script (unshipped, like `cmd/devmgr`) regenerates URL, SHA-256 and min macOS; a test asserts every listed version resolves and validates (`Package.Validate`). Verification stays exactly as today (hashes compiled into the notarized app). Cost: every update needs an app release; MySQL and Redis SHA-256 must be computed by whoever runs the script from the downloaded archive (the vendor does not publish one), recorded together with the GPG check for MySQL. Offline: no network needed to list. A version removed from the list: already-installed packages stay in `state.json` with their own persisted manifest (`catalog.go:32-34`), so they keep working and stay repairable only if the URL/hash persisted in state remain valid; `AvailableVersions` simply stops offering new installs.

**B. Upstream metadata at run time** (as Node.js). Qualifies today: **MongoDB only** (S-G1). MySQL and Redis do not qualify (no SHA-256, no confirmed index). Behavior: `AvailableVersions` would need a cached async list like `nodeReleases` (`view_software.go:117-120`, loader at `view_software.go:231`), `Resolve` would fetch the entry and takes `sha256` from the response; stale/offline shows the cached list with a warning, error shows no list. Trust: the hash comes over TLS from the same host family as the archive (`fastdl.mongodb.org` vs `downloads.mongodb.org`); a compromise of that origin changes both. This is weaker than compiled-in hashes and equals the trust the app already gives Node.js.

**C. Devbase-published manifest** (e.g. a release asset). Covers all three engines uniformly and lets maintainers pin hashes they computed. Needs an authenticity mechanism: a signature verified with a public key compiled into the app (preferred; keeps "hash authority is the notarized app" for the root of trust) rather than relying on TLS to GitHub alone. Cost: key management, a publishing workflow, and a freshness/rollback rule (signed monotonically increasing sequence + expiry). Stale: cached last-good manifest. Disappearing version: same as A.

**Recommendation: A first, then B for MongoDB only if wanted; defer C.** A ships value alone, keeps the current trust model and the strict-hash rule in `CONTRIBUTING.md`, and fits the fact that two of three vendors publish no SHA-256. Multiple patch versions per series (e.g. `8.4.4` and `8.4.13`) is what makes the upgrade operation in section 5 possible at all. **Evidence that would change this**: MySQL or Redis publishing SHA-256 plus an index (then B for those); the maintainer wanting updates between app releases more than the strict compiled-in trust model (then C with signatures); a confirmed machine-readable index for the Redis Stack zip.

## 5. Patch-Upgrade Operation

Proposed `Manager.UpgradeInstancePackage(ctx, expected Instance, target string) error` (name is a proposal).

**Series.** MySQL: `major.minor` of the LTS (8.4.x). MongoDB: `major.minor` (8.0.x). Redis: `major.minor` (7.4.x). Defined by parsing `Package.Version` exactly (`x.y.z`, as `exactRuntimeVersion` does in `catalog.go:159`); same `Name`, same `Arch`, same `x.y`, and strictly greater `z`. PostgreSQL (Homebrew formula major) and Valkey are out of scope (see section 6 for Valkey).

**Preconditions** (all inside the state transaction unless noted):
1. `expected == s.Instances[id]` (same pattern as `database_port_change.go:89`); `verifyDataLocation` passes (`database_port_change.go:86`).
2. Target package is installed (`s.Packages[target]`), integrity-verified via `VerifyPackage` (`package_integrity.go:155`) before the transaction starts, and `pkg.Validate()` passes.
3. Same engine and series; target version greater. **Downgrades are rejected by default**: MySQL documents in-place downgrade within an LTS series (S-M5), but MongoDB and Redis are not confirmed, and a downgrade after the server upgraded on-disk metadata is the data-loss path.
4. Instance job not loaded: use `verifyShutdownTarget(ctx, domain()+"/"+label("db-"+id))` exactly as the port change does (`database_port_change.go:98`); the UI offers "Stop and upgrade", which stops through the existing instance action first. No `requirePackageIdle` conflict: that check is only for repair/uninstall.
5. Resource leases for the instance (`resourceLeases`, as `database_port_change.go:76`) and a shared `packageLease` on the target; no unfinished database work for the instance (`requirePackageDatabaseWorkIdle` applies to the old package).

**Backup first.** Required, using the existing logical `Backup` (`operations.go:426`): it writes to the backup path and never touches or deletes the data directory, so it works for external data directories (`Instance.DataDirectory != ""`). Confirmation text offers it on by default; the operation records the backup name and path in the result. A pure binary swap changes no data layout until the new server starts, but a started new server may upgrade on-disk metadata (MySQL data dictionary at startup, S-M4), after which rollback to the old binary is not guaranteed; the backup is the rollback.

**Order of changes and rollback.**
1. Take backup (read-only for the data dir). Failure: abort, nothing changed.
2. `withStateTransaction`: re-check preconditions, set `i.Package = target`, `s.Instances[id] = i`, return an `undo` hook that restores the old value (the transaction hook pattern of `database_port_change.go:81-165`). Failure saving state: hook runs, state unchanged. Only a state value changes; no file is rewritten because the plist is regenerated at next start (section 1.1).
3. Remove nothing. The old package remains installed; Uninstall remains the user's separate action and is refused while any instance still uses it (`packages.go:325`) or a plist references it (`packages.go:328-340`).
4. The stale plist: because the job is booted out and not loaded, the next `startInstance` rewrites it with `instanceStartArguments` (`database.go:91`). To avoid a window where `jobs/*.plist` still points at the old package, the operation may delete only the instance's own generated plist after confirming the job is not loaded.
5. Optional "start after upgrade": call the normal start; if it fails (health probe `waitService`, `database.go:83`), set `Package` back to the old ID in a second transaction, report the error, and point to the backup. Do not auto-retry. MySQL note: S-M4 says a failed dictionary upgrade reverts, and the old server can be started again after removing redo logs; Devbase should not do that removal for the user.
6. MySQL only: before first start of the new binary, the server wants a slow/fast shutdown (S-M4). Devbase's stop path should be confirmed to cause a clean InnoDB shutdown; if the previous stop was a kill, require the old server to start once and stop cleanly first (open question 4).

State format: unaffected. `Instance.Package` already exists (`model.go:110`); only its value changes, so strict decoding by older builds still works. An older build opening this state would, however, find the target package ID in `state.json`'s `Packages` map only if the newer build installed it there; an older app lacking that catalog entry still reads the persisted manifest (`catalog.go:32-34`).

**UI entry point.** Instance page, "Endpoint & storage" card (`view_databases.go:129-133`), beside the `Version` row: an "Upgrade to 8.4.13..." button shown only when a same-series newer installed package exists. Confirmation: "Upgrade `<id>` from MySQL 8.4.4 to 8.4.13? Devbase will stop the database, take a backup, and start it with the new version. Your data directory is not copied or deleted. After the new version has started, going back to 8.4.4 may not be possible; the backup is your way back." Run via `a.spawn` and report via `a.post` like other database operations (`view_databases.go:148`).

**Telling the user a newer patch exists.** The Software page already lists installed packages (`view_software.go:102-107`). The set of offered versions comes from `AvailableVersions` (a pure in-process list under option A), so "newer patch available" is computed in the state-refresh path, not in render code: when the `developmentState` is rebuilt, derive per-package `NewerPatch string` from `AvailableVersions` and installed versions, and render only that field. Under options B/C the network fetch belongs in the existing background loader pattern (`loadNodeReleases`, `a.spawn` then `a.post`, stale flag as at `view_software.go:118`). No network call in `view_*.go`.

## 6. Valkey

What exists: catalog entry `9.1.2` (`catalog.go:88-100`, universal binary, `MinMacOS 13.0`, BSD-3-Clause) from a GitHub release of a third-party repository (`github.com/alhassanaraouf/Valkey.app`), with a pinned SHA-256; engine code for kind `valkey` in `database.go:19,41,113`, `connections.go:46`, `database_locations.go:17`, `database_other.go:125,149`, `database_port_change.go:92,116`, `instance_initialization.go:196`, with tests (`database_port_change_test.go:37`).

What is missing for the UI: `isDatabaseEngine` (`view_databases.go:36-42`), the "Database engine" choice (`view_databases.go:227`), a Software page entry, `AvailableVersions("valkey")` (`catalog.go:143-157` has no case), `docs/product-scope.md` (lists no Valkey) and `docs/third-party-software.md:15` (states Valkey is not used).

What removing it deletes: one catalog case (`catalog.go:88-100`) and the `|| i.Kind == "valkey"` branches above plus Valkey subtests. No state migration is implied by the product rules; any existing `valkey` instance could not be started once the catalog case goes (the installed package manifest is persisted, so it would still run; only new installs are blocked).

Provenance: the archive is not from the Valkey project's own release channel (not verified in this spike; the URL points at a personal repository), which sits uneasily with "verified sources" (`docs/product-scope.md`, `CONTRIBUTING.md`) and `docs/source-provenance.md`, which records provenance for downloaded software only generically. **Recommendation: remove Valkey** (dead, undocumented, third-party-sourced, contradicts `third-party-software.md:15`), unless the maintainer wants Valkey as a product feature, in which case add it only after identifying an upstream-controlled or Homebrew-core source.

## 7. Open Questions for the Maintainer

1. **Catalog option** — A, B (MongoDB only), or C? Default: A.
2. **Allow downgrades?** Default: no, same-series upgrade only.
3. **Is a backup mandatory before upgrade?** Default: yes, on by default and the operation refuses if backup fails (user may skip only for empty instances).
4. **MySQL clean shutdown**: does the Devbase stop path guarantee a clean InnoDB shutdown before the swap? Default: require it, verify in the follow-on, and run a real MySQL test on a CI runner only.
5. **Re-verify `MinMacOS`** for Redis (14.0) and MongoDB (13.0) against vendor pages (section 3 disagrees for MongoDB: macOS 15+). Default: fix before adding versions.
6. **Redis archive source**: keep the Redis Stack `7.4.0-v8` zip, or move to a source the vendor documents for macOS? Default: keep until a documented archive source with a checksum is identified; do not add Redis patch versions without an authoritative hash source.
7. **Valkey**: remove or promote? Default: remove.
8. **Old package after upgrade**: auto-offer uninstall, or leave it? Default: leave it; show it as unused on the Software page.
9. **MongoDB FCV**: confirm from vendor revision-upgrade guidance that a binary-only patch move needs no FCV action. Default: block until confirmed.

## 8. Follow-on Implementation Plan

Independently shippable, in order:

1. **Catalog tooling and multi-version listing (option A).** `AvailableVersions` returns more than one patch per series for MySQL/MongoDB (Redis only if open question 6 resolved); an unshipped script/`devmgr` subcommand regenerates entries. Tests: every listed version resolves and passes `Package.Validate`; `AvailableVersions` entries match `Resolve` cases; unlisted version errors; existing install of a no-longer-listed version still reads/repairs.
2. **Fix `MinMacOS` facts** (question 5). Tests: a table test of min macOS per package against the documented values.
3. **"Newer patch available" indicator.** Computed in the state refresh, rendered from a plain field. Tests: pure function for same-series newer detection (equal, older, other series, malformed version); a UI test that render code makes no manager call (follow existing view test style).
4. **`UpgradeInstancePackage` in `internal/devplatform`.** Series parser, preconditions, backup, transactional switch with undo. Tests (under `t.TempDir()`, `MSM_DEV_HOME`, mocked `Manager.Run`): refuses running instance, different series, downgrade, uninstalled or unverified target, changed `expected`; success switches `Package` and leaves `DataDirectory` untouched including an external directory; save failure triggers undo; failed start reverts `Package`; old package cannot be uninstalled while an instance uses it, and can after; strict state decoding unchanged (golden `state.json` round trip).
5. **UI entry point.** Upgrade button and confirmation in "Endpoint & storage"; busy handling through `a.spawn`/`a.post`. Tests: button shown only with a newer installed same-series package; confirmation text; result toast; no render-time I/O.
6. **Valkey decision** (remove or promote) as its own change with its own tests and a `docs/product-scope.md` update.
7. **Optional: MongoDB upstream index (option B).** Cached loader with stale handling mirroring Node. Tests: parse a fixture of `current.json`, stale/offline fallback, SHA-256 required and length-validated.

Proposed wording for `docs/product-scope.md` (not applied here): "An existing database instance can be moved to a newer installed patch release of the same engine and series after a backup. Moving between series (major or minor) and downgrades are not supported."

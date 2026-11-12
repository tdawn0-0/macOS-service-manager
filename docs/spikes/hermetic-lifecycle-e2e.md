# Spike: Hermetic Lifecycle and UI Recovery E2E Test Fixtures

## Problem Statement & Purpose

During `go test ./...` in development and PR CI (`.github/workflows/ci.yml`), **30 test events skip** because `MSM_UI_E2E_PACKAGE_CACHE` or `MSM_UI_E2E_HOMEBREW` is unset (`docs/development-verification.md:25,37`). While tests exercising real database engines genuinely require upstream binaries, **13 UI suites (18 test events)** validate Devbase's own internal lease contention, crash recovery, dirty draft isolation, deletion review modals, and task cancellation state machines. This spike audits all skip sites, establishes a three-bucket taxonomy, specifies a hermetic fixture architecture reusing existing devplatform archive patterns, and guarantees non-substitution under `CONTRIBUTING.md:7`.

## Three-Bucket Classification

1. **Bucket A (Opt-in Screenshot Generators)**: Tests that only execute when requested to write images to disk for documentation (`MSM_SHOTS`, `MSM_README_SHOTS`). Retain as opt-in skips.
2. **Bucket B (True Native Binary & Release Gates)**: Tests requiring real upstream Node/Redis/MySQL/PostgreSQL/MongoDB binaries, Homebrew kegs, launchd sessions, or platform features. Required by `scripts/verify-native-release.py` or system environment. MUST NOT be substituted with mocks or synthetic stubs (`docs/resources-acceptance.md:3`).
3. **Bucket C (UI State-Machine, Lease & Recovery Suites)**: Tests validating Devbase UI flows, background task cancellation, crash recovery, draft isolation, and lease contention where the archive is merely a byte payload or loopback responder. Primary candidates for hermetic execution.

## Complete Inventory of Current Skip Sites

The table below lists every `t.Skip` site found in root `*_test.go` and `internal/**/*_test.go`, cross-referenced against `scripts/verify-native-release.py` required events (`CORE`, `MATRIX`, `NATIVE`):

| Test Name / Subtest | File:Line | Gating Condition / Env Var | Native Gate Required | Bucket | Purpose & Boundary |
|---|---|---|---|---|---|
| `TestUITwoPostgreSQLInstancesKeepSameNamedDatabasesIsolated` | `database_operations_e2e_test.go:140` | `MSM_UI_E2E_HOMEBREW != "1"` | No | B | Validates true PostgreSQL multi-instance database isolation |
| `TestUISoftwareInstallAndDatabaseWorkflow` | `development_e2e_test.go:206` | `MSM_UI_E2E_PACKAGE_CACHE == ""` | No | B | End-to-end install & database workflow with real Redis binary |
| `TestUISoftwareCatalogAllFive` | `development_e2e_test.go:297` | `MSM_UI_E2E_PACKAGE_CACHE == ""` | Yes (`MATRIX`) | B | Verifies installation of all 5 real engine packages |
| `TestUISoftwareCatalogAllFive/postgresql` | `development_e2e_test.go:305` | `MSM_UI_E2E_HOMEBREW != "1"` | Yes (`MATRIX`) | B | Homebrew PostgreSQL catalog verification |
| `TestUIStandaloneRuntimeAndHTTPS` | `development_e2e_test.go:592` | `MSM_UI_E2E_PACKAGE_CACHE == ""` | Yes (`CORE`) | B | Standalone Node.js runtime and HTTPS reverse proxy gate |
| `seedRuntimeCache` (Homebrew keg check) | `development_e2e_test.go:675` | `MSM_UI_E2E_HOMEBREW != "1"` | Yes (via `MATRIX`) | B | Helper ensuring exact preinstalled Homebrew keg exists |
| `newUIDeletionInstance` (shared UI fixture helper) | `instance_deletion_e2e_test.go:22` | `MSM_UI_E2E_PACKAGE_CACHE == ""` | No | C | Helper for 5 UI recovery/deletion suites (see sub-rows below) |
| ↳ `TestUIBackgroundDatabaseBackupKeepsConfigurationAndIndependentInstance` | `database_operations_e2e_test.go:19` | via `instance_deletion_e2e_test.go:22` | No | C | Backup task cancellation & independent instance state |
| ↳ `TestUIDatabaseWorkCallerDeathReviewKeepsRealRedisAndBackup` | `database_work_e2e_test.go:41` | via `instance_deletion_e2e_test.go:22` | No | C | Caller crash recovery and preserved backup review UI |
| ↳ `TestUIInstanceDeletionPreviewConfirmationAndPreservedBackup` | `instance_deletion_e2e_test.go:71` | via `instance_deletion_e2e_test.go:22` | No | C | Typed confirmation modal & deletion preview UI |
| ↳ `TestUIInstanceDeletionActualCrashRestoresOrFinishes` | `instance_deletion_e2e_test.go:176` | via `instance_deletion_e2e_test.go:22` | No | C | Crash during staged vs committed database deletion |
| ↳ `TestUIBackgroundInstanceStartupKeepsPhaseConfigurationAndIndependentInstance` | `instance_lifecycle_e2e_test.go:16` | via `instance_deletion_e2e_test.go:22` | No | C | Startup task cancellation & phase configuration UI |
| `TestUIDatabaseInitializationKeepsIndependentDrafts` | `instance_initialization_e2e_test.go:63` | `MSM_UI_E2E_PACKAGE_CACHE == ""` | Yes (`CORE`) | B/C* | UI draft isolation; locked to native release gate under this name |
| `TestUIDatabaseInitializationActualCrashArchiveOrFinish` | `instance_initialization_e2e_test.go:165` | `MSM_UI_E2E_PACKAGE_CACHE == ""` | No | C | Crash recovery during database initialization archive step |
| `TestUIDatabaseInitializationCancellationKeepsAnotherPreparation` | `instance_initialization_e2e_test.go:225` | `MSM_UI_E2E_PACKAGE_CACHE == ""` | No | C | Concurrent initialization cancellation & mutual exclusion |
| `TestUIDatabaseInitializationQuitWaitsAndCancellationKeepsPreparedData` | `instance_initialization_e2e_test.go:274` | `MSM_UI_E2E_PACKAGE_CACHE == ""` | No | C | App quit waiting for active initialization & graceful rollback |
| `TestUIDatabaseInitializationArchiveCrashFinishesWhileReusedInstanceRuns` | `instance_initialization_e2e_test.go:322` | `MSM_UI_E2E_PACKAGE_CACHE == ""` | No | C | Reconciling archived initialization crash while reused ID runs |
| `TestShots` | `main_test.go:659` | `MSM_SHOTS == ""` | No | A | Opt-in screenshot generation for visual regression check |
| `TestUICheckRepairRealNodeAndRefuseRunningUsers` | `package_integrity_e2e_test.go:16` | `MSM_UI_E2E_PACKAGE_CACHE == ""` | No | C | Corrupt file detection UI, repair button, and active user guard |
| `TestUIQuitCanCancelBackgroundPackageBeforeLeaving` | `package_tasks_e2e_test.go:68` | `MSM_UI_E2E_PACKAGE_CACHE == ""` | No | C | Cancelling background package download on window close/quit |
| `TestUIPackageOtherCallerBusyDetailsAndRetry` | `package_tasks_e2e_test.go:145` | `MSM_UI_E2E_PACKAGE_CACHE == ""` | No | C | Mutual exclusion lease conflict UI and retry details |
| `TestUIBackgroundCancelOnePackageKeepsOtherPackageRunning` | `package_tasks_e2e_test.go:213` | `MSM_UI_E2E_PACKAGE_CACHE == ""` | No | C | Independent task cancellation across concurrent downloads |
| `TestReadmeShots` | `readme_shots_test.go:25` | `MSM_README_SHOTS == ""` | No | A | Opt-in README screenshot generator |
| `TestNativeResourceProduct` (4 subtests) | `resource_product_e2e_test.go:19` | `MSM_UI_E2E_PACKAGE_CACHE == ""` | Yes (`NATIVE`) | B | Native release acceptance: terminal, HTTPS, quit, profiles |
| `TestAdditionalEnginesIntegration` | `internal/devplatform/engines_integration_test.go:30` | `MSM_SIX_INTEGRATION_ROOT == ""` | No | B | Integration suite testing installed database engine services |
| `TestHomebrewLiveMetadataAndExistingKeg` | `internal/devplatform/homebrew_test.go:291` | `MSM_HOMEBREW_VERIFY != "1"` | No | B | Live Homebrew API/keg metadata queries |
| `TestLogRotationLaunchdLifecycle` | `internal/devplatform/log_rotation_test.go:61` | `MSM_LOG_INTEGRATION != "1"` | No | B | Real launchd service log rotation in active macOS GUI session |
| `TestSocketJobBindsLoopbackAndDropsPrivileges` | `internal/devplatform/network_test.go:12` | `os.Getuid() == 0` | No | B | Skips only when run as root; normal user execution runs |
| `TestRedisPackageRepairPreservesInstanceData` | `internal/devplatform/package_repair_integration_test.go:16` | `MSM_SIX_INTEGRATION_ROOT == ""` | No | B | Direct package binary repair against existing instance storage |
| `TestBashFishRuntimeStartup/fish` | `internal/devplatform/runtime_shell_extended_test.go:20` | `MSM_TEST_FISH` / fish missing | No | B | Skips only if fish shell binary is absent from PATH |
| `TestPackagePublicationRejectsLateCollision` | `internal/devplatform/storage_paths_test.go:213` | `runtime.GOOS != "darwin"` | No | B | Skips only on non-macOS systems; darwin runs unconditionally |

## Hermetic Fixture Architecture

1. **Synthetic Archive Generation (`cacheRuntimeArchive` pattern)**:
   - Reuse the pattern established in `internal/devplatform/additional_runtimes_test.go:208` and `internal/devplatform/instance_initialization_test.go:21`.
   - Build a shared helper `seedSyntheticUIPackage(t *testing.T, h *devUIHarness, name, version string) devplatform.Package`:
     - Resolves package metadata via `devplatform.Resolve(context.Background(), name, version)`.
     - Packages synthetic executable stubs into an in-memory `.tar.gz` archive (`mode 0755` in `runtime/bin/`).
     - Computes the SHA256 digest, sets `p.SHA256 = hex.EncodeToString(sum[:])`, and writes bytes directly to `filepath.Join(h.root, "downloads", p.SHA256)`.
     - When UI invokes `Download and install`, `m.Install` detects the pre-seeded download, verifies digest, and unpacks locally without HTTP requests.
2. **TCP & RESP Socket Stubbing for Lifecycle Suites**:
   - For database lifecycle tests (`newUIDeletionInstance`), the synthetic archive packages a lightweight stub for `bin/redis-server`:
     - Invoked via Go test-binary re-execution (`os.Args[0] -test.run=^TestRedisServerStubHelper$`) or a minimal POSIX script.
     - Parses the assigned port from arguments or `valkey.conf` and listens on `127.0.0.1:<port>`.
     - Responds with valid RESP protocol tokens: `+PONG\r\n` to `PING`, `+OK\r\n` to `AUTH`, `SET`, `SELECT`, and `SAVE`.
     - Shuts down cleanly on `SIGTERM` / `SIGINT` when stopped via launchctl.
   - A companion `bin/redis-cli` stub parses `SET value 42` and returns `OK`.
3. **Zero Network I/O & Concurrency Isolation**:
   - Pre-seeding `downloads/<sha256>` guarantees zero outbound HTTP requests.
   - Eliminates mutations to global `http.DefaultClient` or `http.DefaultTransport`, preventing data races under `go test -race ./...`.
   - Each test harness operates within an isolated `t.TempDir()`, keeping runtime under 2 seconds per suite.

## Non-Substitution Guarantee (`docs/resources-acceptance.md:3`, `CONTRIBUTING.md:7`)

1. **Strict Separation of Evidence Layers**:
   - `CONTRIBUTING.md:7`: *"Native release tests need a disposable hosted Mac and must not be replaced with mock results."*
   - `docs/resources-acceptance.md:3`: *"Mocked, isolated-native, and system-trust/Chrome evidence are distinct layers and cannot substitute for each other."*
   - Modifying existing test names or silently switching required native gates to synthetic fixtures violates this contract.
2. **Scenario Function Sharing Pattern**:
   - Factor UI test logic into unexported scenario functions parameterized by fixture mode:
     ```go
     type uiFixtureMode int
     const (
         fixtureNativeRelease uiFixtureMode = iota
         fixtureHermeticSynthetic
     )

     func runUIInstanceDeletionScenario(t *testing.T, mode uiFixtureMode, dark bool) {
         // Setup uses real cache if fixtureNativeRelease, or seedSyntheticUIPackage if fixtureHermeticSynthetic
         ...
     }
     ```
   - **Existing Native Gate**: `TestUIInstanceDeletionPreviewConfirmationAndPreservedBackup` continues to gate on `MSM_UI_E2E_PACKAGE_CACHE` and runs real binaries on the disposable release runner.
   - **New Hermetic Test**: `TestUIHermeticInstanceDeletionPreviewConfirmationAndPreservedBackup` runs unconditionally on every local commit and PR.
   - `scripts/verify-native-release.py` event stream validation (`check_events`) remains 100% untouched and unambiguous.

## Proposed Implementation Phases

- **Phase 1: Shared Synthetic Package Fixture**: Implement `seedSyntheticUIPackage` and the `redis-server` loopback stub helper.
- **Phase 2: Package Tasks & Integrity Suites**: Refactor `package_tasks_e2e_test.go` and `package_integrity_e2e_test.go` to expose `TestUIHermetic*` entrypoints.
- **Phase 3: Database Lifecycle & Recovery Suites**: Refactor `instance_deletion_e2e_test.go`, `instance_initialization_e2e_test.go`, `database_operations_e2e_test.go`, `database_work_e2e_test.go`, and `instance_lifecycle_e2e_test.go`.

## Estimated CI Impact & Runtime Budget

- **Current PR CI Profile**: `go test -race ./...` runs in ~2m 30s on GitHub Actions `macos-15` runners, with 30 tests skipping.
- **Hermetic Overhead**: The 13 Bucket C suites execute in ~10–18 seconds total under `-race` because extraction and loopback TCP stubs take <1.5s per suite.
- **Net Impact**: ~10–12% increase in test phase duration, well below the 30-minute job timeout, while eliminating 18 critical regression blind spots in PRs.

## Open Questions for Maintainers

1. **CORE Gate Parity**: Should `TestUIDatabaseInitializationKeepsIndependentDrafts` (currently in `verify-native-release.py` `CORE`) have an accompanying `TestUIHermeticDatabaseInitializationKeepsIndependentDrafts` so PR CI validates draft isolation without altering the release gate?
2. **Stub Binary Mechanism**: Is Go sub-process re-execution (`os.Args[0] -test.run=^TestStubHelper$`) preferred over compiled binary caching or portable POSIX shell scripts for the loopback RESP stub?
3. **Fixture Helper Location**: Should `seedSyntheticUIPackage` reside in `dev_ui_fixtures_test.go` at the root, or within `internal/devplatform` as an exported test helper?

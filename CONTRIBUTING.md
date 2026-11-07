# Contributing

The app uses Go and MyGo. Keep the existing system-service providers working;
new development features share the controller in `internal/devplatform` with
the graphical workspace and CLI.

## Local checks

```sh
go test ./...
go test -race ./...
go vet ./...
go tool mygo build -sign - -skip-notarize
```

See [UI testing](docs/development-ui.md) for real input tests and design renders,
and [verification](docs/development-verification.md) for optional native database
and launchd integration tests. Software integration tests need verified package
downloads; skipped tests must be reported as skipped.

## Changes to managed services

- Use launchd for long-lived processes. Keep archive binaries and instance data
  in app-owned storage, with credentials in private files. Homebrew kegs stay in
  the Cellar.
- Use isolated roots and ports in tests. Stop and unregister every test job.
- Never change the developer's original databases or take over external jobs.
- Keep destructive data deletion separate from removing management records.
- Do not run hosts, certificate-trust or privileged network changes unattended.
  UI E2E tests reject administrator authorization calls.
- A configuration form must be usable without editing JSON. Add UI input tests
  for affected workflows and inspect light/dark layouts at the minimum size.

## Software catalog

PHP uses the `shivammathur/php` Homebrew tap; PostgreSQL uses Homebrew core's
versioned `postgresql@<major>` formulae. Other development software uses verified
upstream archives. The project is unreleased: remove obsolete distribution paths
and test data rather than adding migration or compatibility logic. A Homebrew
failure must not fall back to an archive or a host PATH executable.

Resolve Homebrew release series to exact Cellar keg versions including revisions.
Check receipts for the expected tap, especially PHP aliases that share a rack
with core PHP. Keep app instances on their own launchd jobs, ports and data paths;
do not start the formula's default service. Homebrew installations are shared:
workspace removal unregisters them without deleting the keg. Reinstallation is
performed through Homebrew after stopping all consumers. Formula SHA-256 values
identify formula metadata, not the downloaded bottle; Homebrew verifies bottles
and manages their dependencies. The app's Check files action validates receipts
and executable availability for Homebrew packages, not their entire file tree.

Descriptors must pin version, architecture, source, SHA-256, layout, executable
paths, license information and minimum macOS version. Download and validate the
Apple Silicon (arm64) archive, then report which macOS versions were actually tested. Do not
substitute a related product for the one selected by the user.

Project-authored source uses Apache-2.0; retain LICENSE and NOTICE. Third-party
components retain their own terms. Before a release, review the provenance of
new source/assets and check the copied notice inventory:

```sh
go mod download
python3 scripts/collect-third-party-notices.py --check
```

When dependencies change, regenerate without --check and review the notice diff.
The generator reads cached modules for macOS arm64, disables module network
lookup, and records exact versions and SHA-256. Non-module assets use reviewed,
revision-pinned notices in assets/notices; retain upstream attribution. A Go
toolchain LICENSE is required even when a distribution puts it outside GOROOT.
MyGo copies LICENSE,
NOTICE and licenses into the signed app. Developer ID signing is complete;
Apple notarization is deferred by the user's current delivery choice. Acceptance
uses the macOS version installed on the current computer; record it separately
from the app's declared minimum. Full logout/reboot checks need coordination
because they end the current desktop session. See the
[third-party inventory](docs/third-party-software.md).

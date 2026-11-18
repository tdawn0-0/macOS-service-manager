# Resource product verification

This is a new-version record. Earlier Project/PHP/Site verification does not establish the current Software/Databases/HTTPS product. The [acceptance matrix](resources-acceptance.md) starts each result at NOT RUN; update a row only with the actual commit, command, environment, PASS/SKIP result and evidence.

Only the two latest macOS releases (currently 26 and 27) are verified: CI runs on GitHub `macos-26` runners, and macOS 27 is checked on the development machine. Older releases are out of scope.

The code-level gates are `go test ./...`, `go test -race ./...`, `go vet ./...`, `go mod verify`, Python script tests, notice check, and `go tool mygo build -sign - -skip-notarize`. They show compilation, behavior and an ad-hoc local build. A native release gate on a disposable GitHub-hosted macOS runner requires real installed packages and all parent/child events from `scripts/verify-native-release.py --run`; no skip is accepted. A separate Developer ID signed App must pass local CA authorization and Chrome without a certificate interstitial on an authorized disposable Mac.

Update the status below only when a gate actually changes it, naming the exact commit, command, environment and result. Any unavailable runner, signing identity, Chrome machine or cleanup evidence remains **NOT RUN/BLOCKED**, never inferred from a mock, CLI TLS client, or prior version.

## Current status

All rows of the [acceptance matrix](resources-acceptance.md) remain **NOT RUN** for full acceptance. The native release gate (`scripts/verify-native-release.py --run`) needs a disposable GitHub-hosted macOS arm64 runner, and the signed App, system CA authorization, privileged listener and Chrome runbook needs an authorized disposable machine with a matching Developer ID build. Neither has been observed for the current product. Real upstream Java/Python installation, real PostgreSQL 18 servers, database backup/restore on an external volume and cross-volume hardware behavior are also unverified. The Cloudflare tunnel (AC-TUNNEL) has only code-level tests against a fake Cloudflare API; no real-account run has been observed.

Records of earlier per-change runs (015-B, 018) are in this file's git history; they describe those commits only.

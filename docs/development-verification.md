# Resource product verification

This is a new-version record. Earlier Project/PHP/Site verification does not establish the current Software/Databases/HTTPS product. The [acceptance matrix](resources-acceptance.md) starts each result at NOT RUN; update a row only with the actual commit, command, environment, PASS/SKIP result and evidence.

The code-level gates are `go test ./...`, `go test -race ./...`, `go vet ./...`, `go mod verify`, Python script tests, notice check, and `go tool mygo build -sign - -skip-notarize`. They show compilation, behavior and an ad-hoc local build. A native release gate on a disposable GitHub-hosted macOS runner requires real installed packages and all parent/child events from `scripts/verify-native-release.py --run`; no skip is accepted. A separate Developer ID signed App must pass local hosts/CA authorization and Chrome without a certificate interstitial on an authorized disposable Mac.

Current implementation and checks should be appended below with exact commit and result. Any unavailable runner, signing identity, Chrome machine or cleanup evidence remains **NOT RUN/BLOCKED**, never inferred from a mock, CLI TLS client, or prior version.

## 015-B isolated worktree evidence

The 015-B implementation was checked in an isolated worktree based on `d76d545`; this record is not a release sign-off. `go test ./... -count=1` and `go test -race ./... -count=1` passed. `go vet ./...`, `go mod verify`, 31 Python tests, and the third-party notice check passed. `go tool mygo build -sign - -skip-notarize` built an **ad-hoc signed** arm64 App/DMG. The new native parent test is present but SKIP locally without the verified package cache; this is not a PASS for release acceptance.

The 012, 013, 014, 016, and 017 UI layout suites were each run in this worktree with the Go JSON event gate. Each produced fresh light/dark 900×520 and 1120×720 PNGs in ignored `build/resource-shots/` directories. The event gate confirmed parent and `layouts` child PASS, PNG dimensions and current-run times. Light 900×520 HTTPS and dark 1120×720 connection-profile screenshots were visually inspected. These synthetic screenshots do not include private machine data and are not system-trust evidence.

**NOT RUN/BLOCKED:** `scripts/verify-native-release.py --run` requires a disposable GitHub-hosted macOS arm64 runner. The signed App, system hosts/CA authorization, privileged listener and Chrome runbook requires an authorized disposable machine and matching Developer ID build. This local worktree has neither prerequisite; no hosts, keychain trust, privileged port, or browser acceptance was attempted. All nine acceptance rows remain NOT RUN until their required native evidence is recorded.

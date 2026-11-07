# Contributing

This is an Apple Silicon macOS Go/MyGo application. The primary development surfaces are Software, Databases, and HTTPS. The [product scope](docs/product-scope.md) and [acceptance matrix](docs/resources-acceptance.md) define current behavior; the separate system-service inspector remains supported.

Keep software source checks exact: Node.js, MySQL, Redis, and MongoDB use verified archives; PostgreSQL uses a Homebrew core formula and exact keg/receipt checks. Preserve instance data, task leases, rollback, cancellation, and explicit authorization for hosts, CA trust, and privileged proxy listeners. The HTTPS editor saves a whole `Domain → Local port` rule table; it does not own application processes.

Run focused tests for changed behavior, then `go test ./...`, `go test -race ./...`, `go vet ./...`, `go mod verify`, Python script tests, third-party notice check, and the MyGo build check. Native release tests need a disposable hosted Mac and must not be replaced with mock results. A signed App, system-trust, and Chrome run remains a separate manual gate. See [verification](docs/development-verification.md).

Keep third-party licenses and notices with packaged components. Do not record real machine credentials, database content, private file paths, or certificate material in test fixtures, logs, screenshots, or documentation.

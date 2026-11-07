# macOS Service Manager

A native macOS app for verified development software, independent database instances, and local HTTPS reverse proxy rules. It also retains a separate inspector for system services.

## Development

The three main entries are **Software**, **Databases**, and **HTTPS**. Start your application in a terminal or IDE; this app does not start it.

![HTTPS rule table in the 900×520 test viewport](docs/images/development/https-rules-900x520.png)

- **Software:** install and verify Node.js, MySQL, PostgreSQL, Redis, and MongoDB. PostgreSQL uses Homebrew core; the other four use checked archives. Node.js can use `.node-version`, `.nvmrc`, or `package.json` `engines.node` in any directory without registering it. Set a default and explicitly enable zsh integration in a new terminal.
- **Databases:** create independent MySQL, PostgreSQL, Redis, and MongoDB instances with their own data, port and lifecycle. Create databases/users where supported, back up and restore, and save password-free connection profiles for external clients. Redis uses numbered databases.
- **HTTPS:** edit one table of `Domain → Local port` rules. Each rule forwards to `http://127.0.0.1:<port>` through a local HTTPS proxy. Save the full table, then Apply. Configuring local domains and trusting the local CA are separate explicit actions. The upstream application stays under your control.

The menu bar shows managed database instances and one HTTPS proxy status. Closing the window leaves them running. Quit offers a reviewed keep/stop choice for app-owned resources. System service controls remain under their own source list.

## Installation and permissions

Requires an Apple Silicon Mac. App release artifacts and system requirements are described by the build workflow; this source checkout is not itself an installed release. The app asks for administrator authorization only when a privileged proxy listener, local hosts mapping, CA trust, or system service change needs it. Deleting a database instance has a separate data review and confirmation. No product operation should remove an unrelated application process or shared Homebrew keg.

## Source and verification

The current product scope is [documented here](docs/product-scope.md). [Usage](docs/development-usage.md), [UI guidance](docs/development-ui.md), and the [acceptance matrix](docs/resources-acceptance.md) describe the new behavior. Verification evidence and missing real-machine checks are in [development-verification](docs/development-verification.md).

```sh
go test ./...
go test -race ./...
go vet ./...
go mod verify
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_*.py'
python3 scripts/collect-third-party-notices.py --check
go tool mygo build -sign - -skip-notarize
```

Native release acceptance uses `python3 scripts/verify-native-release.py --run` **only on a disposable GitHub-hosted macOS runner**. It requires the exact parent and child tests to execute and pass without skips. Real system trust and Chrome acceptance require a separately signed App on a disposable Mac, as specified in the [acceptance plan](docs/resources-acceptance.md). Neither an ad-hoc build nor private test CA proves that check.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). The Go/MyGo app, database workers, HTTPS proxy, and macOS service integrations are tested separately. Tests that need real launchd or database binaries must run in an isolated, clean environment.

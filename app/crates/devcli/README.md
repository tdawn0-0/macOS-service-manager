# devcli

The public `devbase` CLI contract (`internal/devcli`) and the app's internal
`--dev` dispatch, in pure Rust. The crate is a library: the shipped app
dispatches into it before nsh starts, and `src/main.rs` is only a
test/verification harness.

## Public CLI

`run_cli`/`run_cli_with_env` implement the byte-compatible `--json` surface and
the `0/1/2` exit codes; see the command modules and the golden contract tests in
`tests/contracts.rs`.

## Internal `--dev` modes

`run_internal`/`run_internal_with` accept only what Devbase's own launchd jobs,
workers and shell setup start (`internal/devcli/cli.go:21-38`):

| argv | Entry point |
|---|---|
| `--dev serve <root>` | `devproxy::proxy::server::serve` (redirects logs to `logs/proxy-sockets.log` under `MSM_DEV_SOCKET_ACTIVATION=1`) |
| `--dev tunnel <root>` | `devproxy::tunnel::client::serve` |
| `--dev maintain <root>` | `devcore::diagnostics::Diagnostics::maintain` |
| `--dev database-worker <root> <id>` | `devcore::databases::Databases::run_database_worker` |
| `--dev node path [dir]` | `devcore::shell::Shell::select_node` |
| `--dev helper [--owner-uid N]` | `devcore::helper::run_activated` |

Anything else, including help/status/install, is refused with the Go message
`this program has no command line; use the window` and exit 1. `InternalRunner`
injects every subsystem so tests assert the exact argv forwarding without
starting a listener, launchd, the helper, or the network.

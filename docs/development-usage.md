# Development: using Software, Databases, and HTTPS

Open the app and choose Software, Databases, or HTTPS. Application servers run in your terminal or IDE; the app manages their tool versions, independent database instances, and local HTTPS forwarding.

## Software and Node.js

Install a checked version of Node.js, MySQL, PostgreSQL, Redis, or MongoDB. Review the source, checksum, architecture, license, and destination before installation. PostgreSQL and Redis use Homebrew core; removing its App registration leaves the shared Homebrew keg alone. Newer same-series updates for installed Node.js and Java releases are flagged for quick review, and cached download archives can be inspected and cleared from Settings. In Node.js, select a default installed version, then click Enable in terminal if you want new sessions of the detected zsh, Bash, and fish shells to use it, including non-interactive AI commands. A directory's `.node-version`, `.nvmrc`, or `package.json` `engines.node` can select another installed version without registering the folder. Copy executable path is available for a verified binary. Disabling the terminal integration removes the App's own blocks across all supported shells, including previously recorded configuration locations.

## Databases

Create an independent instance with an installed engine, instance name, local port, and optional external parent folder (Devbase creates a new subfolder for the instance). Start, stop, restart, inspect existing databases and users on running instances, create a database or user when the engine supports it, view logs inline or in Console, and back up or restore data in the Databases entry. Redis uses database numbers 0–15. Before destructive data deletion, review the target path and typed confirmation. Connection profiles store a database, username, and engine-specific metadata, not a business password. Copy URI, Copy `.env`, Copy JDBC URL, and Open client use the selected instance's current port without exposing credentials. The administrator connection and password action remain separate from business profiles. Removing a profile does not remove a real database or user.

A stopped MySQL or MongoDB instance can be moved to a newer installed patch release of the same series from its page. Data is not copied or deleted, and no backup is taken automatically, so back up what you need first. Going back to the older patch release is not supported. Afterwards Devbase offers to uninstall the older version when no instance uses it, and the Software page marks installed database versions that no instance uses.

Each instance's Backups card lets you choose a database from the running instance and lists its backups with the database, date and size. Use for restore selects a backup, and restoring always creates a new database. Deleting a backup removes only that file and does not change any database.

## HTTPS

Type a name for each forwarding rule; its domain is always `<name>.devbase.localhost`, which macOS resolves to this Mac, so no hosts-file entry or DNS setting is needed. Enter a local HTTP port for each forwarding rule. The proxy forwards to `127.0.0.1` on that port, or to the IP address or host name in the rule's Upstream host field (for example a machine on your network, a container address, or `host.docker.internal`). The upstream is plain HTTP, and an empty field or `localhost` means this Mac. The incoming HTTPS listener port is global. Copy or open any saved rule's HTTPS URL from the row. **Save HTTPS rules** validates and stores the entire table, **Apply proxy** publishes it, and **Save & apply** performs save, Worker sync, and apply in one step; remove a row and save/apply to stop forwarding that domain. Trust local CA is a separate action that may require macOS authorization. Stopping the proxy leaves the saved rules and external application process alone.

### Cloudflare tunnel (optional)

To reach a rule from outside this Mac, paste a Cloudflare API token in the **Cloudflare tunnel** card and choose Save Cloudflare settings; the account ID is detected if left blank. The token is written to a private file in application storage, not to app state. Turn on **Tunnel** for a rule and pick a mode: `workers.dev` serves it at the account's `*.workers.dev` subdomain, and a Custom Domain binds a host name in one of your Cloudflare zones (existing DNS names are suggested). Enable **Protect** on a tunneled rule to require a per-rule access token at the Worker edge (`Bearer`, `Basic`, or `X-Devbase-Token`), with copy and rotate actions on the rule row. Saving the rules deploys one dedicated `devbase-<name>` Worker + Durable Object per tunneled rule and deletes the Devbase Workers of rules that no longer tunnel. Copy or open the tunnel URL from the rule row, and inspect proxy or tunnel logs inline or in Console.

The **Cloudflare tunnel service** switch, on the HTTPS page and in the menu bar, starts or stops a background launchd client that connects to those Workers and forwards their requests to each rule's upstream. Stopping it leaves saved rules, deployed Workers, and the upstream application alone. Anyone with the URL can reach an unprotected rule while the tunnel runs; enable **Protect** or add application-level authentication when needed.

Use Diagnostics to inspect DNS, proxy TLS, upstream and explicit HTTP response separately. Check HTTPS response and Check tunnel response each send one bounded GET `/` on request; a status such as 401 or 404 proves an HTTP response arrived, not that the application is healthy. The export removes domain, username, paths, raw errors, and response data. Missing or changed rules invalidate older observations.

## Window, menu bar, and system services

Closing the window keeps managed resources running. The app then lives only in the menu bar: its Dock icon is hidden until the window opens again from the menu bar. The menu bar has database rows, one HTTPS proxy switch, and a Cloudflare tunnel switch once a tunnel is configured, with hover actions to copy/open a single URL or pick from a multi-rule menu. Quit reviews which app-owned resources to keep or stop. The existing system-service inspector is a separate feature and follows its own permissions. No operation starts or stops an unrelated application server.

## Verification status

See [development-verification.md](development-verification.md). Unit, isolated-native, and signed App/system-trust/Chrome evidence are separate. An unrun native or browser check stays NOT RUN.

## Additional runtimes and database locations

Software also includes Java and Python (uv). The [runtime and data-location guide](development-runtimes-and-data.md) explains exact versions, terminal defaults, existing uv storage, and custom database paths. Database versions are selected per instance; changing the major version requires a separate instance and an explicit data migration outside this feature.

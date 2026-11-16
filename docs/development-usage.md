# Development: using Software, Databases, and HTTPS

Open the app and choose Software, Databases, or HTTPS. Application servers run in your terminal or IDE; the app manages their tool versions, independent database instances, and local HTTPS forwarding.

## Software and Node.js

Install a checked version of Node.js, MySQL, PostgreSQL, Redis, or MongoDB. Review the source, checksum, architecture, license, and destination before installation. PostgreSQL and Redis use Homebrew core; removing its App registration leaves the shared Homebrew keg alone. In Node.js, select a default installed version, then click Enable in terminal if you want new sessions of the detected zsh, Bash, and fish shells to use it, including non-interactive AI commands. A directory's `.node-version`, `.nvmrc`, or `package.json` `engines.node` can select another installed version without registering the folder. Copy executable path is available for a verified binary. Disabling the terminal integration removes the App's own blocks across all supported shells, including previously recorded configuration locations.

## Databases

Create an independent instance with an installed engine, instance name, and local port. Start, stop, restart, create a database or user when the engine supports it, and back up or restore data in the Databases entry. Redis uses database numbers 0–15. Before destructive data deletion, review the target path and typed confirmation. Connection profiles store a database, username, and engine-specific metadata, not a business password. Copy URI and Open client use the selected instance's current port. The administrator connection and password action remain separate from business profiles. Removing a profile does not remove a real database or user.

A stopped MySQL or MongoDB instance can be moved to a newer installed patch release of the same series from its page. Data is not copied or deleted, and no backup is taken automatically, so back up what you need first. Going back to the older patch release is not supported. Afterwards Devbase offers to uninstall the older version when no instance uses it, and the Software page marks installed database versions that no instance uses.

Each instance's Backups card lists its backups with the database, date and size. Use for restore selects a backup, and restoring always creates a new database. Deleting a backup removes only that file and does not change any database.

## HTTPS

Enter a `.test` Domain and a local HTTP port for each forwarding rule. The proxy forwards to `127.0.0.1` on that port. The incoming HTTPS listener port is global. Save validates and stores the entire table. Apply publishes the saved table; remove a row and Save/Apply to stop forwarding that domain. Configure local domains and Trust local CA are separate actions that may require macOS authorization. Stopping the proxy leaves the saved rules and external application process alone.

Use Diagnostics to inspect DNS, proxy TLS, upstream and explicit HTTP response separately. Check HTTPS response sends one bounded GET `/` on request; a status such as 401 or 404 proves an HTTP response arrived, not that the application is healthy. The export removes domain, username, paths, raw errors, and response data. Missing or changed rules invalidate older observations.

## Window, menu bar, and system services

Closing the window keeps managed resources running. The menu bar has database rows and one proxy status/settings entry. Quit reviews which app-owned resources to keep or stop. The existing system-service inspector is a separate feature and follows its own permissions. No operation starts or stops an unrelated application server.

## Verification status

See [development-verification.md](development-verification.md). Unit, isolated-native, and signed App/system-trust/Chrome evidence are separate. An unrun native or browser check stays NOT RUN.

## Additional runtimes and database locations

Software also includes Java and Python (uv). The [runtime and data-location guide](development-runtimes-and-data.md) explains exact versions, terminal defaults, existing uv storage, and custom database paths. Database versions are selected per instance; changing the major version requires a separate instance and an explicit data migration outside this feature.

# Java, Go, uv Python, and database data locations

## Runtime versions

Software now includes Node.js, Java, Go, Python (uv), MySQL, PostgreSQL, Redis, and MongoDB. The original five-software native release gate remains unchanged; additional runtime fixtures and UI tests are separate evidence.

For Java or Go, choose **Refresh available versions**, select an exact version, then review and install the package. Java lists Temurin JDK 17, 21, and 25 release lines for macOS arm64. Build suffixes use `_` in the package identifier, for example `21.0.13_9.0.LTS`. Downloads retain the upstream SHA256 and use the existing safe archive installer. Go lists stable official macOS arm64 archives. Installed versions remain selectable if a catalog request fails.

**Set default** changes only that runtime’s selection. **Enable in terminal** adds its marked block to `.zshrc`; use a new zsh session. Java sets `JAVA_HOME` and prepends its `bin` directory to PATH. Go prepends its `bin` directory without setting global `GOROOT` or `GOPATH`. **Disable in terminal** removes that runtime’s block while preserving other settings and dotfile symlinks. Running processes keep their original environment. Removing the selected package clears its default and setup script; select another installed version before enabling it again.

## Python uses uv

Install uv separately, or select an existing executable by absolute path in **uv executable**. Discovery checks PATH, `~/.local/bin/uv`, and Apple Silicon Homebrew. An explicitly selected executable that disappears produces an error instead of silently switching tools. **Refresh Python versions** shows the actual uv version/path, installed managed interpreters, and available macOS arm64 downloads. The installed list is retained if available-version retrieval fails.

The app calls uv with argument arrays and JSON version output. It uses the normal uv installation store and respects uv’s storage environment variables. It does not create another interpreter store or change shell aliases during installation (`uv python install --no-bin`). Operations run outside the current project directory, using an exact catalog key.

**Set uv default** writes the global uv Python pin. Directory pins, project requirements, and existing virtual environments retain their own precedence. This does not replace the shell’s `python` command. **Copy Python path** supplies the selected interpreter’s path for a terminal or IDE. **Uninstall Python** accepts only an interpreter from uv’s managed installed list; system interpreters are excluded. Virtual-environment directories are not deleted, but environments that reference an uninstalled interpreter need repair. A global pin may still name an uninstalled version; select another installed default explicitly.

The installed uv must support `python list --output-format json`, managed Python filtering, `install --no-bin`, and `pin --global`. Unsupported CLI capabilities report uv’s error; selecting or updating uv is explicit.

## Database versions and data locations

Each new instance selects an exact installed package ID, a unique local port, and a data location. PostgreSQL’s supported Homebrew lines include 16, 17 and 18, resolved to the current exact keg; arbitrary historical versions are not promised. MySQL, Redis and MongoDB retain the verified versions shown in their existing catalogs.

Leave **Data location** blank for application storage. For external storage, enter a **new absolute directory** whose parent already exists. The app creates this location with a private ownership marker and places engine files in its `data` child. Credentials, configuration, launchd jobs and task records stay in application storage. Paths containing spaces are supported. Symlink components, existing directories, overlapping instance locations, application storage, root, and the user’s home directory are rejected. The configured directory’s file identity and owner marker are checked before initialization, start/restart, port changes and backup/restore work.

This layout does not move database files across volumes. Initial preparation uses the selected external location; committing the instance only renames application metadata inside application storage. Cancellation or a failed state commit keeps external files and an unfinished preparation record. Recovery archives the application metadata and reports the retained external data path. Choose a different new location for a retry. There is no automatic adoption, data-directory relocation, or in-place major-version upgrade.

**Remove instance** stops and unregisters the instance while keeping data. External **permanent deletion is disabled** with the actual path and reason. App-managed data still uses the existing reviewed, recoverable permanent-delete flow. Logical backups/restores use the selected instance’s authenticated connection and preserve the existing new-target rules; they do not rename external data directories.

## Sources and verification boundaries

Source contracts were checked against [Adoptium’s API](https://api.adoptium.net/), the [official Go release index](https://go.dev/dl/?mode=json&include=all), [uv CLI documentation](https://docs.astral.sh/uv/reference/cli/), [uv storage](https://docs.astral.sh/uv/reference/storage/), and [PostgreSQL 18’s Homebrew formula](https://formulae.brew.sh/formula/postgresql@18). uv CLI help and JSON enumeration were checked read-only on the development machine. No real interpreter install/uninstall or user database changes were performed.

Focused tests use temporary roots, injected HTTP/uv runners, and exact-keg command fixtures. The custom-location lifecycle fixture runs an isolated launchd socket process; it is not a real database server. PostgreSQL 17/18 fixtures prove separate package, port, metadata and data-path handling, not native SQL compatibility. Native upstream Java/Go/Python installs, real PostgreSQL 18, actual database backup/restore on an external volume, and hardware cross-volume acceptance remain unverified. Signed App/system trust/Chrome acceptance from 015-B also remains unverified.

Run the focused gate with:

```sh
go test ./... -list '^(TestRuntimeCatalogs|TestUVPythonManagement|TestAdditionalRuntimeSelection|TestUIAdditionalRuntimes|TestDatabaseCustomDataDirectory|TestDatabaseVersionIsolation|TestUIDatabaseLocations)$'
go test -race ./... -run '^(TestRuntimeCatalogs|TestUVPythonManagement|TestAdditionalRuntimeSelection|TestUIAdditionalRuntimes|TestDatabaseCustomDataDirectory|TestDatabaseVersionIsolation|TestUIDatabaseLocations)$' -count=1 -json
MSM_ADDITIONAL_SHOTS=docs/images/development/additional go test . -run '^(TestUIAdditionalRuntimes|TestUIDatabaseLocations)$' -count=1
```

Screenshots contain synthetic examples. The database-location screenshot illustrates a path input; it does not claim that a volume was mounted or mutated.

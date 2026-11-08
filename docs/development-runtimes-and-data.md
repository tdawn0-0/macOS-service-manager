# Java, Go, uv Python, and database data locations

## Runtime versions

Software now includes Node.js, Java, Go, Python (uv), MySQL, PostgreSQL, Redis, and MongoDB. The original five-software native release gate remains unchanged; additional runtime fixtures and UI tests are separate evidence.

For Java or Go, choose **Refresh available versions**, select an exact entry, then review and install the package. Java uses the [JetBrains JDK catalog](https://download.jetbrains.com/jdk/feed/v1/jdks.json) used by IDEA, filtered to listed, non-preview macOS arm64 archives with supported layouts. The selection key includes the distribution/flavour and version (for example `temurin-21.0.13` or `semeru-25.0.4.15`), so equal Java versions from different vendors do not share a package identity. Download review shows the vendor/product/flavour, version, archive URL, SHA256, license notice and destination. Go lists stable official macOS arm64 archives. Installed versions remain selectable if a catalog request fails.

**Set default** changes only that runtime’s selection. **Enable in terminal** detects installed zsh, Bash, and fish and writes only their required startup entries. **Disable in terminal** removes that runtime’s marked blocks from every supported shell, including recorded locations for shells that have since been uninstalled or moved. Other settings, permissions, and dotfile symlinks are preserved. Java sets `JAVA_HOME` and prepends its `bin` directory to PATH. Go prepends its `bin` directory without setting global `GOROOT` or `GOPATH`. Running processes keep their original environment. Removing the selected package clears its default and both shell-format setup scripts; select another installed version before enabling it again.

## Storage layout

With the normal application storage root, paths are:

| Content | Location | Reason |
| --- | --- | --- |
| JDK distributions | `~/Library/Java/JavaVirtualMachines/<install_folder_name from catalog>` | User-level JDK discovery, independent exact versions, no administrator access |
| Java home | `<installation>/Contents/Home` or `<installation>` | Uses the archive layout declared by the catalog and preserves upstream metadata |
| Downloaded archives | `~/Library/Caches/ServiceManager/Development/downloads` | Downloadable files can be purged without removing installed versions |
| Node release catalog | `~/Library/Caches/ServiceManager/Development/node-index.json` | Re-fetchable version metadata |
| Go, Node, MySQL, Redis, MongoDB and Caddy archives after installation | `~/Library/Application Support/ServiceManager/Development/packages/<package-id>` | App-managed exact versions; these are durable installations, not caches |
| Python interpreters and uv caches | uv's own resolved locations | Shares uv's installation store and respects uv storage settings |
| PostgreSQL binaries | Homebrew's exact Cellar keg | Homebrew owns the installation and its dependencies |
| Database data | App instance storage or the explicitly selected directory | Durable data stays separate from package versions and downloadable caches |
| State, integrity indexes, terminal scripts, credentials, backups and service logs | Application storage | Keeps persistent service state and diagnostic history with the instance lifecycle |

Java installation names use the catalog's `install_folder_name`, following IDEA's distribution/version convention: `temurin-21.0.13`, then `temurin-21.0.13-1`, `-2`, etc. when the name is occupied. The actual folder is saved in the package receipt; build versions remain exact package IDs. No application prefix or `.jdk` suffix is added. Installation never adopts or overwrites a directory from another installer. Publishing a new directory uses macOS exclusive rename, so a destination created after the existence check is also preserved. Repair still requires the application's ownership marker and verified download receipt. Downloads are staged beside the destination for atomic publication and rollback.

### Existing Java distributions

The Java page automatically scans immediate entries in the user JVM directory and offers an explicit refresh. It reads `Contents/Info.plist` and `Contents/Home/release` (or `release` for a flat JDK), without executing discovered Java binaries. Directory names do not need a `.jdk` suffix. Each row retains the bundle name, vendor, full runtime version/build, architecture and exact path. Semeru/OpenJ9, Temurin, GraalVM, Oracle, Zulu and other distributions remain distinct, including equal Java versions in separate directories. Unknown vendor names are displayed as supplied by metadata; no distribution is converted into a generic OpenJDK entry.

The download catalog supports available distributions such as Temurin, Semeru, GraalVM, Oracle OpenJDK, Zulu, Corretto, Microsoft OpenJDK, Liberica, SapMachine and JetBrains Runtime. The list changes with the feed. Unknown filters, unsafe paths, missing checksums, unsupported layouts and ambiguous download identities are rejected; early-access and hidden entries are not offered. Existing installations can be selected as the terminal default and their `JAVA_HOME` copied. Intel-only installations remain visible and removable, but cannot be selected as an Apple Silicon default. Default selection does not modify an IDE's project SDK.

**Remove JDK** opens a review showing the distribution and actual path. **Move JDK to Trash** moves that one directory to the user's Trash with a unique name; it does not permanently erase it. A symbolic link is moved as a link and its target is kept. The action rechecks the directory identity and metadata from the review. Stale selections are rejected. Cancelling or failing the state transaction restores the moved entry and its default script. Removing the selected default clears the app's Java selection and script. IDEs and tools using the removed path must select another JDK. App-installed JDK removal also clears its package and integrity records. External installations are never registered as downloadable/repairable packages.

No previous App naming scheme is supported or migrated. JDKs already installed by IDEA or another tool are discovered from their actual metadata as part of this feature.

An explicit non-default storage root, including `MSM_DEV_HOME`, remains isolated: JDKs use `<root>/JavaVirtualMachines`, archives use `<root>/downloads`, and the Node catalog uses `<root>/cache`. This prevents disposable CLI/test roots from registering JDKs in the real user's directory. Python continues to delegate storage to uv; this root override does not isolate uv.

There is no automatic migration of the old Java package layout or download cache. Old archives can be removed independently of installed runtime files. The development machine had no Java registration or old Java package directory when this layout changed.

This follows [Apple's cache and application-support guidance](https://developer.apple.com/library/archive/documentation/FileManagement/Conceptual/FileSystemProgrammingGuide/FileSystemOverview/FileSystemOverview.html). JetBrains also uses the [user-level JavaVirtualMachines directory](https://github.com/JetBrains/intellij-community/blob/master/platform/lang-impl/src/com/intellij/openapi/projectRoots/impl/jdkDownloader/JdkInstaller.kt) for JDK downloads.

`TestUserStorageLocations`, `TestJavaStandardDirectoryLifecycle`, `TestJavaProtectsForeignInstallations`, `TestJavaCoexistsWithOtherInstallers` and `TestPackagePublicationRejectsLateCollision` exercise the new layout with temporary directories. They cover publication, integrity checks, default selection, repair cancellation/rollback, successful repair, cache removal, uninstall, foreign directories, symlinks, same-version coexistence and a destination created just before publication. Their executable fixture is `/bin/echo`, not a real JDK; system/IDE discovery remains a native acceptance check.

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

Source contracts were checked against the [JetBrains JDK parser and feed configuration](https://github.com/JetBrains/intellij-community/blob/master/platform/lang-impl/src/com/intellij/openapi/projectRoots/impl/jdkDownloader/JdkList.kt), the [official Go release index](https://go.dev/dl/?mode=json&include=all), [uv CLI documentation](https://docs.astral.sh/uv/reference/cli/), [uv storage](https://docs.astral.sh/uv/reference/storage/), and [PostgreSQL 18’s Homebrew formula](https://formulae.brew.sh/formula/postgresql@18). uv CLI help and JSON enumeration were checked read-only on the development machine. No real interpreter install/uninstall or user database changes were performed.

Focused tests use temporary roots, injected HTTP/uv runners, and exact-keg command fixtures. The custom-location lifecycle fixture runs an isolated launchd socket process; it is not a real database server. PostgreSQL 17/18 fixtures prove separate package, port, metadata and data-path handling, not native SQL compatibility. Native upstream Java/Go/Python installs, real PostgreSQL 18, actual database backup/restore on an external volume, and hardware cross-volume acceptance remain unverified. Signed App/system trust/Chrome acceptance from 015-B also remains unverified.

Run the focused gate with:

```sh
go test ./... -list '^(TestRuntimeCatalogs|TestUVPythonManagement|TestAdditionalRuntimeSelection|TestUIAdditionalRuntimes|TestDatabaseCustomDataDirectory|TestDatabaseVersionIsolation|TestUIDatabaseLocations)$'
go test -race ./... -run '^(TestRuntimeCatalogs|TestUVPythonManagement|TestAdditionalRuntimeSelection|TestUIAdditionalRuntimes|TestDatabaseCustomDataDirectory|TestDatabaseVersionIsolation|TestUIDatabaseLocations)$' -count=1 -json
MSM_ADDITIONAL_SHOTS=docs/images/development/additional go test . -run '^(TestUIAdditionalRuntimes|TestUIDatabaseLocations)$' -count=1
```

Screenshots contain synthetic examples. The database-location screenshot illustrates a path input; it does not claim that a volume was mounted or mutated.

Java inventory verification uses `TestJavaDiscoveryPreservesDistributionIdentity`, `TestExistingJavaDefaultAndRemoval`, `TestJavaRemovalRejectsStaleReviewAndKeepsLinkTarget`, `TestJavaRemovalRollsBackOnCancellation`, `TestJavaIdeaNamingAndManagedTrash`, `TestUIExistingJavaManagement` and `TestUIJavaDistributions`. Mutation tests use temporary JVM/Trash directories. The development machine's existing Semeru and Temurin installations were enumerated read-only; none were selected or removed during verification.

The feed supplies download URLs, SHA256, `package_root_prefix`, `package_to_java_home_prefix` and `install_folder_name`. IDEA fetches the `.json.xz` representation; this application fetches the same plain JSON endpoint. This is a JetBrains-maintained IDE feed, not an availability guarantee. Cached installed receipts remain usable offline; catalog errors do not switch providers or install a different distribution. `TestJetBrainsJavaCatalog` covers distribution isolation, filtering, checksum/HTTPS/path validation and ambiguous identities. `TestJavaFlatArchiveLayout` covers flat-JDK installation, discovery, default selection and Trash removal. Actual upstream archives for every distribution have not been installed during this change.

## Terminal and AI command coverage (macOS)

Enable detects executable shells through PATH, the configured login shell, and the standard macOS system/Homebrew locations. It does not install shells. No fish user configuration is created when fish is absent. Enabling again upgrades older setups and leaves unchanged startup files untouched.

| Shell | Entries written when installed | Non-interactive behavior |
|---|---|---|
| zsh | `.zshenv`, `.zprofile`, `.zshrc` under exported `ZDOTDIR`, otherwise home | Covers `zsh -c` and `zsh -lc`; login setup reapplies PATH after macOS `path_helper`. |
| Bash | `.bashrc` and the first existing `.bash_profile`, `.bash_login`, or `.profile`; creates `.bash_profile` only if none exists | Login commands load the selected profile. Plain `bash -c` reads the managed `BASH_ENV` inherited from a configured shell. |
| fish | `fish/conf.d/service-manager.fish` under exported `XDG_CONFIG_HOME`, otherwise `~/.config` | fish reads this snippet for interactive, non-interactive, login, and non-login sessions. No universal variables are changed. |

The Bash environment is exported from configured zsh, Bash, and fish sessions. An existing literal `BASH_ENV` filename is retained and sourced before managed runtimes. Dynamic `BASH_ENV` expressions containing variable or command expansions remain untouched; use Copy Bash command to explicitly select managed startup in that case. No existing startup file is replaced wholesale. Node resolves the working directory at startup; zsh and fish also update on directory changes. Bash refreshes Node selection when `node`, `npm`, `npx`, or `corepack` is invoked, without replacing existing user functions, `cd`, or traps. Explicit executable paths, `command node`, and `exec node` bypass those Bash functions; use the intended working directory when starting the shell for those forms.

Standalone GUI/AI processes may not inherit any shell environment. **Copy Bash command** supplies a command of the form `BASH_ENV='/managed/path/bash-env.sh' /bin/bash -c 'YOUR_COMMAND'`. Replace `YOUR_COMMAND` with the task command, or set that `BASH_ENV` path in the tool's environment. This covers a clean-environment `bash -c` without changing global launchd settings or system files. Plain `bash -c` cannot automatically discover a user startup file when `BASH_ENV` is absent.

Disable also scans default paths, current custom paths, and paths recorded during earlier enables; it does not create configuration files for missing shells. Each runtime remains independently enabled or disabled. An already-running process retains its environment. Shell options that disable startup (`zsh -f`, `fish --no-config`, or Bash startup suppression), shells invoked as `sh`, and direct execution without an inherited environment are outside automatic setup.

References: [zsh startup files](https://zsh.sourceforge.io/Doc/Release/Files.html), [Bash startup files](https://www.gnu.org/s/bash/manual/html_node/Bash-Startup-Files.html), and [fish configuration](https://fishshell.com/docs/current/language.html#configuration-files). The implementation is currently verified on macOS with its bundled Bash; Linux product support is not claimed.

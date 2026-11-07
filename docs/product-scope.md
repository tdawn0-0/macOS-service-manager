# Product scope: software, databases, and HTTPS

Status: current resource-product contract, defined in 015-A on baseline `2ec48ab` and implemented through the 011–017 worktree chain. Code-level checks pass; full native and system-trust/Chrome acceptance remain **NOT RUN**. See [resources-acceptance.md](resources-acceptance.md) for observable checks.

## Primary navigation

The app has three primary entries: **Software**, **Databases**, and **HTTPS**. The existing system-service inspector remains a secondary capability. The menu bar shows independently managed database instances and one HTTPS proxy; installed software and the number of proxy rules are not running-process counts.

Software installs and manages verified versions of Node.js, MySQL, PostgreSQL, Redis, and MongoDB. PostgreSQL comes from Homebrew core; the other four use verified archives. Node.js retains per-directory version declarations without registering the directory: an explicit choice, a nearby declaration, then the default version determine selection. Terminal integration is explicit and reversible. Software installation alone does not imply that a new terminal uses the version. PHP installation, version selection, repository setup, and terminal integration leave the product; existing PHP installations on the machine are not removed.

Databases owns independent MySQL, PostgreSQL, Redis, and MongoDB instances. Each has its own state, data, port, lifecycle, credentials, and background operations. Start, stop, restart, initialization, backup, restore, and deletion act on the selected instance, with data and busy-operation safeguards. Connection profiles belong to an instance and store no business password. Deleting a profile does not delete a real database or user.

HTTPS provides a single configuration page with a table of **Domain → Local port** rules. Each rule forwards to `http://127.0.0.1:<LocalPort>`. The HTTPS listener port is global and defaults to 443. Rules have no separate resource ID, detail page, enable switch, or process lifecycle. Save commits the whole validated table and listener port; Apply publishes the saved configuration to the proxy. Configure local domains and local CA trust are separate, explicit privileged actions. A published configuration is not proof that DNS, TLS, the upstream, or an HTTP response works. Stopping the proxy does not stop any upstream app or delete saved rules. The app does not start or manage the upstream process; users start it in their terminal or IDE.

## Lifecycle and safety

Closing the window leaves managed work running. Quit offers a clear choice to keep managed resources or stop selected app-owned database jobs and the HTTPS proxy. A proxy listener using a privileged port can continue after the window closes; authorization and trust must be shown as separate operations. The app does not take ownership of unrelated processes, system services, Homebrew kegs, hosts entries, or certificates. Legacy state is rejected without silent conversion or overwrite. This product revision has no old Project schema migration or compatibility layer; tests use a fresh isolated state root. Removing old features is not authorization to delete real databases, shared software, or unknown jobs on a user's machine.

## Explicitly outside scope

Project registration, source-directory management, command execution, dependency installation, app orchestration, project autostart, environment import/export, PHP, independent site management, arbitrary remote upstreams, tunnels, FastCGI, SQL IDE, cloud sync, and team environment sharing. The existing secondary system-service capability remains available under its own controls.

## Delivery sequence

015-A fixes this target and its acceptance matrix. 011 deletes the old model and prepares the new core. 012 handles Node terminal use; 013 handles resource lifecycle; 014 builds the HTTPS rule page; 016 adds diagnostics; 017 adds database connection profiles. 015-B updates public usage documentation and executes the full test gates. The signed App/system-trust/Chrome gate has not been observed, so this is not a release acceptance claim.

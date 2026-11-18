# Development UI

The native MyGo window has three main entries: Software, Databases, and HTTPS. Labels and accessible names must remain stable. The UI is reviewed at 900×520 and 1120×720 in light and dark themes.

## Layout

The window follows the Lobe UI look through [Lobe MyGo](https://mygo.lobehub.com/) components, assembled by the helpers in `ui_kit.go`:

- **Sidebar**: the *Workspace* entries (Software, Databases, HTTPS) with counts — installed packages, running/total instances, and rules — above the *Inspector* section (*All Items*), *Apps & Services*, *launchd*, and *System* (*Settings*). `Cmd+1`–`Cmd+3` switch to Software, Databases, and HTTPS, and `Cmd+,` opens Settings.
- **Title bar**: the section's name (the sidebar holds its count), the work in progress (Package or Database tasks, Cancel operation), then Refresh, Tasks, Logs and Diagnostics as icon buttons.
- **List** (Software and Databases only): Software groups *Runtimes* and *Database engines*, each with its icon and default or installed version. Databases lists instances with port and status dot, a `+` for a new instance, and the workspace-wide *Recovery* reviews (unfinished tasks, initializations, pending deletions). HTTPS has no list: it is a single page.
- **Page**: a header with the subject's icon, name, facts and status tag, and its main actions at the end (Start/Restart/Stop and logs for an instance, Apply proxy for HTTPS). The controls below are grouped in outlined cards by concern; irreversible actions sit in a red *Remove* card.
- **Status bar**: the last operation's result with a status dot, **Live logs** to toggle the workspace live log drawer (`Log target` selector for `proxy`, `tunnel`, and `db-<id>`, `Error`/`Output` switcher, `Filter logs`, `Open in Console`, and `Close logs`), and **Details** for its full text.

Buttons whose accessible name repeats their subject (for example `Check files node 22.14.0`) may show a shorter text (`Check files`); the accessible name is unchanged. A Software page for an installed database engine links to a new instance of it, and a new instance without an installed engine links back to Software.

## Behavior

Software shows the installed package, same-series update badges with quick review buttons, default Node version, and whether Node is enabled for new zsh, Bash, and fish terminals; selecting a **Version** from the dropdown also previews package details in the background (keeping **Review package** and install buttons intact). Settings Workspace displays the download cache size with a confirmed cleanup action and a **Start at login in menu bar** switch (`xyz.jyeu.devbase.login.plist` with `--background`). Databases shows independently managed instances, live database pills and clickable user tags while running, **Drop database** and **Drop user** actions with confirmation dialogs and system-entity guards, a target database dropdown in Backups plus a **Restore target database** / **Restore Redis database (0–15)** field (prefilled by **Use for restore**), **Upgrade to \<version\>…** for stopped MySQL, PostgreSQL, Redis, and MongoDB instances with a newer installed same-series patch/revision, parent-folder selection for external storage, data work, deletion review, Console and live-drawer log actions, administrator connection, and instance-owned password-free business connection profiles with URI, engine-specific `.env` (`DATABASE_URL`, `REDIS_URL`, `MONGODB_URI`), JDBC copy, and **Open CLI in Terminal** (`psql`, `mysql`, `redis-cli`). Editing one instance must not overwrite another instance's draft.

HTTPS is a single rule table. Each row has a Domain name (the `.devbase.localhost` suffix is fixed), an Upstream host, a Local port with a **Pick active port** menu listing listening local user ports, copy/open HTTPS URL buttons, and a Tunnel switch; a tunneled row adds a mode (`workers.dev` or Custom domain, with DNS suggestions), a **Protect** edge-token switch with copy/rotate actions, Copy/Open tunnel URL, and live per-rule worker tags (**Tunnel connected**, **Tunnel connecting**, **Tunnel conflict**) while the tunnel runs. The global listener port is separate. Below the table, **Save HTTPS rules**, **Save & apply**, and **Apply proxy** sit alongside the *HTTPS proxy service* and *Cloudflare tunnel service* switches and log actions; the Cloudflare card holds the masked **Cloudflare API token** (with a required-permissions tooltip), account ID, detected `workers.dev` subdomain and zones, **Save Cloudflare settings** (which auto-discovers account/zone metadata when a token is configured), and a confirmed **Remove Cloudflare token** button. **Local certificate trust** shows an inline CA trust badge (`Trusted`, `Not trusted`, or `Not created`) in its header and warns if the proxy runs while untrusted; CA trust remains its own explicit action. Published means the routing snapshot matches saved intent; it does not mean Chrome can connect. DNS, listener TLS, upstream TCP, and the optional explicit local (`Check HTTPS response`) and public tunnel (`Check tunnel response`) HTTP checks have separate results and never run automatically.

The menu bar shows managed database instances (with **Stop all** in the *Databases* header when instances are running), a failure summary (`<n> running · <m> failed`) in the panel header and tray tooltip when any workspace resource is `Failed`, one HTTPS proxy, and a Cloudflare tunnel row (with its switch and worker count) once a tunnel is configured, offering hover copy/open buttons for a single rule and a popup menu when multiple rules are active. Quit distinguishes keeping app-owned services from stopping selected resources. Diagnostics can take the user to the relevant settings without overwriting a dirty database or HTTPS draft. Screenshots used for review are generated by the relevant layout test during the current run; the [acceptance matrix](resources-acceptance.md) records which test owns each behavior.

These representative images come from the synthetic layout test `TestDevelopmentDesignLayouts` (fixed `designFixture` data, no machine state):

- [Software at 900×520](images/development/software-resource-900x520.png)
- [HTTPS rules at 900×520](images/development/https-rules-900x520.png)

To refresh them, render into the ignored `build/` directory, then copy `build/shots/TestDevelopmentDesignLayouts/Software-900x520-dark-false/Software-900x520-dark-false.png` and the matching `HTTPS-900x520-dark-false` image over these two files:

```sh
MSM_SHOTS=build/shots go test . -run '^TestDevelopmentDesignLayouts$' -count=1
```

Every screenshot test writes to `$MSM_SHOTS/<test name>/` through `saveShot` in `shots_test.go`.

## Java, Python and external database locations

The software list contains seven entries. Java automatically loads exact releases when opened and keeps **Refresh available versions** for manual refresh; installed versions remain usable when the catalog fails. Python has a uv-specific page that automatically loads the selected executable, managed installed versions, global pin, and available downloads when opened (while keeping **Refresh Python versions**). New database instances select an exact installed package, a port, and an optional new absolute data location. Existing instance details show the package version and resolved data path. External permanent deletion is disabled with its path and retention reason. These views have 900×520 and 1120×720 light/dark layout tests in `TestUIAdditionalRuntimes` and `TestUIDatabaseLocations`.

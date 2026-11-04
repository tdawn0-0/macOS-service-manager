# macOS Service Manager

One native app for everything that starts by itself on a Mac: Homebrew
services, launch agents and daemons, and login items. Written in Go with
[MyGo](https://github.com/egoist/mygo); its interface is drawn by MyGo on the
GPU, with no webview and no JavaScript.

## What it manages

| Source | Read | Change |
|---|---|---|
| **Homebrew** services | `brew services`, and launchd for those registered as root | start, stop, restart; start at login |
| **Login items** | System Settings' database, with Full Disk Access; otherwise System Events, for the apps only | add and remove the apps that open at login; helpers that apps registered are read-only (System Settings has their switch) |
| **User agents** `~/Library/LaunchAgents` | property lists + launchd | load, unload, start, stop, restart; enable/disable; **create, edit, remove** |
| **System agents** `/Library/LaunchAgents` | same | load, unload, start, stop, restart; enable/disable; **create, edit, remove**, after macOS asks you to authenticate |
| **Daemons** `/Library/LaunchDaemons` | same, without root | the same, all after macOS asks you to authenticate |

- A sidebar of sources, a searchable list grouped by source with a Running /
  Stopped filter, and details that open with the item chosen: its values, a
  switch for whether it starts by itself, its actions, and its **live logs**
  (the end of its log files, followed as they grow).
- A Homebrew service is listed once: launchd's job for it is not listed again
  under agents or daemons.
- Choose several items (⌘-click, Shift-click) to start or stop them together.
  Daemons changed together ask for your password once.
- **Menu bar:** the app stays in the menu bar when its window is closed. Its
  icon shows how many Homebrew services and user agents run, and opens a panel
  with a switch for each.
- **Welcome guide:** where macOS keeps the app from the login items' database, the
  first launch asks for Full Disk Access, and says what skipping it costs; Login
  Items offers it again. Allowing it opens System Settings: a small page follows
  its window with an icon to drag, the icon shows the way into the list, and
  during the drag all but that window darkens.
- **New jobs** run a shell command or a program, as a service (restarting never,
  after a crash or always; when a disk mounts; while a queue directory has files),
  at an interval, or on a calendar in the fields of cron. A daemon can also set its
  user, group and root directory, which launchd ignores outside the system domain.
- **Editing or removing a system agent or daemon** asks first, and macOS then asks
  for your password once. An edit says which keys of the file change, and asks
  nothing if none do. A removal says that the Trash is no undo for it (see Notes).
- `⌘R` refreshes, `⌘F` goes to the search. A right click on a row lists
  everything that can be done to it.
- Reading never asks for a password. Changing something does only where macOS
  requires it, which the lock icon marks.

## Development

Needs Go 1.27.3 or later, and Homebrew for the Homebrew source.

```bash
go tool mygo dev      # run with live reload
go test ./...         # views run without a window; backends run on stubbed commands
go tool mygo build    # a build for this Mac, signed ad hoc: build/darwin-arm64/*.app and *.dmg
```

### Packaging

```bash
scripts/package.sh                 # universal, signed: build/darwin-universal/*.app and *.dmg
scripts/package.sh --arch arm64    # one architecture
```

The script runs the tests, signs with the keychain's `Developer ID Application`
certificate (or `SIGNING_IDENTITY`), with the hardened runtime and a timestamp,
and checks the app and the disk image. Other Macs warn before opening what is
not notarized, so give it a profile to notarize and staple the disk image:

```bash
xcrun notarytool store-credentials msm --apple-id <you> --team-id <team>
NOTARY_PROFILE=msm scripts/package.sh
```

The app signed this way is a new app to macOS: it asks once more for permission
to control System Events, and needs Full Disk Access allowed again. The same
certificate keeps both across versions.

### Icons

`assets/icons/*.svg` are the sources, and `scripts/icons.sh` (which needs
`rsvg-convert`) renders them: `resources/icon.png` becomes the app's icon,
`resources/icon-dev.png` the icon of `mygo dev`, and `assets/icons/menubar.png`
the menu bar's, built into the binary. That one is black on transparent, for
macOS to tint. `assets/icons/guide.png` and `drag.png` are the welcome guide's
icon, and what follows the pointer as it is dragged.

### Layout

```
main.go, menubar.go     the window, and the menu bar icon with its panel
guide.go                the welcome guide, its window, and the overlay that shows where to drag
app.go                  the state, and the background work that changes it
view*.go                sidebar, list, details, logs, the editor of launchd's jobs
theme.go, icons.go      the look: colors, status dots, buttons, icons
internal/service        the model every source shares: Item, Action, Provider
internal/brew           Homebrew services
internal/launchd        launch agents and daemons; the plist editor (edit.go), calendars (calendar.go)
internal/loginitem      login items, from System Settings' database (database.go)
internal/logs           the end of a log file
internal/system         running commands, as root through macOS' own prompt; where System Settings' window is
```

Each source is a `service.Provider`; the interface knows nothing of Homebrew
or launchd. State is read and written on the main thread only; commands run in
a goroutine and hand their result back with `Window.Update`.

### Notes

- **Login items** come from `/private/var/db/com.apple.backgroundtaskmanagement`,
  which macOS lets an app read only with Full Disk Access. A process started from
  a terminal that has it inherits it, so test this with `open`, not from the
  terminal. The database's format is Apple's own and changes with macOS.
- **Without that access,** System Events lists the apps that open at login, and
  the list says that it leaves out the helpers, with two ways to change that:
  allowing Full Disk Access, once, through the guide, or `sfltool dumpbtm`. While
  the guide is open the source is not read, so that System Events asks nothing
  that allowing the access would make needless. `sfltool` asks for an
  administrator every time it runs, so the app runs it only when the user asks,
  and keeps its answer in the user's cache directory, showing how old it is.
- **Homebrew's autostart** is the service's property list in
  `~/Library/LaunchAgents`, which the switch adds and removes itself, as `brew
  services start` and `stop` do but without starting or stopping the service.
  Start and Stop are `brew services run` or `start`, and `stop` or `stop --keep`,
  chosen to leave the autostart as it is.
- **Services registered as root** (`sudo brew services`) are jobs of the system
  domain, of which `brew services` run as the user reports nothing: their state
  comes from launchd, and they change with `launchctl`, after macOS asks.
- The first time the app uses System Events, to list the login items without
  Full Disk Access, or to add or remove one, macOS asks whether it may control it.
- Daemons are changed with `osascript … with administrator privileges`, which shows
  macOS' authentication dialog. A new daemon or system agent is written to a
  temporary file, checked, and installed by root as `root:wheel`, without which
  launchd refuses it; a daemon loads in the same step, so that macOS asks once.
- An edit of one replaces the file all at once, and only then unloads and loads the
  job, so a failure leaves the job running as it was. A job that launchd will not
  unload is not removed either, since it would run on without a file.
- **Removing a daemon or system agent is not undone by the Trash.** The Trash keeps a
  copy, which root hands over to the user by way of a temporary directory, so Put
  Back cannot return it. To restore it, reinstall the software it came from, or
  install the file as root; for a daemon, `sudo install -m 644 -o root -g wheel
  <file> /Library/LaunchDaemons/` and `sudo launchctl bootstrap system <that path>`.
- Editing a job keeps what the form cannot say: other keys of its property list,
  `KeepAlive` conditions, and calendars that are not all the combinations of their
  values.
- UI tests write screenshots of every pane, light, dark and at the smallest
  window size, with `MSM_SHOTS=<dir> go test -run TestShots .`

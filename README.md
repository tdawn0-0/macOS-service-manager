# macOS Service Manager

One native app for everything that starts by itself on a Mac: Homebrew
services, launch agents and daemons, and login items. Written in Go with
[MyGo](https://github.com/egoist/mygo); its interface is drawn by MyGo on the
GPU, with no webview and no JavaScript.

## What it manages

| Source | Read | Change |
|---|---|---|
| **Homebrew** services | `brew services`, and launchd for those registered as root | start, stop, restart; start at login |
| **Login items** | System Settings' database, which needs no permission | add and remove the apps that open at login; helpers that apps registered are read-only (System Settings has their switch) |
| **User agents** `~/Library/LaunchAgents` | property lists + launchd | load, unload, start, stop, restart; enable/disable; **create, edit, remove** |
| **System agents** `/Library/LaunchAgents` | same | load, unload, start, stop, restart; enable/disable |
| **Daemons** `/Library/LaunchDaemons` | same, without root | the same, after macOS asks you to authenticate |

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
- `⌘R` refreshes, `⌘F` goes to the search. A right click on a row lists
  everything that can be done to it.
- Reading never asks for a password. Changing something does only where macOS
  requires it, which the lock icon marks.

## Development

Needs Go 1.27.3 or later, and Homebrew for the Homebrew source.

```bash
go tool mygo dev      # run with live reload
go test ./...         # views run without a window; backends run on stubbed commands
go tool mygo build    # package the app: build/darwin-arm64/*.app and *.dmg
```

```
main.go, menubar.go     the window, and the menu bar icon with its panel
app.go                  the state, and the background work that changes it
view*.go                sidebar, list, details, logs, the launch agent editor
theme.go, icons.go      the look: colors, status dots, buttons, icons
internal/service        the model every source shares: Item, Action, Provider
internal/brew           Homebrew services
internal/launchd        launch agents and daemons; the plist editor (edit.go)
internal/loginitem      login items, from System Settings' database (database.go)
internal/logs           the end of a log file
internal/system         running commands, and as root through macOS' own prompt
```

Each source is a `service.Provider`; the interface knows nothing of Homebrew
or launchd. State is read and written on the main thread only; commands run in
a goroutine and hand their result back with `Window.Update`.

### Notes

- **Login items** come from `/private/var/db/com.apple.backgroundtaskmanagement`,
  which anyone can read. Its format is Apple's own and changes with macOS. Where
  it cannot be read, System Events lists the apps and `sfltool dumpbtm` the
  rest. `sfltool` asks for an administrator every time it runs, so the app runs
  it at most once per launch, and keeps its answer in the user's cache directory
  until the database changes.
- **Homebrew's autostart** is the service's property list in
  `~/Library/LaunchAgents`, which the switch adds and removes itself, as `brew
  services start` and `stop` do but without starting or stopping the service.
  Start and Stop are `brew services run` or `start`, and `stop` or `stop --keep`,
  chosen to leave the autostart as it is.
- **Services registered as root** (`sudo brew services`) are jobs of the system
  domain, of which `brew services` run as the user reports nothing: their state
  comes from launchd, and they change with `launchctl`, after macOS asks.
- The first time the app adds or removes a login item, macOS asks whether it may
  control System Events.
- Daemons are changed with `osascript … with administrator privileges`, which shows
  macOS' authentication dialog.
- UI tests write screenshots of every pane, light, dark and at the smallest
  window size, with `MSM_SHOTS=<dir> go test -run TestShots .`

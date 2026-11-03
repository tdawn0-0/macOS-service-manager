# macOS Service Manager

One native app for everything that starts by itself on a Mac: Homebrew
services, launch agents and daemons, and login items. Written in Go with
[MyGo](https://github.com/egoist/mygo); its interface is drawn by MyGo on the
GPU, with no webview and no JavaScript.

## What it manages

| Source | Read | Change |
|---|---|---|
| **Homebrew** services | `brew services` | run, start, stop, restart, kill; start at login |
| **User agents** `~/Library/LaunchAgents` | property lists + launchd | load, unload, start, stop, restart; enable/disable; **create, edit, remove** |
| **System agents** `/Library/LaunchAgents` | same | load, unload, start, stop, restart; enable/disable |
| **Daemons** `/Library/LaunchDaemons` | same, without root | the same, after macOS asks you to authenticate |
| **Login items** | System Settings' database (`sfltool dumpbtm`) | add and remove the apps that open at login; helpers that apps registered are read-only (System Settings has their switch) |

- A sidebar of sources, a searchable list with a Running / Stopped filter, and
  a panel for the item chosen: its values, a switch for whether it starts by
  itself, its actions, and its **live logs** (the end of its log files,
  followed as they grow).
- Choose several items (⌘-click, Shift-click) to start or stop them together.
- **Menu bar:** the app stays in the menu bar when its window is closed. Its
  icon shows how many Homebrew services run, and opens a panel with a switch
  for each.
- `⌘R` refreshes.
- Changes that have consequences ask first: Homebrew cannot change whether a
  *running* service starts at login, so the app stops it and starts it again,
  and tells you so before it does; removing an item, and unloading or
  disabling a daemon, ask too.

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
internal/service        the model every source shares: Item, Action, Provider
internal/brew           Homebrew services
internal/launchd        launch agents and daemons; the plist editor (edit.go)
internal/loginitem      login items
internal/logs           the end of a log file
internal/system         running commands, and as root through macOS' own prompt
```

Each source is a `service.Provider`; the interface knows nothing of Homebrew
or launchd. State is read and written on the main thread only; commands run in
a goroutine and hand their result back with `Window.Update`.

### Notes

- The first time the app adds or removes a login item, macOS asks whether it may
  control System Events.
- Daemons are changed with `osascript … with administrator privileges`, which shows
  macOS' authentication dialog.
- `brew services` run as root (`sudo brew services`) belong to the system
  domain: they show up under Daemons.

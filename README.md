# macOS Service Manager

A native desktop application for managing services on macOS. It gives one
interface to Homebrew services and to launchd agents (launchctl).

## Features

- 🍺 Homebrew services
    - List all Homebrew services with their status and user
    - Run / Stop from the list; Run, Start, Restart, Stop and Kill from the details
    - Show a service's details, copy any value, open its logs in Console
- 🚀 LaunchAgents
    - List the agents in `~/Library/LaunchAgents` and `/Library/LaunchAgents`
    - Load / Unload from the list; Start and Stop from the details
    - Show status (loaded, running, PID) and the agent's configuration
      (program, arguments, paths, …)
- `⌘R` refreshes the page being shown.

## Development

The app is written in Go with [MyGo](https://github.com/egoist/mygo). Its
interface is native UI that MyGo draws on the GPU: there is no webview, no
frontend and no JavaScript.

### Prerequisites

- Go 1.27.3 or later
- Homebrew (for the Homebrew services page)

### Commands

```bash
go tool mygo dev      # run with live reload
go test ./...         # test the views (without a window) and the backends
go tool mygo build    # package the app: build/darwin-arm64/*.app and *.dmg
```

### Layout

```
main.go             the window
app.go              the state, and the background work that changes it
view*.go            the interface: tabs, lists and the detail dialog
internal/brew       `brew services` (list, info, start/stop/…)
internal/launchd    launch agents: scans property lists, `launchctl list` and commands
internal/system     runs commands; opens logs in Console
internal/detail     the rows of a detail dialog
```

State is read and written on the main thread only; commands run in a
goroutine and hand their result back with `Window.Update`.

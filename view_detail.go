package main

import (
	"github.com/egoist/mygo/ui"

	"macos-service-manager/internal/brew"
	"macos-service-manager/internal/detail"
	"macos-service-manager/internal/launchd"
)

// detailDialog shows a service in detail over the window: its values, and
// the commands it takes.
func (a *app) detailDialog(c *ui.Context) {
	d := &a.detail
	ui.Modal(c, &d.open, func() {
		ui.Column(c).Width(620).Gap(12).Children(func() {
			ui.Text(c, d.title).FontSize(18).Bold().SingleLine()
			a.detailBody(c)
			ui.Row(c).Gap(8).Justify(ui.End).Children(func() {
				switch d.kind {
				case detailBrew:
					a.brewActions(c, d.key)
				case detailLaunchd:
					a.agentActions(c, d.key)
				}
				if ui.PrimaryButton(c, "Close").Clicked() {
					d.open = false
				}
			})
		})
	})
}

func (a *app) detailBody(c *ui.Context) {
	d := &a.detail
	t := c.Theme()
	switch {
	case d.err != "":
		ui.Text(c, "Error: "+d.err).TextColor(t.Danger)
	case len(d.rows) == 0 && d.loading:
		ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
			ui.Spinner(c).Label("Loading")
			ui.Text(c, "Loading…").TextColor(t.TextMuted)
		})
	case len(d.rows) == 0:
		ui.Text(c, "No data").TextColor(t.TextMuted)
	default:
		ui.Scroll(c).MaxHeight(340).Gap(6).Selectable().Children(func() {
			for _, row := range d.rows {
				a.detailRow(c, row)
			}
		})
	}
}

func (a *app) detailRow(c *ui.Context, row detail.Row) {
	t := c.Theme()
	ui.Row(c.Key(row.Key)).AlignItems(ui.Center).Gap(8).Children(func() {
		ui.Text(c, row.Key).TextColor(t.TextMuted).Width(150)
		ui.Text(c, row.Value).Grow(1).MinWidth(0)
		if row.LogPath {
			ui.Button(c, "Open log").Tooltip("Open log file in Console").
				OnClick(func() { a.openLog(row.Value) })
		}
		ui.Button(c, "Copy").OnClick(func() {
			a.copy(row.Value)
			a.toast("Copied")
		})
	})
}

func (a *app) brewActions(c *ui.Context, name string) {
	busy := a.brewTab.busy[name]
	for _, action := range []struct {
		command brew.Command
		label   string
		tip     string
	}{
		{brew.Run, "Run", "Run without registering to launch at login"},
		{brew.Start, "Start", "Start and register it to launch at login"},
		{brew.Restart, "Restart", "Stop and start immediately and register it to launch at login"},
		{brew.Stop, "Stop", "Stop immediately and unregister it from launching at login"},
		{brew.Kill, "Kill", "Stop immediately but keep it registered to launch at login"},
	} {
		ui.Button(c, action.label).Tooltip(action.tip).Disabled(busy).
			OnClick(func() { a.manageBrew(name, action.command) })
	}
}

func (a *app) agentActions(c *ui.Context, path string) {
	s, ok := a.agent(path)
	if !ok {
		return
	}
	busy := a.agents.busy[path]
	do := func(command launchd.Command) func() {
		return func() { a.manageAgent(s, command) }
	}
	if !s.Loaded {
		ui.Button(c, "Load").Tooltip("Load service (register with launchd)").Disabled(busy).OnClick(do(launchd.Load))
		return
	}
	ui.Button(c, "Unload").Tooltip("Unload service (unregister from launchd)").Disabled(busy).OnClick(do(launchd.Unload))
	if s.Running {
		ui.Button(c, "Stop").Tooltip("Stop the running service").Disabled(busy).OnClick(do(launchd.Stop))
	} else {
		ui.Button(c, "Start").Tooltip("Start the service").Disabled(busy).OnClick(do(launchd.Start))
	}
}

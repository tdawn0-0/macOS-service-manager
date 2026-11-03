package main

import (
	"fmt"
	"path/filepath"

	"github.com/egoist/mygo/ui"

	"macos-service-manager/internal/service"
)

// detailPane shows the item chosen, or what to do with several.
func (a *app) detailPane(c *ui.Context) {
	ui.Column(c).Width(380).Padding(16).Gap(12).Children(func() {
		switch n := a.chosen.Len(); {
		case n > 1:
			a.bulkPane(c, n)
		case n == 1:
			if it, ok := a.target(); ok {
				a.itemPane(c, it)
				return
			}
			fallthrough
		default:
			ui.Column(c).Grow(1).Center().Children(func() {
				ui.Text(c, "Select an item").TextColor(c.Theme().TextMuted)
			})
		}
	})
}

func (a *app) bulkPane(c *ui.Context, n int) {
	t := c.Theme()
	start, stop := a.bulkCounts()
	ui.Text(c, fmt.Sprintf("%d items selected", n)).FontSize(18).Bold()
	ui.Text(c, "Start and stop act on the items they apply to.").TextColor(t.TextMuted).FontSize(12)
	ui.Row(c).Gap(8).Children(func() {
		ui.PrimaryButton(c, fmt.Sprintf("Start %d", start)).Disabled(start == 0).OnClick(func() { a.bulk(true) })
		ui.Button(c, fmt.Sprintf("Stop %d", stop)).Disabled(stop == 0).OnClick(func() { a.bulk(false) })
		ui.Button(c, "Clear selection").OnClick(a.chosen.Clear)
	})
}

// toggleLabel is what the switch of an item's autostart says.
func toggleLabel(it service.Item) string {
	switch it.Source {
	case service.Brew:
		return "Start at login"
	case service.Daemon:
		return "Enabled (starts at boot)"
	}
	return "Enabled (starts at login)"
}

func (a *app) itemPane(c *ui.Context, it service.Item) {
	t := c.Theme()
	p := a.providers[it.Source]
	actions := p.Actions(it)
	busy := a.busy[it.ID]

	ui.Text(c, it.Name).FontSize(18).Bold().Selectable()
	ui.Row(c).AlignItems(ui.Center).Gap(8).Children(func() {
		if it.State != service.StateNone {
			ui.Text(c, "●").TextColor(stateColor(t, it.State)).FontSize(10)
			state := it.State.String()
			if it.PID > 0 {
				state += fmt.Sprintf(" · pid %d", it.PID)
			}
			ui.Text(c, state)
		}
		ui.Badge(c, it.Source.Short())
		if busy {
			ui.Spinner(c).Label("Working")
		}
	})

	if toggle := service.Toggle(actions); toggle != "" {
		on := it.Autostart == service.AutostartOn
		changed := false
		// A click on the label toggles the switch too.
		ui.Form(c, func() {
			ui.Field(c, toggleLabel(it), func() {
				changed = ui.Switch(c.Key("autostart"), &on).Disabled(busy).Changed()
			})
		})
		if changed {
			act := service.Disable
			if on {
				act = service.Enable
			}
			a.request(it, act)
		}
	}

	ui.Row(c).Gap(6).Children(func() {
		for _, act := range actions {
			if act == service.Enable || act == service.Disable || act == service.Remove {
				continue
			}
			ui.Button(c, act.Label()).Tooltip(actionTip(it, act)).Disabled(busy).OnClick(func() { a.request(it, act) })
		}
	})
	ui.Row(c).Gap(6).Children(func() {
		if it.Source == service.UserAgent {
			ui.Button(c, "Edit…").Tooltip("Edit the launch agent").OnClick(func() { a.editAgent(it) })
		}
		if filepath.IsAbs(it.Path) && it.Source != service.LoginItem {
			ui.Button(c, "Reveal").Tooltip("Show the file in Finder").OnClick(func() { a.reveal(it.Path) })
		}
		if it.Source == service.LoginItem {
			if filepath.IsAbs(it.Path) {
				ui.Button(c, "Reveal").Tooltip("Show the app in Finder").OnClick(func() { a.reveal(it.Path) })
			} else {
				ui.Button(c, "Open System Settings").
					Tooltip("This helper belongs to its app: System Settings has its switch").
					OnClick(a.openLoginSettings)
			}
		}
		if service.Has(actions, service.Remove) {
			ui.Button(c, "Remove…").Disabled(busy).OnClick(func() { a.request(it, service.Remove) })
		}
	})

	ui.Tabs(c, &a.detail.tab, "Info", "Logs")
	if a.detail.tab == 0 {
		a.infoTab(c)
	} else {
		a.logTab(c)
	}
}

// actionTip explains an action, where its name does not.
func actionTip(it service.Item, a service.Action) string {
	if it.Source == service.Brew {
		switch a {
		case service.Run:
			return "Run without registering it to start at login"
		case service.Start:
			return "Run it and register it to start at login"
		case service.Restart:
			return "Stop and start again, keeping whether it starts at login"
		case service.Stop:
			return "Stop it and unregister it from starting at login"
		case service.Kill:
			return "Stop it but keep it registered to start at login"
		}
	}
	switch a {
	case service.Load:
		return "Register it with launchd"
	case service.Unload:
		return "Unregister it from launchd, which stops it"
	case service.Start:
		return "Start it now"
	case service.Restart:
		return "Stop it and start it again"
	case service.Stop:
		return "Ask it to stop (SIGTERM); a job that is kept alive starts again"
	}
	return ""
}

func (a *app) infoTab(c *ui.Context) {
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
		ui.Scroll(c).Grow(1).Gap(6).Selectable().Children(func() {
			for _, row := range d.rows {
				ui.Row(c.Key(row.Key)).Gap(8).Children(func() {
					ui.Text(c, row.Key).TextColor(t.TextMuted).Width(104).FontSize(12)
					ui.Text(c, row.Value).Grow(1).MinWidth(0)
					ui.Button(c, "Copy").Tooltip("Copy the value").Padding(2, 8).FontSize(11).OnClick(func() {
						a.copy(row.Value)
						a.toast("Copied")
					})
				})
			}
		})
	}
}

func (a *app) logTab(c *ui.Context) {
	d, l := &a.detail, &a.log
	t := c.Theme()
	if len(d.logs) == 0 {
		if d.loading {
			ui.Text(c, "Loading…").TextColor(t.TextMuted)
		} else {
			ui.Text(c, "This item writes no log file.").TextColor(t.TextMuted)
		}
		return
	}
	if len(d.logs) > 1 {
		labels := make([]string, len(d.logs))
		for i, f := range d.logs {
			labels[i] = f.Label
		}
		ui.Segmented(c, &l.file, labels...).Label("Log file")
	}
	file := d.logs[min(l.file, len(d.logs)-1)]
	ui.Row(c).AlignItems(ui.Center).Gap(8).Children(func() {
		ui.Text(c, file.Path).SingleLine().Grow(1).MinWidth(0).TextColor(t.TextMuted).FontSize(11).Tooltip(file.Path)
		ui.Button(c, "Console").Tooltip("Open in Console").OnClick(func() { a.openLog(file.Path) })
	})
	switch {
	case l.err != "":
		ui.Text(c, l.err).TextColor(t.Danger).FontSize(12)
	case l.lines == nil:
		ui.Text(c, "Reading…").TextColor(t.TextMuted)
	case len(l.lines) == 0:
		ui.Text(c, "The log is empty.").TextColor(t.TextMuted)
	default:
		if l.truncated {
			ui.Text(c, fmt.Sprintf("The last %d lines, updated %s", len(l.lines), ago(l.last))).TextColor(t.TextMuted).FontSize(11)
		}
		ui.List(c, &l.list, len(l.lines), func(i int) {
			ui.Text(c, l.lines[i]).Font("Menlo").FontSize(11).Selectable()
		}).Grow(1)
	}
}

package main

import (
	"github.com/egoist/mygo/ui"

	"macos-service-manager/internal/brew"
)

func (a *app) brewPage(c *ui.Context) {
	tab := &a.brewTab
	header(c, "Homebrew Services", tab.loading, a.refreshBrew)
	if pageMessage(c, tab.loading, tab.err, len(tab.items), "No Homebrew services found", a.refreshBrew) {
		return
	}
	tab.list.Key = func(i int) any { return tab.items[i].Name }
	tab.list.Label = func(i int) string { return tab.items[i].Name }
	ui.List(c, &tab.list, len(tab.items), func(i int) {
		a.brewRow(c, tab.items[i])
	}).Grow(1).Gap(8)
}

func (a *app) brewRow(c *ui.Context, s brew.Service) {
	t := c.Theme()
	busy := a.brewTab.busy[s.Name]
	color := t.TextMuted
	switch s.Status {
	case "started":
		color = t.Success
	case "scheduled":
		color = t.Warning
	case "error":
		color = t.Danger
	}
	command, label := brew.Run, "Run"
	if s.Active() {
		command, label = brew.Stop, "Stop"
	}

	rowCard(c).Children(func() {
		ui.Text(c, s.Name).SingleLine().Grow(1)
		status(c, color, s.Status)
		if s.User != nil && *s.User != "" {
			tag(c, *s.User)
		}
		ui.Button(c, label).Disabled(busy).OnClick(func() { a.manageBrew(s.Name, command) })
		if busy {
			ui.Spinner(c).Label("Working")
		}
		ui.Button(c, "Details").OnClick(func() { a.openBrewDetail(s.Name) })
	})
}

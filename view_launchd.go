package main

import (
	"github.com/egoist/mygo/ui"

	"macos-service-manager/internal/launchd"
)

func (a *app) agentsPage(c *ui.Context) {
	tab := &a.agents
	header(c, "LaunchAgents", tab.loading, a.refreshAgents)
	if pageMessage(c, tab.loading, tab.err, len(tab.items), "No LaunchAgents found", a.refreshAgents) {
		return
	}
	tab.list.Key = func(i int) any { return tab.items[i].Path }
	tab.list.Label = func(i int) string { return tab.items[i].Label }
	ui.List(c, &tab.list, len(tab.items), func(i int) {
		a.agentRow(c, tab.items[i])
	}).Grow(1).Gap(8)
}

func (a *app) agentRow(c *ui.Context, s launchd.Service) {
	t := c.Theme()
	busy := a.agents.busy[s.Path]
	color, word := t.TextMuted, "unloaded"
	switch {
	case s.Running:
		color, word = t.Success, "running"
	case s.Loaded:
		color, word = t.Warning, "loaded"
	}
	command, label := launchd.Load, "Load"
	if s.Loaded {
		command, label = launchd.Unload, "Unload"
	}

	rowCard(c).Children(func() {
		ui.Text(c, s.Label).SingleLine().Grow(1)
		status(c, color, word)
		tag(c, s.Domain)
		ui.Button(c, label).Disabled(busy).OnClick(func() { a.manageAgent(s, command) })
		if busy {
			ui.Spinner(c).Label("Working")
		}
		ui.Button(c, "Details").OnClick(func() { a.openAgentDetail(s) })
	})
}

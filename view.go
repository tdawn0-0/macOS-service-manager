package main

import (
	"fmt"
	"time"

	"github.com/egoist/mygo/ui"

	"macos-service-manager/internal/service"
)

// view builds the window: the sidebar of sources, the list of items, the
// details of the item chosen, and the dialogs and toasts over them.
func (a *app) view(c *ui.Context) {
	for _, message := range a.toasts {
		c.Toast(message)
	}
	a.toasts = a.toasts[:0]
	if c.Shortcut(ui.Cmd, ui.KeyR) {
		a.refreshAll()
	}
	a.build()
	a.syncDetail()
	if a.detail.tab == 1 {
		a.pollLog()
		c.After(logEvery)
	}

	t := c.Theme()
	ui.Row(c).Fill().AlignItems(ui.Stretch).Children(func() {
		a.sidebar(c)
		ui.Column(c).Width(1).Background(t.Border)
		a.listPane(c)
		ui.Column(c).Width(1).Background(t.Border)
		a.detailPane(c)
	})
	a.askDialog(c)
	a.editorDialog(c)
}

func (a *app) sidebar(c *ui.Context) {
	navItem := func(id, label string) {
		total, running := a.counts(id)
		ui.SidebarItem(c, id, nil, label).Children(func() {
			if running > 0 {
				ui.Badge(c, fmt.Sprintf("%d/%d", running, total)).Background(c.Theme().Success).TextColor(c.Theme().AccentText)
			} else if total > 0 {
				ui.Badge(c, fmt.Sprint(total))
			}
		})
	}
	if ui.Sidebar(c, &a.nav, func() {
		navItem(navAll, "All Items")
		ui.SidebarSection(c, "Services", nil, func() {
			navItem(string(service.Brew), service.Brew.Title())
		})
		ui.SidebarSection(c, "Startup Items", nil, func() {
			for _, src := range []service.Source{service.UserAgent, service.SystemAgent, service.Daemon, service.LoginItem} {
				navItem(string(src), src.Title())
			}
		})
	}).Width(210).Changed() {
		a.chosen.Clear()
	}
}

// navTitle is the title of the list for the sidebar's choice.
func (a *app) navTitle() string {
	if a.nav == navAll {
		return "All Items"
	}
	return service.Source(a.nav).Title()
}

func (a *app) listPane(c *ui.Context) {
	t := c.Theme()
	total, running := a.counts(a.nav)
	ui.Column(c).Grow(1).MinWidth(0).Padding(14, 16).Gap(10).Children(func() {
		ui.Row(c).AlignItems(ui.Center).Gap(8).Children(func() {
			ui.Column(c).Grow(1).MinWidth(0).Children(func() {
				ui.Text(c, a.navTitle()).FontSize(20).Bold().SingleLine()
				ui.Text(c, fmt.Sprintf("%d items, %d running", total, running)).TextColor(t.TextMuted).FontSize(12).SingleLine()
			})
			if a.loading() {
				ui.Spinner(c).Label("Loading")
			}
			if a.nav == navAll || a.nav == string(service.UserAgent) {
				ui.Button(c, "New Agent…").Tooltip("Create a launch agent").OnClick(a.newAgent)
			}
			if a.nav == navAll || a.nav == string(service.LoginItem) {
				ui.Button(c, "Add Login Item…").Tooltip("Open an app at login").OnClick(a.addLoginItem)
			}
			ui.Button(c, "Refresh").Tooltip("Refresh (⌘R)").Disabled(a.loading()).OnClick(a.refreshAll)
		})
		ui.Row(c).AlignItems(ui.Center).Gap(8).Children(func() {
			ui.SearchField(c, &a.query).Placeholder("Search name, owner or path").Label("Search").Grow(1)
			ui.Segmented(c, &a.filter, "All", "Running", "Stopped").Label("Filter")
		})
		for _, e := range a.errors() {
			ui.Text(c, e).TextColor(t.Danger).FontSize(12)
		}
		if len(a.visible) == 0 {
			a.emptyList(c)
			return
		}
		bulk := a.chosen.Len() > 1
		ui.List(c, &a.list, len(a.visible)).Grow(1).Gap(2).Rows(func(r ui.ListRow) {
			a.row(r.Context, a.visible[r.Index], bulk, r.Selected() && r.ListFocused())
		})
	})
}

func (a *app) emptyList(c *ui.Context) {
	t := c.Theme()
	ui.Column(c).Grow(1).Center().Gap(4).Children(func() {
		switch {
		case a.loading() && a.visibleSources() == 0:
			ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
				ui.Spinner(c).Label("Loading")
				ui.Text(c, "Loading…").TextColor(t.TextMuted)
			})
		case a.query != "" || a.filter != 0:
			ui.Text(c, "Nothing matches").TextColor(t.TextMuted)
			if ui.Button(c, "Clear search and filter").Clicked() {
				a.query, a.filter = "", 0
			}
		default:
			ui.Text(c, "No items").TextColor(t.TextMuted)
		}
	})
}

// visibleSources counts the sources of the navigation that have loaded.
func (a *app) visibleSources() int {
	n := 0
	for src, st := range a.sources {
		if (a.nav == navAll || a.nav == string(src)) && st.loaded {
			n++
		}
	}
	return n
}

// stateColor is the color of an item's status dot.
func stateColor(t *ui.Theme, s service.State) ui.Color {
	switch s {
	case service.StateRunning:
		return t.Success
	case service.StateScheduled, service.StateLoaded:
		return t.Warning
	case service.StateFailed:
		return t.Danger
	}
	return t.TextMuted
}

// autostartNote is a word about an item's autostart, when it is worth one:
// an item that is set up as the user would not expect.
func autostartNote(it service.Item) string {
	switch {
	case it.Source == service.Brew && it.Autostart == service.AutostartOn:
		return "at login"
	case it.Source == service.LoginItem && it.Autostart == service.AutostartOff:
		return "off"
	case it.Source != service.Brew && it.Source != service.LoginItem && it.Autostart == service.AutostartOff:
		return "disabled"
	}
	return ""
}

// row builds an item of the list. A row chosen while the list has the
// focus is on the accent color, where muted text would not show.
func (a *app) row(c *ui.Context, it service.Item, bulk, onAccent bool) {
	t := c.Theme()
	muted := t.TextMuted
	if onAccent {
		muted = t.AccentText
	}
	busy := a.busy[it.ID]
	actions := a.providers[it.Source].Actions(it)
	ui.Row(c).AlignItems(ui.Center).Gap(10).Padding(7, 10).Children(func() {
		dot := stateColor(t, it.State)
		if it.State == service.StateNone {
			dot = ui.Transparent
		}
		ui.Text(c, "●").TextColor(dot).FontSize(10)
		ui.Text(c, it.Name).SingleLine().Grow(1).MinWidth(0)
		if a.nav == navAll {
			ui.Text(c, it.Source.Short()).TextColor(muted).FontSize(11)
		}
		if note := autostartNote(it); note != "" {
			ui.Badge(c, note)
		}
		if it.State != service.StateNone {
			ui.Text(c, it.State.String()).TextColor(muted).FontSize(12).Width(60)
		}
		if busy {
			ui.Spinner(c).Label("Working")
		}
		if act := service.Primary(actions); act != "" && !bulk {
			ui.Button(c, act.Label()).Disabled(busy).OnClick(func() { a.request(it, act) })
		}
	})
}

func (a *app) askDialog(c *ui.Context) {
	if ui.AlertDialog(c, &a.ask.open, a.ask.title, a.ask.message, "Cancel", a.ask.button) == 1 && a.ask.run != nil {
		run := a.ask.run
		a.ask.run = nil
		run()
	}
}

// ago writes how long ago t was.
func ago(t time.Time) string {
	if t.IsZero() {
		return ""
	}
	d := time.Since(t).Round(time.Second)
	switch {
	case d < 2*time.Second:
		return "just now"
	case d < time.Minute:
		return fmt.Sprintf("%d s ago", int(d.Seconds()))
	case d < time.Hour:
		return fmt.Sprintf("%d min ago", int(d.Minutes()))
	}
	return fmt.Sprintf("%d h ago", int(d.Hours()))
}

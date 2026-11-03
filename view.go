package main

import (
	"github.com/egoist/mygo/ui"
)

var tabNames = []string{"Homebrew", "LaunchAgents"}

// view builds the window: the tabs, the page of the tab chosen, and the
// dialog and toasts over it.
func (a *app) view(c *ui.Context) {
	for _, message := range a.toasts {
		c.Toast(message)
	}
	a.toasts = a.toasts[:0]

	if c.Shortcut(ui.Cmd, ui.KeyR) {
		a.refreshAll()
	}

	ui.Column(c).Fill().Padding(16, 20).Gap(12).Children(func() {
		ui.Tabs(c, &a.tab, tabNames...)
		switch a.tab {
		case 0:
			a.brewPage(c)
		case 1:
			a.agentsPage(c)
		}
	})
	a.detailDialog(c)
}

// header is the title of a page, with the button refreshing it.
func header(c *ui.Context, title string, loading bool, refresh func()) {
	ui.Row(c).AlignItems(ui.Center).Gap(8).Children(func() {
		ui.Text(c, title).FontSize(22).Bold().Grow(1)
		if loading {
			ui.Spinner(c).Label("Loading")
		}
		ui.Button(c, "Refresh").Tooltip("Refresh (⌘R)").Disabled(loading).OnClick(refresh)
	})
}

// pageMessage fills a page that has no services to list with a message and
// reports whether it did.
func pageMessage(c *ui.Context, loading bool, err string, empty int, none string, retry func()) bool {
	t := c.Theme()
	switch {
	case err != "":
		ui.Column(c).Grow(1).Center().Gap(8).Children(func() {
			ui.Text(c, err).TextColor(t.Danger).TextAlign(ui.Center)
			ui.Button(c, "Retry").OnClick(retry)
		})
	case empty == 0 && loading:
		ui.Row(c).Grow(1).Center().Gap(8).Children(func() {
			ui.Spinner(c).Label("Loading")
			ui.Text(c, "Loading…").TextColor(t.TextMuted)
		})
	case empty == 0:
		ui.Column(c).Grow(1).Center().Children(func() {
			ui.Text(c, none).TextColor(t.TextMuted)
		})
	default:
		return false
	}
	return true
}

// status is a colored dot with a word.
func status(c *ui.Context, color ui.Color, word string) {
	ui.Row(c).AlignItems(ui.Center).Gap(4).Width(96).Children(func() {
		ui.Text(c, "●").TextColor(color).FontSize(10)
		ui.Text(c, word)
	})
}

// tag is a short word in a pill, beside a name.
func tag(c *ui.Context, word string) {
	ui.Badge(c, word)
}

// rowCard is the look of a service in a list.
func rowCard(c *ui.Context) ui.Element {
	return ui.Row(c).AlignItems(ui.Center).Gap(10).Padding(8, 12).
		Background(c.Theme().Surface).Radius(8)
}

package main

import (
	"fmt"
	"time"

	"github.com/egoist/mygo/ui"

	"macos-service-manager/internal/service"
)

// barHeight is the height of the title bar's row in each pane, whose top the
// window controls and dragging the window share.
const barHeight = 52

// view builds the window: a sidebar of sources over the desktop's material,
// the list of items, and the details of the items chosen once there are any.
func (a *app) view(c *ui.Context) {
	style(c)
	if !a.panelOpen {
		a.showToasts(c)
	}
	if c.Shortcut(ui.Cmd, ui.KeyR) {
		a.refreshAll()
	}
	a.build()
	a.pruneChosen()
	a.syncDetail()
	if a.detail.tab == 1 && len(a.detail.logs) > 0 {
		a.pollLog()
		c.After(logEvery)
	}

	t := c.Theme()
	if c.Vibrancy() {
		c.Root().Background(ui.Transparent)
	}
	ui.Row(c).Fill().AlignItems(ui.Stretch).Children(func() {
		a.sidebar(c)
		ui.Column(c).Grow(1).MinWidth(0).Background(t.Background).Children(func() { a.listPane(c) })
		if a.chosen.Len() > 0 {
			ui.Column(c).Width(1).Background(hairline(t))
			ui.Column(c).Width(336).Shrink(0).Background(t.Background).Children(func() { a.inspector(c) })
		}
	})
	a.askDialog(c)
	a.editorDialog(c)
}

func (a *app) showToasts(c *ui.Context) {
	for _, message := range a.toasts {
		c.Toast(message)
	}
	a.toasts = a.toasts[:0]
}

// sourceIcon is the icon of a source.
func sourceIcon(src service.Source) *ui.SVG {
	switch src {
	case service.Brew:
		return iconBrew
	case service.LoginItem:
		return iconLogin
	case service.UserAgent:
		return iconUser
	case service.SystemAgent:
		return iconUsers
	case service.Daemon:
		return iconServer
	}
	return iconAll
}

// sidebar has no background over the window's material, where there is one.
func (a *app) sidebar(c *ui.Context) {
	t := c.Theme()
	navItem := func(id string, icon *ui.SVG, label string) {
		total, running := a.counts(id)
		ui.SidebarItem(c, id, icon, label).Children(func() {
			count := fmt.Sprint(total)
			if running > 0 {
				count = fmt.Sprintf("%d/%d", running, total)
			}
			if total > 0 {
				ui.Text(c, count).FontSize(textCaption).Opacity(0.6).SingleLine()
			}
		})
	}
	column := ui.Column(c).Width(216).Shrink(0)
	if !c.Vibrancy() {
		column.Background(t.Surface)
	}
	column.Children(func() {
		ui.Row(c).Height(barHeight).Shrink(0).DragWindow()
		if ui.Sidebar(c, &a.nav, func() {
			navItem(navAll, iconAll, "All Items")
			ui.SidebarSection(c, "Apps & Services", nil, func() {
				for _, src := range []service.Source{service.Brew, service.LoginItem} {
					navItem(string(src), sourceIcon(src), src.Title())
				}
			})
			ui.SidebarSection(c, "launchd", nil, func() {
				for _, src := range []service.Source{service.UserAgent, service.SystemAgent, service.Daemon} {
					navItem(string(src), sourceIcon(src), src.Title())
				}
			})
		}).Grow(1).MinHeight(0).Background(ui.Transparent).Changed() {
			a.chosen.Clear()
			a.list.ScrollTo(0, ui.Start)
		}
	})
	ui.Column(c).Width(1).Background(hairline(t))
}

// navTitle is the title of the list for the sidebar's choice.
func (a *app) navTitle() string {
	if a.nav == navAll {
		return "All Items"
	}
	return service.Source(a.nav).Title()
}

func plural(n int, one string) string {
	if n == 1 {
		return "1 " + one
	}
	return fmt.Sprintf("%d %ss", n, one)
}

func (a *app) listPane(c *ui.Context) {
	t := c.Theme()
	total, running := a.counts(a.nav)
	ui.Row(c).Height(barHeight).Shrink(0).Padding(0, 10, 0, 18).AlignItems(ui.Center).Gap(6).DragWindow().Children(func() {
		ui.Column(c).Grow(1).MinWidth(0).Gap(1).Children(func() {
			ui.Text(c, a.navTitle()).FontSize(15).FontWeight(650).SingleLine()
			ui.Text(c, fmt.Sprintf("%s · %d running", plural(total, "item"), running)).TextColor(t.TextMuted).FontSize(textCaption).SingleLine()
		})
		if a.loading() {
			ui.Spinner(c).Label("Loading").Size(14, 14)
		}
		a.addButton(c)
		b := iconButton(c, iconRestart, "Refresh (⌘R)", t.TextMuted).Disabled(a.loading())
		b.OnClick(a.refreshAll)
	})
	ui.Row(c).Shrink(0).Padding(0, 16, 10).AlignItems(ui.Center).Gap(8).Children(func() {
		field := ui.SearchField(c, &a.query).Placeholder("Search").Label("Search").Grow(1).MinWidth(0)
		if c.Shortcut(ui.Cmd, ui.KeyF) {
			field.Focus()
		}
		ui.Segmented(c, &a.filter, "All", "Running", "Stopped").Label("Filter").Shrink(0)
	})
	ui.Column(c).Height(1).Shrink(0).Background(hairline(t))
	for _, e := range a.errors() {
		ui.Row(c).Shrink(0).Margin(8, 12, 0).Padding(7, 10).Gap(8).Radius(8).Background(t.Danger.Alpha(0.1)).TextColor(t.Danger).Children(func() {
			ui.Icon(c, iconWarn).FontSize(13).AlignSelf(ui.Start).Margin(1, 0, 0)
			ui.Text(c, e).FontSize(textSmall).Grow(1).MinWidth(0)
		})
	}
	a.restrictionNote(c)
	if len(a.visible) == 0 {
		a.emptyList(c)
		return
	}
	bulk := a.chosen.Len() > 1
	ui.List(c, &a.list, len(a.rows)).Grow(1).MinHeight(0).Padding(4, 8, 10).Rows(func(r ui.ListRow) {
		e := a.rows[r.Index]
		if e.header {
			a.sectionHeader(r.Context, e)
			return
		}
		a.row(r, e.item, bulk)
	})
}

// restrictionNote says what macOS keeps the source shown from listing, and how
// to change that. It only offers: asking for a password is the user's choice.
func (a *app) restrictionNote(c *ui.Context) {
	src := service.Source(a.nav)
	st := a.sources[src]
	if st == nil || st.limit == nil {
		return
	}
	t := c.Theme()
	ui.Column(c).Shrink(0).Margin(8, 12, 0).Padding(9, 10).Gap(8).Radius(8).Background(t.Text.Alpha(0.05)).Children(func() {
		ui.Row(c).Gap(8).Children(func() {
			ui.Icon(c, iconLock).FontSize(13).TextColor(t.TextMuted).AlignSelf(ui.Start).Margin(1, 0, 0)
			ui.Text(c, st.limit.Text).FontSize(textSmall).TextColor(t.TextMuted).Grow(1).MinWidth(0)
		})
		ui.Row(c).Wrap().Gap(6).Children(func() {
			if st.limit.Settings {
				labeledButton(c, iconOpen, "Allow Full Disk Access…", false).OnClick(func() { a.openGuide(src, false) })
			}
			labeledButton(c, iconLock, "Read Once with Password", false).Disabled(st.reading).OnClick(func() { a.readRestricted(src) })
		})
	})
}

// addButton offers what can be added to the list shown.
func (a *app) addButton(c *ui.Context) {
	t := c.Theme()
	switch a.nav {
	case navAll:
		iconButton(c, iconPlus, "Add", t.TextMuted).Menu(func(m *ui.Menu) {
			if m.Item("New Launch Agent…").Chosen() {
				a.newAgent()
			}
			if m.Item("Add Login Item…").Chosen() {
				a.addLoginItem()
			}
		})
	case string(service.UserAgent):
		iconButton(c, iconPlus, "New Launch Agent…", t.TextMuted).OnClick(a.newAgent)
	case string(service.LoginItem):
		iconButton(c, iconPlus, "Add Login Item…", t.TextMuted).OnClick(a.addLoginItem)
	}
}

// sectionHeader heads the items of a source, in the list of all of them.
func (a *app) sectionHeader(c *ui.Context, e listRow) {
	t := c.Theme()
	ui.Row(c).Height(30).Padding(0, 10).Gap(7).AlignItems(ui.Center).Background(t.Background).Children(func() {
		ui.Icon(c, sourceIcon(e.src)).FontSize(12).TextColor(t.TextMuted)
		ui.Text(c, e.src.Title()).FontSize(textCaption).FontWeight(650).TextColor(t.TextMuted).Grow(1)
		ui.Text(c, fmt.Sprint(e.count)).FontSize(textCaption).TextColor(t.TextMuted).Opacity(0.8)
	})
}

func (a *app) emptyList(c *ui.Context) {
	t := c.Theme()
	ui.Column(c).Grow(1).Center().Gap(10).Children(func() {
		switch {
		case a.loading() && a.visibleSources() == 0:
			ui.Spinner(c).Label("Loading").Size(20, 20)
			ui.Text(c, "Loading…").TextColor(t.TextMuted)
		case a.query != "" || a.filter != 0:
			ui.Icon(c, iconSearch).FontSize(26).TextColor(t.TextMuted).Opacity(0.5)
			ui.Text(c, "Nothing matches").TextColor(t.TextMuted)
			if ui.Button(c, "Clear search and filter").Clicked() {
				a.query, a.filter = "", 0
			}
		default:
			ui.Icon(c, sourceIcon(service.Source(a.nav))).FontSize(26).TextColor(t.TextMuted).Opacity(0.5)
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

// autostartNote is a word about an item's autostart, when it is worth one:
// an item that is set up as the user would not expect.
func autostartNote(it service.Item) string {
	switch {
	case it.Source == service.Brew && it.Admin:
		return "at boot"
	case it.Source == service.Brew && it.Autostart == service.AutostartOn:
		return "at login"
	case it.Source == service.Brew:
	case it.Source == service.LoginItem && it.Autostart == service.AutostartOff:
		return "off"
	case it.Source != service.LoginItem && it.Autostart == service.AutostartOff:
		return "disabled"
	}
	return ""
}

// actionIcon is the icon of the main action of an item in its row.
func actionIcon(act service.Action) *ui.SVG {
	switch act {
	case service.Start, service.Load:
		return iconPlay
	case service.Stop:
		return iconStop
	}
	return iconRestart
}

// row builds an item of the list: a state, a name and at most one button,
// which shows only while the pointer is over the row or the row is chosen. A
// row chosen is on the accent color, where muted text would not show.
func (a *app) row(r ui.ListRow, it service.Item, bulk bool) {
	c := r.Context
	t := c.Theme()
	onAccent := r.Selected()
	muted := t.TextMuted
	if onAccent {
		muted = t.AccentText.Alpha(0.8)
	}
	busy := a.busy[it.ID]
	row := ui.Row(c).Height(32).AlignItems(ui.Center).Gap(10).Padding(0, 4, 0, 10)
	hovered := row.Hovered()
	row.ContextMenu(func(m *ui.Menu) { a.itemMenu(m, it, true) })
	row.Children(func() {
		statusDot(c, it.State, onAccent)
		ui.Text(c, it.Name).SingleLine().Grow(1).MinWidth(0).Tooltip(it.Name)
		if it.Source == service.LoginItem && it.Owner != "" {
			ui.Text(c, it.Owner).TextColor(muted).FontSize(textCaption).SingleLine().MaxWidth(150)
		}
		if it.Admin {
			ui.Icon(c, iconLock).FontSize(11).TextColor(muted).Shrink(0).Tooltip("Changing it asks for your password")
		}
		if note := autostartNote(it); note != "" {
			ui.Text(c, note).TextColor(muted).FontSize(textCaption).SingleLine().Shrink(0)
		}
		if a.nav != string(service.LoginItem) || it.State != service.StateNone {
			ui.Text(c, it.State.String()).TextColor(muted).FontSize(textSmall).Width(60).TextAlign(ui.End).SingleLine().Shrink(0)
		}
		ui.Box(c).Size(26, 26).Center().Shrink(0).Children(func() {
			act := service.Primary(a.providers[it.Source].Actions(it))
			switch {
			case busy:
				ui.Spinner(c).Label("Working").Size(13, 13)
			case act != "" && !bulk && (hovered || r.Selected()):
				tint := t.Text
				if onAccent {
					tint = t.AccentText
				}
				iconButton(c, actionIcon(act), act.Label(), tint).OnClick(func() { a.request(it, act) })
			}
		})
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

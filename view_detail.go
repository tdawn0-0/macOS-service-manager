package main

import (
	"fmt"
	"path/filepath"
	"slices"
	"strings"

	"github.com/egoist/mygo/ui"

	"macos-service-manager/internal/service"
)

// inspector shows the item chosen, or what to do with several.
func (a *app) inspector(c *ui.Context) {
	t := c.Theme()
	ui.Row(c).Height(barHeight).Shrink(0).Padding(0, 10).AlignItems(ui.Center).Justify(ui.End).DragWindow().Children(func() {
		iconButton(c, iconClose, "Close details", t.TextMuted).OnClick(a.chosen.Clear)
	})
	if n := a.chosen.Len(); n > 1 {
		a.bulkPane(c, n)
	} else if it, ok := a.target(); ok {
		a.itemPane(c, it)
	}
}

func (a *app) bulkPane(c *ui.Context, n int) {
	t := c.Theme()
	start, stop := a.bulkCounts()
	ui.Column(c).Padding(0, 16).Gap(12).Children(func() {
		ui.Column(c).Gap(3).Children(func() {
			ui.Text(c, fmt.Sprintf("%d items selected", n)).FontSize(16).FontWeight(650)
			ui.Text(c, "Start and stop act on the items they apply to.").TextColor(t.TextMuted).FontSize(textSmall)
		})
		ui.Row(c).Gap(8).Children(func() {
			labeledButton(c, iconPlay, fmt.Sprintf("Start %d", start), true).Grow(1).Disabled(start == 0).OnClick(func() { a.bulk(true) })
			labeledButton(c, iconStop, fmt.Sprintf("Stop %d", stop), false).Grow(1).Disabled(stop == 0).OnClick(func() { a.bulk(false) })
		})
		ui.Button(c, "Clear selection").OnClick(a.chosen.Clear)
	})
}

// toggleLabel is what the switch of an item's autostart says.
func toggleLabel(it service.Item) string {
	switch {
	case it.Source == service.Brew && it.Admin:
		return "Start at boot"
	case it.Source == service.Brew:
		return "Start at login"
	case it.Source == service.Daemon:
		return "Load at boot"
	}
	return "Load at login"
}

// stateChip says what an item is doing, in the color of its state.
func stateChip(c *ui.Context, it service.Item) {
	t := c.Theme()
	color := stateColor(t, it.State)
	label := it.State.String()
	switch {
	case it.State == service.StateNone:
		label = "Not running"
	case it.PID > 0:
		label += fmt.Sprintf(" · PID %d", it.PID)
	}
	ui.Row(c).AlignSelf(ui.Start).AlignItems(ui.Center).Gap(7).Padding(3, 9, 3, 8).Radius(99).Background(color.Alpha(0.12)).Children(func() {
		if it.State != service.StateNone {
			statusDot(c, it.State, false)
		}
		ui.Text(c, label).FontSize(textCaption).FontWeight(600).TextColor(color).SingleLine()
	})
}

func (a *app) itemPane(c *ui.Context, it service.Item) {
	t := c.Theme()
	p := a.providers[it.Source]
	actions := p.Actions(it)
	busy := a.busy[it.ID]

	ui.Column(c).Shrink(0).Padding(0, 16, 12).Gap(12).Children(func() {
		// What it is.
		ui.Row(c).Gap(11).AlignItems(ui.Center).Children(func() {
			ui.Box(c).Size(38, 38).Shrink(0).Center().Radius(10).Background(t.Accent.Alpha(0.12)).Children(func() {
				ui.Icon(c, sourceIcon(it.Source)).FontSize(18).TextColor(t.Accent)
			})
			ui.Column(c).Grow(1).MinWidth(0).Gap(2).Children(func() {
				ui.Text(c, it.Name).FontSize(14).FontWeight(650).MaxLines(2).Selectable().Tooltip(it.Name)
				subtitle := it.Source.Title()
				if it.Kind != "" {
					subtitle = it.Kind
				}
				if it.Owner != "" && it.Source == service.LoginItem {
					subtitle += " · " + it.Owner
				}
				ui.Text(c, subtitle).FontSize(textCaption).TextColor(t.TextMuted).SingleLine()
			})
		})
		ui.Row(c).AlignItems(ui.Center).Gap(8).Children(func() {
			stateChip(c, it)
			if busy {
				ui.Spinner(c).Label("Working").Size(14, 14)
			}
		})
		a.actionRow(c, it, actions, busy)
		a.autostartCard(c, it, actions, busy)
	})

	d := &a.detail
	tab := d.tab
	if len(d.logs) > 0 {
		ui.Box(c).Shrink(0).Padding(0, 12).Children(func() { ui.Tabs(c, &d.tab, "Info", "Logs") })
	} else {
		tab = 0
	}
	if tab == 0 {
		a.infoTab(c)
	} else {
		a.logTab(c)
	}
}

// actionRow holds the actions an item is mostly used for, each with a place
// of its own, and a menu of the rest.
func (a *app) actionRow(c *ui.Context, it service.Item, actions []service.Action, busy bool) {
	t := c.Theme()
	ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
		primary := service.Primary(actions)
		if primary != "" {
			b := labeledButton(c, actionIcon(primary), primary.Label(), primary != service.Stop).Grow(1).Disabled(busy)
			b.Tooltip(actionTip(it, primary)).OnClick(func() { a.request(it, primary) })
		}
		if service.Has(actions, service.Restart) {
			b := labeledButton(c, iconRestart, service.Restart.Label(), false).Grow(1).Disabled(busy)
			b.Tooltip(actionTip(it, service.Restart)).OnClick(func() { a.request(it, service.Restart) })
		}
		if it.Source == service.LoginItem {
			if service.Has(actions, service.Remove) {
				ui.Button(c, "Remove…").Grow(1).Disabled(busy).OnClick(func() { a.request(it, service.Remove) })
			} else {
				labeledButton(c, iconOpen, "Open System Settings", false).Grow(1).OnClick(a.openLoginSettings)
			}
		}
		more := ui.Button(c, "").Label("More").Tooltip("More").Padding(6, 9).Shrink(0)
		more.Children(func() { ui.Icon(c, iconMore).FontSize(14).TextColor(t.Text) })
		more.Menu(func(m *ui.Menu) { a.itemMenu(m, it, false) })
	})
}

// itemMenu lists what can be done to an item. In a row's menu that is
// everything; below the buttons, what has no button of its own.
func (a *app) itemMenu(m *ui.Menu, it service.Item, inRow bool) {
	actions := a.providers[it.Source].Actions(it)
	busy := a.busy[it.ID]
	primary := service.Primary(actions)
	for _, act := range actions {
		switch {
		case act == service.Enable, act == service.Disable, act == service.Remove:
		case !inRow && (act == primary || act == service.Restart):
		default:
			if m.Item(act.Label()).Disabled(busy).Chosen() {
				a.request(it, act)
			}
		}
	}
	if slices.Contains(jobKinds, it.Source) && m.Item("Edit…").Chosen() {
		a.editJob(it)
	}
	if filepath.IsAbs(it.Path) {
		if m.Item("Reveal in Finder").Chosen() {
			a.reveal(it.Path)
		}
		if m.Item("Copy Path").Chosen() {
			a.copy(it.Path)
			a.toast("Copied")
		}
	}
	if it.Source == service.LoginItem && m.Item("Open System Settings").Chosen() {
		a.openLoginSettings()
	}
	if service.Has(actions, service.Remove) && (inRow || it.Source != service.LoginItem) {
		m.Separator()
		if m.Item("Remove…").Disabled(busy).Chosen() {
			a.request(it, service.Remove)
		}
	}
}

// card is a rounded group of a pane.
func card(c *ui.Context) ui.Element {
	return ui.Column(c).Radius(10).Background(c.Theme().Surface)
}

// autostartCard shows whether an item starts by itself: a switch, or for a
// helper, which belongs to its app, what System Settings says.
func (a *app) autostartCard(c *ui.Context, it service.Item, actions []service.Action, busy bool) {
	t := c.Theme()
	toggle := service.Toggle(actions)
	if it.Source == service.LoginItem {
		if len(actions) > 0 {
			return
		}
		on := "Off"
		if it.Autostart == service.AutostartOn {
			on = "On"
		}
		card(c).Padding(9, 12).Gap(2).Children(func() {
			ui.Row(c).Children(func() {
				ui.Text(c, "Run in the background").Grow(1)
				ui.Text(c, on).TextColor(t.TextMuted)
			})
			ui.Text(c, "Its app or System Settings turns this on or off.").FontSize(textCaption).TextColor(t.TextMuted)
		})
		return
	}
	if toggle == "" {
		return
	}
	on := it.Autostart == service.AutostartOn
	label := toggleLabel(it)
	set := func(on bool) {
		if on {
			a.request(it, service.Enable)
		} else {
			a.request(it, service.Disable)
		}
	}
	card(c).Padding(9, 12).Children(func() {
		ui.Row(c).AlignItems(ui.Center).Gap(10).Children(func() {
			// A click on the words does what a click on the switch does.
			words := ui.Column(c).Grow(1).MinWidth(0).Gap(1).Disabled(busy)
			words.OnClick(func() { set(!on) })
			words.Children(func() {
				ui.Text(c, label)
				if it.Admin {
					ui.Text(c, "Changing it asks for your password.").FontSize(textCaption).TextColor(t.TextMuted)
				}
			})
			if ui.Switch(c.Key("autostart"), &on).Label(label).Disabled(busy).Changed() {
				set(on)
			}
		})
	})
}

// actionTip explains an action, where its name does not.
func actionTip(it service.Item, a service.Action) string {
	if it.Admin {
		return a.Label() + " (asks for your password)"
	}
	if it.Source == service.Brew {
		switch a {
		case service.Start:
			return "Start it now; whether it starts at login stays as it is"
		case service.Stop:
			return "Stop it now; whether it starts at login stays as it is"
		case service.Restart:
			return "Stop it and start it again"
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
		ui.Text(c, d.err).TextColor(t.Danger).FontSize(textSmall).Padding(8, 16)
	case len(d.rows) == 0 && d.loading:
		ui.Row(c).Padding(12, 16).Gap(8).AlignItems(ui.Center).Children(func() {
			ui.Spinner(c).Label("Loading").Size(14, 14)
			ui.Text(c, "Loading…").TextColor(t.TextMuted)
		})
	case len(d.rows) > 0:
		ui.Scroll(c).Grow(1).MinHeight(0).Padding(8, 16, 16).Gap(14).Selectable().Children(func() {
			for start := 0; start < len(d.rows); {
				end := start + 1
				for end < len(d.rows) && d.rows[end].Group == d.rows[start].Group {
					end++
				}
				a.infoGroup(c, d.rows[start].Group, d.rows[start:end])
				start = end
			}
		})
	}
}

// infoGroup shows the values of a group in a card under its title.
func (a *app) infoGroup(c *ui.Context, title string, rows []service.Row) {
	t := c.Theme()
	ui.Column(c.Key(title)).Gap(6).Shrink(0).Children(func() {
		if title != "" {
			ui.Text(c, title).FontSize(textCaption).FontWeight(650).TextColor(t.TextMuted).Padding(0, 4)
		}
		card(c).Gap(1).Dividers(1, hairline(t)).Children(func() {
			for _, row := range rows {
				a.infoRow(c, row)
			}
		})
	})
}

// looksLikeCode reports whether a value reads better in a fixed font.
func looksLikeCode(row service.Row) bool {
	return strings.HasPrefix(row.Value, "/") || row.Key == "Command"
}

// infoRow shows a value: beside its name if it fits, below if not, with a
// button to copy it while the pointer is over the row.
func (a *app) infoRow(c *ui.Context, row service.Row) {
	t := c.Theme()
	box := ui.Box(c.Key(row.Key)).Padding(7, 12)
	hovered := box.Hovered()
	box.Children(func() {
		value := func() ui.Element {
			v := ui.Text(c, row.Value)
			if looksLikeCode(row) {
				v.Font("Menlo").FontSize(textCaption)
			} else {
				v.FontSize(textSmall)
			}
			return v
		}
		if len(row.Value) > 30 || looksLikeCode(row) {
			ui.Column(c).Gap(3).Children(func() {
				ui.Text(c, row.Key).FontSize(textCaption).TextColor(t.TextMuted)
				value()
			})
		} else {
			ui.Row(c).Gap(10).AlignItems(ui.Center).Children(func() {
				ui.Text(c, row.Key).FontSize(textSmall).TextColor(t.TextMuted).Grow(1).SingleLine()
				value().TextAlign(ui.End)
			})
		}
		if hovered {
			copyButton := iconButton(c, iconCopy, "Copy "+row.Key, t.TextMuted)
			copyButton.Absolute().Top(2).Right(4).Background(t.Surface)
			copyButton.OnClick(func() {
				a.copy(row.Value)
				a.toast("Copied")
			})
		}
	})
}

func (a *app) logTab(c *ui.Context) {
	d, l := &a.detail, &a.log
	t := c.Theme()
	ui.Column(c).Grow(1).MinHeight(0).Padding(8, 16, 16).Gap(8).Children(func() {
		if len(d.logs) > 1 {
			labels := make([]string, len(d.logs))
			for i, f := range d.logs {
				labels[i] = f.Label
			}
			ui.Segmented(c, &l.file, labels...).Label("Log file").AlignSelf(ui.Start)
		}
		file := d.logs[min(l.file, len(d.logs)-1)]
		ui.Row(c).AlignItems(ui.Center).Gap(6).Children(func() {
			ui.Text(c, file.Path).SingleLine().Grow(1).MinWidth(0).TextColor(t.TextMuted).FontSize(textCaption).Tooltip(file.Path)
			iconButton(c, iconOpen, "Open in Console", t.TextMuted).OnClick(func() { a.openLog(file.Path) })
		})
		switch {
		case l.err != "":
			ui.Text(c, l.err).TextColor(t.Danger).FontSize(textSmall)
		case l.lines == nil:
			ui.Text(c, "Reading…").TextColor(t.TextMuted).FontSize(textSmall)
		case len(l.lines) == 0:
			ui.Text(c, "The log is empty.").TextColor(t.TextMuted).FontSize(textSmall)
		default:
			if l.truncated {
				ui.Text(c, fmt.Sprintf("The last %d lines, updated %s", len(l.lines), ago(l.last))).TextColor(t.TextMuted).FontSize(textCaption)
			}
			card(c).Grow(1).MinHeight(0).Clip().Children(func() {
				ui.List(c, &l.list, len(l.lines), func(i int) {
					ui.Text(c, l.lines[i]).Font("Menlo").FontSize(textCaption).Selectable()
				}).Grow(1).MinHeight(0).Padding(8, 10).Gap(2)
			})
		}
	})
}

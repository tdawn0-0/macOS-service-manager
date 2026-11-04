package main

import (
	_ "embed"
	"fmt"

	"github.com/egoist/mygo"
	"github.com/egoist/mygo/ui"

	"macos-service-manager/internal/service"
)

// menubarIcon is black on transparent for macOS to tint to the menu bar, from
// assets/icons/menubar.svg (see scripts/icons.sh).
//
//go:embed assets/icons/menubar.png
var menubarIcon []byte

// menubar is the app's icon in the menu bar, and the panel it opens: the
// services the user runs by hand, with a switch each, for starting and
// stopping them without opening the window. System agents and daemons are
// left out: their changes affect the machine, and daemons ask for a password.
type menubar struct {
	a     *app
	win   *mygo.Window // the main window
	tray  *mygo.Tray
	panel *mygo.Window // the open panel, or nil
}

// panelSources lists the sources the panel shows.
var panelSources = []service.Source{service.Brew, service.UserAgent}

// panelItems returns the items of a source in the panel.
func (a *app) panelItems(src service.Source) []service.Item {
	st := a.sources[src]
	if st == nil {
		return nil
	}
	var items []service.Item
	for _, it := range st.items {
		if a.shown(it) {
			items = append(items, it)
		}
	}
	return items
}

// panelRunning counts what runs among the items the panel shows.
func (a *app) panelRunning() int {
	running := 0
	for _, src := range panelSources {
		for _, it := range a.panelItems(src) {
			if it.Up() {
				running++
			}
		}
	}
	return running
}

// menubarView builds the panel.
func (a *app) menubarView(c *ui.Context, openWindow func()) {
	style(c)
	if a.panelOpen {
		a.showToasts(c)
	}
	t := c.Theme()
	c.Root().Background(ui.Transparent)
	box := ui.Column(c).Fill()
	if !c.Vibrancy() {
		box.Background(t.Background).Radius(12).Border(1, t.Border)
	}
	box.Children(func() {
		ui.Row(c).Shrink(0).Padding(12, 10, 8, 16).AlignItems(ui.Center).Gap(6).Children(func() {
			ui.Column(c).Grow(1).MinWidth(0).Gap(1).Children(func() {
				ui.Text(c, "Service Manager").FontSize(14).FontWeight(650).SingleLine()
				ui.Text(c, fmt.Sprintf("%d running", a.panelRunning())).TextColor(t.TextMuted).FontSize(textCaption)
			})
			if a.loading() {
				ui.Spinner(c).Label("Loading").Size(14, 14)
			}
			iconButton(c, iconRestart, "Refresh", t.TextMuted).Disabled(a.loading()).OnClick(func() {
				for _, src := range panelSources {
					a.refresh(src)
				}
			})
		})
		ui.Scroll(c).Grow(1).MinHeight(0).Padding(0, 8, 8).Gap(2).Children(func() {
			empty := true
			for _, src := range panelSources {
				items := a.panelItems(src)
				if st := a.sources[src]; st != nil && st.err != "" {
					ui.Text(c, src.Title()+": "+st.err).TextColor(t.Danger).FontSize(textCaption).Padding(4, 8)
				}
				if len(items) == 0 {
					continue
				}
				empty = false
				ui.Text(c, src.Title()).FontSize(textCaption).FontWeight(650).TextColor(t.TextMuted).Padding(8, 8, 4)
				for _, it := range items {
					a.menubarRow(c, it)
				}
			}
			if empty {
				ui.Text(c, map[bool]string{true: "Loading…", false: "No services"}[a.loading()]).TextColor(t.TextMuted).Padding(8)
			}
		})
		ui.Column(c).Height(1).Shrink(0).Background(hairline(t))
		ui.Row(c).Shrink(0).Padding(8, 10).Gap(6).AlignItems(ui.Center).Children(func() {
			labeledButton(c, iconWindow, "Open Service Manager", false).Grow(1).OnClick(openWindow)
			iconButton(c, iconQuit, "Quit", t.TextMuted).OnClick(mygo.App.Quit)
		})
	})
}

// switchAction returns the action a panel's switch runs, turned on or off.
func switchAction(actions []service.Action, on bool) service.Action {
	order := []service.Action{service.Stop, service.Unload}
	if on {
		order = []service.Action{service.Start, service.Load, service.Restart}
	}
	for _, act := range order {
		if service.Has(actions, act) {
			return act
		}
	}
	return ""
}

func (a *app) menubarRow(c *ui.Context, it service.Item) {
	on := it.Up()
	busy := a.busy[it.ID]
	row := ui.Row(c.Key(it.ID)).Height(32).Shrink(0).AlignItems(ui.Center).Gap(10).Padding(0, 8).Radius(7)
	if row.Hovered() {
		row.Background(c.Theme().Text.Alpha(0.06))
	}
	row.Children(func() {
		statusDot(c, it.State, false)
		ui.Text(c, it.Name).SingleLine().Grow(1).MinWidth(0).Tooltip(it.Name)
		if busy {
			ui.Spinner(c).Label("Working").Size(13, 13)
		}
		if ui.Switch(c, &on).Label(it.Name).Disabled(busy).Changed() {
			if act := switchAction(a.providers[it.Source].Actions(it), on); act != "" {
				a.request(it, act)
			}
		}
	})
}

// update shows how many services run, beside the icon, and redraws the panel:
// a window of its own, which a change to the main window's state does not.
func (m *menubar) update() {
	if m.panel != nil {
		m.panel.Invalidate()
	}
	if m.tray == nil {
		return
	}
	running := m.a.panelRunning()
	title := ""
	if running > 0 {
		title = fmt.Sprint(running)
	}
	m.tray.SetTitle(title)
	m.tray.SetToolTip(fmt.Sprintf("Service Manager: %d running", running))
}

func (m *menubar) showWindow() {
	if m.panel != nil {
		m.panel.Close()
	}
	m.win.Show()
	m.win.Focus()
}

// panelHeight fits the panel to its rows, up to where it scrolls.
func (a *app) panelHeight() int {
	const header, footer, row, heading = 54, 50, 34, 30
	height := header + footer + 16
	for _, src := range panelSources {
		if n := len(a.panelItems(src)); n > 0 {
			height += heading + n*row
		}
	}
	return min(max(height, 200), 540)
}

// toggle opens the panel below the icon, or closes it.
func (m *menubar) toggle() {
	if m.panel != nil {
		m.panel.Close()
		return
	}
	m.a.panelOpen = true
	m.panel = mygo.NewFlyout(mygo.FlyoutOptions{
		Tray:      m.tray,
		Placement: mygo.PlacementBottom,
		Gap:       4,
		Width:     320,
		Height:    m.a.panelHeight(),
		Focusable: true,
		Shadow:    true,
		Popover:   true,
		Content:   ui.View(func(c *ui.Context) { m.a.menubarView(c, m.showWindow) }),
	})
	m.panel.OnClosed(func() {
		m.panel = nil
		m.a.panelOpen = false
	})
	// What the list shows may be old.
	for _, src := range panelSources {
		m.a.refresh(src)
	}
}

// install puts the icon in the menu bar.
func (m *menubar) install() error {
	tray, err := mygo.NewTray(mygo.TrayOptions{
		ID:             "xyz.jyeu.msm.menubar",
		Icon:           menubarIcon,
		IconIsTemplate: true,
		ToolTip:        "Service Manager",
	})
	if err != nil {
		return err
	}
	m.tray = tray
	menu := mygo.NewMenu([]*mygo.MenuItem{
		{Label: "Open Service Manager", Click: func(*mygo.MenuItem, *mygo.Window) { m.showWindow() }},
		{Label: "Refresh", Click: func(*mygo.MenuItem, *mygo.Window) { m.a.refreshAll() }},
		mygo.Separator(),
		{Role: mygo.RoleQuit},
	})
	tray.OnClick(m.toggle)
	tray.OnRightClick(func() {
		if m.panel != nil {
			m.panel.Close()
		}
		// The menu for this popup only, so that clicks keep opening the panel.
		tray.SetMenu(menu)
		defer tray.SetMenu(nil)
		tray.PopUpMenu()
	})
	mygo.App.OnWillQuit(func(*mygo.QuitEvent) { tray.Destroy() })
	return nil
}

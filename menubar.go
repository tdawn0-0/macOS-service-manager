package main

import (
	"bytes"
	"fmt"
	"image"
	"image/color"
	"image/png"

	"github.com/egoist/mygo"
	"github.com/egoist/mygo/ui"

	"macos-service-manager/internal/service"
)

// menubar is the app's icon in the menu bar, and the panel it opens: the
// Homebrew services with a switch each, for starting and stopping them
// without opening the window.
type menubar struct {
	a     *app
	win   *mygo.Window // the main window
	tray  *mygo.Tray
	panel *mygo.Window // the open panel, or nil
}

// brewItems returns the Homebrew services, as the panel lists them.
func (a *app) brewItems() []service.Item {
	if st := a.sources[service.Brew]; st != nil {
		return st.items
	}
	return nil
}

// menubarView builds the panel.
func (a *app) menubarView(c *ui.Context, openWindow func()) {
	t := c.Theme()
	c.Root().Background(ui.Transparent)
	box := ui.Column(c).Fill()
	if !c.Vibrancy() {
		box.Background(t.Surface).Radius(10).Border(1, t.Border)
	}
	items := a.brewItems()
	running := 0
	for _, it := range items {
		if it.Active() {
			running++
		}
	}
	box.Padding(14).Gap(10).Children(func() {
		ui.Row(c).AlignItems(ui.Center).Gap(8).Children(func() {
			ui.Text(c, "Homebrew Services").FontSize(15).Bold().Grow(1)
			ui.Text(c, fmt.Sprintf("%d running", running)).TextColor(t.TextMuted).FontSize(12)
		})
		switch st := a.sources[service.Brew]; {
		case st != nil && st.err != "":
			ui.Text(c, st.err).TextColor(t.Danger).FontSize(12)
		case len(items) == 0:
			ui.Text(c, map[bool]string{true: "Loading…", false: "No Homebrew services"}[a.loading()]).TextColor(t.TextMuted)
		default:
			ui.Scroll(c).Grow(1).Gap(2).Children(func() {
				for _, it := range items {
					a.menubarRow(c, it)
				}
			})
		}
		ui.Divider(c)
		ui.Row(c).Gap(8).Children(func() {
			ui.PrimaryButton(c, "Open Service Manager").OnClick(openWindow)
			ui.Spacer(c)
			ui.Button(c, "Refresh").Disabled(a.loading()).OnClick(func() { a.refresh(service.Brew) })
			ui.Button(c, "Quit").OnClick(mygo.App.Quit)
		})
	})
}

func (a *app) menubarRow(c *ui.Context, it service.Item) {
	t := c.Theme()
	on := it.Active()
	busy := a.busy[it.ID]
	ui.Row(c.Key(it.ID)).AlignItems(ui.Center).Gap(8).Padding(4, 2).Children(func() {
		ui.Text(c, "●").TextColor(stateColor(t, it.State)).FontSize(9)
		ui.Text(c, it.Name).SingleLine().Grow(1).MinWidth(0)
		if busy {
			ui.Spinner(c).Label("Working")
		}
		if ui.Switch(c, &on).Label(it.Name).Disabled(busy).Changed() {
			if on {
				a.request(it, service.Run)
			} else {
				a.request(it, service.Stop)
			}
		}
	})
}

// update shows how many Homebrew services run, beside the icon.
func (m *menubar) update() {
	// The panel is a window of its own, which a change to the main window's
	// state does not redraw.
	if m.panel != nil {
		m.panel.Invalidate()
	}
	if m.tray == nil {
		return
	}
	running := 0
	for _, it := range m.a.brewItems() {
		if it.Active() {
			running++
		}
	}
	title := ""
	if running > 0 {
		title = fmt.Sprint(running)
	}
	m.tray.SetTitle(title)
	m.tray.SetToolTip(fmt.Sprintf("Service Manager: %d Homebrew services running", running))
}

func (m *menubar) showWindow() {
	if m.panel != nil {
		m.panel.Close()
	}
	m.win.Show()
	m.win.Focus()
}

// toggle opens the panel below the icon, or closes it.
func (m *menubar) toggle() {
	if m.panel != nil {
		m.panel.Close()
		return
	}
	height := min(max(120+34*len(m.a.brewItems()), 170), 460)
	m.panel = mygo.NewFlyout(mygo.FlyoutOptions{
		Tray:      m.tray,
		Placement: mygo.PlacementBottom,
		Gap:       4,
		Width:     320,
		Height:    height,
		Focusable: true,
		Shadow:    true,
		Popover:   true,
		Content:   ui.View(func(c *ui.Context) { m.a.menubarView(c, m.showWindow) }),
	})
	m.panel.OnClosed(func() { m.panel = nil })
	// What the list shows may be old.
	m.a.refresh(service.Brew)
}

// install puts the icon in the menu bar.
func (m *menubar) install() error {
	tray, err := mygo.NewTray(mygo.TrayOptions{
		ID:             "xyz.jyeu.msm.menubar",
		Icon:           menubarIcon(),
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

// menubarIcon draws a power symbol, black on transparent for macOS to tint
// to the menu bar.
func menubarIcon() []byte {
	const size = 32
	img := image.NewNRGBA(image.Rect(0, 0, size, size))
	ink := color.NRGBA{A: 255}
	for y := range size {
		for x := range size {
			dx, dy := float64(x)-15.5, float64(y)-15.5+1.5
			d := dx*dx + dy*dy
			ring := d < 12.5*12.5 && d > 9.5*9.5 && !(dy < -2 && dx > -3.2 && dx < 3.2)
			bar := dx > -1.6 && dx < 1.6 && dy > -13 && dy < -2
			if ring || bar {
				img.Set(x, y, ink)
			}
		}
	}
	var b bytes.Buffer
	_ = png.Encode(&b, img)
	return b.Bytes()
}

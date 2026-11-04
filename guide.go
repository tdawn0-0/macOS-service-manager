package main

import (
	"bytes"
	_ "embed"
	"image"
	"image/png"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/egoist/mygo"
	"github.com/egoist/mygo/transfer"
	"github.com/egoist/mygo/ui"

	"macos-service-manager/internal/system"
)

const (
	// The page as it opens, and beside System Settings.
	guideWidth, guideHeight     = 420, 412
	sidecarWidth, sidecarHeight = 288, 330
	sidecarGap                  = 14
	guideTitleBar               = 40

	// How the icon shows the way into System Settings: a flight, a pause, a few
	// times over, and again whenever it is clicked.
	flightTime     = 1300 * time.Millisecond
	flightPause    = 1200 * time.Millisecond
	flightsShown   = 4
	flightIcon     = 72
	rippleTime     = 450 * time.Millisecond
	guideDoneAfter = 1400 * time.Millisecond

	// settingsRadius is about the corners of System Settings' window.
	settingsRadius = 18
)

//go:embed assets/icons/guide.png
var guideIconPNG []byte

//go:embed assets/icons/drag.png
var dragIconPNG []byte

var (
	white, black = ui.RGB(255, 255, 255), ui.RGB(0, 0, 0)

	guideIconOnce   sync.Once
	guideIconSource ui.ImageSource
)

// guideIcon returns the icon the pages show.
func guideIcon() ui.ImageSource {
	guideIconOnce.Do(func() { guideIconSource, _ = ui.DecodeImage(guideIconPNG) })
	return guideIconSource
}

func dragPreview() image.Image {
	img, err := png.Decode(bytes.NewReader(dragIconPNG))
	if err != nil {
		return nil
	}
	return img
}

// bundlePath returns the app bundle this process runs from, or "" if none.
func bundlePath() string {
	exe, err := os.Executable()
	if err != nil {
		return ""
	}
	return bundleOf(exe)
}

func bundleOf(exe string) string {
	macos := filepath.Dir(exe)
	if !strings.HasSuffix(macos, ".app/Contents/MacOS") {
		return ""
	}
	return filepath.Dir(filepath.Dir(macos))
}

// guideWindow is the guide's window, above the others: System Settings comes
// to the front, and the icon has to stay within reach to be dragged into it.
// Once that happens it follows the window of System Settings, and an overlay
// that lets the pointer through shows where the icon goes.
type guideWindow struct {
	a       *app
	win     *mygo.Window
	overlay *mygo.Window
	// overlayBounds is what the overlay was asked to cover. macOS keeps it clear
	// of the menu bar, so where it is says nothing of whether the display changed.
	overlayBounds mygo.Rectangle
	stop          chan struct{}
}

func (g *guideWindow) show() {
	if g.win != nil {
		g.win.Show()
		g.win.Focus()
		return
	}
	g.win = mygo.NewWindow(mygo.WindowOptions{
		Title:             "Welcome to Service Manager",
		Width:             guideWidth,
		Height:            guideHeight,
		DisableResize:     true,
		DisableMinimize:   true,
		DisableMaximize:   true,
		DisableFullScreen: true,
		AlwaysOnTop:       true,
		TitleBarStyle:     mygo.TitleBarHiddenInset,
		TitleBarHeight:    guideTitleBar,
		Content:           ui.View(g.a.guideView),
	})
	g.win.OnClosed(func() {
		g.win = nil
		g.endFollowing()
		g.a.closeGuide()
	})
}

func (g *guideWindow) close() {
	if g.win != nil {
		g.win.Close()
	}
	g.endFollowing()
}

func (g *guideWindow) endFollowing() {
	if g.stop != nil {
		close(g.stop)
		g.stop = nil
	}
	if g.overlay != nil {
		g.overlay.Close()
		g.overlay = nil
	}
}

// update redraws the windows: a change to the main window's state does not.
func (g *guideWindow) update() {
	if g.win != nil {
		g.win.Invalidate()
	}
	if g.overlay != nil {
		if g.a.guide.allowedAt.IsZero() {
			g.overlay.Invalidate()
		} else {
			g.overlay.Hide()
		}
	}
}

// join makes the window the small page that follows System Settings.
func (g *guideWindow) join() {
	if g.win == nil || g.stop != nil {
		return
	}
	g.win.SetSize(sidecarWidth, sidecarHeight)
	g.stop = make(chan struct{})
	go g.follow(g.win, g.stop)
}

// follow watches the window of System Settings, which tells nobody as it moves.
func (g *guideWindow) follow(win *mygo.Window, stop chan struct{}) {
	ticker := time.NewTicker(time.Second / 30)
	defer ticker.Stop()
	var last system.Rect
	lastOpen, first := false, true
	for {
		select {
		case <-stop:
			return
		case <-ticker.C:
		}
		r, open := system.SettingsWindow()
		if !first && r == last && open == lastOpen {
			continue
		}
		first, last, lastOpen = false, r, open
		win.Update(func() { g.place(r, open) })
	}
}

// place puts the window beside System Settings, and the overlay over its display.
func (g *guideWindow) place(r system.Rect, open bool) {
	guide := &g.a.guide
	if g.win == nil || !guide.open {
		return
	}
	guide.settings, guide.settingsOpen = r, open
	if !open {
		if g.overlay != nil {
			g.overlay.Hide()
		}
		return
	}
	display := mygo.Screen.DisplayNearestPoint(mygo.Point{X: r.X + r.Width/2, Y: r.Y + r.Height/2})
	area := display.WorkArea
	x, y := sidecarOrigin(r, system.Rect{X: area.X, Y: area.Y, Width: area.Width, Height: area.Height}, sidecarWidth, sidecarHeight)
	g.win.SetPosition(x, y)
	guide.windowAt = image.Pt(x, y)
	g.placeOverlay(display.Bounds)
	if guide.flightStart.IsZero() {
		guide.flightStart = time.Now()
	}
	g.update()
}

func (g *guideWindow) placeOverlay(bounds mygo.Rectangle) {
	if g.overlay == nil {
		g.overlay = mygo.NewWindow(mygo.WindowOptions{
			X:                 bounds.X,
			Y:                 bounds.Y,
			Width:             bounds.Width,
			Height:            bounds.Height,
			Hidden:            true,
			Frameless:         true,
			Transparent:       true,
			DisableShadow:     true,
			DisableResize:     true,
			DisableMinimize:   true,
			DisableMaximize:   true,
			DisableFullScreen: true,
			AlwaysOnTop:       true,
			SkipTaskbar:       true,
			Content:           ui.View(g.a.overlayView),
		})
		g.overlay.SetIgnoreMouseEvents(true)
		// Not one more window for Stage Manager to arrange.
		g.overlay.SetVisibleOnAllWorkspaces(true)
		g.overlay.OnMove(g.overlayMoved)
		// At (0, 0), which options take to mean no position, it would be centered.
		g.overlay.SetBounds(bounds)
	} else if g.overlayBounds != bounds {
		g.overlay.SetBounds(bounds)
	}
	g.overlayBounds = bounds
	if g.a.guide.allowedAt.IsZero() && !g.overlay.IsVisible() {
		g.overlay.ShowInactive()
		// Stage Manager moves a window clear of its strip as it shows.
		g.overlay.SetBounds(bounds)
	}
	g.overlayMoved()
}

// overlayMoved notes where the overlay is: macOS keeps it clear of the menu bar,
// and what it draws is relative to it.
func (g *guideWindow) overlayMoved() {
	if g.overlay != nil {
		at := g.overlay.Bounds()
		g.a.guide.overlayAt = image.Pt(at.X, at.Y)
	}
}

// sidecarOrigin places a window beside the rectangle: at its right if it fits
// in the area, otherwise at its left, otherwise over its corner.
func sidecarOrigin(s, area system.Rect, width, height int) (x, y int) {
	switch {
	case s.X+s.Width+sidecarGap+width <= area.X+area.Width:
		x = s.X + s.Width + sidecarGap
	case s.X-sidecarGap-width >= area.X:
		x = s.X - sidecarGap - width
	default:
		x = area.X + area.Width - width - sidecarGap
	}
	y = min(max(s.Y, area.Y), area.Y+area.Height-height)
	return x, y
}

// flight is where the icon is on its way, at a time after it first set off.
type flight struct {
	x, y, scale, opacity float32
	// flying says the icon shows, and ripple how far the circle of its landing
	// has gone, from 0 to 1, or -1 if there is none. done says no flight follows.
	flying bool
	ripple float32
	done   bool
}

func flightAt(from, to image.Point, elapsed time.Duration) flight {
	f := flight{ripple: -1}
	cycle := flightTime + flightPause
	if elapsed < 0 || elapsed >= flightsShown*cycle {
		f.done = true
		return f
	}
	at := elapsed % cycle
	if at >= flightTime {
		if since := at - flightTime; since < rippleTime {
			f.ripple = float32(since) / float32(rippleTime)
		}
		f.x, f.y = float32(to.X), float32(to.Y)
		return f
	}
	f.flying = true
	t := float32(at) / float32(flightTime)
	// Slow to leave and to arrive, as a hand moves.
	eased := t * t * (3 - 2*t)
	x0, y0, x2, y2 := float32(from.X), float32(from.Y), float32(to.X), float32(to.Y)
	// An arc: the middle of the way, lifted by a part of its length.
	dx, dy := x2-x0, y2-y0
	if dx < 0 {
		dx = -dx
	}
	if dy < 0 {
		dy = -dy
	}
	x1, y1 := (x0+x2)/2, (y0+y2)/2-(dx+dy)*0.22
	u := 1 - eased
	f.x = u*u*x0 + 2*u*eased*x1 + eased*eased*x2
	f.y = u*u*y0 + 2*u*eased*y1 + eased*eased*y2
	f.scale = 1 - 0.14*eased
	switch {
	case t < 0.12:
		f.opacity = t / 0.12
	case t > 0.82:
		f.opacity = (1 - t) / 0.18
	default:
		f.opacity = 1
	}
	f.opacity *= 0.92
	return f
}

// dropPoint returns where in System Settings the icon goes: in the list, which
// is to the right of its sidebar.
func dropPoint(s system.Rect) image.Point {
	return image.Pt(s.X+s.Width*5/8, s.Y+s.Height*11/20)
}

// guideView is the page, as it opens and then beside System Settings.
func (a *app) guideView(c *ui.Context) {
	style(c)
	if a.guide.open {
		a.checkGuide()
		c.After(time.Second)
	}
	t := c.Theme()
	ui.Column(c).Fill().Background(t.Background).Children(func() {
		ui.Row(c).Height(guideTitleBar).Shrink(0).DragWindow()
		switch {
		case !a.guide.allowedAt.IsZero():
			a.guideDone(c)
		case a.guide.joined:
			a.guideSidecar(c)
		default:
			a.guideWelcome(c)
		}
	})
}

// guideWelcome says what the app is, and offers the one thing it may need.
func (a *app) guideWelcome(c *ui.Context) {
	t := c.Theme()
	title, subtitle := "See Every Login Item", "One permission, and the list is complete."
	skip := "Close"
	if a.guide.welcome {
		title, subtitle = "Welcome to Service Manager", "Everything that starts on your Mac, in one place."
		skip = "Skip for Now"
	}
	ui.Column(c).Grow(1).MinHeight(0).Padding(0, 28, 24).AlignItems(ui.Center).Children(func() {
		ui.Image(c, guideIcon()).Size(84, 84).Shrink(0)
		ui.Text(c, title).FontSize(20).FontWeight(700).Margin(10, 0, 4).TextAlign(ui.Center)
		ui.Text(c, subtitle).TextColor(t.TextMuted).TextAlign(ui.Center)
		ui.Row(c).Shrink(0).AlignSelf(ui.Stretch).Margin(22, 0, 0).Padding(12).Gap(12).AlignItems(ui.Center).Radius(12).Background(t.Text.Alpha(0.045)).Children(func() {
			ui.Row(c).Size(40, 40).Shrink(0).Center().Radius(10).Background(t.Accent).Children(func() {
				ui.Icon(c, iconDisk).FontSize(22).TextColor(t.AccentText)
			})
			ui.Column(c).Grow(1).MinWidth(0).Gap(2).Children(func() {
				ui.Text(c, "Full Disk Access").FontWeight(600)
				ui.Text(c, "Only to read the list of login items.").FontSize(textSmall).TextColor(t.TextMuted)
			})
			ui.PrimaryButton(c, "Allow…").Shrink(0).OnClick(func() { a.openRestrictionSettings(a.guide.src) })
		})
		ui.Text(c, "Without it, some login items may not show. Everything else works, and you can do this later from Login Items.").
			FontSize(textSmall).TextColor(t.TextMuted).TextAlign(ui.Center).Margin(12, 8, 0)
		ui.Column(c).Grow(1)
		ui.Button(c, skip).OnClick(a.closeGuide)
	})
}

// guideSidecar is the icon to drag, beside System Settings.
func (a *app) guideSidecar(c *ui.Context) {
	t := c.Theme()
	ui.Column(c).Grow(1).MinHeight(0).Padding(0, 20, 18).AlignItems(ui.Center).Children(func() {
		if a.appPath != "" {
			a.dragTile(c)
			ui.Text(c, "Drag into the list").FontSize(15).FontWeight(650).Margin(14, 0, 4).TextAlign(ui.Center)
			ui.Text(c, "Then confirm with Touch ID or password.").FontSize(textSmall).TextColor(t.TextMuted).TextAlign(ui.Center)
		} else {
			ui.Image(c, guideIcon()).Size(84, 84).Shrink(0)
			ui.Text(c, "Add this app to the list").FontSize(15).FontWeight(650).Margin(14, 0, 4).TextAlign(ui.Center)
			ui.Text(c, "With the + button under the list.").FontSize(textSmall).TextColor(t.TextMuted).TextAlign(ui.Center)
		}
		ui.Column(c).Grow(1)
		if a.guide.settingsOpen {
			ui.Row(c).Shrink(0).Gap(6).AlignItems(ui.Center).Margin(0, 0, 10).Children(func() {
				ui.Spinner(c).Label("Waiting").Size(12, 12)
				ui.Text(c, "Waiting for access").FontSize(textSmall).TextColor(t.TextMuted)
			})
		} else {
			ui.Button(c, "Open Settings").Margin(0, 0, 10).OnClick(func() { a.openRestrictionSettings(a.guide.src) })
		}
		skip := "Close"
		if a.guide.welcome {
			skip = "Skip for Now"
		}
		ui.Button(c, skip).OnClick(a.closeGuide)
	})
}

// guideDone says it worked, and closes the page in a moment.
func (a *app) guideDone(c *ui.Context) {
	t := c.Theme()
	if a.guide.open {
		if c.Now().Sub(a.guide.allowedAt) >= guideDoneAfter {
			a.post(a.closeGuide)
		} else {
			c.After(guideDoneAfter)
		}
	}
	ui.Column(c).Grow(1).Justify(ui.Center).AlignItems(ui.Center).Padding(0, 20, 40).Children(func() {
		ui.Row(c).Size(56, 56).Center().Radius(28).Background(t.Success).Children(func() {
			ui.Icon(c, iconCheck).FontSize(30).TextColor(white)
		})
		ui.Text(c, "Full Disk Access is on").FontSize(15).FontWeight(650).Margin(14, 0, 4)
		ui.Text(c, "Every login item is listed.").FontSize(textSmall).TextColor(t.TextMuted)
	})
}

// dragTile is the app's icon, which drags as the app's bundle, and shows the
// way once more when clicked.
func (a *app) dragTile(c *ui.Context) {
	t := c.Theme()
	tile := ui.Column(c).Size(132, 132).Shrink(0).Center().Radius(26).Cursor(ui.CursorGrab).
		Border(1, t.Accent.Alpha(0.35)).Background(t.Accent.Alpha(0.07)).
		Label("Drag Service Manager into the list").Tooltip("Drag into the list in System Settings")
	if tile.Hovered() {
		tile.Background(t.Accent.Alpha(0.14)).Border(1, t.Accent)
	}
	b := tile.Bounds()
	a.guide.icon = image.Pt(int(b.X+b.W/2), int(b.Y+b.H/2))
	tile.OnClick(func() {
		a.guide.dragged = false
		a.guide.flightStart = time.Now()
		a.redrawGuide()
	})
	tile.DragDataFrom(func() transfer.Data {
		a.guide.dragging, a.guide.dragged = true, true
		a.redrawGuide()
		data, _ := a.guideDragData()
		return data
	}, transfer.DragOptions{
		Operations: transfer.Copy,
		Preview:    dragPreview(),
		Hotspot:    image.Pt(36, 36),
		Done: func(transfer.Result) {
			a.guide.dragging = false
			a.redrawGuide()
		},
	})
	tile.Children(func() { ui.Image(c, guideIcon()).Size(104, 104) })
}

func (a *app) guideDragData() (transfer.Data, error) { return transfer.FileData(a.appPath) }

// overlayView covers the display of System Settings, and lets the pointer
// through. It shows the icon on its way there, and while the user drags the
// icon, darkens all but that window.
func (a *app) overlayView(c *ui.Context) {
	g := &a.guide
	t := c.Theme()
	c.Root().Background(ui.Transparent)
	root := ui.Column(c).Fill().Background(ui.Transparent)
	darkness := float32(0)
	if g.dragging && g.settingsOpen {
		darkness = 1
	}
	dim := root.Animate("dim", darkness, 200*time.Millisecond)
	root.Children(func() {
		if !g.open || !g.settingsOpen || !g.allowedAt.IsZero() {
			return
		}
		s := g.settings
		s.X -= g.overlayAt.X
		s.Y -= g.overlayAt.Y
		if dim > 0.001 {
			// All but the window: a hole in a frame wider than the screen.
			width, height := c.Size()
			frame := max(width, height) + 8
			ui.Column(c).Absolute().Left(float32(s.X)-frame).Top(float32(s.Y)-frame).
				Size(float32(s.Width)+2*frame, float32(s.Height)+2*frame).
				Radius(frame+settingsRadius).Border(frame, black.Alpha(0.5*dim))
			ui.Column(c).Absolute().Left(float32(s.X)-2).Top(float32(s.Y)-2).
				Size(float32(s.Width)+4, float32(s.Height)+4).
				Radius(settingsRadius+2).Border(2, t.Accent.Alpha(dim))
			top := float32(s.Y) - 46
			if top < 8 {
				top = float32(s.Y+s.Height) + 12
			}
			a.overlayCaption(c, "Drop it in the list to allow access", float32(s.X+s.Width/2), top, dim)
		}
		if g.dragging || g.dragged {
			return
		}
		to := dropPoint(s)
		a.overlayCaption(c, "Drop here", float32(to.X), float32(to.Y)+52, 1)
		if c.Preferences().ReduceMotion || g.flightStart.IsZero() {
			return
		}
		from := g.windowAt.Add(g.icon).Sub(g.overlayAt)
		f := flightAt(from, to, c.Now().Sub(g.flightStart))
		if f.done {
			return
		}
		c.After(time.Second / 60)
		if f.ripple >= 0 {
			size := 56 + 68*f.ripple
			ui.Column(c).Absolute().Left(f.x-size/2).Top(f.y-size/2).Size(size, size).Radius(size/2).Border(2, t.Accent.Alpha(1-f.ripple))
		}
		if f.flying {
			ui.Image(c, guideIcon()).Absolute().Left(f.x-flightIcon/2).Top(f.y-flightIcon/2).Size(flightIcon, flightIcon).
				Scale(f.scale, f.scale).Opacity(f.opacity)
		}
	})
}

// overlayCaption is a short text in a pill, centered at x.
func (a *app) overlayCaption(c *ui.Context, text string, x, y, opacity float32) {
	const width = 300
	ui.Row(c).Absolute().Left(x - width/2).Top(y).Width(width).Justify(ui.Center).Opacity(opacity).Children(func() {
		ui.Row(c).Padding(7, 14).Radius(999).Background(black.Alpha(0.74)).Children(func() {
			ui.Text(c, text).FontSize(13).FontWeight(600).TextColor(white).SingleLine()
		})
	})
}

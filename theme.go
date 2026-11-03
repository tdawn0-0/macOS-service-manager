package main

import (
	"github.com/egoist/mygo/ui"

	"macos-service-manager/internal/service"
)

// style sets the look of a window: the system's accent and appearance, with
// quieter surfaces, the colors of macOS and tighter controls.
func style(c *ui.Context) {
	t := *c.Theme()
	t.Spacing = 3.5
	t.Radius = 7
	if t.Dark {
		t.Background = ui.Hex("#1c1c1e")
		t.Surface = ui.Hex("#2a2a2d")
		t.SurfaceHover = ui.Hex("#343437")
		t.SurfacePressed = ui.Hex("#3d3d41")
		t.Border = ui.RGBA(255, 255, 255, 0.11)
		t.Text = ui.Hex("#f5f5f7")
		t.TextMuted = ui.Hex("#98989f")
		t.Success = ui.Hex("#30d158")
		t.Warning = ui.Hex("#ff9f0a")
		t.Danger = ui.Hex("#ff453a")
	} else {
		t.Background = ui.Hex("#ffffff")
		t.Surface = ui.Hex("#f5f5f7")
		t.SurfaceHover = ui.Hex("#ececf0")
		t.SurfacePressed = ui.Hex("#e3e3e8")
		t.Border = ui.RGBA(0, 0, 0, 0.11)
		t.Text = ui.Hex("#1d1d1f")
		t.TextMuted = ui.Hex("#6e6e73")
		t.Success = ui.Hex("#28b04f")
		t.Warning = ui.Hex("#f59300")
		t.Danger = ui.Hex("#e5352b")
	}
	c.SetTheme(&t)
}

// hairline is the color of the thin lines between parts of a window.
func hairline(t *ui.Theme) ui.Color { return t.Text.Alpha(0.09) }

// Sizes of text below the theme's 13.
const (
	textSmall   = 12
	textCaption = 11
)

// stateColor is the color of an item's state: green runs, blue waits for its
// time, red failed, and gray is at rest, which is nothing to worry about.
func stateColor(t *ui.Theme, s service.State) ui.Color {
	switch s {
	case service.StateRunning:
		return t.Success
	case service.StateScheduled:
		return t.Accent
	case service.StateFailed:
		return t.Danger
	}
	return t.TextMuted
}

// statusDot shows an item's state: hollow where launchd has not even loaded
// it, solid at rest, and with a halo while it runs. On the accent color of a
// row chosen, only its shape tells.
func statusDot(c *ui.Context, s service.State, onAccent bool) {
	const size = 8
	t := c.Theme()
	color := stateColor(t, s)
	if onAccent {
		color = t.AccentText
	}
	dot := ui.Box(c).Size(size, size).Radius(size / 2).Shrink(0)
	switch s {
	case service.StateNone:
		dot.Invisible()
	case service.StateStopped:
		dot.Border(1.5, color.Alpha(0.65))
	case service.StateRunning:
		dot.Background(color).Shadow(0, 0, 0, 2.5, color.Alpha(0.18))
	case service.StateLoaded:
		dot.Background(color.Alpha(0.65))
	default:
		dot.Background(color)
	}
}

// iconButton is a button showing only an icon, without a face until hovered.
func iconButton(c *ui.Context, icon *ui.SVG, label string, tint ui.Color) ui.Element {
	const size = 26
	b := ui.ButtonBase(c).Size(size, size).Center().Radius(6).Shrink(0).Label(label).Tooltip(label)
	if b.Hovered() {
		b.Background(tint.Alpha(0.12))
	}
	b.Children(func() { ui.Icon(c, icon).FontSize(14).TextColor(tint) })
	return b
}

// labeledButton is a button showing an icon before its label.
func labeledButton(c *ui.Context, icon *ui.SVG, label string, primary bool) ui.Element {
	var b ui.Element
	if primary {
		b = ui.PrimaryButton(c, "")
	} else {
		b = ui.Button(c, "")
	}
	b.Justify(ui.Center).Children(func() {
		ui.Icon(c, icon).FontSize(13)
		ui.Text(c, label).SingleLine()
	})
	return b
}

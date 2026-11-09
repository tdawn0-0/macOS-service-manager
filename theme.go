package main

import (
	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/action"
	"github.com/lobehub/lobe-mygo/pkg/theme"

	"macos-service-manager/internal/service"
)

// style sets the look of a window to the Lobe UI design tokens.
func style(c *ui.Context) {
	theme.Apply(c)
	t := *c.Theme()
	t.Spacing = 3.5
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

// iconButton is a button showing only an icon, styled with Lobe ActionIcon.
func iconButton(c *ui.Context, icon *ui.SVG, label string, tint ui.Color) ui.Element {
	return action.ActionIcon(c, icon, action.ActionIconProps{
		Variant: action.ActionIconBorderless,
		Size:    theme.Small,
		Title:   label,
	})
}

// labeledButton is a button showing an icon before its label, styled with Lobe Button.
func labeledButton(c *ui.Context, icon *ui.SVG, label string, primary bool) ui.Element {
	variant := action.Default
	if primary {
		variant = action.Primary
	}
	b := ui.ButtonBase(c)
	action.StyleButton(c, b, "", action.ButtonProps{
		Variant: variant,
		Size:    theme.Small,
	})
	b.Label(label)
	b.Children(func() {
		ui.Row(c).Center().Gap(6).Children(func() {
			ui.Icon(c, icon).FontSize(13)
			ui.Text(c, label).SingleLine()
		})
	})
	return b
}

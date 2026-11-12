package main

import (
	"strings"

	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/action"
	"github.com/lobehub/lobe-mygo/pkg/theme"

	"devbase/internal/service"
)

// style sets the look of a window to the Lobe UI design tokens.
func style(c *ui.Context) {
	theme.Apply(c)
	t := *c.Theme()
	t.Spacing = 3.5
	c.SetTheme(&t)
}

// hairline is the color of the thin lines between parts of a window.
func hairline(t *ui.Theme) ui.Color { return t.Text.Alpha(0.08) }

// glassTone is the window's frosted glass. The desktop shows through the
// whole window, frosted by the system; the sidebar, the lists and the bars
// sit right on that frost, the page lies on a translucent sheet over it,
// and each group of controls on a frosted panel on the sheet. Glass is
// never laid twice where text has to read.
type glassTone struct {
	// sheet is the page's layer over the window's frost.
	sheet ui.Color
	// panel fills a card on the sheet, edge outlines it and lift is its
	// shadow, none in the dark where depth comes from the steps.
	panel, edge, lift ui.Color
	// inset is a well inside a panel, for one item of a list in it.
	inset ui.Color
	// hover and selected are the rows of a list over the frost.
	hover, selected ui.Color
}

func glassOf(t *ui.Theme) glassTone {
	if t.Dark {
		return glassTone{
			sheet:    ui.RGB(16, 16, 18).Alpha(0.42),
			panel:    ui.RGB(255, 255, 255).Alpha(0.045),
			edge:     ui.RGB(255, 255, 255).Alpha(0.08),
			inset:    ui.RGB(255, 255, 255).Alpha(0.04),
			hover:    ui.RGB(255, 255, 255).Alpha(0.05),
			selected: ui.RGB(255, 255, 255).Alpha(0.1),
		}
	}
	return glassTone{
		sheet:    ui.RGB(255, 255, 255).Alpha(0.38),
		panel:    ui.RGB(255, 255, 255).Alpha(0.68),
		edge:     ui.RGB(0, 0, 0).Alpha(0.07),
		lift:     ui.RGB(0, 0, 0).Alpha(0.04),
		inset:    ui.RGB(0, 0, 0).Alpha(0.035),
		hover:    ui.RGB(0, 0, 0).Alpha(0.04),
		selected: ui.RGB(0, 0, 0).Alpha(0.07),
	}
}

// glassPanel styles e as a frosted panel on the sheet.
func glassPanel(c *ui.Context, e ui.Element) ui.Element {
	g := glassOf(c.Theme())
	e.Radius(12).Background(g.panel).Border(1, g.edge)
	if g.lift.A > 0 {
		e.Shadow(0, 1, 2, 0, g.lift)
	}
	return e
}

// mono is the face of what a developer copies: addresses, ports, versions
// and paths, in the system's monospace with figures of one width.
func mono(e ui.Element) ui.Element { return e.Font("monospace").FontFeatures("tnum") }

// isAddress tells a word that is an address, a port or a version, as
// 127.0.0.1:5432, :443 or 17.11_1, which shows in mono, from a count or a
// word.
func isAddress(w string) bool {
	if w == "" || !(w[0] == ':' || w[0] >= '0' && w[0] <= '9') {
		return false
	}
	return strings.ContainsAny(w, ".:")
}

// factLine writes facts about a subject, split by " · ", the facts that
// are an address in mono.
func factLine(c *ui.Context, facts string, color ui.Color) {
	ui.Row(c).Gap(6).AlignItems(ui.Center).MinWidth(0).Children(func() {
		parts := strings.Split(facts, " · ")
		for i, fact := range parts {
			if i > 0 {
				ui.Text(c, "·").FontSize(textSmall).TextColor(color.Alpha(0.5)).Shrink(0)
			}
			t := ui.Text(c, fact).FontSize(textSmall).TextColor(color).SingleLine()
			if i == len(parts)-1 {
				t.MinWidth(0)
			} else {
				t.Shrink(0)
			}
			if isAddress(fact) {
				mono(t).FontSize(textCaption)
			}
		}
	})
}

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
	if !primary {
		g := glassOf(c.Theme())
		b.Background(g.panel).Border(1, g.edge)
		if b.Hovered() && !b.IsDisabled() {
			b.Background(g.selected)
		}
	}
	b.Label(label)
	b.Children(func() {
		ui.Row(c).Center().Gap(6).Children(func() {
			ui.Icon(c, icon).FontSize(13)
			ui.Text(c, label).SingleLine()
		})
	})
	return b
}

package main

import (
	"strings"

	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/action"
	"github.com/lobehub/lobe-mygo/pkg/display"
	"github.com/lobehub/lobe-mygo/pkg/feedback"
	"github.com/lobehub/lobe-mygo/pkg/layout"
	lu "github.com/lobehub/lobe-mygo/pkg/lucide"
	"github.com/lobehub/lobe-mygo/pkg/theme"
)

// The building blocks of the workspace's pages, on Lobe UI components: a
// page header with the subject's icon, cards that group what belongs
// together, and buttons whose visible text can be shorter than the name
// assistive technology and the tests find them by.

// Icons of the workspace, parsed once.
var (
	icoHexagon   = lu.Hexagon()
	icoCoffee    = lu.Coffee()
	icoRabbit    = lu.Rabbit()
	icoCode      = lu.CodeXml()
	icoDatabase  = lu.Database()
	icoLeaf      = lu.Leaf()
	icoZap       = lu.Zap()
	icoFish      = lu.Fish()
	icoPlay      = lu.Play()
	icoStop      = lu.Square()
	icoRotate    = lu.RotateCw()
	icoCopy      = lu.Copy()
	icoFolder    = lu.FolderOpen()
	icoTrash     = lu.Trash2()
	icoPlus      = lu.Plus()
	icoTerminal  = lu.SquareTerminal()
	icoShield    = lu.ShieldCheck()
	icoGlobe     = lu.Globe()
	icoLink      = lu.ExternalLink()
	icoKey       = lu.KeyRound()
	icoSearch    = lu.Search()
	icoScroll    = lu.ScrollText()
	icoWrench    = lu.Wrench()
	icoCheck     = lu.CircleCheck()
	icoDownload  = lu.Download()
	icoArchive   = lu.Archive()
	icoRestore   = lu.ArchiveRestore()
	icoHistory   = lu.History()
	icoRoute     = lu.Route()
	icoPlug      = lu.Plug()
	icoUndo      = lu.Undo2()
	icoSave      = lu.Save()
	icoStetho    = lu.Stethoscope()
	icoFileCheck = lu.FileSearch()
	icoUser      = lu.KeyRound()
	icoSettings  = lu.SlidersHorizontal()
	icoArrow     = lu.ArrowRight()
	icoEye       = lu.Eye()
	icoUserRound = lu.UserRound()
)

// softwareMeta is how a piece of software shows: its icon and brand tint.
func softwareMeta(name string) (*ui.SVG, ui.Color) {
	switch name {
	case "node":
		return icoHexagon, ui.RGB(95, 160, 78)
	case "java":
		return icoCoffee, ui.RGB(231, 111, 0)
	case "python":
		return icoCode, ui.RGB(55, 118, 171)
	case "mysql":
		return icoFish, ui.RGB(0, 117, 143)
	case "postgresql":
		return icoDatabase, ui.RGB(51, 103, 145)
	case "redis":
		return icoZap, ui.RGB(220, 56, 45)
	case "mongodb":
		return icoLeaf, ui.RGB(71, 162, 72)
	}
	return iconPackage, ui.RGB(128, 128, 128)
}

type btnKind int

const (
	btnDefault btnKind = iota
	btnPrimary
	btnText
	btnDanger
	btnFilled
)

// btn is a small Lobe button showing text after an optional icon.
func btn(c *ui.Context, text string, icon *ui.SVG, kind btnKind) ui.Element {
	variant := action.Default
	switch kind {
	case btnPrimary:
		variant = action.Primary
	case btnText:
		variant = action.Text
	case btnFilled:
		variant = action.Filled
	}
	b := ui.ButtonBase(c)
	action.StyleButton(c, b, "", action.ButtonProps{Variant: variant, Size: theme.Small})
	// Lobe's outlined buttons are opaque: on glass they take its fill.
	if kind == btnDefault || kind == btnDanger {
		g := glassOf(c.Theme())
		b.Background(g.panel).Border(1, g.edge)
		if b.Hovered() && !b.IsDisabled() {
			b.Background(g.selected)
		}
	}
	if kind == btnDanger {
		tk := theme.TokensOf(c)
		b.TextColor(tk.Error).Border(1, tk.ErrorBorder.Alpha(0.6))
		if b.Hovered() {
			b.Background(tk.ErrorFillTertiary)
		}
	}
	b.Shrink(0)
	b.Children(func() {
		if icon != nil {
			ui.Icon(c, icon).FontSize(13).Shrink(0)
		}
		ui.Text(c, text).SingleLine()
	})
	return b
}

// buttonRow lays buttons out in a row that wraps.
func devButtonRow(c *ui.Context, children func()) {
	ui.Row(c).Wrap().Gap(8).AlignItems(ui.Center).Children(children)
}

// iconTile is the rounded tile of a subject's icon, tinted with its color.
func iconTile(c *ui.Context, icon *ui.SVG, tint ui.Color, size float32) {
	ui.Box(c).Size(size, size).Radius(size*0.3).Shrink(0).Center().Background(tint.Alpha(0.14)).Border(1, tint.Alpha(0.22)).Children(func() {
		ui.Icon(c, icon).FontSize(size * 0.5).TextColor(tint)
	})
}

// pageHeader titles a page: the subject's icon, its name, data about it and
// its status, with the page's main actions at the end.
func pageHeader(c *ui.Context, icon *ui.SVG, tint ui.Color, title, meta string, status func(), actions func()) {
	tk := theme.TokensOf(c)
	ui.Row(c).Wrap().Gap(16).AlignItems(ui.Center).Margin(0, 0, 4).Children(func() {
		ui.Row(c).Gap(16).AlignItems(ui.Center).Grow(1).MinWidth(220).Children(func() {
			if icon != nil {
				iconTile(c, icon, tint, 40)
			}
			ui.Column(c).Gap(4).Grow(1).MinWidth(0).Children(func() {
				ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
					ui.Text(c, title).FontSize(20).FontWeight(600).TextColor(tk.TextHeading).SingleLine().MinWidth(0)
					if status != nil {
						status()
					}
				})
				if meta != "" {
					factLine(c, meta, tk.TextDescription)
				}
			})
		})
		if actions != nil {
			ui.Row(c).Gap(8).AlignItems(ui.Center).Shrink(0).Children(actions)
		}
	})
}

// card groups the controls of one concern on an outlined Lobe Block, under
// a title with optional controls at its end.
func devCard(c *ui.Context, title string, extra func(), body func()) {
	devCardWith(c, nil, title, "", extra, body)
}

// cardWith is a card whose title has an icon and a note after it.
func devCardWith(c *ui.Context, icon *ui.SVG, title, note string, extra func(), body func()) {
	tk := theme.TokensOf(c)
	glassPanel(c, ui.Column(c)).Padding(16, 20, 20).Gap(12).MinWidth(0).Children(func() {
		if title != "" || extra != nil {
			ui.Row(c).Gap(8).AlignItems(ui.Center).MinHeight(24).Children(func() {
				ui.Text(c, title).FontSize(13).FontWeight(500).TextColor(tk.TextHeading).SingleLine().Shrink(0)
				if note != "" {
					ui.Text(c, note).FontSize(textSmall).TextColor(tk.TextTertiary).SingleLine().MinWidth(0)
				}
				ui.Spacer(c)
				if extra != nil {
					extra()
				}
			})
		}
		body()
	})
}

// dangerCard is a card for what cannot be undone, outlined in the error
// color.
func devDangerCard(c *ui.Context, title string, body func()) {
	tk := theme.TokensOf(c)
	ui.Column(c).Padding(16, 20, 20).Gap(12).Radius(12).Border(1, tk.ErrorBorder.Alpha(0.6)).Background(tk.ErrorFillTertiary.Alpha(0.35)).MinWidth(0).Children(func() {
		ui.Row(c).Gap(8).AlignItems(ui.Center).MinHeight(24).Children(func() {
			ui.Icon(c, iconWarn).FontSize(14).TextColor(tk.Error)
			ui.Text(c, title).FontSize(13).FontWeight(500).TextColor(tk.ErrorText)
		})
		body()
	})
}

// subtle is a line of secondary text.
func devSubtle(c *ui.Context, s string) ui.Element {
	return ui.Text(c, s).FontSize(textSmall).TextColor(theme.TokensOf(c).TextDescription)
}

// kv shows a value under its name, as one of the facts about a subject.
func devKV(c *ui.Context, key, value string) {
	tk := theme.TokensOf(c)
	ui.Row(c).Gap(12).AlignItems(ui.Start).MinWidth(0).Children(func() {
		ui.Text(c, key).FontSize(textSmall).TextColor(tk.TextDescription).Width(96).Shrink(0)
		v := ui.Text(c, value).FontSize(textSmall).Selectable().Grow(1).MinWidth(0)
		if strings.HasPrefix(value, "/") || isAddress(value) {
			mono(v).FontSize(textCaption)
		}
	})
}

// inset is a filled area inside a card, for one item of a list in it.
func devInset(c *ui.Context, children func()) {
	ui.Column(c).Radius(8).Background(glassOf(c.Theme()).inset).Padding(12).Gap(8).MinWidth(0).Children(children)
}

// statusKind sorts the workspace's status words into Lobe's status colors.
func statusKind(status string) feedback.BadgeStatus {
	switch status {
	case "Healthy", "Running", "Succeeded", "Verified", "Published", "Ready", "Trusted", "Available", "Enabled", "Installed", "Allowed":
		return feedback.BadgeSuccess
	case "Failed", "Needs repair":
		return feedback.BadgeError
	case "Not ready", "Partly running", "Restarting", "Unknown", "Interrupted", "Pending", "In use", "Not installed", "Restricted":
		return feedback.BadgeWarning
	case "Starting", "Stopping", "Working", "Checking":
		return feedback.BadgeProcessing
	}
	return feedback.BadgeDefault
}

// statusTag is a status as a small round Lobe tag in its color.
func statusTag(c *ui.Context, status string) {
	if status == "" {
		return
	}
	color := display.TagColor("")
	switch statusKind(status) {
	case feedback.BadgeSuccess:
		color = display.TagSuccess
	case feedback.BadgeError:
		color = display.TagError
	case feedback.BadgeWarning:
		color = display.TagWarning
	case feedback.BadgeProcessing:
		color = display.TagProcessing
	}
	display.Tag(c, status, display.TagProps{Color: color, Size: theme.Small, Round: true})
}

// tag is a neutral small tag, for data such as a version.
func devTag(c *ui.Context, s string, color display.TagColor) {
	display.Tag(c, s, display.TagProps{Color: color, Size: theme.Small})
}

// masterGroup heads a group of the list beside a page.
func masterGroup(c *ui.Context, title string, extra func()) {
	tk := theme.TokensOf(c)
	ui.Row(c).Height(28).Padding(0, 4, 0, 8).Margin(8, 0, 0).AlignItems(ui.Center).Gap(8).Children(func() {
		ui.Text(c, title).FontSize(textSmall).FontWeight(500).TextColor(tk.TextTertiary).Grow(1).MinWidth(0).SingleLine()
		if extra != nil {
			extra()
		}
	})
}

// masterItem is a row of the list beside a page: the subject's icon in its
// tint, its name, and a short fact or its status at the end.
func masterItem(c *ui.Context, label string, icon *ui.SVG, tint ui.Color, title, trailing, status string, selected bool) bool {
	tk := theme.TokensOf(c)
	g := glassOf(c.Theme())
	b := ui.ButtonBase(c).Label(label).Height(32).Padding(0, 8).Radius(8).AlignItems(ui.Center).Cursor(ui.CursorPointer)
	switch {
	case selected:
		b.Background(g.selected)
	case b.Hovered():
		b.Background(g.hover)
	}
	b.Children(func() {
		ui.Row(c).Grow(1).MinWidth(0).AlignItems(ui.Center).Gap(8).Children(func() {
			// The subject's color is its page's: the list stays neutral.
			if icon != nil {
				color := tk.TextTertiary
				if selected {
					color = tk.TextSecondary
				}
				ui.Box(c).Size(20, 20).Center().Shrink(0).Children(func() {
					ui.Icon(c, icon).FontSize(16).TextColor(color)
				})
			}
			weight := 400
			if selected {
				weight = 500
			}
			ui.Text(c, title).FontWeight(weight).SingleLine().Grow(1).MinWidth(0)
			if trailing != "" {
				mono(ui.Text(c, trailing).FontSize(textCaption).TextColor(tk.TextTertiary).SingleLine().Shrink(0))
			}
			if status != "" {
				devStatusDot(c, status)
			}
		})
	})
	return b.Clicked()
}

// hint is a soft Lobe alert with a line of guidance.
func devHint(c *ui.Context, kind feedback.AlertType, title, description string) {
	feedback.Alert(c, title, feedback.AlertProps{Type: kind, Description: description})
}

// fieldGrid lays fields out in as many columns as fit, one when narrow.
func devFieldGrid(c *ui.Context, children func()) {
	layout.Grid(c, layout.GridProps{MaxItemWidth: 200, MaxColumns: 2, Gap: 12}).Children(children)
}

// devEmpty says a list has nothing in it, in Lobe's empty look.
func devEmpty(c *ui.Context, title string) {
	display.Empty(c, display.EmptyProps{Icon: icoCheck, Title: title})
}

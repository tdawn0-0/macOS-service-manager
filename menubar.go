package main

import (
	_ "embed"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	"github.com/egoist/mygo"
	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/choice"
	"github.com/lobehub/lobe-mygo/pkg/theme"

	"devbase/internal/devplatform"
	"devbase/internal/service"
)

// menubarIcon is black on transparent for macOS to tint to the menu bar, from
// assets/icons/menubar.svg (see scripts/icons.sh).
//
//go:embed assets/icons/menubar.png
var menubarIcon []byte

// menubar is the app's icon in the menu bar, and the panel it opens: a
// glance at what runs, with a switch for each, for starting and stopping it
// without opening the window. The workspace's own resources come first:
// database instances and the HTTPS proxy. Below them are the services the
// user runs by hand, in sections that fold. System agents and daemons are
// left out: their changes affect the whole machine, and daemons run as root.
type menubar struct {
	a     *app
	win   *mygo.Window // the main window
	tray  *mygo.Tray
	panel *mygo.Window // the open panel, or nil
}

// The panel's width, and the most height it grows to before it scrolls.
const (
	panelWidth     = 340
	panelMaxHeight = 620
)

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
		if a.shown(it) && !strings.HasPrefix(it.Name, "xyz.jyeu.devbase.dev.") {
			items = append(items, it)
		}
	}
	return items
}

// panelRunning counts what runs among the items the panel shows.
func (a *app) panelRunning() int {
	running := 0
	for id := range a.dev.state.Instances {
		if a.dev.statuses["instance:"+id] == "Running" {
			running++
		}
	}
	if a.dev.statuses["proxy"] == "Running" {
		running++
	}
	if a.dev.statuses["tunnel"] == "Running" {
		running++
	}
	for _, src := range panelSources {
		for _, it := range a.panelItems(src) {
			if it.Up() {
				running++
			}
		}
	}
	return running
}

// menubarView builds the panel. It takes the height of what it shows, up to
// panelMaxHeight, beyond which its middle scrolls.
func (a *app) menubarView(c *ui.Context, openWindow func()) {
	style(c)
	if a.panelOpen {
		a.showToasts(c)
	}
	tk := theme.TokensOf(c)
	c.Root().Background(ui.Transparent)
	box := ui.Column(c).Grow(1).MinHeight(0)
	if !c.Vibrancy() {
		box.Background(tk.BgLayout).Radius(12).Border(1, hairline(c.Theme()))
	}
	box.Children(func() {
		a.panelHeader(c)
		ui.Column(c).Height(1).Shrink(0).Background(hairline(c.Theme()))
		ui.Scroll(c.Key("panel-body")).Grow(1).MinHeight(0).Padding(6, 8, 8).Children(func() {
			ui.Column(c).Gap(2).Children(func() {
				a.panelDatabases(c, openWindow)
				a.panelProxy(c, openWindow)
				for _, src := range panelSources {
					a.panelServices(c, src)
				}
			})
		})
		ui.Column(c).Height(1).Shrink(0).Background(hairline(c.Theme()))
		ui.Row(c).Shrink(0).Padding(8, 10).Gap(6).AlignItems(ui.Center).Children(func() {
			if btn(c, "Open Devbase", iconWindow, btnDefault).Grow(1).Clicked() {
				openWindow()
			}
			iconButton(c, iconQuit, "Quit", tk.TextSecondary).OnClick(a.requestQuit)
		})
	})
}

// panelHeader names the app and says how much runs.
func (a *app) panelHeader(c *ui.Context) {
	tk := theme.TokensOf(c)
	ui.Row(c).Shrink(0).Padding(12, 10, 10, 14).AlignItems(ui.Center).Gap(10).Children(func() {
		iconTile(c, iconPackage, tk.Primary, 30)
		ui.Column(c).Grow(1).MinWidth(0).Gap(1).Children(func() {
			ui.Text(c, "Devbase").FontSize(14).FontWeight(500).SingleLine()
			ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
				running := a.panelRunning()
				status := "Stopped"
				if running > 0 {
					status = "Running"
				}
				devStatusDot(c, status)
				ui.Text(c, fmt.Sprintf("%d running", running)).TextColor(tk.TextDescription).FontSize(textCaption).SingleLine()
			})
		})
		if a.loading() || a.dev.busy {
			ui.Spinner(c).Label("Loading").Size(14, 14)
		}
		iconButton(c, icoRotate, "Refresh", tk.TextSecondary).Disabled(a.loading() || a.dev.busy).OnClick(func() {
			a.devTask(func(*devplatform.Manager) (string, error) { return "Workspace refreshed.", nil })
			for _, src := range panelSources {
				a.refresh(src)
			}
		})
	})
}

// panelSection heads a section of the panel, with a count and controls at its
// end. A section that folds does so on a click on its title, and reports
// whether it is open.
func (a *app) panelSection(c *ui.Context, key, title, count string, folds bool, extra func()) bool {
	tk := theme.TokensOf(c)
	open := !a.panelFolded[key]
	ui.Row(c).Height(28).Shrink(0).Padding(0, 4, 0, 6).Margin(4, 0, 0).AlignItems(ui.Center).Gap(6).Children(func() {
		var head ui.Element
		if folds {
			head = ui.ButtonBase(c).Label(title).Grow(1).MinWidth(0).AlignItems(ui.Center).Cursor(ui.CursorPointer)
		} else {
			head = ui.Row(c).Grow(1).MinWidth(0)
		}
		head.Children(func() {
			ui.Row(c).Grow(1).MinWidth(0).Gap(6).AlignItems(ui.Center).Justify(ui.Start).Children(func() {
				ui.Text(c, title).FontSize(textSmall).FontWeight(500).TextColor(tk.TextSecondary).SingleLine().Shrink(0)
				if count != "" {
					ui.Text(c, count).FontSize(textCaption).TextColor(tk.TextTertiary).SingleLine()
				}
				if folds {
					chevron := "▾"
					if !open {
						chevron = "▸"
					}
					ui.Text(c, chevron).FontSize(textCaption).TextColor(tk.TextTertiary)
				}
			})
		})
		if folds && head.Clicked() {
			if a.panelFolded == nil {
				a.panelFolded = map[string]bool{}
			}
			a.panelFolded[key] = open
			open = !open
		}
		if extra != nil {
			extra()
		}
	})
	return open
}

// panelRow is a row of the panel: an icon or a status dot, a title over a
// line about it, the actions that show while the pointer is on the row, and
// a switch at its end. A click on the title opens it, where open is set.
func panelRow(c *ui.Context, key string, lead func(), title, sub, openLabel string, open func(), hover func(), end func()) {
	tk := theme.TokensOf(c)
	row := ui.Row(c.Key(key)).Height(40).Shrink(0).AlignItems(ui.Center).Gap(10).Padding(0, 8).Radius(tk.BorderRadius)
	hovered := row.Hovered()
	if hovered {
		row.Background(tk.FillTertiary)
	}
	row.Children(func() {
		lead()
		var body ui.Element
		if open != nil {
			body = ui.ButtonBase(c).Label(openLabel).Grow(1).MinWidth(0).AlignItems(ui.Center).Cursor(ui.CursorPointer)
		} else {
			body = ui.Row(c).Grow(1).MinWidth(0)
		}
		body.Children(func() {
			ui.Column(c).Grow(1).MinWidth(0).Gap(1).AlignItems(ui.Start).Justify(ui.Center).Children(func() {
				ui.Text(c, title).SingleLine().MinWidth(0).Tooltip(title)
				if sub != "" {
					ui.Text(c, sub).FontSize(textCaption).TextColor(tk.TextDescription).SingleLine().MinWidth(0)
				}
			})
		})
		if open != nil && body.Clicked() {
			open()
		}
		if hover != nil && hovered {
			hover()
		}
		end()
	})
}

// panelDatabases lists the managed database instances, each with a switch
// that starts or stops it, and copies its connection or opens its logs on
// hover.
func (a *app) panelDatabases(c *ui.Context, openWindow func()) {
	d := &a.dev
	running := 0
	for id := range d.state.Instances {
		if s := d.statuses["instance:"+id]; s == "Running" || s == "Healthy" {
			running++
		}
	}
	count := ""
	if n := len(d.state.Instances); n > 0 {
		count = fmt.Sprintf("%d/%d", running, n)
	}
	if !a.panelSection(c, "databases", "Databases", count, false, nil) {
		return
	}
	if len(d.state.Instances) == 0 {
		panelRow(c, "menu-db-none", func() { iconTile(c, icoDatabase, theme.TokensOf(c).TextTertiary, 26) }, "No instances", "", "New database instance", func() {
			a.openNewInstance("postgresql")
			openWindow()
		}, nil, func() {})
		return
	}
	for _, id := range sortedKeys(d.state.Instances) {
		a.panelDatabase(c, d.state.Instances[id], openWindow)
	}
}

func (a *app) panelDatabase(c *ui.Context, instance devplatform.Instance, openWindow func()) {
	d := &a.dev
	id := instance.ID
	status := d.statuses["instance:"+id]
	if status == "" {
		status = "Unknown"
	}
	on := status == "Running" || status == "Healthy"
	busy := d.busy || a.databaseTaskRunning(id) || a.quit.open
	switch status {
	case "Unknown", "Starting", "Stopping", "Restarting":
		busy = true
	}
	icon, _ := softwareMeta(instance.Kind)
	tint := theme.TokensOf(c).TextSecondary
	sub := softwareTitle(instance.Kind) + " · :" + strconv.Itoa(instance.Port)
	if news := devNews(status); news != "" {
		sub = news + " · " + sub
	}
	panelRow(c, "menu-db-"+id, func() {
		ui.Box(c).Size(26, 26).Shrink(0).Children(func() {
			iconTile(c, icon, tint, 26)
			ui.Box(c).Absolute().Right(-2).Bottom(-2).Size(10, 10).Radius(5).Center().Background(theme.TokensOf(c).BgContainer).Children(func() {
				devStatusDot(c, status)
			})
		})
	}, id, sub, "Open "+id, func() {
		a.openInstance(id)
		openWindow()
	}, func() {
		if iconButton(c, icoCopy, "Copy connection URI for "+id, theme.TokensOf(c).TextSecondary).Disabled(d.busy).Clicked() {
			a.devTask(func(m *devplatform.Manager) (string, error) {
				info, err := m.ConnectionInfo(id)
				if err == nil {
					a.post(func() {
						if a.copy != nil {
							a.copy(info.URI)
						}
						a.toasts = append(a.toasts, "Copied connection URI for "+id)
					})
				}
				return "Connection URI copied", err
			})
		}
		if iconButton(c, icoScroll, "Open logs for "+id, theme.TokensOf(c).TextSecondary).Disabled(d.busy).Clicked() {
			a.openInstanceLogs(id, openWindow)
		}
	}, func() {
		if a.databaseTaskRunning(id) {
			ui.Spinner(c).Label("Working").Size(13, 13)
		}
		verb, label := "start", "Start "+id
		if on {
			verb, label = "stop", "Stop "+id
		}
		if choice.Switch(c, &on, choice.SwitchProps{Size: theme.Small}).Label(label).Disabled(busy).Changed() {
			result := map[string]string{"start": "Started ", "stop": "Stopped "}[verb] + id
			a.startDatabaseOperation(instance, verb, func(m *devplatform.Manager) (string, error) {
				return result, m.InstanceAction(m.Context, id, verb)
			})
		}
	})
}

// openInstanceLogs opens an instance's log file in Console, or its page in
// the window when it has not written one yet.
func (a *app) openInstanceLogs(id string, openWindow func()) {
	a.openJobLogs("db-"+id, id, func() {
		a.openInstance(id)
		openWindow()
	})
}

// openJobLogs opens a background job's log file in Console, or falls back to
// displaying its bounded log tail in the workspace status drawer.
func (a *app) openJobLogs(jobID, displayLabel string, onFallback func()) {
	a.devTask(func(m *devplatform.Manager) (string, error) {
		for _, name := range []string{jobID + ".err.log", jobID + ".log"} {
			path := filepath.Join(m.Root, "logs", name)
			info, err := os.Stat(path)
			if err != nil || !info.Mode().IsRegular() || info.Size() == 0 {
				continue
			}
			if _, err := m.Run(m.Context, "/usr/bin/open", "-a", "Console", path); err != nil {
				break
			}
			a.post(func() { a.toasts = append(a.toasts, "Opened logs for "+displayLabel+" in Console") })
			return "Opened logs in Console", nil
		}
		a.post(func() {
			if onFallback != nil {
				onFallback()
			}
			a.dev.showResult = true
		})
		return m.Logs(jobID)
	})
}

// panelProxyURL is the address the one saved rule answers at, or "" unless
// exactly one rule is saved and the proxy is running.
func (a *app) panelProxyURL() string {
	d := &a.dev
	if len(d.state.ProxyRules) != 1 || d.statuses["proxy"] != "Running" {
		return ""
	}
	url := "https://" + d.state.ProxyRules[0].Domain
	if port := d.state.ProxyListenerPort; port != 0 && port != 443 {
		url = fmt.Sprintf("%s:%d", url, port)
	}
	return url
}

func (a *app) savedProxyURLs() []string {
	d := &a.dev
	urls := make([]string, 0, len(d.state.ProxyRules))
	for _, rule := range d.state.ProxyRules {
		if rule.Domain == "" {
			continue
		}
		u := "https://" + rule.Domain
		if port := d.state.ProxyListenerPort; port != 0 && port != 443 {
			u = fmt.Sprintf("%s:%d", u, port)
		}
		urls = append(urls, u)
	}
	return urls
}

func (a *app) savedTunnelURLs() []string {
	d := &a.dev
	urls := make([]string, 0, len(d.state.ProxyRules))
	for _, rule := range d.state.ProxyRules {
		if !rule.Tunnel {
			continue
		}
		u := ""
		if w, ok := d.state.TunnelWorkers[rule.ResolvedTunnelWorker()]; ok && w.PublicURL != "" {
			u = strings.TrimSpace(w.PublicURL)
		} else {
			u = rule.PreviewTunnelURL(d.state.CloudflareSubdomain)
		}
		if u != "" {
			urls = append(urls, u)
		}
	}
	return urls
}

func (a *app) panelURLHoverActions(c *ui.Context, active bool, urls []string, copyLabel, openLabel, openOutput string) {
	if !active || len(urls) == 0 {
		return
	}
	tint := theme.TokensOf(c).TextSecondary
	if len(urls) == 1 {
		u := urls[0]
		if iconButton(c, icoCopy, copyLabel, tint).Clicked() {
			if a.copy != nil {
				a.copy(u)
			}
			a.toast("Copied %s", u)
		}
		if iconButton(c, icoLink, openLabel, tint).Disabled(a.dev.busy).Clicked() {
			a.devTask(func(m *devplatform.Manager) (string, error) {
				_, err := m.Run(m.Context, "/usr/bin/open", u)
				return openOutput, err
			})
		}
		return
	}
	iconButton(c, icoLink, openLabel, tint).Disabled(a.dev.busy).Menu(func(m *ui.Menu) {
		for _, u := range urls {
			u := u
			if m.Item("Open " + u).Chosen() {
				a.devTask(func(mgr *devplatform.Manager) (string, error) {
					_, err := mgr.Run(mgr.Context, "/usr/bin/open", u)
					return openOutput, err
				})
			}
		}
		m.Separator()
		for _, u := range urls {
			u := u
			if m.Item("Copy " + u).Chosen() {
				if a.copy != nil {
					a.copy(u)
				}
				a.toast("Copied %s", u)
			}
		}
	})
}

// openInstance goes to an instance's page in the window.
func (a *app) openInstance(id string) {
	d := &a.dev
	i, ok := d.state.Instances[id]
	a.nav = navDevelopment
	d.tab = devTabDatabases
	if !ok || id != d.dbID && a.connectionProfileDirty() {
		return
	}
	d.dbNewDraft, d.databaseWorkOpen, d.initializationsOpen, d.deletionsOpen = false, false, false, false
	d.instanceDeletionPreview = nil
	d.dbID, d.dbKind, d.dbPackage, d.dbPort, d.persistent = id, i.Kind, i.Package, strconv.Itoa(i.Port), i.Persistent
	d.detailScroll[devTabDatabases].Y = 0
	a.loadConnectionProfileDraft(id, "")
}

// panelProxy is the single HTTPS proxy: its state, a switch that applies the
// saved rules or stops it, and the way to its settings. Its rules are not
// resources of their own, and are not listed. With exactly one rule saved and
// the proxy running, a button on the row opens its HTTPS address.
func (a *app) panelProxy(c *ui.Context, openWindow func()) {
	d := &a.dev
	status := d.statuses["proxy"]
	if status == "" {
		status = "Stopped"
		if len(d.state.ProxyRules) > 0 {
			status = "Unknown"
		}
	}
	openSettings := func() {
		a.nav = navDevelopment
		d.tab = devTabHTTPS
		openWindow()
	}
	a.panelSection(c, "https", "HTTPS", "", false, func() {
		if btn(c, "HTTPS settings", nil, btnText).Clicked() {
			openSettings()
		}
	})
	listener := d.state.ProxyListenerPort
	if listener == 0 {
		listener = 443
	}
	sub := fmt.Sprintf("%s · :%d", plural(len(d.state.ProxyRules), "rule"), listener)
	on := status == "Running"
	panelRow(c, "menu-proxy", func() {
		ui.Box(c).Size(26, 26).Shrink(0).Children(func() {
			iconTile(c, icoShield, theme.TokensOf(c).TextSecondary, 26)
			ui.Box(c).Absolute().Right(-2).Bottom(-2).Size(10, 10).Radius(5).Center().Background(theme.TokensOf(c).BgContainer).Children(func() {
				devStatusDot(c, status)
			})
		})
	}, "Proxy · "+status, sub, "Open HTTPS proxy", openSettings, func() {
		a.panelURLHoverActions(c, on, a.savedProxyURLs(), "Copy HTTPS URL", "Open HTTPS URL", "Opened saved HTTPS rule in browser.")
	}, func() {
		label := "Apply proxy"
		if on {
			label = "Stop HTTPS proxy"
		}
		if choice.Switch(c, &on, choice.SwitchProps{Size: theme.Small}).Label(label).Disabled(d.busy || a.quit.open || len(d.state.ProxyRules) == 0 && !on).Changed() {
			a.devTask(func(m *devplatform.Manager) (string, error) {
				if on {
					return "Proxy configuration published. Check DNS, TLS and upstream separately.", m.ApplyProxy(m.Context)
				}
				return "Stopped HTTPS proxy", m.StopProxy(m.Context)
			})
		}
	})
	if a.tunnelConfigured() {
		tunnelStatus := d.statuses["tunnel"]
		if tunnelStatus == "" {
			tunnelStatus = "Stopped"
		}
		tunnelSub := fmt.Sprintf("%d workers", len(d.state.TunnelWorkers))
		if len(d.state.TunnelWorkers) == 1 {
			tunnelSub = "1 worker"
		} else if len(d.state.TunnelWorkers) == 0 {
			tunnelSub = "Cloudflare Workers"
		}
		if detail := d.statuses["tunnel:detail"]; tunnelStatus == "Running" && detail != "" {
			tunnelSub = detail
		}
		tunnelOn := tunnelStatus == "Running"
		hasRules := false
		for _, rule := range d.state.ProxyRules {
			if rule.Tunnel {
				hasRules = true
				break
			}
		}
		panelRow(c, "menu-tunnel", func() {
			ui.Box(c).Size(26, 26).Shrink(0).Children(func() {
				iconTile(c, icoGlobe, theme.TokensOf(c).TextSecondary, 26)
				ui.Box(c).Absolute().Right(-2).Bottom(-2).Size(10, 10).Radius(5).Center().Background(theme.TokensOf(c).BgContainer).Children(func() {
					devStatusDot(c, tunnelStatus)
				})
			})
		}, "Tunnel · "+tunnelStatus, tunnelSub, "Open Cloudflare tunnel settings", openSettings, func() {
			a.panelURLHoverActions(c, tunnelOn, a.savedTunnelURLs(), "Copy tunnel URL", "Open tunnel URL", "Opened tunnel URL in browser.")
		}, func() {
			label := "Apply tunnel"
			if tunnelOn {
				label = "Stop tunnel"
			}
			if choice.Switch(c, &tunnelOn, choice.SwitchProps{Size: theme.Small}).Label(label).Disabled(d.busy || a.quit.open || (!hasRules && !tunnelOn)).Changed() {
				a.devTask(func(m *devplatform.Manager) (string, error) {
					if tunnelOn {
						return "Cloudflare tunnel started.", m.ApplyTunnel(m.Context)
					}
					return "Stopped Cloudflare tunnel", m.StopTunnel(m.Context)
				})
			}
		})
	}
}

func (a *app) tunnelConfigured() bool {
	d := &a.dev
	if d.statuses["tunnel:token"] == "Configured" || len(d.state.TunnelWorkers) > 0 || d.statuses["tunnel"] == "Running" {
		return true
	}
	for _, rule := range d.state.ProxyRules {
		if rule.Tunnel {
			return true
		}
	}
	return false
}

// panelServices lists the services of a source, under a heading that folds.
func (a *app) panelServices(c *ui.Context, src service.Source) {
	items := a.panelItems(src)
	st := a.sources[src]
	failed := st != nil && st.err != ""
	if len(items) == 0 && !failed {
		return
	}
	running := 0
	for _, it := range items {
		if it.Up() {
			running++
		}
	}
	if !a.panelSection(c, "src:"+string(src), src.Title(), fmt.Sprintf("%d/%d", running, len(items)), true, nil) {
		return
	}
	if failed {
		ui.Text(c, st.err).TextColor(theme.TokensOf(c).ErrorText).FontSize(textCaption).Padding(2, 8, 4)
	}
	for _, it := range items {
		a.menubarRow(c, it)
	}
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
	tk := theme.TokensOf(c)
	row := ui.Row(c.Key(it.ID)).Height(32).Shrink(0).AlignItems(ui.Center).Gap(10).Padding(0, 8).Radius(tk.BorderRadius)
	if row.Hovered() {
		row.Background(tk.FillTertiary)
	}
	row.Children(func() {
		ui.Box(c).Size(26, 26).Shrink(0).Center().Children(func() { statusDot(c, it.State, false) })
		ui.Text(c, it.Name).SingleLine().Grow(1).MinWidth(0).Tooltip(it.Name)
		if busy {
			ui.Spinner(c).Label("Working").Size(13, 13)
		}
		if choice.Switch(c, &on, choice.SwitchProps{Size: theme.Small}).Label(it.Name).Disabled(busy).Changed() {
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
	m.tray.SetToolTip(fmt.Sprintf("Devbase: %d running", running))
}

func (m *menubar) showWindow() {
	if m.panel != nil {
		m.panel.Close()
	}
	// The icon comes back with the window; showing it activates the app.
	m.a.showDockIcon(true)
	m.win.Show()
	m.win.Focus()
	m.a.post(m.a.windowShown)
}

// panelHeight estimates the panel's height from its rows: the size it opens
// at, before it measures what it shows.
func (a *app) panelHeight() int {
	const header, footer, row, heading = 56, 50, 42, 32
	height := header + footer + 16
	height += heading + max(len(a.dev.state.Instances), 1)*row
	height += heading + row // The single HTTPS proxy.
	if a.tunnelConfigured() {
		height += row
	}
	for _, src := range panelSources {
		if n := len(a.panelItems(src)); n > 0 {
			height += heading
			if !a.panelFolded["src:"+string(src)] {
				height += n * 34
			}
		}
	}
	return min(max(height, 220), panelMaxHeight)
}

// toggle opens the panel below the icon, or closes it.
func (m *menubar) toggle() {
	if m.panel != nil {
		m.panel.Close()
		return
	}
	m.a.panelOpen = true
	m.panel = mygo.NewFlyout(mygo.FlyoutOptions{
		Tray:       m.tray,
		Placement:  mygo.PlacementBottom,
		Gap:        4,
		Width:      panelWidth,
		Height:     m.a.panelHeight(),
		FitContent: true,
		MinWidth:   panelWidth,
		MaxWidth:   panelWidth,
		MinHeight:  180,
		MaxHeight:  panelMaxHeight,
		Focusable:  true,
		Shadow:     true,
		Popover:    true,
		Content:    ui.View(func(c *ui.Context) { m.a.menubarView(c, m.showWindow) }),
	})
	m.panel.OnClosed(func() {
		m.panel = nil
		m.a.panelOpen = false
	})
	// What the list shows may be old.
	if !m.a.dev.busy {
		m.a.devTask(func(*devplatform.Manager) (string, error) { return "Workspace refreshed.", nil })
	}
	for _, src := range panelSources {
		m.a.refresh(src)
	}
}

// install puts the icon in the menu bar.
func (m *menubar) install() error {
	tray, err := mygo.NewTray(mygo.TrayOptions{
		ID:             "xyz.jyeu.devbase.menubar",
		Icon:           menubarIcon,
		IconIsTemplate: true,
		ToolTip:        "Devbase",
	})
	if err != nil {
		return err
	}
	m.tray = tray
	menu := mygo.NewMenu([]*mygo.MenuItem{
		{Label: "Open Devbase", Click: func(*mygo.MenuItem, *mygo.Window) { m.showWindow() }},
		{Label: "Refresh", Click: func(*mygo.MenuItem, *mygo.Window) {
			m.a.refreshNavigation()
			for _, src := range panelSources {
				m.a.refresh(src)
			}
		}},
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

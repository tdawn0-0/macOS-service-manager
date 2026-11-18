package main

import (
	"context"
	"errors"
	"fmt"
	"sort"
	"strconv"
	"strings"
	"time"

	"devbase/internal/devplatform"
	"devbase/internal/privhelper"
	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/action"
	"github.com/lobehub/lobe-mygo/pkg/theme"
)

const (
	devTabSoftware = iota
	devTabDatabases
	devTabHTTPS
)

const (
	tunnelModeWorkersDev   = "workers.dev (Auto)"
	tunnelModeCustomDomain = "Custom domain"
)

type proxyRuleDraft struct {
	Domain       string
	LocalPort    string
	UpstreamHost string
	Tunnel       bool
	TunnelAuth   bool
	TunnelWorker string
	TunnelMode   string
	TunnelDomain string
	TunnelURL    string
}

func effectiveDraftTunnelDomain(row proxyRuleDraft) string {
	if row.TunnelMode != tunnelModeCustomDomain {
		return ""
	}
	return strings.ToLower(strings.TrimSpace(row.TunnelDomain))
}

type developmentState struct {
	javaInstallations                                                                                   []devplatform.JavaInstallation
	javaLoaded                                                                                          bool
	javaError                                                                                           string
	javaRemove                                                                                          *devplatform.JavaInstallation
	runtimeVersions                                                                                     map[string][]string
	uvPath, uvDescription, uvPin, uvVersion                                                             string
	uvInstalled, uvAvailable                                                                            []devplatform.UVPython
	databaseWorkOpen                                                                                    bool
	databaseWork                                                                                        []devplatform.DatabaseWorkInfo
	databaseTasks                                                                                       map[string]*databaseTask
	databaseNeedsRefresh, dbNewDraft, initializationsOpen                                               bool
	lastDatabaseTask                                                                                    *databaseTask
	initializations                                                                                     []devplatform.InstanceInitializationInfo
	packageTasks                                                                                        map[string]*packageTask
	packageNeedsRefresh                                                                                 bool
	lastPackageTask                                                                                     *packageTask
	packageContention                                                                                   *devplatform.PackageBusyError
	instanceDeletionPreview                                                                             *devplatform.InstanceDeletionPreview
	instanceDeletions                                                                                   []devplatform.InstanceDeletionInfo
	deletionsOpen                                                                                       bool
	deleteConfirmation                                                                                  string
	operationsOpen                                                                                      bool
	operations                                                                                          []devplatform.OperationRecord
	operationID, operationsError                                                                        string
	operationScroll                                                                                     ui.ScrollState
	authorityOpen                                                                                       bool
	authorityStatus                                                                                     *devplatform.AuthorityStatus
	portChecks                                                                                          map[int]devplatform.PortInspection
	diagnosticsOpen                                                                                     bool
	diagnosticReport                                                                                    *devplatform.DiagnosticReport
	diagnosticScroll                                                                                    ui.ScrollState
	diagnosticJSON                                                                                      string
	diagnosticPendingTarget                                                                             *devplatform.DiagnosticTarget
	packagePreview                                                                                      *devplatform.PackageDetails
	packageDetailsText                                                                                  string
	packageChecks                                                                                       map[string]devplatform.PackageIntegrityResult
	detailScroll                                                                                        [3]ui.ScrollState
	managerFactory                                                                                      func() *devplatform.Manager
	statuses                                                                                            map[string]string
	statusPolling                                                                                       bool
	statusLast                                                                                          time.Time
	taskSequence                                                                                        uint64
	loaded, busy                                                                                        bool
	tab                                                                                                 int
	state                                                                                               devplatform.State
	output                                                                                              string
	cancel                                                                                              context.CancelFunc
	software, version                                                                                   string
	dbDataDirectory                                                                                     string
	dbKind, dbID, dbPackage, dbPort, dbName, dbUser, dbPassword, backupPath, restoreDB                  string
	profileDraft                                                                                        devplatform.ConnectionProfile
	profileSelected, profileDraftInstance, profilePendingInstance, profilePendingSelect, profileRedisDB string
	profileOffer                                                                                        *connectionOperationOffer
	persistent                                                                                          bool
	backups                                                                                             []devplatform.BackupFile
	backupsFor, backupsError                                                                            string
	backupsLoading                                                                                      bool
	dbCatalog                                                                                           devplatform.InstanceCatalog
	dbCatalogFor                                                                                        string
	dbCatalogLoading                                                                                    bool
	proxyDraft                                                                                          []proxyRuleDraft
	proxyListenerPort                                                                                   string
	proxyDraftRevision                                                                                  uint64
	proxyDraftLoaded                                                                                    bool
	cloudflareAccountID, cloudflareTokenDraft, tunnelClientID                                           string
	tunnelDraftLoaded                                                                                   bool
	proxyPublicationState                                                                               string
	proxyChecks                                                                                         map[string]devplatform.ProxyHTTPResult
	proxyChecking                                                                                       map[string]bool
	proxyCheckGeneration                                                                                uint64
	tunnelChecks                                                                                        map[string]devplatform.ProxyHTTPResult
	tunnelChecking                                                                                      map[string]bool
	tunnelCheckGeneration                                                                               uint64
	nodeReleases                                                                                        []devplatform.NodeRelease
	nodeReleasesTried, nodeReleasesRefresh, nodeReleasesStale                                           bool
	nodeReleasesError                                                                                   string
	nodeShell                                                                                           int
	runtimeShell                                                                                        map[string]int
	showResult                                                                                          bool
}

func (a *app) devTask(fn func(*devplatform.Manager) (string, error)) {
	a.devTaskThen(fn, nil)
}
func (a *app) devTaskThen(fn func(*devplatform.Manager) (string, error), after func(error)) {
	a.devTaskKeepingDraft(fn, after, false)
}
func (a *app) devTaskKeepingDraft(fn func(*devplatform.Manager) (string, error), after func(error), keepDraft bool) {
	if a.dev.busy {
		return
	}
	a.dev.busy = true
	a.dev.taskSequence++
	initialize := !a.dev.loaded && a.dev.managerFactory == nil
	ctx, cancel := context.WithCancel(context.Background())
	previousState := a.dev.state
	refreshJava := a.dev.javaLoaded || a.dev.software == "java"
	a.dev.cancel = cancel
	a.spawn(func() {
		defer cancel()
		m := devplatform.New(devplatform.DefaultRoot())
		if a.dev.managerFactory != nil {
			m = a.dev.managerFactory()
		}
		m.Context = ctx
		m.Event = func(message string) { a.post(func() { a.dev.output = message }) }
		var maintenanceError error
		if initialize {
			maintenanceError = m.EnsureLogMaintenance(ctx)
			_ = m.RefreshNodeShellIfPresent()
		}
		out, e := fn(m)
		var contention *devplatform.PackageBusyError
		if errors.As(e, &contention) {
			a.post(func() { a.dev.packageContention = contention })
		}
		var portError *devplatform.PortUnavailableError
		if errors.As(e, &portError) {
			inspection, inspectErr := m.InspectPort(ctx, portError.Port)
			if inspectErr == nil {
				a.post(func() { a.storePortInspection(inspection) })
			}
		}
		if maintenanceError != nil {
			out += "\nLog maintenance: " + maintenanceError.Error()
		}
		s, se := m.Read()
		if se != nil {
			s = previousState
		}
		var javaRows []devplatform.JavaInstallation
		var javaErr error
		if refreshJava && se == nil {
			javaRows, javaErr = m.JavaInstallations(ctx)
		}
		snapshot := m.Snapshot(context.Background(), s)
		operations, oe := m.Operations(context.Background())
		pubState, pubErr := m.ProxyPublicationState()
		if pubErr != nil || pubState == "" {
			pubState = "Unknown"
		}
		helperStatus := privhelper.Default().Status()
		a.post(func() {
			a.dev.busy = false
			a.helperStatus = helperStatus
			if oe == nil {
				a.dev.operations = operations
				a.dev.operationsError = ""
			} else {
				a.dev.operationsError = "Task history could not be read. Refresh after inspecting the storage folder."
			}
			a.dev.cancel = nil
			a.dev.loaded = true
			a.dev.state = s
			if refreshJava {
				a.dev.javaLoaded = true
				a.dev.javaError = ""
				if javaErr != nil {
					a.dev.javaError = javaErr.Error()
				} else if se == nil {
					a.dev.javaInstallations = javaRows
				}
			}
			if !a.dev.proxyDraftLoaded || a.dev.proxyDraftRevision != s.ProxyRulesRevision && !a.dev.proxyDraftDirty() {
				a.dev.loadProxyDraft(s)
			}
			if !a.dev.tunnelDraftLoaded || (!keepDraft && !a.dev.tunnelDraftDirty()) {
				a.dev.loadTunnelDraft(s)
			}
			for id := range a.dev.packageChecks {
				if _, ok := s.Packages[id]; !ok {
					delete(a.dev.packageChecks, id)
				}
			}
			if a.dev.packagePreview != nil {
				installed, ok := s.Packages[a.dev.packagePreview.Package.ID()]
				a.dev.packagePreview.Installed = ok && installed.SHA256 == a.dev.packagePreview.Package.SHA256
			}
			a.dev.proxyPublicationState = pubState
			a.dev.statuses = snapshot.States
			a.applyDatabaseTaskPhases()
			a.dev.statusLast = time.Now()
			if e != nil {
				out += "\nError: " + e.Error()
				a.helperProblem(e, nil)
				if privhelper.NeedsReinstall(e) {
					a.helperStatus = privhelper.HelperStatus{State: privhelper.StateNotInstalled, Message: "The installed helper was set up by a different Devbase build. Reinstall it."}
				}
			}
			if se != nil {
				out += "\nError: Could not read saved settings. " + se.Error()
			}
			a.dev.output = out
			if after != nil {
				if e != nil {
					after(e)
				} else {
					after(se)
				}
			}
			a.refreshFinishedPackages()
		})
	})
}
func devField(c *ui.Context, label string, value *string, placeholder string) {
	ui.Field(c, label, func() { ui.TextInput(c, value).Placeholder(placeholder) })
}
func devChoice(c *ui.Context, label string, value *string, options []string) bool {
	changed := false
	ui.Field(c, label, func() { changed = ui.Select(c, value, options).Changed() })
	return changed
}
func sortedKeys[T any](m map[string]T) []string {
	a := make([]string, 0, len(m))
	for key := range m {
		a = append(a, key)
	}
	sort.Strings(a)
	return a
}
func (d *developmentState) packages(kind string) []string {
	var ids []string
	for id, p := range d.state.Packages {
		if kind == "" || p.Name == kind {
			ids = append(ids, id)
		}
	}
	sort.Strings(ids)
	return ids
}

func (a *app) developmentView(c *ui.Context) {
	a.pollDevStatus()
	c.After(5 * time.Second)
	d := &a.dev
	if !d.loaded && !d.busy {
		a.devTask(func(m *devplatform.Manager) (string, error) { return "Ready to build locally.", nil })
	}
	if d.tab < devTabSoftware || d.tab > devTabHTTPS {
		d.tab = devTabSoftware
	}
	g := glassOf(c.Theme())
	// The bars and the list lie on the window's frost, the page on a sheet.
	ui.Column(c).Grow(1).MinWidth(0).Children(func() {
		a.devTopBar(c)
		ui.Column(c).Height(1).Shrink(0).Background(hairline(c.Theme()))
		switch {
		case d.operationsOpen:
			a.devOperations(c)
		case d.diagnosticsOpen:
			ui.Scroll(c.Key("development-diagnostics")).TrackScroll(&d.diagnosticScroll).Grow(1).MinHeight(0).Padding(24, 28, 28).Background(g.sheet).Children(func() {
				ui.Column(c).Gap(16).MaxWidth(880).Children(func() { a.devDiagnostics(c) })
			})
		default:
			ui.Row(c).Grow(1).MinHeight(0).AlignItems(ui.Stretch).Children(func() {
				// HTTPS is one table: it has no list beside it.
				if d.tab != devTabHTTPS {
					ui.Column(c).Width(228).Shrink(0).Children(func() {
						ui.Scroll(c.Key("dev-master-"+strconv.Itoa(d.tab))).Grow(1).MinHeight(0).Padding(4, 8, 12).Children(func() {
							ui.Column(c).Gap(2).Children(func() { a.devMaster(c) })
						})
					})
					ui.Column(c).Width(1).Background(hairline(c.Theme()))
				}
				ui.Scroll(c.Key(fmt.Sprintf("dev-detail-%d", d.tab))).TrackScroll(&d.detailScroll[d.tab]).Grow(1).MinWidth(0).MinHeight(0).Padding(24, 28, 28).Background(g.sheet).Children(func() {
					ui.Column(c).Gap(16).MaxWidth(880).Disabled(d.busy).Children(func() {
						switch d.tab {
						case devTabSoftware:
							a.devSoftware(c)
						case devTabDatabases:
							a.devDatabases(c)
						case devTabHTTPS:
							a.devHTTPS(c)
						}
					})
				})
			})
		}
		a.devLogDrawer(c)
		a.devStatusBar(c)
	})
}

// devTitle is the title of the workspace's bar, with a count of what the
// section holds.
func (a *app) devTitle() (string, string) {
	d := &a.dev
	switch {
	case d.operationsOpen:
		return "Task history", ""
	case d.diagnosticsOpen:
		return "Diagnostics", ""
	}
	switch d.tab {
	case devTabSoftware:
		if n := len(d.state.Packages); n > 0 {
			return devSections[d.tab], plural(n, "package") + " installed"
		}
	case devTabDatabases:
		running := 0
		for id := range d.state.Instances {
			if s := d.statuses["instance:"+id]; s == "Running" || s == "Healthy" {
				running++
			}
		}
		if n := len(d.state.Instances); n > 0 {
			return devSections[d.tab], fmt.Sprintf("%s · %d running", plural(n, "instance"), running)
		}
	case devTabHTTPS:
		return devSections[d.tab], plural(len(d.state.ProxyRules), "rule")
	}
	return devSections[d.tab], ""
}

// devTopBar is the workspace's title bar: the section and its count, the
// work going on, and the workspace-wide actions as icons.
func (a *app) devTopBar(c *ui.Context) {
	d := &a.dev
	// The sidebar counts what the section holds: the bar only names it.
	title, _ := a.devTitle()
	ui.Row(c).Height(barHeight).Shrink(0).Padding(0, 12, 0, 20).AlignItems(ui.Center).Gap(8).DragWindow().Children(func() {
		ui.Text(c, title).FontSize(15).FontWeight(600).SingleLine().Shrink(0)
		ui.Spacer(c)
		if d.busy || a.backgroundTasksRunning() {
			ui.Spinner(c).Size(14, 14)
		}
		if !d.busy && len(d.databaseTasks) > 0 && (d.tab == devTabDatabases || a.databaseTasksRunning()) {
			if btn(c, "Database tasks", icoHistory, btnFilled).Clicked() {
				a.showDatabaseTasks()
			}
		} else if !d.busy && len(d.packageTasks) > 0 {
			if btn(c, "Package tasks", icoDownload, btnFilled).Clicked() {
				a.showPackageTasks()
			}
		}
		if d.busy {
			if btn(c, "Cancel operation", nil, btnDefault).Clicked() && d.cancel != nil {
				d.cancel()
			}
		}
		ui.Box(c).Width(1).Height(16).Margin(0, 4).Background(hairline(c.Theme()))
		muted := c.Theme().TextMuted
		if iconButton(c, icoRotate, "Refresh", muted).Disabled(d.busy).Clicked() {
			d.nodeReleasesTried, d.nodeReleasesRefresh = false, true
			a.devTask(func(m *devplatform.Manager) (string, error) { return "Workspace refreshed.", nil })
		}
		if action.ActionIcon(c, icoHistory, action.ActionIconProps{Variant: action.ActionIconBorderless, Size: theme.Small, Title: "Tasks", Active: d.operationsOpen}).Disabled(d.busy).Clicked() {
			a.openOperations()
		}
		if action.ActionIcon(c, icoScroll, action.ActionIconProps{Variant: action.ActionIconBorderless, Size: theme.Small, Title: "Logs", Active: a.devLog.open}).Clicked() {
			a.toggleDevLogs()
		}
		if action.ActionIcon(c, icoStetho, action.ActionIconProps{Variant: action.ActionIconBorderless, Size: theme.Small, Title: "Diagnostics", Active: d.diagnosticsOpen}).Disabled(d.busy).Clicked() {
			d.operationsOpen = false
			a.runDevDiagnostics()
		}
	})
}

// devStatusBar says what the last operation did, along the bottom.
func (a *app) devStatusBar(c *ui.Context) {
	d := &a.dev
	tk := theme.TokensOf(c)
	ui.Column(c).Height(1).Shrink(0).Background(hairline(c.Theme()))
	ui.Row(c).Height(36).Shrink(0).Padding(0, 12, 0, 20).Gap(8).AlignItems(ui.Center).Children(func() {
		// A dot, an optional word for what needs a look, and the result line.
		t := c.Theme()
		status, dot := "", t.Success
		if a.backgroundTasksRunning() {
			status, dot = "Background tasks", t.Warning
		}
		if d.busy {
			status, dot = "Working", t.Warning
		}
		if strings.Contains(d.output, "Error:") {
			status, dot = "Needs attention", t.Danger
		}
		ui.Box(c).Size(8, 8).Radius(4).Shrink(0).Background(dot)
		if status != "" {
			ui.Text(c, status).FontSize(textSmall).FontWeight(500).TextColor(tk.TextHeading).SingleLine().Shrink(0)
		}
		ui.Text(c, resultLine(d.output)).FontSize(textSmall).TextColor(tk.TextDescription).SingleLine().Grow(1).MinWidth(0)
		if action.Button(c, "Live logs", action.ButtonProps{Variant: action.Text, Size: theme.Small}).Clicked() {
			a.toggleDevLogs()
		}
		if action.Button(c, "Details", action.ButtonProps{Variant: action.Text, Size: theme.Small}).Clicked() {
			d.showResult = !d.showResult
		}
	})
	if d.showResult {
		ui.Row(c).Shrink(0).Padding(0, 20, 12).Children(func() {
			ui.TextArea(c, &d.output).Lines(3, 8).ReadOnly(true).Label("Operation result").Grow(1).MinWidth(0)
		})
	}
}

// devDivider heads a group of controls inside a pane: a small title over a
// line that runs to the end of the row.
func devDivider(c *ui.Context, title string) {
	tk := theme.TokensOf(c)
	ui.Row(c).Gap(10).AlignItems(ui.Center).Margin(8, 0, 0).Children(func() {
		ui.Text(c, title).FontSize(textSmall).FontWeight(500).TextColor(tk.TextSecondary).SingleLine().Shrink(0)
		ui.Box(c).Height(1).Grow(1).Background(hairline(c.Theme()))
	})
}

// devPaneHeader titles a pane that replaces a detail, with the buttons that
// leave or refresh it at the end of the same row.
func devPaneHeader(c *ui.Context, title string, buttons func()) {
	ui.Row(c).Wrap().Gap(8).AlignItems(ui.Center).Margin(0, 0, 6).Children(func() {
		ui.Text(c, title).FontSize(20).FontWeight(600).TextColor(theme.TokensOf(c).TextHeading).Grow(1).MinWidth(220)
		buttons()
	})
}

// devStatusDot is the dot of a status in a list: green runs, red failed,
// orange needs a look, and a ring is at rest.
func devStatusDot(c *ui.Context, status string) {
	t := c.Theme()
	const size = 8
	dot := ui.Box(c).Size(size, size).Radius(size / 2).Shrink(0).Tooltip(status)
	switch status {
	case "Healthy", "Running":
		dot.Background(t.Success).Shadow(0, 0, 0, 2.5, t.Success.Alpha(0.18))
	case "Succeeded":
		dot.Background(t.Success)
	case "Failed":
		dot.Background(t.Danger)
	case "Not ready", "Partly running", "Restarting", "Unknown", "Interrupted":
		dot.Background(t.Warning)
	default:
		dot.Border(1.5, t.TextMuted.Alpha(0.65))
	}
}

// devListEntry is a row of a list of the workspace: a dot for its status when
// it has one, its name, and a word at its end only when the word is news.
func devListEntry(c *ui.Context, label, title, status, trailing string, selected bool) bool {
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
			if status != "" {
				devStatusDot(c, status)
			}
			ui.Text(c, title).FontWeight(map[bool]int{true: 500, false: 400}[selected]).SingleLine().Grow(1).MinWidth(0)
			if trailing != "" {
				ui.Text(c, trailing).FontSize(textCaption).TextColor(tk.TextTertiary).SingleLine().Shrink(0)
			}
		})
	})
	return b.Clicked()
}

// devNews is the status worth saying in words: what is not plainly running or
// at rest.
func devNews(status string) string {
	switch status {
	case "", "Healthy", "Running", "Stopped", "Configured":
		return ""
	}
	return status
}

func (a *app) devMaster(c *ui.Context) {
	d := &a.dev
	switch d.tab {
	case devTabSoftware:
		groups := []struct {
			title string
			names []string
		}{{"Runtimes", []string{"node", "java", "python"}}, {"Database engines", []string{"mysql", "postgresql", "redis", "mongodb"}}}
		for _, g := range groups {
			masterGroup(c, g.title, nil)
			for _, name := range g.names {
				trailing := d.softwareDefault(name)
				if n := d.softwareCount(name); trailing == "" && n == 1 && len(d.packages(name)) == 1 {
					trailing = d.state.Packages[d.packages(name)[0]].Version
				} else if trailing == "" && n > 0 {
					trailing = strconv.Itoa(n)
				}
				icon, tint := softwareMeta(name)
				if masterItem(c, "Software "+name, icon, tint, softwareTitle(name), trailing, "", d.software == name) {
					d.software = name
					d.version = a.defaultVersion(name)
				}
			}
		}
	case devTabDatabases:
		recovering := d.databaseWorkOpen || d.initializationsOpen || d.deletionsOpen
		masterGroup(c, "Instances", func() {
			if action.ActionIcon(c, icoPlus, action.ActionIconProps{Variant: action.ActionIconBorderless, Size: theme.Small, Title: "New database instance", Active: d.dbNewDraft && !recovering}).Disabled(d.busy).Clicked() {
				a.newDatabaseDraft()
			}
		})
		if len(d.state.Instances) == 0 {
			ui.Text(c, "No instances").FontSize(textSmall).TextColor(theme.TokensOf(c).TextTertiary).Padding(6, 10)
		}
		for _, id := range sortedKeys(d.state.Instances) {
			i := d.state.Instances[id]
			status := d.statuses["instance:"+id]
			icon, tint := softwareMeta(i.Kind)
			if status == "" {
				status = "Stopped"
			}
			if masterItem(c, "Instance "+id, icon, tint, id, ":"+strconv.Itoa(i.Port), status, d.dbID == id && !d.dbNewDraft && !recovering && d.instanceDeletionPreview == nil) {
				if id != d.dbID && a.connectionProfileDirty() {
					d.profilePendingInstance = id
					continue
				}
				d.dbNewDraft = false
				d.databaseWorkOpen = false
				d.initializationsOpen = false
				d.deletionsOpen = false
				d.instanceDeletionPreview = nil
				d.deleteConfirmation = ""
				d.dbID = id
				d.dbKind = i.Kind
				d.dbPackage = i.Package
				d.dbPort = strconv.Itoa(i.Port)
				d.persistent = i.Persistent
				a.loadConnectionProfileDraft(id, "")
			}
		}
		// The reviews of work an exited caller left concern every instance.
		masterGroup(c, "Recovery", nil)
		tint := theme.TokensOf(c).TextSecondary
		if masterItem(c, "Unfinished database tasks", icoHistory, tint, "Unfinished tasks", "", "", d.databaseWorkOpen) && !d.busy {
			a.openDatabaseWork()
		}
		if masterItem(c, "Unfinished initializations", icoRotate, tint, "Initializations", "", "", d.initializationsOpen) && !d.busy {
			a.openInitializations()
		}
		if masterItem(c, "Review pending data deletions", icoTrash, tint, "Pending deletions", "", "", d.deletionsOpen) && !d.busy {
			a.openInstanceDeletions()
		}
	}
}

func (d *developmentState) loadProxyDraft(s devplatform.State) {
	d.proxyDraft = make([]proxyRuleDraft, len(s.ProxyRules))
	for i, rule := range s.ProxyRules {
		mode := tunnelModeWorkersDev
		if rule.TunnelDomain != "" {
			mode = tunnelModeCustomDomain
		}
		d.proxyDraft[i] = proxyRuleDraft{
			Domain:       strings.TrimSuffix(rule.Domain, devplatform.LocalDomainSuffix),
			LocalPort:    strconv.Itoa(rule.LocalPort),
			UpstreamHost: rule.UpstreamHost,
			Tunnel:       rule.Tunnel,
			TunnelAuth:   rule.TunnelAuth,
			TunnelWorker: rule.TunnelWorker,
			TunnelMode:   mode,
			TunnelDomain: rule.TunnelDomain,
			TunnelURL:    rule.TunnelURL,
		}
	}
	port := s.ProxyListenerPort
	if port == 0 {
		port = 443
	}
	d.proxyListenerPort = strconv.Itoa(port)
	d.proxyDraftRevision = s.ProxyRulesRevision
	d.proxyDraftLoaded = true
}
func (d *developmentState) proxyDraftDirty() bool {
	if !d.proxyDraftLoaded {
		return false
	}
	if d.proxyListenerPort != strconv.Itoa(d.state.ProxyListenerPort) || len(d.proxyDraft) != len(d.state.ProxyRules) {
		return true
	}
	for i, row := range d.proxyDraft {
		rule := d.state.ProxyRules[i]
		if devplatform.LocalDomain(row.Domain) != rule.Domain ||
			row.LocalPort != strconv.Itoa(rule.LocalPort) ||
			strings.TrimSpace(row.UpstreamHost) != rule.UpstreamHost ||
			row.Tunnel != rule.Tunnel ||
			row.TunnelAuth != rule.TunnelAuth ||
			row.TunnelWorker != rule.TunnelWorker ||
			effectiveDraftTunnelDomain(row) != rule.TunnelDomain {
			return true
		}
	}
	return false
}
func (d *developmentState) loadTunnelDraft(s devplatform.State) {
	d.cloudflareAccountID = s.CloudflareAccountID
	d.tunnelClientID = s.TunnelClientID
	d.cloudflareTokenDraft = ""
	d.tunnelDraftLoaded = true
}
func (d *developmentState) tunnelDraftDirty() bool {
	if !d.tunnelDraftLoaded {
		return false
	}
	return d.cloudflareAccountID != d.state.CloudflareAccountID ||
		d.tunnelClientID != d.state.TunnelClientID ||
		d.cloudflareTokenDraft != ""
}

func (a *app) devManager() *devplatform.Manager {
	if a.dev.managerFactory != nil {
		return a.dev.managerFactory()
	}
	return devplatform.New(devplatform.DefaultRoot())
}

// resultLine is the line of an operation result that the status bar shows:
// the error when there is one, else the first line. The rest is under Details.
func resultLine(s string) string {
	s = strings.TrimSpace(s)
	lines := strings.Split(s, "\n")
	line := lines[0]
	for _, l := range lines {
		if strings.HasPrefix(l, "Error:") {
			line = l
			break
		}
	}
	if len(lines) > 1 && line == lines[0] {
		line += " …"
	}
	return line
}

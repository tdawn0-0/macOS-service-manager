package main

import (
	"context"
	"errors"
	"fmt"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/egoist/mygo"
	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/action"
	"github.com/lobehub/lobe-mygo/pkg/choice"
	"github.com/lobehub/lobe-mygo/pkg/display"
	"github.com/lobehub/lobe-mygo/pkg/feedback"
	"github.com/lobehub/lobe-mygo/pkg/theme"
	"devbase/internal/devplatform"
)

const (
	devTabSoftware = iota
	devTabDatabases
	devTabHTTPS
)

type proxyRuleDraft struct {
	Domain    string
	LocalPort string
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
	softwareContext                                                                                     string
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
	dbKind, dbID, dbPackage, dbPort, dbName, dbUser, dbPassword, backupPath                             string
	profileDraft                                                                                        devplatform.ConnectionProfile
	profileSelected, profileDraftInstance, profilePendingInstance, profilePendingSelect, profileRedisDB string
	profileOffer                                                                                        *connectionOperationOffer
	persistent                                                                                          bool
	proxyDraft                                                                                          []proxyRuleDraft
	proxyListenerPort                                                                                   string
	proxyDraftRevision                                                                                  uint64
	proxyDraftLoaded                                                                                    bool
	proxyChecks                                                                                         map[string]devplatform.ProxyHTTPResult
	proxyChecking                                                                                       map[string]bool
	proxyCheckGeneration                                                                                uint64
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
		a.post(func() {
			a.dev.busy = false
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
			for id := range a.dev.packageChecks {
				if _, ok := s.Packages[id]; !ok {
					delete(a.dev.packageChecks, id)
				}
			}
			if a.dev.packagePreview != nil {
				installed, ok := s.Packages[a.dev.packagePreview.Package.ID()]
				a.dev.packagePreview.Installed = ok && installed.SHA256 == a.dev.packagePreview.Package.SHA256
			}
			a.dev.statuses = snapshot.States
			a.applyDatabaseTaskPhases()
			a.dev.statusLast = time.Now()
			if e != nil {
				out += "\nError: " + e.Error()
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
	tk := theme.TokensOf(c)
	ui.Column(c).Grow(1).MinWidth(0).Background(tk.BgLayout).Children(func() {
		a.devTopBar(c)
		ui.Column(c).Height(1).Shrink(0).Background(hairline(c.Theme()))
		switch {
		case d.operationsOpen:
			a.devOperations(c)
		case d.diagnosticsOpen:
			ui.Scroll(c.Key("development-diagnostics")).TrackScroll(&d.diagnosticScroll).Grow(1).MinHeight(0).Padding(22, 28, 28).Children(func() {
				ui.Column(c).Gap(12).MaxWidth(880).Children(func() { a.devDiagnostics(c) })
			})
		default:
			ui.Row(c).Grow(1).MinHeight(0).AlignItems(ui.Stretch).Children(func() {
				// HTTPS is one table: it has no list beside it.
				if d.tab != devTabHTTPS {
					ui.Column(c).Width(228).Shrink(0).Background(tk.BgContainer).Children(func() {
						ui.Scroll(c.Key("dev-master-"+strconv.Itoa(d.tab))).Grow(1).MinHeight(0).Padding(4, 8, 10).Children(func() {
							ui.Column(c).Gap(2).Children(func() { a.devMaster(c) })
						})
					})
					ui.Column(c).Width(1).Background(hairline(c.Theme()))
				}
				ui.Scroll(c.Key(fmt.Sprintf("dev-detail-%d", d.tab))).TrackScroll(&d.detailScroll[d.tab]).Grow(1).MinWidth(0).MinHeight(0).Padding(22, 28, 28).Children(func() {
					ui.Column(c).Gap(14).MaxWidth(880).Disabled(d.busy).Children(func() {
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
	tk := theme.TokensOf(c)
	title, meta := a.devTitle()
	ui.Row(c).Height(barHeight).Shrink(0).Padding(0, 12, 0, 20).AlignItems(ui.Center).Gap(8).Background(tk.BgContainer).DragWindow().Children(func() {
		ui.Text(c, title).FontSize(15).FontWeight(650).SingleLine().Shrink(0)
		if meta != "" {
			ui.Text(c, meta).FontSize(textSmall).TextColor(tk.TextTertiary).SingleLine().MinWidth(0)
		}
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
		ui.Box(c).Width(1).Height(16).Margin(0, 4).Background(tk.BorderSecondary)
		muted := c.Theme().TextMuted
		if iconButton(c, icoRotate, "Refresh", muted).Disabled(d.busy).Clicked() {
			d.nodeReleasesTried, d.nodeReleasesRefresh = false, true
			a.devTask(func(m *devplatform.Manager) (string, error) { return "Workspace refreshed.", nil })
		}
		if action.ActionIcon(c, icoHistory, action.ActionIconProps{Variant: action.ActionIconBorderless, Size: theme.Small, Title: "Tasks", Active: d.operationsOpen}).Disabled(d.busy).Clicked() {
			a.openOperations()
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
	ui.Row(c).Height(36).Shrink(0).Padding(0, 10, 0, 18).Gap(10).AlignItems(ui.Center).Background(tk.BgContainer).Children(func() {
		status, kind := "", feedback.BadgeDefault
		if a.backgroundTasksRunning() {
			status, kind = "Background tasks", feedback.BadgeProcessing
		}
		if d.busy {
			status, kind = "Working", feedback.BadgeProcessing
		}
		if strings.Contains(d.output, "Error:") {
			status, kind = "Needs attention", feedback.BadgeError
		}
		if status != "" {
			feedback.Badge(c, feedback.BadgeProps{Status: kind, Text: status}, nil).Shrink(0)
		} else {
			devStatusDot(c, "Succeeded")
		}
		ui.Text(c, strings.ReplaceAll(d.output, "\n", " · ")).FontSize(textSmall).TextColor(tk.TextDescription).SingleLine().Grow(1).MinWidth(0)
		if action.Button(c, "Details", action.ButtonProps{Variant: action.Text, Size: theme.Small}).Clicked() {
			d.showResult = !d.showResult
		}
	})
	if d.showResult {
		ui.Row(c).Shrink(0).Padding(0, 18, 12).Background(tk.BgContainer).Children(func() {
			ui.TextArea(c, &d.output).Lines(3, 5).ReadOnly(true).Label("Operation result").Grow(1).MinWidth(0)
		})
	}
}

// devHeading titles a detail. meta is data about the subject, never prose.
func devHeading(c *ui.Context, title, meta string) {
	pageHeader(c, nil, ui.Color{}, title, meta, nil, nil)
}

// devDivider heads a group of controls inside a pane: a small title over a
// line that runs to the end of the row.
func devDivider(c *ui.Context, title string) {
	tk := theme.TokensOf(c)
	ui.Row(c).Gap(10).AlignItems(ui.Center).Margin(8, 0, 0).Children(func() {
		ui.Text(c, title).FontSize(textSmall).FontWeight(600).TextColor(tk.TextSecondary).SingleLine().Shrink(0)
		ui.Box(c).Height(1).Grow(1).Background(tk.BorderSecondary)
	})
}

// devPaneHeader titles a pane that replaces a detail, with the buttons that
// leave or refresh it at the end of the same row.
func devPaneHeader(c *ui.Context, title string, buttons func()) {
	ui.Row(c).Wrap().Gap(8).AlignItems(ui.Center).Margin(0, 0, 6).Children(func() {
		ui.Text(c, title).FontSize(20).FontWeight(650).TextColor(theme.TokensOf(c).TextHeading).Grow(1).MinWidth(220)
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
	b := ui.ButtonBase(c).Label(label).Height(34).Padding(0, 10).Radius(tk.BorderRadius).AlignItems(ui.Center).Cursor(ui.CursorPointer)
	switch {
	case selected:
		b.Background(tk.FillSecondary)
	case b.Hovered():
		b.Background(tk.FillTertiary)
	}
	b.Children(func() {
		ui.Row(c).Grow(1).MinWidth(0).AlignItems(ui.Center).Gap(9).Children(func() {
			if status != "" {
				devStatusDot(c, status)
			}
			ui.Text(c, title).FontWeight(map[bool]int{true: 600, false: 400}[selected]).SingleLine().Grow(1).MinWidth(0)
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
func softwareTitle(name string) string {
	if title, ok := map[string]string{"node": "Node.js", "java": "Java", "python": "Python (uv)", "mysql": "MySQL", "postgresql": "PostgreSQL", "redis": "Redis", "mongodb": "MongoDB"}[name]; ok {
		return title
	}
	return name
}

// softwareDefault is the version a piece of software runs by default, when
// one is chosen.
func (d *developmentState) softwareDefault(name string) string {
	id := ""
	switch name {
	case "node":
		id = d.state.NodeDefault
	case "java":
		id = d.state.RuntimeDefaults[name]
	}
	if p, ok := d.state.Packages[id]; ok {
		return p.Version
	}
	return ""
}

// softwareCount is how many versions of a piece of software are installed.
func (d *developmentState) softwareCount(name string) int {
	switch {
	case name == "python":
		return len(d.uvInstalled)
	case name == "java" && d.javaLoaded:
		return len(d.javaInstallations)
	}
	return len(d.packages(name))
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
					d.softwareContext = ""
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

// newDatabaseDraft opens the form of a new instance, unless a connection
// draft would be lost.
func (a *app) newDatabaseDraft() {
	d := &a.dev
	if a.connectionProfileDirty() {
		d.profilePendingInstance = "new"
		return
	}
	d.dbNewDraft = true
	d.databaseWorkOpen = false
	d.initializationsOpen = false
	d.deletionsOpen = false
	d.instanceDeletionPreview = nil
	d.deleteConfirmation = ""
	d.dbID = ""
	d.dbKind = "postgresql"
	d.dbPort = "55432"
	d.dbPackage = ""
	d.dbDataDirectory = ""
	d.persistent = true
}

func isDatabaseEngine(name string) bool {
	switch name {
	case "mysql", "postgresql", "redis", "mongodb":
		return true
	}
	return false
}

// openNewInstance goes to the form of a new instance of an engine.
func (a *app) openNewInstance(engine string) {
	d := &a.dev
	a.nav = navDevelopment
	d.tab = devTabDatabases
	d.detailScroll[devTabDatabases].Y = 0
	a.newDatabaseDraft()
	if !d.dbNewDraft {
		return
	}
	d.dbKind = engine
	d.dbPort = map[string]string{"mysql": "3306", "postgresql": "5432", "redis": "6379", "mongodb": "27017"}[engine]
	if p := d.packages(engine); len(p) > 0 {
		d.dbPackage = p[0]
	}
}

// openSoftware goes to a piece of software's page.
func (a *app) openSoftware(name string) {
	d := &a.dev
	a.nav = navDevelopment
	d.tab = devTabSoftware
	d.detailScroll[devTabSoftware].Y = 0
	d.software = name
	d.softwareContext = ""
	d.version = a.defaultVersion(name)
}

func (d *developmentState) loadProxyDraft(s devplatform.State) {
	d.proxyDraft = make([]proxyRuleDraft, len(s.ProxyRules))
	for i, rule := range s.ProxyRules {
		d.proxyDraft[i] = proxyRuleDraft{Domain: rule.Domain, LocalPort: strconv.Itoa(rule.LocalPort)}
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
		if row.Domain != d.state.ProxyRules[i].Domain || row.LocalPort != strconv.Itoa(d.state.ProxyRules[i].LocalPort) {
			return true
		}
	}
	return false
}

func (a *app) devSoftware(c *ui.Context) {
	d := &a.dev
	if d.software == "" {
		d.software = "node"
		d.version = devplatform.AvailableVersions("node")[0]
	}
	icon, tint := softwareMeta(d.software)
	meta := "Not installed"
	if n := d.softwareCount(d.software); n > 0 {
		meta = plural(n, "version") + " installed"
	}
	if v := d.softwareDefault(d.software); v != "" {
		meta += " · default " + v
	}
	if d.software == "python" && d.uvPin != "" {
		meta += " · " + d.uvPin
	}
	var actions func()
	if engine := d.software; isDatabaseEngine(engine) && len(d.packages(engine)) > 0 {
		// An installed engine is a step towards an instance of it.
		actions = func() {
			if btn(c, "New instance", icoPlus, btnDefault).Disabled(d.busy).Clicked() {
				a.openNewInstance(engine)
			}
		}
	}
	pageHeader(c, icon, tint, softwareTitle(d.software), meta, nil, actions)
	if d.software == "python" {
		a.devPython(c)
		return
	}
	a.devPackageTasks(c)
	if d.software == "java" {
		a.devJavaInstallations(c)
		devCardWith(c, icoDownload, "Install a JDK", "", nil, func() {
			a.devRuntimeCatalog(c)
			a.devPackageReview(c)
		})
		devCardWith(c, icoTerminal, "Terminal", "", nil, func() { a.devAdditionalRuntimeShell(c) })
		return
	}
	if ids := d.packages(d.software); len(ids) > 0 {
		devCardWith(c, iconPackage, "Installed", "", nil, func() {
			for _, id := range ids {
				a.devInstalledPackage(c, id, d.state.Packages[id])
			}
		})
	}
	devCardWith(c, icoDownload, "Install a version", "", nil, func() {
		if d.software == "node" {
			a.loadNodeReleases()
			devFieldGrid(c, func() {
				devNodeChoice(c, &d.version, d.nodeReleases)
				devField(c, "Exact version", &d.version, "22.14.0, 22, lts or latest")
			})
			if d.nodeReleasesError != "" {
				devHint(c, feedback.Warning, d.nodeReleasesError, "")
			} else if d.nodeReleasesStale {
				devHint(c, feedback.Warning, "nodejs.org could not be reached. Showing the last release list.", "")
			}
		} else {
			devFieldGrid(c, func() {
				devChoice(c, "Version", &d.version, devplatform.AvailableVersions(d.software))
				devField(c, "Exact version", &d.version, "For example 22.14.0")
			})
		}
		a.devPackageReview(c)
	})
	if d.software == "node" {
		devCardWith(c, icoTerminal, "Terminal", "", nil, func() { a.devNodeTerminal(c) })
	}
}

// devInstalledPackage is an installed version in the list of its software:
// its name and version, whether it is the default, and what can be done
// with it, with the checks of its files under them.
func (a *app) devInstalledPackage(c *ui.Context, id string, p devplatform.Package) {
	d := &a.dev
	devInset(c, func() {
		ui.Row(c).Wrap().Gap(8).AlignItems(ui.Center).Children(func() {
			ui.Row(c).Gap(8).AlignItems(ui.Center).Grow(1).MinWidth(160).Children(func() {
				ui.Text(c, softwareTitle(p.Name)+" · "+p.Version).FontWeight(600).SingleLine().MinWidth(100)
				if p.Formula != "" {
					devTag(c, "Homebrew", display.TagColor(""))
				}
				if p.Name == "node" && id == d.state.NodeDefault {
					display.Tag(c, "Default", display.TagProps{Color: display.TagGreen, Size: theme.Small})
				}
				if p.Name == "java" && d.state.RuntimeDefaults[p.Name] == id {
					display.Tag(c, "Default", display.TagProps{Color: display.TagGreen, Size: theme.Small})
				}
			})
			ui.Row(c).Gap(6).AlignItems(ui.Center).Shrink(0).Children(func() {
				if p.Name == "node" && id != d.state.NodeDefault {
					if btn(c, "Set default", nil, btnDefault).Disabled(d.busy).Clicked() {
						a.devTask(func(m *devplatform.Manager) (string, error) {
							return "Default is Node.js " + p.Version, m.SetNodeDefault(m.Context, id)
						})
					}
				}
				if p.Name == "node" {
					a.devNodeExecutable(c, id)
				}
				if p.Name == "java" {
					a.devAdditionalRuntimeInstalled(c, id, p)
				}
			})
		})
		devSubtle(c, "License: "+p.License)
		a.devPackageIntegrity(c, id, p)
		removeText := "Uninstall"
		if p.Formula != "" {
			removeText = "Remove from workspace"
		}
		if btn(c, removeText, icoTrash, btnDanger).AlignSelf(ui.Start).Disabled(d.busy || a.packageTaskRunning(id)).Clicked() {
			a.devTask(func(m *devplatform.Manager) (string, error) {
				if p.Formula != "" {
					return "Removed workspace registration. The shared Homebrew installation is kept.", m.Uninstall(m.Context, id)
				}
				return "Uninstalled " + p.Name, m.Uninstall(m.Context, id)
			})
		}
	})
}

// devManager is the manager the window's tasks would use.
// defaultVersion is the version offered first: for Node.js the newest
// long-term support release once nodejs.org's list is read.
func (a *app) defaultVersion(name string) string {
	if name == "node" {
		for _, r := range a.dev.nodeReleases {
			if r.LTS != "" {
				return r.Version
			}
		}
	}
	if versions := devplatform.AvailableVersions(name); len(versions) > 0 {
		return versions[0]
	}
	return ""
}

func (a *app) devManager() *devplatform.Manager {
	if a.dev.managerFactory != nil {
		return a.dev.managerFactory()
	}
	return devplatform.New(devplatform.DefaultRoot())
}

// loadNodeReleases reads nodejs.org's release list in the background, once,
// and again when the workspace is refreshed.
func (a *app) loadNodeReleases() {
	d := &a.dev
	if d.nodeReleasesTried || a.post == nil {
		return
	}
	d.nodeReleasesTried = true
	refresh := d.nodeReleasesRefresh
	d.nodeReleasesRefresh = false
	m := a.devManager()
	a.spawn(func() {
		ctx, cancel := context.WithTimeout(context.Background(), 45*time.Second)
		defer cancel()
		releases, stale, e := m.NodeReleases(ctx, refresh)
		a.post(func() {
			a.dev.nodeReleases, a.dev.nodeReleasesStale, a.dev.nodeReleasesError = releases, stale, ""
			if e != nil {
				a.dev.nodeReleasesError = "Releases could not be read from nodejs.org: " + e.Error()
			}
		})
	})
}

// devNodeChoice picks among the newest release of each major version, with
// the long-term support lines named. Any other version goes in the field
// below it.
func devNodeChoice(c *ui.Context, version *string, releases []devplatform.NodeRelease) {
	if len(releases) == 0 {
		devChoice(c, "Version", version, devplatform.AvailableVersions("node"))
		return
	}
	var options []string
	lookup := map[string]string{}
	display := ""
	seen := map[string]bool{}
	for _, r := range releases {
		major := strings.SplitN(r.Version, ".", 2)[0]
		if seen[major] {
			continue
		}
		seen[major] = true
		label := r.Version
		if r.LTS != "" {
			label += " · LTS " + r.LTS
		}
		options = append(options, label)
		lookup[label] = r.Version
		if r.Version == *version {
			display = label
		}
	}
	if devChoice(c, "Version", &display, options) {
		*version = lookup[display]
	}
}
func (a *app) devDatabases(c *ui.Context) {
	d := &a.dev
	if d.databaseWorkOpen {
		a.devDatabaseWork(c)
		return
	}
	if d.initializationsOpen {
		a.devInitializations(c)
		return
	}
	if d.instanceDeletionPreview != nil {
		a.devInstanceDeletionPreview(c)
		return
	}
	if d.deletionsOpen {
		a.devInstanceDeletions(c)
		return
	}
	if d.dbKind == "" {
		d.dbKind = "postgresql"
		d.dbPort = "55432"
		d.persistent = true
	}
	instance, exists := d.state.Instances[d.dbID]
	if !exists || d.dbNewDraft {
		a.devDatabaseTasks(c)
		a.devNewInstance(c)
		return
	}
	busy := d.busy || a.databaseTaskRunning(instance.ID)
	status := d.statuses["instance:"+instance.ID]
	icon, tint := softwareMeta(instance.Kind)
	meta := softwareTitle(instance.Kind)
	if p, ok := d.state.Packages[instance.Package]; ok {
		meta += " " + p.Version
	}
	meta += " · 127.0.0.1:" + strconv.Itoa(instance.Port)
	pageHeader(c, icon, tint, instance.ID, meta, func() {
		if status == "" {
			status = "Stopped"
		}
		statusTag(c, status)
	}, func() {
		for _, act := range []struct {
			label, text string
			icon        *ui.SVG
		}{{"Start instance", "Start", icoPlay}, {"Restart instance", "Restart", icoRotate}, {"Stop instance", "Stop", icoStop}} {
			kind := btnDefault
			if act.text == "Start" && status != "Running" && status != "Healthy" {
				kind = btnPrimary
			}
			if btn(c, act.text, act.icon, kind).Disabled(busy).Clicked() {
				id := instance.ID
				verb := strings.TrimSuffix(strings.ToLower(act.label), " instance")
				a.startDatabaseOperation(instance, verb, func(m *devplatform.Manager) (string, error) {
					result := map[string]string{"start": "Started ", "stop": "Stopped ", "restart": "Restarted "}[verb] + id
					return result, m.InstanceAction(m.Context, id, verb)
				})
			}
		}
		if iconButton(c, icoScroll, "Read instance logs", c.Theme().TextMuted).Disabled(d.busy).Clicked() {
			id := "db-" + instance.ID
			d.showResult = true
			a.devTask(func(m *devplatform.Manager) (string, error) { return m.Logs(id) })
		}
	})
	a.devDatabaseTasks(c)
	a.devDatabaseConnections(c, instance)
	devCardWith(c, icoPlug, "Endpoint & storage", "", nil, func() {
		devKV(c, "Version", d.state.Packages[instance.Package].Version)
		devKV(c, "Data", a.devManager().DatabaseDataDirectory(instance))
		a.devDatabasePort(c, instance)
	})
	title := "Databases & users"
	if d.dbKind == "redis" {
		title = "Database"
	}
	devCardWith(c, icoDatabase, title, "", nil, func() {
		if d.dbKind == "redis" {
			devField(c, "Database number (0–15)", &d.dbName, "0")
			return
		}
		devFieldGrid(c, func() {
			devField(c, "Database name", &d.dbName, "app")
		})
		if btn(c, "Create database", icoPlus, btnDefault).AlignSelf(ui.Start).Disabled(busy).Clicked() {
			id, name := d.dbID, d.dbName
			a.startDatabaseOperationWithOffer(d.state.Instances[id], "create database", func(m *devplatform.Manager) (string, error) {
				return "Created database " + name, m.CreateDatabase(m.Context, id, name)
			}, &connectionOperationOffer{InstanceID: id, Kind: d.dbKind, Database: name})
		}
		devDivider(c, "User")
		devFieldGrid(c, func() {
			devField(c, "Database user", &d.dbUser, "app_user")
			ui.Field(c, "New user password", func() { ui.TextInput(c, &d.dbPassword).Password() })
		})
		if btn(c, "Create database user", icoUser, btnDefault).AlignSelf(ui.Start).Disabled(busy).Clicked() {
			id, name, password, db := d.dbID, d.dbUser, d.dbPassword, d.dbName
			d.dbPassword = ""
			a.startDatabaseOperation(d.state.Instances[id], "create user", func(m *devplatform.Manager) (string, error) {
				return "Created user " + name, m.CreateUser(m.Context, id, name, password, db)
			})
		}
	})
	devCardWith(c, icoArchive, "Backups", "", nil, func() {
		if btn(c, "Back up database", icoArchive, btnDefault).AlignSelf(ui.Start).Disabled(busy).Clicked() {
			id, name := d.dbID, d.dbName
			a.startDatabaseOperation(d.state.Instances[id], "backup", func(m *devplatform.Manager) (string, error) { return m.Backup(m.Context, id, name) })
		}
		devDivider(c, "Restore")
		devField(c, "Backup file", &d.backupPath, "Select a backup file")
		devButtonRow(c, func() {
			if btn(c, "Choose backup file…", icoFolder, btnDefault).Clicked() {
				paths, e := mygo.Dialog.Open(mygo.OpenDialogOptions{Title: "Restore database backup"})
				if e != nil {
					d.output = e.Error()
				} else if len(paths) > 0 {
					d.backupPath = paths[0]
				}
			}
			if btn(c, "Restore into new database", icoRestore, btnDefault).Disabled(busy).Clicked() {
				id, name, path := d.dbID, d.dbName, d.backupPath
				a.startDatabaseOperation(d.state.Instances[id], "restore", func(m *devplatform.Manager) (string, error) {
					return "Restored into " + name, m.Restore(m.Context, id, name, path)
				})
			}
		})
	})
	devDangerCard(c, "Remove", func() {
		devButtonRow(c, func() {
			if btn(c, "Remove instance (keep data files)", icoTrash, btnDanger).Disabled(busy).Clicked() {
				id := d.dbID
				a.startDatabaseOperation(d.state.Instances[id], "remove", func(m *devplatform.Manager) (string, error) {
					return "Removed instance", m.InstanceAction(m.Context, id, "remove")
				})
			}
			if btn(c, "Review permanent data deletion", nil, btnDanger).Disabled(busy).Clicked() {
				a.previewInstanceDeletion(d.dbID)
			}
		})
	})
}

// devNewInstance is the form of a new database instance.
func (a *app) devNewInstance(c *ui.Context) {
	d := &a.dev
	d.dbNewDraft = true
	icon, tint := softwareMeta(d.dbKind)
	pageHeader(c, icon, tint, "New instance", softwareTitle(d.dbKind), nil, nil)
	packages := d.packages(d.dbKind)
	if len(packages) == 0 {
		ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
			ui.Column(c).Grow(1).MinWidth(0).Children(func() {
				devHint(c, feedback.Warning, "Install "+softwareTitle(d.dbKind)+" first", "")
			})
			kind := d.dbKind
			if btn(c, "Open Software", icoArrow, btnDefault).Clicked() {
				a.openSoftware(kind)
			}
		})
	} else if d.dbPackage == "" {
		d.dbPackage = packages[0]
	}
	devCardWith(c, icoDatabase, "Instance", "", nil, func() {
		devFieldGrid(c, func() {
			devField(c, "Instance name", &d.dbID, "app-db")
			if devChoice(c, "Database engine", &d.dbKind, []string{"mysql", "postgresql", "redis", "mongodb"}) {
				d.dbPackage = ""
				if p := d.packages(d.dbKind); len(p) > 0 {
					d.dbPackage = p[0]
				}
				d.dbPort = map[string]string{"mysql": "3306", "postgresql": "5432", "redis": "6379", "mongodb": "27017"}[d.dbKind]
			}
			devPackageChoice(c, "Database version", &d.dbPackage, d.packages(d.dbKind), d.state.Packages, false)
			devField(c, "Local port", &d.dbPort, "55432")
		})
		a.devPortControls(c, &d.dbPort, "database")
		if d.dbKind == "redis" {
			choice.Checkbox(c, &d.persistent, "Persist database contents", choice.CheckboxProps{})
		} else {
			d.persistent = true
		}
	})
	devCardWith(c, icoFolder, "Storage", "", nil, func() {
		devField(c, "Data location", &d.dbDataDirectory, "Blank for app storage, or a new absolute directory")
		devSubtle(c, "External locations keep data when the instance is removed. Existing data is not adopted.")
	})
	if btn(c, "Create instance", icoPlus, btnPrimary).AlignSelf(ui.Start).Disabled(d.busy || d.databaseTasks[d.dbID] != nil && d.databaseTasks[d.dbID].Running).Clicked() {
		port, e := strconv.Atoi(d.dbPort)
		if e != nil {
			d.output = "Error: Enter a valid port"
		} else {
			i := devplatform.Instance{ID: d.dbID, Kind: d.dbKind, Package: d.dbPackage, Port: port, Persistent: d.persistent, DataDirectory: d.dbDataDirectory}
			d.dbNewDraft = true
			a.startDatabaseTask(i)
		}
	}
}

func devPackageChoice(c *ui.Context, label string, selected *string, ids []string, packages map[string]devplatform.Package, optional bool) bool {
	var options []string
	lookup := map[string]string{}
	display := ""
	if optional {
		options = append(options, "External executable (unmanaged)")
		lookup["External executable (unmanaged)"] = ""
		if *selected == "" {
			display = "External executable (unmanaged)"
		}
	}
	for _, id := range ids {
		p := packages[id]
		name := softwareTitle(p.Name) + " · " + p.Version
		options = append(options, name)
		lookup[name] = id
		if *selected == id {
			display = name
		}
	}
	if devChoice(c, label, &display, options) {
		*selected = lookup[display]
		return true
	}
	return false
}

func (d *developmentState) previewMatches() bool {
	return d.packagePreview != nil && d.packagePreview.Package.Name == d.software && d.packagePreview.Package.Version == strings.TrimPrefix(strings.TrimSpace(d.version), "v")
}
func packageDetailsText(info devplatform.PackageDetails) string {
	if info.Package.Formula != "" {
		return fmt.Sprintf("Homebrew formula: %s\nVersion: %s\nSource: %s\nFormula SHA-256: %s\nArchitecture: %s\nLicense: %s\nCellar folder: %s", info.Package.Formula, info.Package.Version, info.Package.URL, info.Package.SHA256, info.Package.Arch, info.Package.License, info.Location)
	}
	minimum := "See upstream requirements"
	if info.Package.MinMacOS != "" {
		minimum = "macOS " + info.Package.MinMacOS + " or newer"
	}
	text := fmt.Sprintf("Source: %s\nSHA-256: %s\nArchitecture: %s\nSystem: %s\nLicense: %s\nInstallation folder: %s", info.Package.URL, info.Package.SHA256, info.Package.Arch, minimum, info.Package.License, info.Location)
	if info.Package.JavaProduct != "" {
		text = "Distribution: " + info.Package.JavaProduct + "\n" + text
	}
	return text
}

// devPackageReview reviews the version chosen and installs it: the review
// comes first, and Install acts on what it showed.
func (a *app) devPackageReview(c *ui.Context) {
	d := &a.dev
	if d.softwareContext != "" {
		devSubtle(c, d.softwareContext)
	}
	valid := d.previewMatches()
	if valid {
		info := d.packagePreview
		devInset(c, func() {
			ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
				ui.Text(c, softwareTitle(info.Package.Name)+" "+info.Package.Version).FontWeight(600).SingleLine().MinWidth(0)
				if info.Installed {
					statusTag(c, "Installed")
				}
				if info.Cached {
					devTag(c, "Cached", display.TagColor(""))
				}
				ui.Spacer(c)
				if btn(c, "Source", icoCopy, btnText).Clicked() && a.copy != nil {
					a.copy(info.Package.URL)
				}
				if btn(c, "Checksum", icoCopy, btnText).Clicked() && a.copy != nil {
					a.copy(info.Package.SHA256)
				}
			})
			size := "Download size is unavailable until transfer starts"
			if info.ArchiveBytes > 0 {
				size = fmt.Sprintf("Archive size: %.1f MiB", float64(info.ArchiveBytes)/(1<<20))
			}
			if info.Cached {
				size += " · verified cache"
			}
			if info.Installed && info.ArchiveBytes == 0 {
				size = "Installed version · download size unknown"
			}
			if info.Package.Formula != "" {
				size = "Homebrew downloads and verifies the bottle and its dependencies."
			}
			devSubtle(c, size)
			ui.Text(c, info.Compatibility).FontSize(textSmall).TextColor(func() ui.Color {
				if !info.Compatible {
					return c.Theme().Danger
				}
				return c.Theme().TextMuted
			}())
			ui.Text(c, d.packageDetailsText).FontSize(textSmall).Selectable().Label("Package details")
			for _, reference := range info.References {
				devSubtle(c, reference)
			}
			if info.Installed && info.Integrity != "" {
				devSubtle(c, "Installed files: "+info.Integrity+". "+info.IntegrityNote)
			}
		})
	}
	installLabel := "Download and install"
	if valid && d.packagePreview.Package.Formula != "" {
		installLabel = "Install with Homebrew"
	}
	if valid && d.packagePreview.Installed {
		installLabel = "Already installed"
	}
	devButtonRow(c, func() {
		reviewKind := btnPrimary
		if valid {
			reviewKind = btnDefault
		}
		if btn(c, "Review package", icoFileCheck, reviewKind).Disabled(d.busy || d.software == "" || strings.TrimSpace(d.version) == "").Clicked() {
			name, version := d.software, strings.TrimSpace(d.version)
			a.devTask(func(m *devplatform.Manager) (string, error) {
				pkg, e := m.ResolveInstalledOrCatalog(m.Context, name, version)
				if e != nil {
					return "", e
				}
				info, e := m.PackageDetails(m.Context, pkg)
				if e == nil {
					a.post(func() {
						a.dev.packagePreview = &info
						a.dev.packageDetailsText = packageDetailsText(info)
						if (pkg.Formula != "" || name == "node") && a.dev.software == name && strings.TrimSpace(a.dev.version) == version {
							a.dev.version = pkg.Version
						}
					})
				}
				return "Package details ready.", e
			})
		}
		installKind := btnDefault
		if valid {
			installKind = btnPrimary
		}
		if btn(c, installLabel, icoDownload, installKind).Disabled(d.busy || !valid || d.packagePreview.Installed || !d.packagePreview.Compatible || (d.packagePreview.Integrity == "Needs repair" && d.packagePreview.Package.Formula == "") || a.packageTaskRunning(d.packagePreview.Package.ID())).Clicked() {
			pkg := d.packagePreview.Package
			a.startPackageTask(pkg, "install", func(m *devplatform.Manager) (string, error) {
				if err := m.Install(m.Context, pkg); err != nil {
					return "Install did not complete.", err
				}
				return "Installed " + softwareTitle(pkg.Name) + " " + pkg.Version, nil
			}, nil)
		}
	})
}

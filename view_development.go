package main

import (
	"context"
	"errors"
	"fmt"
	"github.com/egoist/mygo"
	"github.com/egoist/mygo/ui"
	"macos-service-manager/internal/devplatform"
	"sort"
	"strconv"
	"strings"
	"time"
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
	databaseWorkOpen                                                        bool
	databaseWork                                                            []devplatform.DatabaseWorkInfo
	databaseTasks                                                           map[string]*databaseTask
	databaseNeedsRefresh, dbNewDraft, initializationsOpen                   bool
	lastDatabaseTask                                                        *databaseTask
	initializations                                                         []devplatform.InstanceInitializationInfo
	packageTasks                                                            map[string]*packageTask
	packageNeedsRefresh                                                     bool
	lastPackageTask                                                         *packageTask
	packageContention                                                       *devplatform.PackageBusyError
	instanceDeletionPreview                                                 *devplatform.InstanceDeletionPreview
	instanceDeletions                                                       []devplatform.InstanceDeletionInfo
	deletionsOpen                                                           bool
	deleteConfirmation                                                      string
	operationsOpen                                                          bool
	operations                                                              []devplatform.OperationRecord
	operationID, operationsError                                            string
	operationScroll                                                         ui.ScrollState
	authorityOpen                                                           bool
	authorityStatus                                                         *devplatform.AuthorityStatus
	portChecks                                                              map[int]devplatform.PortInspection
	diagnosticsOpen                                                         bool
	diagnosticReport                                                        *devplatform.DiagnosticReport
	diagnosticScroll                                                        ui.ScrollState
	diagnosticJSON                                                          string
	packagePreview                                                          *devplatform.PackageDetails
	packageDetailsText                                                      string
	packageChecks                                                           map[string]devplatform.PackageIntegrityResult
	detailScroll                                                            [3]ui.ScrollState
	managerFactory                                                          func() *devplatform.Manager
	softwareContext                                                         string
	statuses                                                                map[string]string
	statusPolling                                                           bool
	statusLast                                                              time.Time
	taskSequence                                                            uint64
	loaded, busy                                                            bool
	tab                                                                     int
	state                                                                   devplatform.State
	output                                                                  string
	cancel                                                                  context.CancelFunc
	software, version                                                       string
	dbKind, dbID, dbPackage, dbPort, dbName, dbUser, dbPassword, backupPath string
	persistent                                                              bool
	proxyDraft                                                              []proxyRuleDraft
	proxyListenerPort                                                       string
	proxyDraftRevision                                                      uint64
	proxyDraftLoaded                                                        bool
	nodeReleases                                                            []devplatform.NodeRelease
	nodeReleasesTried, nodeReleasesRefresh, nodeReleasesStale               bool
	nodeReleasesError                                                       string
	nodeShell                                                               int
	showResult                                                              bool
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
	titles := devSections[:]
	if d.tab < devTabSoftware || d.tab > devTabHTTPS {
		d.tab = devTabSoftware
	}
	ui.Column(c).Grow(1).MinWidth(0).Background(c.Theme().Background).Children(func() {
		ui.Row(c).Height(barHeight).Shrink(0).Padding(0, 14, 0, 20).AlignItems(ui.Center).Gap(6).DragWindow().Children(func() {
			title := titles[d.tab]
			if d.operationsOpen {
				title = "Task history"
			}
			if d.diagnosticsOpen {
				title = "Diagnostics"
			}
			ui.Text(c, title).FontSize(15).FontWeight(650).SingleLine().Grow(1).MinWidth(0)
			if d.busy || a.backgroundTasksRunning() {
				ui.Spinner(c).Size(14, 14)
			}
			if !d.busy && len(d.databaseTasks) > 0 && (d.tab == devTabDatabases || a.databaseTasksRunning()) {
				if ui.Button(c, "Database tasks").Clicked() {
					a.showDatabaseTasks()
				}
			} else if !d.busy && len(d.packageTasks) > 0 {
				if ui.Button(c, "Package tasks").Clicked() {
					a.showPackageTasks()
				}
			}
			if d.busy {
				if ui.Button(c, "Cancel operation").Clicked() && d.cancel != nil {
					d.cancel()
				}
			}
			muted := c.Theme().TextMuted
			if iconButton(c, iconRestart, "Refresh", muted).Disabled(d.busy).Clicked() {
				d.nodeReleasesTried, d.nodeReleasesRefresh = false, true
				a.devTask(func(m *devplatform.Manager) (string, error) { return "Workspace refreshed.", nil })
			}
			if iconButton(c, iconTasks, "Tasks", muted).Disabled(d.busy).Clicked() {
				a.openOperations()
			}
			if iconButton(c, iconPulse, "Diagnostics", muted).Disabled(d.busy).Clicked() {
				d.operationsOpen = false
				a.runDevDiagnostics()
			}
		})
		ui.Column(c).Height(1).Background(hairline(c.Theme()))
		if d.operationsOpen {
			a.devOperations(c)
		} else if d.diagnosticsOpen {
			ui.Scroll(c.Key("development-diagnostics")).TrackScroll(&d.diagnosticScroll).Grow(1).MinHeight(0).Padding(20, 24).Children(func() { a.devDiagnostics(c) })
		} else {
			ui.Row(c).Grow(1).MinHeight(0).AlignItems(ui.Stretch).Children(func() {
				ui.Column(c).Width(224).Shrink(0).Background(c.Theme().Surface).Children(func() {
					ui.Scroll(c.Key("dev-master-" + strconv.Itoa(d.tab))).Grow(1).MinHeight(0).Padding(10).Children(func() { ui.Column(c).Gap(4).Children(func() { a.devMaster(c) }) })
				})
				ui.Column(c).Width(1).Background(hairline(c.Theme()))
				ui.Scroll(c.Key(fmt.Sprintf("dev-detail-%d", d.tab))).TrackScroll(&d.detailScroll[d.tab]).Grow(1).MinWidth(0).MinHeight(0).Padding(20, 24).Children(func() {
					ui.Column(c).Gap(10).Disabled(d.busy).Children(func() {
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
		ui.Column(c).Height(1).Background(hairline(c.Theme()))
		ui.Row(c).Padding(9, 18).Gap(10).AlignItems(ui.Center).Children(func() {
			status := ""
			if status == "" && a.backgroundTasksRunning() {
				status = "Background tasks"
			}
			if d.busy {
				status = "Working"
			}
			if strings.Contains(d.output, "Error:") {
				status = "Needs attention"
			}
			if status != "" {
				ui.Text(c, status).FontSize(textCaption).FontWeight(600).TextColor(func() ui.Color {
					if status == "Needs attention" {
						return c.Theme().Danger
					}

					return c.Theme().Text
				}())
			}
			ui.Text(c, strings.ReplaceAll(d.output, "\n", " · ")).FontSize(textSmall).TextColor(c.Theme().TextMuted).SingleLine().Grow(1).MinWidth(0)
			if ui.Button(c, "Details").Clicked() {
				d.showResult = !d.showResult
			}
		})
		if d.showResult {
			ui.TextArea(c, &d.output).Lines(3, 5).ReadOnly(true).Label("Operation result").Margin(0, 18, 12)
		}
	})
}

// devHeading titles a detail. meta is data about the subject, never prose.
func devHeading(c *ui.Context, title, meta string) {
	ui.Text(c, title).FontSize(18).FontWeight(650)
	if meta != "" {
		ui.Text(c, meta).FontSize(textSmall).TextColor(c.Theme().TextMuted)
	}
}
func devDivider(c *ui.Context, title string) {
	ui.Column(c).Height(1).Background(hairline(c.Theme())).Margin(12, 0, 2)
	ui.Text(c, title).FontSize(textSmall).FontWeight(650).TextColor(c.Theme().TextMuted)
}

// devPaneHeader titles a pane that replaces a detail, with the buttons that
// leave or refresh it at the end of the same row.
func devPaneHeader(c *ui.Context, title string, buttons func()) {
	ui.Row(c).Wrap().Gap(8).AlignItems(ui.Center).Children(func() {
		ui.Text(c, title).FontSize(18).FontWeight(650).Grow(1).MinWidth(0)
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
	t := c.Theme()
	b := ui.ButtonBase(c).Label(label).Height(32).Padding(0, 10).Radius(7).AlignItems(ui.Center)
	switch {
	case selected:
		b.Background(t.Accent.Alpha(.14))
	case b.Hovered():
		b.Background(t.Text.Alpha(.06))
	}
	b.Children(func() {
		ui.Row(c).Grow(1).MinWidth(0).AlignItems(ui.Center).Gap(9).Children(func() {
			if status != "" {
				devStatusDot(c, status)
			}
			ui.Text(c, title).FontWeight(map[bool]int{true: 600, false: 400}[selected]).SingleLine().Grow(1).MinWidth(0)
			if trailing != "" {
				ui.Text(c, trailing).FontSize(textCaption).TextColor(t.TextMuted).SingleLine().Shrink(0)
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
	if title, ok := map[string]string{"node": "Node.js", "mysql": "MySQL", "postgresql": "PostgreSQL", "redis": "Redis", "mongodb": "MongoDB"}[name]; ok {
		return title
	}
	return name
}
func (a *app) devMaster(c *ui.Context) {
	d := &a.dev
	switch d.tab {
	case devTabSoftware:
		for _, name := range []string{"node", "mysql", "postgresql", "redis", "mongodb"} {
			count := ""
			if n := len(d.packages(name)); n > 0 {
				count = strconv.Itoa(n)
			}
			if devListEntry(c, "Software "+name, softwareTitle(name), "", count, d.software == name) {
				d.software = name
				d.softwareContext = ""
				d.version = a.defaultVersion(name)
			}
		}
	case devTabDatabases:
		if ui.PrimaryButton(c, "New database instance").Disabled(d.busy).Clicked() {
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
			d.persistent = true
		}
		for _, id := range sortedKeys(d.state.Instances) {
			i := d.state.Instances[id]
			status := d.statuses["instance:"+id]
			if devListEntry(c, "Instance "+id, id, status, devNews(status), d.dbID == id) {
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
			}
		}
	case devTabHTTPS:
		ui.Text(c, "Proxy rules").FontSize(textSmall).TextColor(c.Theme().TextMuted).Padding(8)
		ui.Text(c, strconv.Itoa(len(d.proxyDraft))+" domains").FontSize(textCaption).Padding(8)
	}
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
	devHeading(c, softwareTitle(d.software), "")
	a.devPackageTasks(c)
	if d.software == "node" {
		a.loadNodeReleases()
		devNodeChoice(c, &d.version, d.nodeReleases)
		if d.nodeReleasesError != "" {
			ui.Text(c, d.nodeReleasesError).FontSize(textSmall).TextColor(c.Theme().Warning)
		} else if d.nodeReleasesStale {
			ui.Text(c, "nodejs.org could not be reached. Showing the last release list.").FontSize(textSmall).TextColor(c.Theme().Warning)
		}
		devField(c, "Exact version", &d.version, "22.14.0, 22, lts or latest")
	} else {
		devChoice(c, "Version", &d.version, devplatform.AvailableVersions(d.software))
		devField(c, "Exact version", &d.version, "For example 22.14.0")
	}
	a.devPackageReview(c)
	for _, id := range sortedKeys(d.state.Packages) {
		p := d.state.Packages[id]
		if p.Name != d.software {
			continue
		}
		devDivider(c, "Installed")
		ui.Text(c, "License: "+p.License).FontSize(textSmall).TextColor(c.Theme().TextMuted)
		ui.Row(c).Gap(8).Children(func() {
			ui.Text(c, softwareTitle(p.Name)+" · "+p.Version).Grow(1).MinWidth(0).SingleLine()
			ui.Spacer(c)
			if p.Name == "node" {
				if id == d.state.NodeDefault {
					ui.Text(c, "Default").FontSize(textSmall).FontWeight(600).TextColor(c.Theme().Accent).Shrink(0)
				} else if ui.Button(c, "Set default "+p.Version).Disabled(d.busy).Clicked() {
					a.devTask(func(m *devplatform.Manager) (string, error) {
						return "Default is Node.js " + p.Version, m.SetNodeDefault(m.Context, id)
					})
				}
			}
			if p.Name == "node" {
				a.devNodeExecutable(c, id)
			}
			removeLabel := "Uninstall " + p.Name + " " + p.Version
			if p.Formula != "" {
				removeLabel = "Remove from workspace " + p.Name + " " + p.Version
			}
			if ui.Button(c, removeLabel).AlignSelf(ui.Start).Disabled(d.busy || a.packageTaskRunning(id)).Clicked() {
				a.devTask(func(m *devplatform.Manager) (string, error) {
					if p.Formula != "" {
						return "Removed workspace registration. The shared Homebrew installation is kept.", m.Uninstall(m.Context, id)
					}
					return "Uninstalled " + p.Name, m.Uninstall(m.Context, id)
				})
			}
		})
		a.devPackageIntegrity(c, id, p)
	}
	if d.software == "node" {
		a.devNodeTerminal(c)
	}
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
	a.devDatabaseTasks(c)
	if d.dbKind == "" {
		d.dbKind = "postgresql"
		d.dbPort = "55432"
		d.persistent = true
	}
	if instance, exists := d.state.Instances[d.dbID]; exists && !d.dbNewDraft {
		devHeading(c, instance.ID, softwareTitle(instance.Kind)+" · 127.0.0.1:"+strconv.Itoa(instance.Port))
		ui.Row(c).Wrap().Gap(8).Children(func() {
			for _, action := range []string{"Start instance", "Restart instance", "Stop instance"} {
				if ui.Button(c, action).AlignSelf(ui.Start).Disabled(d.busy || a.databaseTaskRunning(instance.ID)).Clicked() {
					id := instance.ID
					verb := strings.TrimSuffix(strings.ToLower(action), " instance")
					a.startDatabaseOperation(instance, verb, func(m *devplatform.Manager) (string, error) {
						result := map[string]string{"start": "Started ", "stop": "Stopped ", "restart": "Restarted "}[verb] + id
						return result, m.InstanceAction(m.Context, id, verb)
					})
				}
			}
		})
		ui.Row(c).Wrap().Gap(8).Children(func() {
			if ui.Button(c, "Show connection details").AlignSelf(ui.Start).Disabled(d.busy).Clicked() {
				id := d.dbID
				a.devTask(func(m *devplatform.Manager) (string, error) { return m.Connection(id) })
			}
			if ui.Button(c, "Copy connection URI").AlignSelf(ui.Start).Disabled(d.busy).Clicked() {
				id := d.dbID
				a.devTask(func(m *devplatform.Manager) (string, error) {
					info, e := m.ConnectionInfo(id)
					if e == nil {
						a.post(func() {
							if a.copy != nil {
								a.copy(info.URI)
							}
						})
					}
					return "Connection URI copied", e
				})
			}
			if ui.Button(c, "Copy database password").AlignSelf(ui.Start).Disabled(d.busy).Clicked() {
				id := d.dbID
				a.devTask(func(m *devplatform.Manager) (string, error) {
					password, e := m.DatabasePassword(id)
					if e == nil {
						a.post(func() {
							if a.copy != nil {
								a.copy(password)
							}
						})
					}
					return "Database password copied", e
				})
			}
			if ui.Button(c, "Open external database client").Disabled(d.busy).Clicked() {
				id := d.dbID
				a.devTask(func(m *devplatform.Manager) (string, error) {
					info, e := m.ConnectionInfo(id)
					if e != nil {
						return "", e
					}
					_, e = m.Run(m.Context, "/usr/bin/open", info.URI)
					return "Opening the default client for " + softwareTitle(instance.Kind), e
				})
			}
			if ui.Button(c, "Read instance logs").AlignSelf(ui.Start).Disabled(d.busy).Clicked() {
				id := "db-" + instance.ID
				d.showResult = true
				a.devTask(func(m *devplatform.Manager) (string, error) { return m.Logs(id) })
			}
		})

		a.devDatabasePort(c, instance)
		devDivider(c, "Data")
	} else {
		d.dbNewDraft = true
		devHeading(c, "New instance", "")
		if packages := d.packages(d.dbKind); len(packages) == 0 {
			ui.Text(c, "Install "+softwareTitle(d.dbKind)+" first").FontSize(textSmall).TextColor(c.Theme().Warning)
		} else if d.dbPackage == "" {
			d.dbPackage = packages[0]
		}
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
		a.devPortControls(c, &d.dbPort, "database")
		if d.dbKind == "redis" {
			ui.Checkbox(c, &d.persistent, "Persist database contents")
		} else {
			d.persistent = true
		}
		if ui.PrimaryButton(c, "Create instance").AlignSelf(ui.Start).Disabled(d.busy || d.databaseTasks[d.dbID] != nil && d.databaseTasks[d.dbID].Running).Clicked() {
			port, e := strconv.Atoi(d.dbPort)
			if e != nil {
				d.output = "Error: Enter a valid port"
			} else {
				i := devplatform.Instance{ID: d.dbID, Kind: d.dbKind, Package: d.dbPackage, Port: port, Persistent: d.persistent}
				d.dbNewDraft = true
				a.startDatabaseTask(i)
			}
		}
		a.devDatabaseRecovery(c)
		return
	}
	if d.dbKind == "redis" {
		devField(c, "Database number (0–15)", &d.dbName, "0")
	} else {
		devField(c, "Database name", &d.dbName, "app")
	}
	if d.dbKind != "redis" {
		if ui.Button(c, "Create database").AlignSelf(ui.Start).Disabled(d.busy || a.databaseTaskRunning(d.dbID)).Clicked() {
			id, name := d.dbID, d.dbName
			a.startDatabaseOperation(d.state.Instances[id], "create database", func(m *devplatform.Manager) (string, error) {
				return "Created database " + name, m.CreateDatabase(m.Context, id, name)
			})
		}
		devField(c, "Database user", &d.dbUser, "app_user")
		ui.Field(c, "New user password", func() { ui.TextInput(c, &d.dbPassword).Password() })
		if ui.Button(c, "Create database user").AlignSelf(ui.Start).Disabled(d.busy || a.databaseTaskRunning(d.dbID)).Clicked() {
			id, name, password, db := d.dbID, d.dbUser, d.dbPassword, d.dbName
			d.dbPassword = ""
			a.startDatabaseOperation(d.state.Instances[id], "create user", func(m *devplatform.Manager) (string, error) {
				return "Created user " + name, m.CreateUser(m.Context, id, name, password, db)
			})
		}
	}
	devDivider(c, "Backups")
	if ui.Button(c, "Back up database").AlignSelf(ui.Start).Disabled(d.busy || a.databaseTaskRunning(d.dbID)).Clicked() {
		id, name := d.dbID, d.dbName
		a.startDatabaseOperation(d.state.Instances[id], "backup", func(m *devplatform.Manager) (string, error) { return m.Backup(m.Context, id, name) })
	}
	devField(c, "Backup file", &d.backupPath, "Select a backup file")
	ui.Row(c).Wrap().Gap(8).Children(func() {
		if ui.Button(c, "Choose backup file…").AlignSelf(ui.Start).Clicked() {
			paths, e := mygo.Dialog.Open(mygo.OpenDialogOptions{Title: "Restore database backup"})
			if e != nil {
				d.output = e.Error()
			} else if len(paths) > 0 {
				d.backupPath = paths[0]
			}
		}
		if ui.Button(c, "Restore into new database").AlignSelf(ui.Start).Disabled(d.busy || a.databaseTaskRunning(d.dbID)).Clicked() {
			id, name, path := d.dbID, d.dbName, d.backupPath
			a.startDatabaseOperation(d.state.Instances[id], "restore", func(m *devplatform.Manager) (string, error) {
				return "Restored into " + name, m.Restore(m.Context, id, name, path)
			})
		}
	})
	devDivider(c, "Remove")
	ui.Row(c).Wrap().Gap(8).Children(func() {
		if ui.Button(c, "Remove instance (keep data files)").AlignSelf(ui.Start).Disabled(d.busy || a.databaseTaskRunning(d.dbID)).Clicked() {
			id := d.dbID
			a.startDatabaseOperation(d.state.Instances[id], "remove", func(m *devplatform.Manager) (string, error) {
				return "Removed instance", m.InstanceAction(m.Context, id, "remove")
			})
		}
		if ui.Button(c, "Review permanent data deletion").AlignSelf(ui.Start).Disabled(d.busy || a.databaseTaskRunning(d.dbID)).Clicked() {
			a.previewInstanceDeletion(d.dbID)
		}
	})
	a.devDatabaseRecovery(c)
}

// devDatabaseRecovery offers the reviews of work an exited caller left.
func (a *app) devDatabaseRecovery(c *ui.Context) {
	d := &a.dev
	devDivider(c, "Recovery")
	ui.Row(c).Wrap().Gap(8).Children(func() {
		if ui.Button(c, "Unfinished database tasks").AlignSelf(ui.Start).Disabled(d.busy).Clicked() {
			a.openDatabaseWork()
		}
		if ui.Button(c, "Unfinished initializations").AlignSelf(ui.Start).Disabled(d.busy).Clicked() {
			a.openInitializations()
		}
		if ui.Button(c, "Review pending data deletions").AlignSelf(ui.Start).Disabled(d.busy).Clicked() {
			a.openInstanceDeletions()
		}
	})
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
	return fmt.Sprintf("Source: %s\nSHA-256: %s\nArchitecture: %s\nSystem: %s\nLicense: %s\nInstallation folder: %s", info.Package.URL, info.Package.SHA256, info.Package.Arch, minimum, info.Package.License, info.Location)
}
func (a *app) devPackageReview(c *ui.Context) {
	d := &a.dev
	if d.softwareContext != "" {
		ui.Text(c, d.softwareContext).FontSize(textSmall).TextColor(c.Theme().TextMuted)
	}
	if ui.Button(c, "Review package").AlignSelf(ui.Start).Disabled(d.busy || d.software == "" || strings.TrimSpace(d.version) == "").Clicked() {
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
	valid := d.previewMatches()
	if valid {
		info := d.packagePreview
		devDivider(c, "Package")
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
		ui.Text(c, size).FontSize(textSmall).TextColor(c.Theme().TextMuted)
		ui.Text(c, info.Compatibility).FontSize(textSmall).TextColor(func() ui.Color {
			if !info.Compatible {
				return c.Theme().Danger
			}
			return c.Theme().TextMuted
		}())
		ui.Column(c).Padding(12).Radius(8).Background(c.Theme().Surface).MinWidth(0).Children(func() {
			ui.Text(c, d.packageDetailsText).FontSize(textSmall).Selectable().Label("Package details")
		})
		ui.Row(c).Gap(8).Children(func() {
			if ui.Button(c, "Copy package source").AlignSelf(ui.Start).Clicked() && a.copy != nil {
				a.copy(info.Package.URL)
			}
			if ui.Button(c, "Copy package checksum").AlignSelf(ui.Start).Clicked() && a.copy != nil {
				a.copy(info.Package.SHA256)
			}
		})
		for _, reference := range info.References {
			ui.Text(c, reference).FontSize(textSmall).TextColor(c.Theme().TextMuted)
		}
		if info.Installed && info.Integrity != "" {
			ui.Text(c, "Installed files: "+info.Integrity+". "+info.IntegrityNote).FontSize(textSmall).TextColor(c.Theme().TextMuted)
		}
	}
	installLabel := "Download and install"
	if valid && d.packagePreview.Package.Formula != "" {
		installLabel = "Install with Homebrew"
	}
	if valid && d.packagePreview.Installed {
		installLabel = "Already installed"
	}
	if ui.PrimaryButton(c, installLabel).AlignSelf(ui.Start).Disabled(d.busy || !valid || d.packagePreview.Installed || !d.packagePreview.Compatible || (d.packagePreview.Integrity == "Needs repair" && d.packagePreview.Package.Formula == "") || a.packageTaskRunning(d.packagePreview.Package.ID())).Clicked() {
		pkg := d.packagePreview.Package
		a.startPackageTask(pkg, "install", func(m *devplatform.Manager) (string, error) {
			if err := m.Install(m.Context, pkg); err != nil {
				return "Install did not complete.", err
			}
			return "Installed " + softwareTitle(pkg.Name) + " " + pkg.Version, nil
		}, nil)

	}
}

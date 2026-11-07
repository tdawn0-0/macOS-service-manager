package main

import (
	"context"
	"errors"
	"fmt"
	"github.com/egoist/mygo"
	"github.com/egoist/mygo/ui"
	"macos-service-manager/internal/devplatform"
	"macos-service-manager/internal/launchd"
	"os"
	"path/filepath"
	"reflect"
	"slices"
	"sort"
	"strconv"
	"strings"
	"time"
)

type developmentState struct {
	commandWorkOpen                                                                  bool
	commandWork                                                                      []devplatform.ProjectCommandInfo
	databaseWorkOpen                                                                 bool
	databaseWork                                                                     []devplatform.DatabaseWorkInfo
	projectTasks                                                                     map[string]*projectTask
	projectNeedsRefresh                                                              bool
	lastProjectTask                                                                  *projectTask
	databaseTasks                                                                    map[string]*databaseTask
	databaseNeedsRefresh, dbNewDraft, initializationsOpen                            bool
	lastDatabaseTask                                                                 *databaseTask
	initializations                                                                  []devplatform.InstanceInitializationInfo
	runtimeEditRevisions                                                             map[string]uint64
	packageTasks                                                                     map[string]*packageTask
	packageNeedsRefresh                                                              bool
	lastPackageTask                                                                  *packageTask
	packageContention                                                                *devplatform.PackageBusyError
	instanceDeletionPreview                                                          *devplatform.InstanceDeletionPreview
	instanceDeletions                                                                []devplatform.InstanceDeletionInfo
	deletionsOpen                                                                    bool
	deleteConfirmation                                                               string
	operationsOpen                                                                   bool
	operations                                                                       []devplatform.OperationRecord
	operationID, operationsError                                                     string
	operationScroll                                                                  ui.ScrollState
	authorityOpen                                                                    bool
	authorityStatus                                                                  *devplatform.AuthorityStatus
	portChecks                                                                       map[int]devplatform.PortInspection
	folderPreview                                                                    *devplatform.ProjectFolderPreview
	folderDraft                                                                      *devplatform.Project
	folderErrors                                                                     map[string]string
	exportRecovery                                                                   *devplatform.ExportRecovery
	exportRecoveryDirectory                                                          string
	diagnosticsOpen                                                                  bool
	diagnosticReport                                                                 *devplatform.DiagnosticReport
	diagnosticScroll                                                                 ui.ScrollState
	diagnosticJSON                                                                   string
	packagePreview                                                                   *devplatform.PackageDetails
	packageDetailsText                                                               string
	packageChecks                                                                    map[string]devplatform.PackageIntegrityResult
	runtimeTargetProject, runtimeTargetService, softwareContext                      string
	siteAdvanced, siteOptionsLoaded                                                  bool
	siteConnectTimeout, siteHeaderTimeout, siteHostPolicy                            string
	serviceOriginalID                                                                string
	detailScroll                                                                     [4]ui.ScrollState
	managerFactory                                                                   func() *devplatform.Manager
	projectTab                                                                       int
	logService, logText                                                              string
	importing, showResult                                                            bool
	statuses                                                                         map[string]string
	serviceStatuses                                                                  map[string]devplatform.ServiceStatus
	statusPolling                                                                    bool
	statusLast                                                                       time.Time
	taskSequence                                                                     uint64
	envFile                                                                          string
	loaded, busy                                                                     bool
	tab                                                                              int
	state                                                                            devplatform.State
	output                                                                           string
	cancel                                                                           context.CancelFunc
	directory, id                                                                    string
	draft                                                                            devplatform.Project
	serviceID, servicePackage, arguments, workdir, environment, dependencies, health string
	serviceRestart                                                                   bool
	localArguments                                                                   []devplatform.LocalArgument
	serviceReadinessTimeout, serviceRestartThrottle                                  string
	software, version                                                                string
	dbKind, dbID, dbPackage, dbPort, dbName, dbUser, dbPassword, backupPath          string
	persistent                                                                       bool
	siteID, siteDomain, siteUpstream, sitePort, siteProject                          string
	siteDisabled                                                                     bool
	siteOriginalID, siteOriginalProject                                              string
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
	keepDraft = keepDraft && a.dev.projectDirty()
	draftProject := a.dev.id
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
			if p, ok := s.Projects[a.dev.id]; ok && !reflect.DeepEqual(a.dev.state.Projects[a.dev.id], p) && !(keepDraft && a.dev.id == draftProject) {
				tab, service := a.dev.projectTab, a.dev.serviceOriginalID
				a.dev.selectProject(p)
				a.dev.projectTab = tab
				for _, candidate := range p.Services {
					if candidate.ID == service {
						a.dev.selectService(candidate)
					}
				}
			}
			if _, was := a.dev.state.Projects[a.dev.id]; was {
				if _, ok := s.Projects[a.dev.id]; !ok {
					a.dev.id = ""
					a.dev.draft = devplatform.Project{}
				}
			}
			a.dev.state = s
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
			a.applyProjectTaskPhases()
			a.dev.serviceStatuses = snapshot.Services
			a.dev.folderErrors = snapshot.FolderErrors
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
func (d *developmentState) selectProject(p devplatform.Project) {
	if d.runtimeTargetProject != "" && d.runtimeTargetProject != p.ID {
		d.runtimeTargetProject = ""
		d.runtimeTargetService = ""
		d.softwareContext = ""
	}
	d.id = p.ID
	d.folderPreview = nil
	d.folderDraft = nil
	d.draft = p.Clone()
	d.selectService(devplatform.Service{})
	d.logText = ""
	d.logService = ""
	d.projectTab = 0
	d.detailScroll[0].Y = 0
	if len(p.Services) > 0 {
		d.selectService(p.Services[0])
		d.logService = p.Services[0].ID
	}
}
func (d *developmentState) selectService(s devplatform.Service) {
	d.serviceOriginalID = s.ID
	d.serviceID = s.ID
	d.servicePackage = s.Package
	d.arguments = displayServiceCommand(s)
	d.localArguments = slices.Clone(s.LocalArguments)
	d.workdir = s.Directory
	d.environment = launchd.FormatEnv(s.Environment)
	d.dependencies = strings.Join(s.DependsOn, ",")
	d.health = s.Health
	d.envFile = s.EnvFile
	d.serviceRestart = s.Restart
	d.serviceReadinessTimeout = serviceSeconds(s.ReadinessTimeoutSeconds, 30)
	d.serviceRestartThrottle = serviceSeconds(s.RestartThrottleSeconds, 10)
}
func serviceSeconds(n, fallback int) string {
	if n == 0 {
		n = fallback
	}
	return strconv.Itoa(n)
}
func commaList(s string) []string {
	var out []string
	for _, v := range strings.Split(s, ",") {
		if v = strings.TrimSpace(v); v != "" {
			out = append(out, v)
		}
	}
	return out
}
func (d *developmentState) applyService() error {
	env, e := launchd.ParseEnv(d.environment)
	if e != nil {
		return e
	}
	s := devplatform.Service{ID: d.serviceID, Package: d.servicePackage, Command: launchd.ParseLines(d.arguments), Directory: d.workdir, Environment: env, EnvFile: d.envFile, DependsOn: commaList(d.dependencies), Health: d.health, Restart: d.serviceRestart}
	for _, old := range d.draft.Services {
		if old.ID == d.serviceOriginalID {
			s.RequiredEnvironment = slices.Clone(old.RequiredEnvironment)
			s.NeedsEnvironmentFile = old.NeedsEnvironmentFile
			s.NeedsHealthReview = old.NeedsHealthReview
		}
	}
	s.LocalArguments = slices.Clone(d.localArguments)
	for n := range s.LocalArguments {
		arg := &s.LocalArguments[n]
		if arg.Index < 0 || arg.Index >= len(s.Command) {
			return fmt.Errorf("keep local argument positions, or remove this service and add a new command")
		}
		if arg.Value == "" && s.Command[arg.Index] != devplatform.LocalArgumentPlaceholder(arg.Index) {
			arg.Value = strings.TrimPrefix(s.Command[arg.Index], arg.Prefix)
		}
		if arg.Value == "" {
			s.Command[arg.Index] = devplatform.LocalArgumentPlaceholder(arg.Index)
		} else {
			s.Command[arg.Index] = arg.Prefix + arg.Value
		}
	}
	readiness, re := strconv.Atoi(d.serviceReadinessTimeout)
	throttle, te := strconv.Atoi(d.serviceRestartThrottle)
	if re != nil || te != nil || readiness < 1 || throttle < 1 {
		return fmt.Errorf("enter whole seconds from 1–3600 for readiness and restart throttle")
	}
	s.ReadinessTimeoutSeconds = readiness
	s.RestartThrottleSeconds = throttle
	if e := s.Validate(); e != nil {
		return e
	}
	for _, old := range d.draft.Services {
		if old.ID == s.ID && old.ID != d.serviceOriginalID {
			return fmt.Errorf("a service with this name already exists")
		}
	}
	for n, old := range d.draft.Services {
		if old.ID == d.serviceOriginalID {
			d.draft.Services[n] = s
			if old.ID != s.ID {
				for n := range d.draft.Services {
					for j, ref := range d.draft.Services[n].DependsOn {
						if ref == old.ID {
							d.draft.Services[n].DependsOn[j] = s.ID
						}
					}
				}
			}
			d.serviceOriginalID = s.ID
			if d.logService == old.ID {
				d.logService = s.ID
			}
			return nil
		}
	}
	d.draft.Services = append(d.draft.Services, s)
	d.serviceOriginalID = s.ID
	return nil
}

func (d *developmentState) projectDirty() bool {
	if d.id == "" {
		return false
	}
	if !reflect.DeepEqual(d.draft, d.state.Projects[d.id]) {
		return true
	}
	if d.serviceID != "" {
		for _, s := range d.draft.Services {
			if s.ID == d.serviceOriginalID {
				return d.serviceID != s.ID || d.servicePackage != s.Package || d.arguments != displayServiceCommand(s) || !slices.Equal(d.localArguments, s.LocalArguments) || d.workdir != s.Directory || d.environment != launchd.FormatEnv(s.Environment) || d.dependencies != strings.Join(s.DependsOn, ",") || d.health != s.Health || d.envFile != s.EnvFile || d.serviceRestart != s.Restart || d.serviceReadinessTimeout != serviceSeconds(s.ReadinessTimeoutSeconds, 30) || d.serviceRestartThrottle != serviceSeconds(s.RestartThrottleSeconds, 10)
			}
		}
		return true
	}
	return false
}
func (d *developmentState) flushService() bool {
	if d.serviceID == "" {
		return true
	}
	if e := d.applyService(); e != nil {
		d.output = "Error: " + e.Error()
		return false
	}
	return true
}
func (a *app) developmentView(c *ui.Context) {
	a.pollDevStatus()
	c.After(5 * time.Second)
	d := &a.dev
	if !d.loaded && !d.busy {
		a.devTask(func(m *devplatform.Manager) (string, error) { return "Ready to build locally.", nil })
	}
	titles := devSections[:]
	if d.tab < 0 || d.tab > 3 {
		d.tab = 0
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
			if !d.busy && len(d.projectTasks) > 0 && (d.tab == 0 || a.projectTasksRunning() && !a.databaseTasksRunning() && !a.packageTasksRunning()) {
				if ui.Button(c, "Project tasks").Clicked() {
					a.showProjectTasks()
				}
			} else if !d.busy && len(d.databaseTasks) > 0 && (d.tab == 2 || a.databaseTasksRunning() && !a.packageTasksRunning()) {
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
				ui.Scroll(c.Key(fmt.Sprintf("dev-detail-%d-%d", d.tab, d.projectTab))).TrackScroll(&d.detailScroll[d.tab]).Grow(1).MinWidth(0).MinHeight(0).Padding(20, 24).Children(func() {
					ui.Column(c).Gap(10).Disabled(d.busy).Children(func() {
						switch d.tab {
						case 0:
							a.devProjects(c)
						case 1:
							a.devSoftware(c)
						case 2:
							a.devDatabases(c)
						case 3:
							a.devHTTPS(c)
						}
					})
				})
			})
		}
		ui.Column(c).Height(1).Background(hairline(c.Theme()))
		ui.Row(c).Padding(9, 18).Gap(10).AlignItems(ui.Center).Children(func() {
			if !d.operationsOpen && !d.diagnosticsOpen && !d.commandWorkOpen && d.tab == 0 && d.id != "" && !d.importing {
				if ui.PrimaryButton(c, "Save project").AlignSelf(ui.Start).Disabled(d.busy || a.projectTaskRunning(d.id)).Clicked() {
					var e error
					if d.serviceID != "" {
						e = d.applyService()
					}
					if e != nil {
						d.output = "Error: " + e.Error()
					} else {
						p := d.draft.Clone()
						if p.Directory != d.state.Projects[p.ID].Directory {
							a.reviewProjectFolder(p)
						} else {
							a.devTask(func(m *devplatform.Manager) (string, error) { return "Saved " + p.Name, m.SaveProject(m.Context, p) })
						}
					}
				}
			}
			status := ""
			if d.tab == 0 && !d.importing && d.folderErrors[d.id] != "" {
				status = "Folder unavailable"
			}
			if d.tab == 0 && d.exportRecovery != nil && ((d.importing && d.directory == d.exportRecoveryDirectory) || (!d.importing && d.draft.Directory == d.exportRecoveryDirectory)) {
				status = "Export recovery"
			}
			if d.tab == 0 && d.projectDirty() {
				status = "Unsaved changes"
			}
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
					if status == "Export recovery" || status == "Folder unavailable" {
						return c.Theme().Warning
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
	if title, ok := map[string]string{"php": "PHP", "node": "Node.js", "mysql": "MySQL", "postgresql": "PostgreSQL", "redis": "Redis", "mongodb": "MongoDB"}[name]; ok {
		return title
	}
	return name
}
func (a *app) devMaster(c *ui.Context) {
	d := &a.dev
	switch d.tab {
	case 0:
		if ui.PrimaryButton(c, "Import a project").Disabled(d.busy).Clicked() {
			d.importing = true
			d.commandWorkOpen = false
		}
		for _, id := range sortedKeys(d.state.Projects) {
			p := d.state.Projects[id]
			status := d.statuses["project:"+id]
			if devListEntry(c, "Project "+p.Name, p.Name, status, devNews(status), d.id == id && !d.importing) {
				d.selectProject(p)
				d.commandWorkOpen = false
				d.importing = false
			}
		}
	case 1:
		for _, name := range []string{"php", "node", "mysql", "postgresql", "redis", "mongodb"} {
			count := ""
			if n := len(d.packages(name)); n > 0 {
				count = strconv.Itoa(n)
			}
			if devListEntry(c, "Software "+name, softwareTitle(name), "", count, d.software == name) {
				d.software = name
				d.runtimeTargetProject = ""
				d.runtimeTargetService = ""
				d.softwareContext = ""
				versions := devplatform.AvailableVersions(name)
				if len(versions) > 0 {
					d.version = versions[0]
				}
			}
		}
	case 2:
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
	case 3:
		if ui.PrimaryButton(c, "New HTTPS site").Disabled(d.busy).Clicked() {
			d.newSite()
		}
		for _, pid := range sortedKeys(d.state.Projects) {
			p := d.state.Projects[pid]
			for _, site := range p.Sites {
				if devListEntry(c, "Site "+site.Domain, site.Domain, "", siteNews(site), d.siteID == site.ID && d.siteProject == pid) {
					d.selectSite(site, pid)
				}
			}
		}
		for _, id := range sortedKeys(d.state.Sites) {
			site := d.state.Sites[id]
			if devListEntry(c, "Site "+site.Domain, site.Domain, "", siteNews(site), d.siteID == id && d.siteProject == "") {
				d.selectSite(site, "")
			}
		}
	}
}
func siteNews(site devplatform.Site) string {
	switch {
	case site.NeedsReview:
		return "Review"
	case site.Disabled:
		return "Off"
	}
	return ""
}
func (d *developmentState) selectSite(site devplatform.Site, project string) {
	d.authorityOpen = false
	d.siteOriginalID = site.ID
	d.siteOriginalProject = project
	d.siteID = site.ID
	d.siteDomain = site.Domain
	d.siteUpstream = site.Upstream
	d.sitePort = strconv.Itoa(site.Port)
	d.siteProject = project
	d.siteDisabled = site.Disabled
	options := site.ProxyOptions()
	d.siteOptionsLoaded = true
	d.siteAdvanced = site.Proxy != nil
	d.siteConnectTimeout = strconv.Itoa(options.ConnectTimeoutSeconds)
	d.siteHeaderTimeout = strconv.Itoa(options.ResponseHeaderTimeoutSeconds)
	d.siteHostPolicy = "Keep local domain"
	if options.HostPolicy == "upstream" {
		d.siteHostPolicy = "Use upstream host"
	}
}
func (d *developmentState) newSite() {
	d.authorityOpen = false
	d.siteOriginalID = ""
	d.siteOriginalProject = ""
	d.siteID = ""
	d.siteDomain = ""
	d.siteUpstream = "http://127.0.0.1:3000"
	d.sitePort = "443"
	d.siteProject = ""
	d.siteDisabled = false
	d.siteAdvanced = false
	d.siteOptionsLoaded = true
	d.siteConnectTimeout = "10"
	d.siteHeaderTimeout = "30"
	d.siteHostPolicy = "Keep local domain"
	d.detailScroll[3].Y = 0
}
func (a *app) devProjects(c *ui.Context) {
	d := &a.dev
	if d.commandWorkOpen {
		a.devProjectCommands(c)
		return
	}
	a.devProjectTasks(c)
	if d.id == "" || d.importing {
		devField(c, "Project folder", &d.directory, "/Users/me/project")
		ui.Row(c).Gap(8).Children(func() {
			if ui.Button(c, "Choose project folder…").AlignSelf(ui.Start).Disabled(d.busy).Clicked() {
				paths, e := mygo.Dialog.Open(mygo.OpenDialogOptions{Title: "Import project", Directory: true})
				if e != nil {
					d.output = e.Error()
				} else if len(paths) > 0 {
					d.directory = paths[0]
				}
			}
			if ui.PrimaryButton(c, "Import project").AlignSelf(ui.Start).Tooltip("Importing never runs project scripts.").Disabled(d.busy).Clicked() {
				dir := d.directory
				a.devTask(func(m *devplatform.Manager) (string, error) {
					p, e := m.Import(m.Context, dir)
					if e == nil {
						a.post(func() { a.dev.selectProject(p); a.dev.importing = false })
					} else {
						_, _ = a.inspectDevExport(m, dir)
					}
					return "Imported " + p.Name, e
				})
			}
		})
		a.devExportRecovery(c, d.directory)
		return
	}
	status := d.statuses["project:"+d.id]
	ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
		devStatusDot(c, status)
		ui.Text(c, d.draft.Name).FontSize(18).FontWeight(650).Grow(1).MinWidth(0)
		if news := devNews(status); news != "" {
			ui.Text(c, news).FontSize(textSmall).FontWeight(600).TextColor(devStatusColor(c, status)).Shrink(0)
		}
	})
	ui.Row(c).Wrap().Gap(8).Children(func() {
		for _, action := range []string{"Start project", "Restart project", "Stop project"} {
			if ui.Button(c, action).AlignSelf(ui.Start).Disabled(d.busy || a.projectTaskRunning(d.id) || (action != "Stop project" && (d.projectDirty() || len(d.draft.MissingLocalSettings()) > 0 || d.folderErrors[d.id] != ""))).Clicked() {
				kind := map[string]string{"Start project": "up", "Stop project": "down", "Restart project": "restart"}[action]
				a.runProjectAction(d.state.Projects[d.id], kind)
			}
		}
		for _, site := range d.draft.Sites {
			a.devOpenSite(c, site)
		}
		if ui.Button(c, "Add project HTTPS site").Disabled(d.busy || d.projectDirty()).Clicked() {
			a.addProjectHTTPS()
		}
	})
	if message := d.folderErrors[d.id]; message != "" {
		ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
			ui.Text(c, message).FontSize(textSmall).TextColor(c.Theme().Warning).Grow(1).MinWidth(0)
			if ui.Button(c, "Locate the project folder").Clicked() {
				d.projectTab = 0
				d.detailScroll[0].Y = 0
			}
		})
	}
	if missing := d.draft.MissingLocalSettings(); len(missing) > 0 {
		ui.Column(c).Gap(4).Padding(10, 12).Radius(8).Background(c.Theme().Warning.Alpha(0.1)).Children(func() {
			ui.Text(c, fmt.Sprintf("Local setup required · %d items", len(missing))).FontSize(textSmall).FontWeight(600).TextColor(c.Theme().Warning)
			ui.Text(c, strings.Join(missing, "\n")).FontSize(textSmall).TextColor(c.Theme().TextMuted)
			if ui.Button(c, "Complete local setup").AlignSelf(ui.Start).Disabled(d.busy).Clicked() && d.flushService() {
				a.devChooseLocalSetup()
			}
		})
	}
	if ui.Segmented(c, &d.projectTab, "Overview", "Services", "Environment", "Logs").Label("Project settings section").Changed() {
		d.detailScroll[0].Y = 0
	}
	switch d.projectTab {
	case 0:
		devField(c, "Project name", &d.draft.Name, "")
		a.devProjectFolder(c)
		ui.Checkbox(c, &d.draft.Autostart, "Start this project at login")
		a.devRuntimeHints(c)
		if len(d.state.Instances) > 0 {
			devDivider(c, "Databases")
		}
		for _, id := range sortedKeys(d.state.Instances) {
			on := false
			for _, ref := range d.draft.Instances {
				if ref == id {
					on = true
				}
			}
			if ui.Checkbox(c, &on, id).Changed() {
				refs := []string{}
				for _, ref := range d.draft.Instances {
					if ref != id {
						refs = append(refs, ref)
					}
				}
				if on {
					refs = append(refs, id)
				}
				d.draft.Instances = refs
			}
		}

		if _, e := os.Stat(filepath.Join(d.draft.Directory, "package.json")); e == nil {
			if ui.Button(c, "Install project dependencies").AlignSelf(ui.Start).Tooltip("Runs npm ci in the saved environment, including repository lifecycle scripts. Source and dependency changes remain after cancellation.").Disabled(d.busy || a.projectTaskRunning(d.id) || d.projectDirty()).Clicked() {
				id := d.id
				a.startProjectOperation(d.state.Projects[id], "dependencies", func(m *devplatform.Manager) (string, error) { return m.Exec(m.Context, id, []string{"npm", "ci"}) })
			}
		}
		devDivider(c, "Environment")
		ui.Row(c).Wrap().Gap(8).Children(func() {
			if ui.Button(c, "Export environment").AlignSelf(ui.Start).Tooltip("Exports software versions and database declarations. Local paths, ports, secrets and database contents stay on this Mac.").Disabled(d.busy || d.projectDirty()).Clicked() {
				id := d.id
				dir := d.draft.Directory
				a.devTask(func(m *devplatform.Manager) (string, error) {
					err := m.ExportProject(m.Context, id)
					_, _ = a.inspectDevExport(m, dir)
					if err != nil {
						return "Environment export stopped.", err
					}
					return "Environment exported to the project folder", nil
				})
			}
			if ui.Button(c, "Prepare imported environment").AlignSelf(ui.Start).Tooltip("Installs the locked software and initializes separate databases for this working copy. Runs no project scripts.").Disabled(d.busy || a.projectTaskRunning(d.id) || d.projectDirty()).Clicked() {
				a.runProjectAction(d.state.Projects[d.id], "prepare")
			}
		})
		a.devExportRecovery(c, d.draft.Directory)
		devDivider(c, "Maintenance")
		ui.Row(c).Wrap().Gap(8).Children(func() {
			if ui.Button(c, "Unfinished project commands").AlignSelf(ui.Start).Disabled(d.busy).Clicked() {
				a.openProjectCommands()
			}
			if ui.Button(c, "Remove project (keep source and database files)").AlignSelf(ui.Start).Disabled(d.busy || a.projectTaskRunning(d.id)).Clicked() {
				a.runProjectAction(d.state.Projects[d.id], "remove")
			}
		})
	case 1:
		ui.Row(c).Wrap().Gap(8).Children(func() {
			for _, svc := range d.draft.Services {
				if ui.Button(c, "Edit "+svc.ID).Clicked() {
					if d.flushService() {
						for _, current := range d.draft.Services {
							if current.ID == svc.ID {
								d.selectService(current)
							}
						}
					}
				}
			}
			if ui.Button(c, "Add service").Clicked() {
				if d.flushService() {
					for n := 1; n <= len(d.draft.Services)+1; n++ {
						id := fmt.Sprintf("service-%d", n)
						used := false
						for _, s := range d.draft.Services {
							used = used || s.ID == id
						}
						if !used {
							d.selectService(devplatform.Service{ID: id})
							break
						}
					}
					d.projectTab = 1
				}
			}
		})
		devField(c, "Service name", &d.serviceID, "web")
		if devPackageChoice(c, "Runtime version", &d.servicePackage, d.packages(""), d.state.Packages, true) {
			d.noteRuntimeEdit(d.id, d.serviceOriginalID)
		}
		a.devLocalArguments(c)
		ui.Field(c, "Program and arguments", func() {
			ui.TextArea(c, &d.arguments).Lines(3, 7).Label("Program and arguments").Placeholder("npm\nrun\ndev")
		})
		devField(c, "Service subfolder", &d.workdir, "Relative folder")
		devField(c, "Readiness check", &d.health, "tcp://127.0.0.1:3000 or http(s)://…")
		if port := devplatform.ReadinessPort(d.health); port > 0 {
			if ui.Button(c, "Check service readiness port").AlignSelf(ui.Start).Disabled(d.busy).Clicked() {
				a.checkDevPort(port)
			}
			a.devPortResult(c, port)
		}
		devField(c, "Readiness timeout (seconds)", &d.serviceReadinessTimeout, "30")
		devField(c, "Wait for services", &d.dependencies, "Comma separated service names")
		ui.Checkbox(c, &d.serviceRestart, "Restart this service after it exits")
		if d.serviceRestart {
			devField(c, "Restart throttle (seconds)", &d.serviceRestartThrottle, "10")
		}
		if status, ok := d.serviceStatuses[devplatform.ServiceStatusKey(d.id, d.serviceOriginalID)]; ok {
			line := status.Status
			if status.PID > 0 {
				line += fmt.Sprintf(" · PID %d · %d launches", status.PID, status.Launches)
			}
			if status.LastExitCode != nil {
				line += fmt.Sprintf(" · exit %d", *status.LastExitCode)
			}
			ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
				devStatusDot(c, status.Status)
				ui.Text(c, line).FontSize(textSmall).FontWeight(600).TextColor(devStatusColor(c, status.Status))
			})
			if status.Status != "Healthy" && status.Message != "" {
				ui.Text(c, status.Message).FontSize(textSmall).TextColor(c.Theme().TextMuted)
			}
		}
		if ui.Button(c, "Remove service from project").AlignSelf(ui.Start).Disabled(d.busy).Clicked() {
			for n, s := range d.draft.Services {
				if s.ID == d.serviceOriginalID {
					d.draft.Services = append(d.draft.Services[:n], d.draft.Services[n+1:]...)
					d.serviceID = ""
					break
				}
			}
		}
	case 2:
		var services []string
		for _, service := range d.draft.Services {
			services = append(services, service.ID)
		}
		selected := d.serviceID
		if devChoice(c, "Environment for service", &selected, services) {
			if d.flushService() {
				for _, service := range d.draft.Services {
					if service.ID == selected {
						d.selectService(service)
					}
				}
			}
		}
		devField(c, "Environment file (optional)", &d.envFile, ".env.local")
		if ui.Button(c, "Choose environment file…").AlignSelf(ui.Start).Disabled(d.busy).Clicked() {
			paths, err := mygo.Dialog.Open(mygo.OpenDialogOptions{Title: "Choose a local environment file"})
			if err != nil {
				d.output = "Error: " + err.Error()
			} else if len(paths) > 0 {
				d.envFile = paths[0]
			}
		}
		ui.Field(c, "Environment variables", func() {
			ui.TextArea(c, &d.environment).Lines(2, 5).Label("Environment variables").Placeholder("NAME=value")
		})
		for _, svc := range d.draft.Services {
			if svc.ID == d.serviceOriginalID && len(svc.RequiredEnvironment) > 0 {
				ui.Text(c, "Required by this environment: "+strings.Join(svc.RequiredEnvironment, ", ")).FontSize(textSmall).TextColor(c.Theme().TextMuted)
			}
		}

	case 3:
		rotation := !d.state.LogRotationDisabled
		ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
			if ui.Checkbox(c, &rotation, "Rotate service logs while the app is closed").Tooltip("Every minute, a user launch agent keeps two 5 MiB archives per service log. Busy logs can grow between checks.").Changed() {
				enabled := rotation
				a.devTask(func(m *devplatform.Manager) (string, error) {
					return "Log rotation settings saved", m.SetLogRotation(m.Context, enabled)
				})
			}
			ui.Spacer(c)
			if ui.Button(c, "Rotate logs now").Clicked() {
				a.devTask(func(m *devplatform.Manager) (string, error) { return "Logs rotated", m.RotateLogs(m.Context) })
			}
		})
		var services []string
		for _, service := range d.draft.Services {
			services = append(services, service.ID)
		}
		devChoice(c, "Log service", &d.logService, services)
		if ui.Button(c, "Read service logs").AlignSelf(ui.Start).Disabled(d.busy || d.logService == "").Clicked() {
			job := "p-" + d.id + "-" + d.logService
			a.devTask(func(m *devplatform.Manager) (string, error) {
				text, e := m.Logs(job)
				if e == nil {
					a.post(func() { a.dev.logText = text })
				}
				return "Service logs refreshed", e
			})
		}
		ui.TextArea(c, &d.logText).Lines(8, 18).ReadOnly(true).Label("Service logs")
	}

}
func (a *app) devSoftware(c *ui.Context) {
	d := &a.dev
	if d.software == "" {
		d.software = "node"
		d.version = devplatform.AvailableVersions("node")[0]
	}
	devHeading(c, softwareTitle(d.software), "")
	a.devPackageTasks(c)
	devChoice(c, "Version", &d.version, devplatform.AvailableVersions(d.software))
	devField(c, "Exact version", &d.version, "For example 22.14.0")
	if d.software == "php" {
		if ui.Button(c, "Add shivammathur/php repository").AlignSelf(ui.Start).Disabled(d.busy || a.packageTasksRunning()).Clicked() {
			a.devTask(func(m *devplatform.Manager) (string, error) {
				if err := m.AddPHPRepository(m.Context); err != nil {
					return "PHP repository setup did not complete.", err
				}
				return "PHP repository added. Review the version to install.", nil
			})
		}
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

		linked := false
		for _, pid := range sortedKeys(d.state.Projects) {
			p := d.state.Projects[pid]
			for _, ref := range p.Instances {
				if ref == instance.ID {
					if !linked {
						devDivider(c, "Projects using this instance")
					}
					linked = true
					ui.Text(c, p.Name+" · "+d.statuses["project:"+pid]).FontSize(textSmall).SingleLine().Tooltip(p.Directory)
					break
				}
			}
		}
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
func (a *app) devHTTPS(c *ui.Context) {
	d := &a.dev
	if d.authorityOpen {
		a.devAuthority(c)
		return
	}
	title := "New site"
	if d.siteOriginalID != "" {
		title = d.siteDomain
	}
	ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
		ui.Text(c, title).FontSize(18).FontWeight(650).Grow(1).MinWidth(0)
		if ui.Button(c, "Local certificates").Disabled(d.busy).Clicked() {
			d.authorityOpen = true
			d.detailScroll[3].Y = 0
			a.authorityTask(nil)
		}
	})
	if d.sitePort == "" {
		d.sitePort = "443"
	}
	if !d.siteOptionsLoaded {
		d.siteOptionsLoaded = true
		d.siteConnectTimeout = "10"
		d.siteHeaderTimeout = "30"
		d.siteHostPolicy = "Keep local domain"
	}
	if site, ok := d.state.Sites[d.siteID]; ok {
		a.devOpenSite(c, site)
	}
	if p, ok := d.state.Projects[d.siteOriginalProject]; ok {
		for _, site := range p.Sites {
			if site.ID == d.siteOriginalID {
				a.devOpenSite(c, site)
			}
		}
	}
	devField(c, "Site name", &d.siteID, "web")
	devField(c, "Local domain", &d.siteDomain, "app.test")
	devField(c, "Upstream address", &d.siteUpstream, "http://127.0.0.1:3000")
	devField(c, "HTTPS port", &d.sitePort, "443")
	a.devPortControls(c, &d.sitePort, "HTTPS")
	devProjectChoice(c, &d.siteProject, d.state.Projects)
	ui.Checkbox(c, &d.siteDisabled, "Disable this site")
	ui.Checkbox(c, &d.siteAdvanced, "Customize proxy settings")
	if d.siteAdvanced {
		devField(c, "Connection timeout (seconds)", &d.siteConnectTimeout, "10")
		devField(c, "Response header timeout (seconds)", &d.siteHeaderTimeout, "30")
		devChoice(c, "Upstream Host header", &d.siteHostPolicy, []string{"Keep local domain", "Use upstream host"})
	}
	if ui.PrimaryButton(c, "Save HTTPS site").AlignSelf(ui.Start).Disabled(d.busy).Clicked() {
		port, e := strconv.Atoi(d.sitePort)
		var options *devplatform.ProxyOptions
		if d.siteAdvanced {
			connection, ce := strconv.Atoi(d.siteConnectTimeout)
			headers, he := strconv.Atoi(d.siteHeaderTimeout)
			if ce != nil || he != nil {
				e = fmt.Errorf("enter whole seconds for proxy timeouts")
			} else {
				policy := "local"
				if d.siteHostPolicy == "Use upstream host" {
					policy = "upstream"
				}
				options = &devplatform.ProxyOptions{ConnectTimeoutSeconds: connection, ResponseHeaderTimeoutSeconds: headers, HostPolicy: policy}
			}
		}
		if e != nil {
			d.output = "Error: Enter a valid HTTPS port and whole seconds for proxy timeouts"
		} else {
			s := devplatform.Site{ID: d.siteID, Domain: d.siteDomain, Upstream: d.siteUpstream, Port: port, Disabled: d.siteDisabled, Proxy: options}
			project := d.siteProject
			oldOwner, oldID := d.siteOriginalProject, d.siteOriginalID
			a.devTask(func(m *devplatform.Manager) (string, error) {
				e := m.EditSite(m.Context, oldOwner, oldID, project, s)
				if e == nil {
					a.post(func() { a.dev.selectSite(s, project) })
				}
				return "Saved site. Apply proxy to activate changes.", e
			})
		}
	}
	if d.siteOriginalID != "" {
		if ui.Button(c, "Delete HTTPS site").AlignSelf(ui.Start).Disabled(d.busy).Clicked() {
			owner, id := d.siteOriginalProject, d.siteOriginalID
			a.devTask(func(m *devplatform.Manager) (string, error) {
				e := m.RemoveSite(m.Context, owner, id)
				if e == nil {
					a.post(func() { a.dev.newSite() })
				}
				return "Deleted site. Apply proxy to activate changes.", e
			})
		}
	}
	groups := []struct {
		title   string
		tip     string
		actions []string
	}{
		{"Proxy", "", []string{"Apply proxy", "Restart HTTPS proxy", "Stop HTTPS proxy"}},
		{"This Mac", "Hosts and system trust changes show the macOS authorization dialog.", []string{"Configure local domains", "Trust local CA", "Remove CA trust", "Diagnose HTTPS"}},
	}
	for _, group := range groups {
		devDivider(c, group.title)
		ui.Row(c).Wrap().Gap(8).Children(func() {
			for _, action := range group.actions {
				if ui.Button(c, action).AlignSelf(ui.Start).Tooltip(group.tip).Disabled(d.busy).Clicked() {
					a.devTask(func(m *devplatform.Manager) (string, error) {
						var e error
						switch action {
						case "Apply proxy":
							e = m.ApplyProxy(m.Context)
						case "Restart HTTPS proxy":
							e = m.RestartProxy(m.Context)
						case "Stop HTTPS proxy":
							e = m.StopProxy(m.Context)
						case "Configure local domains":
							e = m.ConfigureHosts(m.Context)
						case "Trust local CA":
							e = m.ConfigureTrust(m.Context, false)
						case "Remove CA trust":
							e = m.ConfigureTrust(m.Context, true)
						case "Diagnose HTTPS":
							report, err := m.Diagnose(m.Context)
							if err == nil {
								a.post(func() { a.showDevDiagnostics(report) })
							}
							return "Diagnostics finished. Review checks by layer.", err
						}
						return action, e
					})
				}
			}
		})
	}
}

func (a *app) devOpenSite(c *ui.Context, site devplatform.Site) {
	if site.NeedsReview {
		ui.Text(c, "Review and save this site before applying HTTPS.").FontSize(textSmall).TextColor(c.Theme().Warning)
	}
	if ui.Button(c, "Open "+site.URL()).AlignSelf(ui.Start).Disabled(a.dev.busy || site.Disabled || site.NeedsReview).Clicked() {
		a.devTask(func(m *devplatform.Manager) (string, error) {
			_, e := m.Run(m.Context, "/usr/bin/open", site.URL())
			return "Opening " + site.URL(), e
		})
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

func devProjectChoice(c *ui.Context, selected *string, projects map[string]devplatform.Project) {
	options := []string{"Independent site"}
	lookup := map[string]string{"Independent site": ""}
	counts := map[string]int{}
	for _, p := range projects {
		counts[p.Name]++
	}
	display := "Independent site"
	for _, id := range sortedKeys(projects) {
		p := projects[id]
		label := p.Name
		if counts[p.Name] > 1 {
			label += " · " + tildePath(p.Directory)
		}
		options = append(options, label)
		lookup[label] = id
		if *selected == id {
			display = label
		}
	}
	if devChoice(c, "Project", &display, options) {
		*selected = lookup[display]
	}
}

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
	titles := []string{"Projects", "Software", "Databases", "Local HTTPS"}
	if d.tab < 0 || d.tab > 3 {
		d.tab = 0
	}
	ui.Column(c).Grow(1).MinWidth(0).Background(c.Theme().Background).Children(func() {
		ui.Row(c).Height(barHeight).Padding(0, 20).AlignItems(ui.Center).Gap(8).DragWindow().Children(func() {
			title := titles[d.tab]
			if d.operationsOpen {
				title = "Task history"
			}
			if d.diagnosticsOpen {
				title = "Diagnostics"
			}
			ui.Text(c, title).FontSize(16).FontWeight(650)
			ui.Spacer(c)
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
			if ui.Button(c, "Refresh").Disabled(d.busy).Clicked() {
				a.devTask(func(m *devplatform.Manager) (string, error) { return "Workspace refreshed.", nil })
			}
			if ui.Button(c, "Tasks").Disabled(d.busy).Clicked() {
				a.openOperations()
			}
			if ui.Button(c, "Diagnostics").Disabled(d.busy).Clicked() {
				d.operationsOpen = false
				a.runDevDiagnostics()
			}
		})
		ui.Row(c).Padding(0, 20, 12).Children(func() {
			if ui.Segmented(c, &d.tab, "Projects", "Software", "Databases", "HTTPS").Label("Development section").Changed() {
				d.detailScroll[d.tab].Y = 0
				d.diagnosticsOpen = false
				d.operationsOpen = false
			}
		})
		ui.Column(c).Height(1).Background(hairline(c.Theme()))
		if d.operationsOpen {
			a.devOperations(c)
		} else if d.diagnosticsOpen {
			ui.Scroll(c.Key("development-diagnostics")).TrackScroll(&d.diagnosticScroll).Grow(1).MinHeight(0).Padding(20, 24).Children(func() { a.devDiagnostics(c) })
		} else {
			ui.Row(c).Grow(1).MinHeight(0).AlignItems(ui.Stretch).Children(func() {
				ui.Column(c).Width(238).Shrink(0).Background(c.Theme().Surface).Children(func() {
					ui.Scroll(c.Key("dev-master-" + strconv.Itoa(d.tab))).Grow(1).MinHeight(0).Padding(12).Children(func() { ui.Column(c).Gap(8).Children(func() { a.devMaster(c) }) })
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
				if ui.PrimaryButton(c, "Save project").Disabled(d.busy || a.projectTaskRunning(d.id)).Clicked() {
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
			status := "Ready"
			if d.tab == 0 && !d.importing && d.folderErrors[d.id] != "" {
				status = "Folder unavailable"
			}
			if d.tab == 0 && d.exportRecovery != nil && ((d.importing && d.directory == d.exportRecoveryDirectory) || (!d.importing && d.draft.Directory == d.exportRecoveryDirectory)) {
				status = "Export recovery"
			}
			if d.tab == 0 && d.projectDirty() {
				status = "Unsaved changes"
			}
			if status == "Ready" && a.backgroundTasksRunning() {
				status = "Background tasks"
			}
			if d.busy {
				status = "Working"
			}
			if strings.Contains(d.output, "Error:") {
				status = "Needs attention"
			}
			ui.Text(c, status).FontSize(textCaption).FontWeight(600).TextColor(func() ui.Color {
				if status == "Needs attention" {
					return c.Theme().Danger
				}
				if status == "Export recovery" || status == "Folder unavailable" {
					return c.Theme().Warning
				}
				return c.Theme().Text
			}())
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
func devHeading(c *ui.Context, title, description string) {
	ui.Text(c, title).FontSize(22).FontWeight(650)
	ui.Text(c, description).FontSize(textSmall).TextColor(c.Theme().TextMuted).Margin(0, 0, 10)
}
func devDivider(c *ui.Context, title string) {
	ui.Column(c).Height(1).Background(hairline(c.Theme())).Margin(10, 0)
	ui.Text(c, title).FontSize(13).FontWeight(650)
}
func devListEntry(c *ui.Context, label, title, subtitle string, selected bool) bool {
	bg := ui.Transparent
	if selected {
		bg = c.Theme().Accent.Alpha(.12)
	}
	b := ui.ButtonBase(c).Label(label).Padding(10, 12).Radius(8).Background(bg)
	b.Children(func() {
		ui.Column(c).Grow(1).MinWidth(0).AlignItems(ui.Start).Gap(3).Children(func() {
			ui.Text(c, title).FontWeight(600).SingleLine().MinWidth(0)
			ui.Text(c, subtitle).FontSize(textCaption).TextColor(c.Theme().TextMuted).SingleLine().MinWidth(0)
		})
	})
	return b.Clicked()
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
		ui.Text(c, fmt.Sprintf("%d PROJECTS", len(d.state.Projects))).FontSize(textCaption).TextColor(c.Theme().TextMuted).Margin(4, 4, 8)
		if ui.PrimaryButton(c, "Import a project").Disabled(d.busy).Clicked() {
			d.importing = true
			d.commandWorkOpen = false
		}
		if len(d.state.Projects) == 0 {
			ui.Text(c, "Your projects live here. Add a folder to get started.").FontSize(textSmall).TextColor(c.Theme().TextMuted).Margin(12, 4)
		}
		for _, id := range sortedKeys(d.state.Projects) {
			p := d.state.Projects[id]
			status := d.statuses["project:"+id]
			if status == "" {
				status = "Configured"
			}
			if devListEntry(c, "Project "+p.Name, p.Name, fmt.Sprintf("%s · %d services", status, len(p.Services)), d.id == id && !d.importing) {
				d.selectProject(p)
				d.commandWorkOpen = false
				d.importing = false
			}
		}
	case 1:
		ui.Text(c, "DEVELOPMENT SOFTWARE").FontSize(textCaption).TextColor(c.Theme().TextMuted).Margin(4, 4, 8)
		for _, name := range []string{"php", "node", "mysql", "postgresql", "redis", "mongodb"} {
			n := len(d.packages(name))
			detail := "Available to install"
			if n > 0 {
				detail = fmt.Sprintf("%d versions installed", n)
			}
			if devListEntry(c, "Software "+name, softwareTitle(name), detail, d.software == name) {
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
		ui.Text(c, fmt.Sprintf("%d INSTANCES", len(d.state.Instances))).FontSize(textCaption).TextColor(c.Theme().TextMuted).Margin(4, 4, 8)
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
			if status == "" {
				status = "Configured"
			}
			if devListEntry(c, "Instance "+id, id, softwareTitle(i.Kind)+" · "+status, d.dbID == id) {
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
		ui.Text(c, "LOCAL WEBSITES").FontSize(textCaption).TextColor(c.Theme().TextMuted).Margin(4, 4, 8)
		if ui.PrimaryButton(c, "New HTTPS site").Disabled(d.busy).Clicked() {
			d.newSite()
		}
		for _, pid := range sortedKeys(d.state.Projects) {
			p := d.state.Projects[pid]
			for _, site := range p.Sites {
				if devListEntry(c, "Site "+site.Domain, site.Domain, p.Name, d.siteID == site.ID && d.siteProject == pid) {
					d.selectSite(site, pid)
				}
			}
		}
		for _, id := range sortedKeys(d.state.Sites) {
			site := d.state.Sites[id]
			if devListEntry(c, "Site "+site.Domain, site.Domain, "Independent site", d.siteID == id && d.siteProject == "") {
				d.selectSite(site, "")
			}
		}
	}
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
	if ui.Button(c, "Unfinished project commands").Disabled(d.busy).Clicked() {
		a.openProjectCommands()
		return
	}
	a.devProjectTasks(c)
	if d.id == "" || d.importing {
		devHeading(c, "Bring your project home", "Choose an existing folder. Importing never runs project scripts.")
		if ui.Button(c, "Choose project folder…").Disabled(d.busy).Clicked() {
			paths, e := mygo.Dialog.Open(mygo.OpenDialogOptions{Title: "Import project", Directory: true})
			if e != nil {
				d.output = e.Error()
			} else if len(paths) > 0 {
				d.directory = paths[0]
			}
		}
		devField(c, "Project folder", &d.directory, "/Users/me/project")
		if ui.Button(c, "Import project").Disabled(d.busy).Clicked() {
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
		a.devExportRecovery(c, d.directory)
		return
	}
	devHeading(c, d.draft.Name, "Configure, run, and open your local development environment.")
	if status := d.statuses["project:"+d.id]; status != "" {
		ui.Text(c, "Project status: "+status).FontSize(textSmall).FontWeight(600).TextColor(devStatusColor(c, status))
	}
	ui.Row(c).Gap(8).Children(func() {
		for _, action := range []string{"Start project", "Restart project", "Stop project"} {
			if ui.Button(c, action).Disabled(d.busy || a.projectTaskRunning(d.id) || (action != "Stop project" && (d.projectDirty() || len(d.draft.MissingLocalSettings()) > 0 || d.folderErrors[d.id] != ""))).Clicked() {
				kind := map[string]string{"Start project": "up", "Stop project": "down", "Restart project": "restart"}[action]
				a.runProjectAction(d.state.Projects[d.id], kind)
			}
		}
	})
	if d.projectDirty() {
		ui.Text(c, "Save your changes before starting or exporting this environment.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
	}
	if message := d.folderErrors[d.id]; message != "" {
		ui.Text(c, message).FontSize(textSmall).TextColor(c.Theme().Warning)
		if ui.Button(c, "Locate the project folder").Clicked() {
			d.projectTab = 0
			d.detailScroll[0].Y = 0
		}
	}
	if missing := d.draft.MissingLocalSettings(); len(missing) > 0 {
		ui.Text(c, fmt.Sprintf("Local setup required · %d items", len(missing))).FontSize(textSmall).FontWeight(600).TextColor(c.Theme().Warning)
		ui.Text(c, strings.Join(missing, "\n")).FontSize(textSmall).TextColor(c.Theme().TextMuted)
		if ui.Button(c, "Complete local setup").Disabled(d.busy).Clicked() && d.flushService() {
			a.devChooseLocalSetup()
		}
	}
	for _, site := range d.draft.Sites {
		a.devOpenSite(c, site)
	}
	if ui.Button(c, "Add project HTTPS site").Disabled(d.busy || d.projectDirty()).Clicked() {
		a.addProjectHTTPS()
	}
	if ui.Segmented(c, &d.projectTab, "Overview", "Services", "Environment", "Logs").Label("Project settings section").Changed() {
		d.detailScroll[0].Y = 0
	}
	switch d.projectTab {
	case 0:
		devDivider(c, "Project settings")
		devField(c, "Project name", &d.draft.Name, "")
		a.devProjectFolder(c)
		ui.Checkbox(c, &d.draft.Autostart, "Start this project at login")
		a.devRuntimeHints(c)
		devDivider(c, "Connected databases")
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
			if ui.Button(c, "Install project dependencies").Disabled(d.busy || a.projectTaskRunning(d.id) || d.projectDirty()).Clicked() {
				id := d.id
				a.startProjectOperation(d.state.Projects[id], "dependencies", func(m *devplatform.Manager) (string, error) { return m.Exec(m.Context, id, []string{"npm", "ci"}) })
			}
			ui.Text(c, "Runs npm ci in the saved project environment, including repository lifecycle scripts. Source and dependency changes remain after cancellation.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
		}
		devDivider(c, "Share this environment")
		ui.Text(c, "Export software versions and database declarations. Local paths, port numbers, secrets, and database contents stay on this Mac. Import assigns ports and lists values to complete.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
		if ui.Button(c, "Export environment").Disabled(d.busy || d.projectDirty()).Clicked() {
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
		a.devExportRecovery(c, d.draft.Directory)
		if ui.Button(c, "Prepare imported environment").Disabled(d.busy || a.projectTaskRunning(d.id) || d.projectDirty()).Clicked() {
			a.runProjectAction(d.state.Projects[d.id], "prepare")
		}
		ui.Text(c, "Preparing installs the locked software and initializes separate databases for this working copy. It does not run project scripts.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
		if ui.Button(c, "Remove project (keep source and database files)").Disabled(d.busy || a.projectTaskRunning(d.id)).Clicked() {
			a.runProjectAction(d.state.Projects[d.id], "remove")
		}
	case 1:
		devDivider(c, "Application services")
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
		devField(c, "Service name", &d.serviceID, "web")
		if devPackageChoice(c, "Runtime version", &d.servicePackage, d.packages(""), d.state.Packages, true) {
			d.noteRuntimeEdit(d.id, d.serviceOriginalID)
		}
		a.devLocalArguments(c)
		ui.Text(c, "Program and arguments — one item per line").FontSize(textSmall)
		ui.TextArea(c, &d.arguments).Lines(3, 7).Label("Program and arguments").Placeholder("npm\nrun\ndev")
		devField(c, "Service subfolder", &d.workdir, "Optional relative folder")
		devField(c, "Readiness check", &d.health, "tcp://127.0.0.1:3000")
		if port := devplatform.ReadinessPort(d.health); port > 0 {
			if ui.Button(c, "Check service readiness port").Disabled(d.busy).Clicked() {
				a.checkDevPort(port)
			}
			a.devPortResult(c, port)
			ui.Text(c, "A listener alone does not prove this service owns the readiness port. Change its command or environment and readiness address together when choosing a new application port.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
		}
		ui.Text(c, "Use process, TCP or HTTP(S). An empty check verifies only that the process starts, and is shown as Running rather than Healthy.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
		devField(c, "Readiness timeout (seconds)", &d.serviceReadinessTimeout, "30")
		devField(c, "Wait for services", &d.dependencies, "Comma separated service names")
		ui.Checkbox(c, &d.serviceRestart, "Restart this service after it exits")
		if d.serviceRestart {
			devField(c, "Restart throttle (seconds)", &d.serviceRestartThrottle, "10")
			ui.Text(c, "launchd limits repeated launches with this throttle. This is not a retry-count limit; stopping the project disables restarts.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
		}
		if status, ok := d.serviceStatuses[devplatform.ServiceStatusKey(d.id, d.serviceOriginalID)]; ok {
			ui.Text(c, "Service status: "+status.Status).FontSize(textSmall).FontWeight(600).TextColor(devStatusColor(c, status.Status))
			ui.Text(c, status.Message).FontSize(textSmall).TextColor(c.Theme().TextMuted)
			if status.PID > 0 {
				ui.Text(c, fmt.Sprintf("PID %d · %d launches", status.PID, status.Launches)).FontSize(textSmall).TextColor(c.Theme().TextMuted)
			}
			if status.LastExitCode != nil {
				ui.Text(c, fmt.Sprintf("Last exit code: %d", *status.LastExitCode)).FontSize(textSmall).TextColor(c.Theme().TextMuted)
			}
		}
		ui.Text(c, "Save project stores these fields and the service environment.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
		if ui.Button(c, "Remove service from project").Disabled(d.busy).Clicked() {
			for n, s := range d.draft.Services {
				if s.ID == d.serviceOriginalID {
					d.draft.Services = append(d.draft.Services[:n], d.draft.Services[n+1:]...)
					d.serviceID = ""
					break
				}
			}
		}

	case 2:
		devDivider(c, "Environment")
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
		if ui.Button(c, "Choose environment file…").Disabled(d.busy).Clicked() {
			paths, err := mygo.Dialog.Open(mygo.OpenDialogOptions{Title: "Choose a local environment file"})
			if err != nil {
				d.output = "Error: " + err.Error()
			} else if len(paths) > 0 {
				d.envFile = paths[0]
			}
		}
		ui.Text(c, "Environment variables — NAME=value per line").FontSize(textSmall)
		ui.TextArea(c, &d.environment).Lines(2, 5).Label("Environment variables")
		for _, svc := range d.draft.Services {
			if svc.ID == d.serviceOriginalID && len(svc.RequiredEnvironment) > 0 {
				ui.Text(c, "Required by this environment: "+strings.Join(svc.RequiredEnvironment, ", ")).FontSize(textSmall).TextColor(c.Theme().TextMuted)
			}
		}

		ui.Text(c, "Save project stores the environment for the selected service.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
	case 3:
		devDivider(c, "Service output")
		rotation := !d.state.LogRotationDisabled
		if ui.Checkbox(c, &rotation, "Rotate service logs while the app is closed").Changed() {
			enabled := rotation
			a.devTask(func(m *devplatform.Manager) (string, error) {
				return "Log rotation settings saved", m.SetLogRotation(m.Context, enabled)
			})
		}
		ui.Text(c, "Every minute, a user launch agent keeps two 5 MiB archives per service log. Busy logs can grow between checks.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
		if ui.Button(c, "Rotate logs now").Clicked() {
			a.devTask(func(m *devplatform.Manager) (string, error) { return "Logs rotated", m.RotateLogs(m.Context) })
		}
		var services []string
		for _, service := range d.draft.Services {
			services = append(services, service.ID)
		}
		devChoice(c, "Log service", &d.logService, services)
		if ui.Button(c, "Read service logs").Disabled(d.busy || d.logService == "").Clicked() {
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
	description := "The app downloads and manages verified binaries. Versions stay separate from your system."
	if d.software == "php" || d.software == "postgresql" {
		description = "Homebrew manages the software. Projects and databases keep their own settings and data."
	}
	devHeading(c, softwareTitle(d.software), description)
	a.devPackageTasks(c)
	if devChoice(c, "Software", &d.software, []string{"php", "node", "mysql", "postgresql", "redis", "mongodb"}) {
		d.runtimeTargetProject = ""
		d.runtimeTargetService = ""
		d.softwareContext = ""
		versions := devplatform.AvailableVersions(d.software)
		if len(versions) > 0 {
			d.version = versions[0]
		}
	}
	devChoice(c, "Version", &d.version, devplatform.AvailableVersions(d.software))
	devField(c, "Exact version", &d.version, "For example 22.14.0")
	if d.software == "php" || d.software == "postgresql" {
		ui.Text(c, "Choose a release series or exact Homebrew version. Review resolves the current patch and revision.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
		if d.software == "php" {
			if ui.Button(c, "Add shivammathur/php repository").Disabled(d.busy || a.packageTasksRunning()).Clicked() {
				a.devTask(func(m *devplatform.Manager) (string, error) {
					if err := m.AddPHPRepository(m.Context); err != nil {
						return "PHP repository setup did not complete.", err
					}
					return "PHP repository added. Review the version to install.", nil
				})
			}
		}
	} else {
		ui.Text(c, "Listed versions have integration tests. Other exact versions use available upstream archives.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
	}
	a.devPackageReview(c)
	for _, id := range sortedKeys(d.state.Packages) {
		p := d.state.Packages[id]
		if p.Name != d.software {
			continue
		}
		devDivider(c, "Installed version")
		ui.Text(c, "License: "+p.License).FontSize(textSmall).TextColor(c.Theme().TextMuted)
		ui.Row(c).Gap(8).Children(func() {
			ui.Text(c, softwareTitle(p.Name)+" · "+p.Version).Grow(1).MinWidth(0).SingleLine()
			ui.Spacer(c)
			removeLabel := "Uninstall " + p.Name + " " + p.Version
			if p.Formula != "" {
				removeLabel = "Remove from workspace " + p.Name + " " + p.Version
			}
			if ui.Button(c, removeLabel).Disabled(d.busy || a.packageTaskRunning(id)).Clicked() {
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
	if ui.Button(c, "Unfinished database tasks").Disabled(d.busy).Clicked() {
		a.openDatabaseWork()
		return
	}
	if ui.Button(c, "Unfinished initializations").Disabled(d.busy).Clicked() {
		a.openInitializations()
		return
	}
	a.devDatabaseTasks(c)
	if ui.Button(c, "Review pending data deletions").Disabled(d.busy).Clicked() {
		a.openInstanceDeletions()
		return
	}
	if d.dbKind == "" {
		d.dbKind = "postgresql"
		d.dbPort = "55432"
		d.persistent = true
	}
	if instance, exists := d.state.Instances[d.dbID]; exists && !d.dbNewDraft {
		devHeading(c, instance.ID, softwareTitle(instance.Kind)+" · 127.0.0.1:"+strconv.Itoa(instance.Port))
		ui.Row(c).Gap(8).Children(func() {
			for _, action := range []string{"Start instance", "Restart instance", "Stop instance"} {
				if ui.Button(c, action).Disabled(d.busy || a.databaseTaskRunning(instance.ID)).Clicked() {
					id := instance.ID
					verb := strings.TrimSuffix(strings.ToLower(action), " instance")
					a.startDatabaseOperation(instance, verb, func(m *devplatform.Manager) (string, error) {
						result := map[string]string{"start": "Started ", "stop": "Stopped ", "restart": "Restarted "}[verb] + id
						return result, m.InstanceAction(m.Context, id, verb)
					})
				}
			}
		})
		if ui.Button(c, "Show connection details").Disabled(d.busy).Clicked() {
			id := d.dbID
			a.devTask(func(m *devplatform.Manager) (string, error) { return m.Connection(id) })
		}
		if ui.Button(c, "Copy connection URI").Disabled(d.busy).Clicked() {
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
				return "Connection URI copied. Use the password copy button when the client asks.", e
			})
		}
		if ui.Button(c, "Copy database password").Disabled(d.busy).Clicked() {
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
		if ui.Button(c, "Read instance logs").Disabled(d.busy).Clicked() {
			id := "db-" + instance.ID
			d.showResult = true
			a.devTask(func(m *devplatform.Manager) (string, error) { return m.Logs(id) })
		}

		devDivider(c, "Projects using this instance")
		linked := false
		for _, pid := range sortedKeys(d.state.Projects) {
			p := d.state.Projects[pid]
			for _, ref := range p.Instances {
				if ref == instance.ID {
					linked = true
					ui.Text(c, p.Name+" · "+d.statuses["project:"+pid]).FontSize(textSmall).SingleLine().Tooltip(p.Directory)
					break
				}
			}
		}
		if !linked {
			ui.Text(c, "No project uses this instance.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
		}
		a.devDatabasePort(c, instance)
		devDivider(c, "Data and backups")
	} else {
		d.dbNewDraft = true
		devHeading(c, "Create a database", "A separate version, port, and data folder for every instance.")
		if packages := d.packages(d.dbKind); len(packages) == 0 {
			ui.Text(c, "Install "+softwareTitle(d.dbKind)+" in Software before creating an instance.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
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
			ui.Text(c, "Database files are kept across restarts and removal of the management record.")
		}
		if ui.PrimaryButton(c, "Create instance").Disabled(d.busy || d.databaseTasks[d.dbID] != nil && d.databaseTasks[d.dbID].Running).Clicked() {
			port, e := strconv.Atoi(d.dbPort)
			if e != nil {
				d.output = "Error: Enter a valid port"
			} else {
				i := devplatform.Instance{ID: d.dbID, Kind: d.dbKind, Package: d.dbPackage, Port: port, Persistent: d.persistent}
				d.dbNewDraft = true
				a.startDatabaseTask(i)
			}
		}
		return
	}
	if d.dbKind == "redis" {
		devField(c, "Database number (0–15)", &d.dbName, "0")
	} else {
		devField(c, "Database name", &d.dbName, "app")
	}
	if d.dbKind != "redis" {
		if ui.Button(c, "Create database").Disabled(d.busy || a.databaseTaskRunning(d.dbID)).Clicked() {
			id, name := d.dbID, d.dbName
			a.startDatabaseOperation(d.state.Instances[id], "create database", func(m *devplatform.Manager) (string, error) {
				return "Created database " + name, m.CreateDatabase(m.Context, id, name)
			})
		}
		devField(c, "Database user", &d.dbUser, "app_user")
		ui.Field(c, "New user password", func() { ui.TextInput(c, &d.dbPassword).Password() })
		if ui.Button(c, "Create database user").Disabled(d.busy || a.databaseTaskRunning(d.dbID)).Clicked() {
			id, name, password, db := d.dbID, d.dbUser, d.dbPassword, d.dbName
			d.dbPassword = ""
			a.startDatabaseOperation(d.state.Instances[id], "create user", func(m *devplatform.Manager) (string, error) {
				return "Created user " + name, m.CreateUser(m.Context, id, name, password, db)
			})
		}
	}
	if ui.Button(c, "Back up database").Disabled(d.busy || a.databaseTaskRunning(d.dbID)).Clicked() {
		id, name := d.dbID, d.dbName
		a.startDatabaseOperation(d.state.Instances[id], "backup", func(m *devplatform.Manager) (string, error) { return m.Backup(m.Context, id, name) })
	}
	devField(c, "Backup file", &d.backupPath, "Select a backup file")
	if ui.Button(c, "Choose backup file…").Clicked() {
		paths, e := mygo.Dialog.Open(mygo.OpenDialogOptions{Title: "Restore database backup"})
		if e != nil {
			d.output = e.Error()
		} else if len(paths) > 0 {
			d.backupPath = paths[0]
		}
	}
	if ui.Button(c, "Restore into new database").Disabled(d.busy || a.databaseTaskRunning(d.dbID)).Clicked() {
		id, name, path := d.dbID, d.dbName, d.backupPath
		a.startDatabaseOperation(d.state.Instances[id], "restore", func(m *devplatform.Manager) (string, error) {
			return "Restored into " + name, m.Restore(m.Context, id, name, path)
		})
	}
	if ui.Button(c, "Review permanent data deletion").Disabled(d.busy || a.databaseTaskRunning(d.dbID)).Clicked() {
		a.previewInstanceDeletion(d.dbID)
	}
	if ui.Button(c, "Remove instance (keep data files)").Disabled(d.busy || a.databaseTaskRunning(d.dbID)).Clicked() {
		id := d.dbID
		a.startDatabaseOperation(d.state.Instances[id], "remove", func(m *devplatform.Manager) (string, error) {
			return "Removed instance", m.InstanceAction(m.Context, id, "remove")
		})
	}
}
func (a *app) devHTTPS(c *ui.Context) {
	d := &a.dev
	if d.authorityOpen {
		a.devAuthority(c)
		return
	}
	devHeading(c, "A real address for localhost", "Use a memorable .test domain with trusted HTTPS.")
	if ui.Button(c, "Local certificates").Disabled(d.busy).Clicked() {
		d.authorityOpen = true
		d.detailScroll[3].Y = 0
		a.authorityTask(nil)
	}
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
		ui.Text(c, "Zero disables that timeout. Streaming bodies and WebSockets are not limited by the response header timeout. Forwarded headers describe the original HTTPS request.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
	}
	ui.Text(c, "If your dev server checks allowed hosts or HMR origins, allow this local HTTPS domain in its settings.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
	if ui.PrimaryButton(c, "Save HTTPS site").Disabled(d.busy).Clicked() {
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
		if ui.Button(c, "Delete HTTPS site").Disabled(d.busy).Clicked() {
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
	ui.Text(c, "Local CA trust allows this app to issue certificates trusted by this Mac. Hosts and system trust changes show the macOS authorization dialog.")
	for _, action := range []string{"Apply proxy", "Restart HTTPS proxy", "Stop HTTPS proxy", "Configure local domains", "Trust local CA", "Remove CA trust", "Diagnose HTTPS"} {
		if ui.Button(c, action).Disabled(d.busy).Clicked() {
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
}

func (a *app) devOpenSite(c *ui.Context, site devplatform.Site) {
	if site.NeedsReview {
		ui.Text(c, "Review the upstream address and save this site before applying HTTPS.").FontSize(textSmall).TextColor(c.Theme().Warning)
	}
	if ui.Button(c, "Open "+site.URL()).Disabled(a.dev.busy || site.Disabled || site.NeedsReview).Clicked() {
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

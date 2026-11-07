package main

import (
	"fmt"
	"github.com/egoist/mygo/ui"
	"image/png"
	"macos-service-manager/internal/devplatform"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func designFixture(t *testing.T) *app {
	t.Helper()
	a := newAppWith(nil, nil, t.TempDir())
	a.nav = "development"
	a.copy = func(string) {}
	a.reveal = func(string) {}
	a.dev.loaded = true
	state := devplatform.State{SchemaVersion: 1, Projects: map[string]devplatform.Project{}, Packages: map[string]devplatform.Package{}, Instances: map[string]devplatform.Instance{}, Sites: map[string]devplatform.Site{}}
	for _, pair := range [][2]string{{"php", "8.5.12"}, {"node", "22.14.0"}, {"mysql", "8.4.4"}, {"postgresql", "17-2.9.6"}, {"redis", "7.4.7"}, {"mongodb", "8.0.5"}} {
		p := devplatform.Package{Name: pair[0], Version: pair[1], Arch: "arm64", License: "See the license included with the package"}
		if p.Name == "postgresql" {
			p.Arch = "universal"
		}
		state.Packages[p.ID()] = p
	}
	p := devplatform.Project{SchemaVersion: 1, ID: "atlas", Name: "Atlas API", Directory: "/Users/developer/Projects/atlas", Services: []devplatform.Service{{ID: "api", Package: "node@22.14.0-arm64", Command: []string{"npm", "run", "dev"}, Health: "tcp://127.0.0.1:3000"}}, Instances: []string{"atlas-postgres"}, Sites: []devplatform.Site{{ID: "api", Domain: "atlas.test", Upstream: "http://127.0.0.1:3000", Port: 443}}}
	state.Projects[p.ID] = p
	state.Projects["storefront"] = devplatform.Project{ID: "storefront", Name: "Storefront", Directory: "/Users/developer/Projects/storefront"}
	state.Projects["unicode"] = devplatform.Project{ID: "unicode", Name: "开发环境 · " + strings.Repeat("a-long-project-name-", 8), Directory: "/Users/developer/Projects/worst-case"}
	state.Instances["atlas-postgres"] = devplatform.Instance{ID: "atlas-postgres", Kind: "postgresql", Package: "postgresql@17-2.9.6-universal", Port: 5432, Persistent: true}
	state.Instances["cache"] = devplatform.Instance{ID: "cache", Kind: "redis", Package: "redis@7.4.7-arm64", Port: 6379, Persistent: true}
	a.dev.state = state
	a.dev.statuses = map[string]string{"project:atlas": "Healthy", "project:storefront": "Stopped", "project:unicode": "Stopped", "instance:atlas-postgres": "Running", "instance:cache": "Stopped"}
	a.dev.serviceStatuses = map[string]devplatform.ServiceStatus{devplatform.ServiceStatusKey("atlas", "api"): {Status: "Healthy", PID: 456, Launches: 1, Message: "Configured readiness check passed."}}
	a.dev.selectProject(p)
	a.dev.software = "node"
	a.dev.version = "22.14.0"
	a.dev.dbID = "atlas-postgres"
	a.dev.dbKind = "postgresql"
	a.dev.dbPackage = "postgresql@17-2.9.6-universal"
	a.dev.dbPort = "5432"
	a.dev.selectSite(p.Sites[0], p.ID)
	a.dev.output = "Workspace ready"
	a.dev.logText = "api.log:\nServer listening on 127.0.0.1:3000\nGET / 200\nWebSocket client connected\n"
	return a
}
func TestDevelopmentDesignLayouts(t *testing.T) {
	pages := []struct {
		name            string
		tab, projectTab int
	}{{"project-command-running", 0, 0}, {"project-command-complete", 0, 0}, {"project-command-inspection", 0, 0}, {"project-command-caller", 0, 0}, {"project-command-background", 0, 0}, {"database-work-running", 2, 0}, {"database-work-complete", 2, 0}, {"database-work-inspection", 2, 0}, {"database-work-caller", 2, 0}, {"projects", 0, 0}, {"folder-missing", 0, 0}, {"folder-review", 0, 0}, {"project-https", 3, 0}, {"port-conflict", 3, 0}, {"database-port-conflict", 2, 0}, {"service-port-check", 0, 1}, {"database-port-repair", 2, 0}, {"tasks-interrupted", 1, 0}, {"tasks-restore", 2, 0}, {"database-delete", 2, 0}, {"database-delete-blocked", 2, 0}, {"database-delete-pending", 2, 0}, {"database-delete-restore", 2, 0}, {"project-starting", 0, 0}, {"project-stopping", 0, 0}, {"project-restarting", 0, 0}, {"project-shared-busy", 0, 0}, {"project-background-quit", 0, 0}, {"package-background", 1, 0}, {"package-busy", 1, 0}, {"package-background-quit", 1, 0}, {"database-background", 2, 0}, {"database-backup", 2, 0}, {"database-starting", 2, 0}, {"database-stopping", 2, 0}, {"database-restarting", 2, 0}, {"database-restore", 2, 0}, {"initialization-incomplete", 2, 0}, {"initialization-published", 2, 0}, {"initialization-archived", 2, 0}, {"database-background-quit", 2, 0}, {"ca-status", 3, 0}, {"ca-prepared", 3, 0}, {"ca-saved", 3, 0}, {"runtime-declarations", 0, 0}, {"services", 0, 1}, {"health-settings", 0, 1}, {"environment", 0, 2}, {"local-settings", 0, 1}, {"local-environment", 0, 2}, {"export-recovery", 0, 0}, {"export-conflict", 0, 0}, {"logs", 0, 3}, {"software", 1, 0}, {"software-details", 1, 0}, {"package-verified", 1, 0}, {"package-repair", 1, 0}, {"databases", 2, 0}, {"https", 3, 0}, {"proxy-settings", 3, 0}, {"diagnostics", 3, 0}, {"quit-scope", 0, 0}, {"quit-failure", 0, 0}}
	for _, size := range [][2]int{{1120, 720}, {900, 520}} {
		for _, page := range pages {
			for _, dark := range []bool{false, true} {
				name := fmt.Sprintf("%s-%dx%d-dark-%t", page.name, size[0], size[1], dark)
				t.Run(name, func(t *testing.T) {
					a := designFixture(t)
					a.dev.tab = page.tab
					a.dev.projectTab = page.projectTab
					if strings.HasPrefix(page.name, "project-command-") {
						if page.name == "project-command-background" {
							p := a.dev.state.Projects["atlas"]
							task := &projectTask{Project: p, Kind: "dependencies", Progress: "Installing project dependencies for Atlas API", Running: true}
							a.dev.projectTasks = map[string]*projectTask{p.ID: task}
							a.dev.lastProjectTask = task
						} else {
							r := devplatform.ProjectCommandInfo{ID: "0123456789abcdef", ProjectID: "atlas", Status: "Running without caller", Running: true, CanResolve: true, HasOutput: true, Detail: "The caller exited but the recorded command is still running. Wait and refresh, or explicitly stop it. Changes already written are retained."}
							if page.name == "project-command-complete" {
								r.Status = "Succeeded"
								r.Running = false
								r.Detail = "The worker finished. Finish review to release its resources. The original caller's interrupted history is kept."
							}
							if page.name == "project-command-inspection" {
								r.Status = "Needs inspection"
								r.CanResolve = false
								r.Detail = "Command ownership or its live state could not be verified. No process will be stopped."
							}
							if page.name == "project-command-caller" {
								r.Status = "Running"
								r.CallerActive = true
								r.CanResolve = false
								r.Detail = "The caller still owns this command. Cancel it there or wait."
							}
							a.dev.commandWorkOpen = true
							a.dev.commandWork = []devplatform.ProjectCommandInfo{r}
						}
					}
					if strings.HasPrefix(page.name, "database-work-") {
						r := devplatform.DatabaseWorkInfo{ID: "0123456789abcdef", InstanceID: "atlas-postgres", Kind: "backup", Database: "app", Status: "Running without caller", Running: true, CanResolve: true, Detail: "The caller exited, but the recorded worker is still running. Refresh to wait, or explicitly stop it. Existing database changes are not undone.", Output: "/Users/developer/Library/Application Support/ServiceManager/Development/backups/atlas-postgres-app-0123456789abcdef.dump.partial"}
						if page.name == "database-work-complete" {
							r.Status = "Succeeded"
							r.Running = false
							r.Detail = "The worker finished. Finish review to release this instance. Database and output files are retained."
							r.Output = strings.TrimSuffix(r.Output, ".partial")
						}
						if page.name == "database-work-inspection" {
							r.Status = "Needs inspection"
							r.CanResolve = false
							r.Detail = "Worker ownership or its live state could not be verified. No process will be stopped."
						}
						if page.name == "database-work-caller" {
							r.Status = "Running"
							r.CallerActive = true
							r.CanResolve = false
							r.Detail = "The caller still owns this task. Cancel it there or wait for completion."
						}
						a.dev.databaseWorkOpen = true
						a.dev.databaseWork = []devplatform.DatabaseWorkInfo{r}
					}
					if page.name == "tasks-interrupted" || page.name == "tasks-restore" {
						a.dev.operationsOpen = true
						r := devplatform.OperationRecord{Schema: 1, ID: "0123456789abcdef", Request: devplatform.OperationRequest{Kind: "install", Resource: "node@22.14.0-arm64", Name: "node", Version: "22.14.0"}, Status: "Interrupted", Phase: "Extracting", StartedAt: time.Date(2026, 11, 7, 14, 0, 0, 0, time.UTC), UpdatedAt: time.Date(2026, 11, 7, 14, 0, 5, 0, time.UTC), Result: "The caller exited without a recorded result. Review current state before retrying.", Revision: "example"}
						if page.name == "tasks-restore" {
							r.Request = devplatform.OperationRequest{Kind: "restore", Resource: "atlas-postgres", Database: "restored_app", SourcePath: "/Users/developer/Backups/app.dump"}
							r.Phase = "Restoring"
							a.dev.operationScroll.Y = 180
						}
						a.dev.operations = []devplatform.OperationRecord{r}
						a.dev.operationID = r.ID
					}
					if page.name == "database-delete" || page.name == "database-delete-blocked" {
						a.dev.statuses["instance:atlas-postgres"] = "Stopped"
						p := devplatform.InstanceDeletionPreview{InstanceID: "atlas-postgres", Kind: "postgresql", Package: "postgresql@17-2.9.6-universal", Path: "/Users/developer/Library/Application Support/ServiceManager/Development/instances/atlas-postgres", Entries: 1248, Bytes: 32 << 20, CanDelete: true, Detail: "Permanently removes this instance record, data, local configuration and credentials.", Fingerprint: "example"}
						if page.name == "database-delete-blocked" {
							p.CanDelete = false
							p.Detail = "Unlink this database from every project before deleting its data."
							p.References = []string{"Atlas API (atlas)"}
						}
						a.dev.instanceDeletionPreview = &p
						a.dev.detailScroll[2].Y = 160
					}
					if page.name == "database-delete-pending" || page.name == "database-delete-restore" {
						a.dev.statuses["instance:atlas-postgres"] = "Stopped"
						a.dev.deletionsOpen = true
						r := devplatform.InstanceDeletionInfo{ID: "0123456789abcdef", InstanceID: "atlas-postgres", Kind: "postgresql", OriginalPath: "/Users/developer/Library/Application Support/ServiceManager/Development/instances/atlas-postgres", StagedPath: "/Users/developer/Library/Application Support/ServiceManager/Development/deletions/0123456789abcdef/data", Status: "Ready to erase", Detail: "Instance removal committed. Finish erases only the staged data. Other instances and backups are retained.", Entries: 1248, Bytes: 32 << 20, CanErase: true, Revision: "example"}
						if page.name == "database-delete-restore" {
							r.Status = "Restore required"
							r.Detail = "Data moved but instance removal did not commit. Restore the data folder; do not erase it."
							r.CanErase = false
							r.CanRestore = true
						}
						a.dev.instanceDeletions = []devplatform.InstanceDeletionInfo{r}
						if page.name == "database-delete-pending" {
							delete(a.dev.state.Instances, "atlas-postgres")
							a.dev.dbID = ""
							a.dev.detailScroll[2].Y = 400
						} else {
							a.dev.detailScroll[2].Y = 250
						}
					}
					if page.name == "project-starting" || page.name == "project-stopping" || page.name == "project-restarting" || page.name == "project-shared-busy" || page.name == "project-background-quit" {
						p := a.dev.state.Projects["atlas"]
						kind, progress := "up", "Starting api"
						if page.name == "project-stopping" {
							kind, progress = "down", "Stopping api"
						}
						if page.name == "project-restarting" {
							kind, progress = "restart", "Restarting project atlas"
						}
						task := &projectTask{Project: p, Kind: kind, Progress: progress, Running: true}
						if page.name == "project-shared-busy" {
							task.Running = false
							task.Result = "Project operation did not complete.\nError: database instance atlas-postgres is reserved by another operation; finish or cancel it in its caller and retry"
							a.dev.output = task.Result
							a.dev.statuses["project:atlas"] = "Stopped"
						}
						a.dev.projectTasks = map[string]*projectTask{p.ID: task}
						a.dev.lastProjectTask = task
						a.applyProjectTaskPhases()
						if page.name == "project-background-quit" {
							a.quit.open = true
						}
					}
					if page.name == "package-background" || page.name == "package-busy" || page.name == "package-background-quit" {
						pkg := devplatform.Package{Name: "node", Version: "20.19.0", Arch: "arm64"}
						task := &packageTask{Package: pkg, Kind: "install", Running: true, Progress: "Downloading node@20.19.0-arm64 · 8.0 / 30.0 MiB"}
						if page.name == "package-busy" {
							task.Running = false
							task.Result = "Install did not complete.\nError: another caller is preparing this package."
							a.dev.output = task.Result
							a.dev.packageContention = &devplatform.PackageBusyError{PackageID: pkg.ID()}
						}
						a.dev.packageTasks = map[string]*packageTask{pkg.ID(): task}
						a.dev.lastPackageTask = task
						a.dev.software = pkg.Name
						a.dev.version = pkg.Version
						if page.name == "package-background-quit" {
							a.quit.open = true
						}
					}
					if page.name == "database-background" || page.name == "database-background-quit" {
						i := devplatform.Instance{ID: "new-database", Kind: "postgresql", Package: "postgresql@17-2.9.6-universal", Port: 55432, Persistent: true}
						task := &databaseTask{Instance: i, Running: true, Progress: "Initializing database new-database"}
						a.dev.databaseTasks = map[string]*databaseTask{i.ID: task}
						a.dev.lastDatabaseTask = task
						a.dev.dbID = i.ID
						a.dev.dbKind = i.Kind
						a.dev.dbPackage = i.Package
						a.dev.dbPort = "55432"
						a.dev.dbNewDraft = true
						if page.name == "database-background-quit" {
							a.quit.open = true
						}
					}
					if page.name == "database-starting" || page.name == "database-stopping" || page.name == "database-restarting" {
						i := a.dev.state.Instances["atlas-postgres"]
						kind, progress := "start", "Starting database atlas-postgres"
						if page.name == "database-stopping" {
							kind, progress = "stop", "Stopping database atlas-postgres"
						}
						if page.name == "database-restarting" {
							kind, progress = "restart", "Restarting database atlas-postgres"
						}
						task := &databaseTask{Instance: i, Kind: kind, Running: true, Progress: progress}
						a.dev.databaseTasks = map[string]*databaseTask{i.ID: task}
						a.dev.lastDatabaseTask = task
						a.applyDatabaseTaskPhases()
						a.dev.detailScroll[2].Y = 70
					}
					if page.name == "database-backup" || page.name == "database-restore" {
						i := a.dev.state.Instances["atlas-postgres"]
						kind, progress := "backup", "Backing up atlas-postgres"
						if page.name == "database-restore" {
							kind, progress = "restore", "Restoring into restored_app"
						}
						task := &databaseTask{Instance: i, Kind: kind, Running: true, Progress: progress}
						a.dev.databaseTasks = map[string]*databaseTask{i.ID: task}
						a.dev.lastDatabaseTask = task
						a.dev.dbName = "restored_app"
						a.dev.backupPath = "/Users/developer/Backups/app.dump"
						a.dev.detailScroll[2].Y = 70
					}
					if page.name == "initialization-incomplete" || page.name == "initialization-published" || page.name == "initialization-archived" {
						r := devplatform.InstanceInitializationInfo{ID: "0123456789abcdef", InstanceID: "new-database", Kind: "postgresql", Package: "postgresql@17-2.9.6-universal", Port: 55432, Path: "/Users/developer/Library/Application Support/ServiceManager/Development/initializations/0123456789abcdef/data", Status: "Incomplete", CanResolve: true, Detail: "Stop the recorded initialization jobs and archive the prepared data. Existing files are retained.", Revision: "example"}
						if page.name == "initialization-published" {
							r.Status = "Published"
							r.Path = "/Users/developer/Library/Application Support/ServiceManager/Development/instances/new-database"
							r.Detail = "This data is registered. Finish clears only the initialization record; the database remains intact."
						}
						if page.name == "initialization-archived" {
							r.Status = "Archived"
							r.Path = "/Users/developer/Library/Application Support/ServiceManager/Development/retained-initializations/0123456789abcdef"
							r.Detail = "The prepared data is already archived. Finish clears only the initialization record and keeps the archive."
						}
						a.dev.initializationsOpen = true
						a.dev.initializations = []devplatform.InstanceInitializationInfo{r}
						a.dev.detailScroll[2].Y = 170
					}
					if page.name == "folder-missing" {
						a.dev.folderErrors = map[string]string{"atlas": "Project folder is unavailable. Choose its current location in project settings."}
						a.dev.statuses["project:atlas"] = "Stopped"
					}
					if page.name == "folder-review" {
						p := a.dev.state.Projects["atlas"].Clone()
						p.Services[0].EnvFile = p.Directory + "/.env.local"
						a.dev.state.Projects["atlas"] = p
						a.dev.selectProject(p)
						a.dev.draft.Directory = "/Users/developer/Code/atlas"
						if err := a.dev.applyService(); err != nil {
							t.Fatal(err)
						}
						draft := a.dev.draft.Clone()
						a.dev.folderDraft = &draft
						a.dev.folderPreview = &devplatform.ProjectFolderPreview{ProjectID: p.ID, ProjectName: p.Name, From: p.Directory, To: draft.Directory, Fingerprint: "preview", Changes: []devplatform.FolderPathChange{{Service: "api", Field: "Environment file", From: p.Services[0].EnvFile, To: draft.Directory + "/.env.local"}}}
						a.dev.detailScroll[0].Y = 450
						a.dev.statuses["project:atlas"] = "Stopped"
					}
					if page.name == "project-https" {
						a.addProjectHTTPS()
					}
					if page.name == "port-conflict" || page.name == "database-port-conflict" || page.name == "service-port-check" || page.name == "database-port-repair" {
						port := 443
						if page.name == "database-port-conflict" || page.name == "database-port-repair" {
							port = 5432
							a.dev.statuses["instance:atlas-postgres"] = "Stopped"
							a.dev.detailScroll[2].Y = 460
						}
						if page.name == "service-port-check" {
							port = 3000
							a.dev.statuses["project:atlas"] = "Not ready"
							a.dev.detailScroll[0].Y = 650
						}
						if page.name == "port-conflict" {
							a.dev.detailScroll[3].Y = 180
						}
						r := devplatform.PortInspection{Port: port, Status: "In use", CheckedAt: time.Date(2026, 11, 7, 14, 0, 0, 0, time.UTC), Listeners: []devplatform.PortListener{{PID: 2374, UID: 501, Name: "another-development-server", Address: fmt.Sprintf("127.0.0.1:%d", port)}}, Detail: "Listeners reported for this TCP port. Names and PIDs do not prove app ownership. Stop an external service in its own app, or choose another port."}
						if page.name == "database-port-repair" {
							r.Status = "Available"
							r.Listeners = nil
							r.Detail = "IPv4 loopback was available at this check. Saving a suggestion does not reserve it."
							a.dev.detailScroll[2].Y = 650
						}
						a.dev.portChecks = map[int]devplatform.PortInspection{port: r}
					}
					if page.name == "ca-status" || page.name == "ca-prepared" || page.name == "ca-saved" {
						a.dev.authorityOpen = true
						info := devplatform.AuthorityInfo{ID: "example-active", Name: "Service Manager Local Development CA", Status: "Ready", Trust: "Trusted", Fingerprint: strings.Repeat("A", 64), Usable: true, Detail: "Certificate and key match and are within their validity period.", NotBefore: time.Date(2026, 11, 7, 0, 0, 0, 0, time.UTC), NotAfter: time.Date(2031, 11, 7, 0, 0, 0, 0, time.UTC)}
						status := devplatform.AuthorityStatus{CheckedAt: time.Date(2026, 11, 7, 14, 0, 0, 0, time.UTC), Active: info, Revision: "example-review"}
						if page.name == "ca-prepared" {
							status.Active.Status = "Expired"
							status.Active.Usable = false
							status.Active.Trust = "Not trusted"
							status.Active.NotAfter = time.Date(2026, 11, 6, 0, 0, 0, 0, time.UTC)
							status.Active.Detail = "Prepare, trust and activate a replacement authority."
							prepared := info
							prepared.ID = "example-prepared"
							prepared.Fingerprint = strings.Repeat("B", 64)
							prepared.Trust = "Not trusted"
							status.Prepared = &prepared
							a.dev.detailScroll[3].Y = 680
						}
						if page.name == "ca-saved" {
							previous := info
							previous.ID = "example-previous"
							previous.Fingerprint = strings.Repeat("C", 64)
							previous.Trust = "Not checked"
							status.Archives = []devplatform.AuthorityInfo{previous}
							status.Previous = &previous
							a.dev.detailScroll[3].Y = 850
						}
						a.dev.authorityStatus = &status
					}
					if page.name == "package-verified" || page.name == "package-repair" {
						id := "node@22.14.0-arm64"
						baseline := strings.Repeat("b", 64)
						a.dev.state.PackageIntegrity = map[string]string{id: baseline}
						r := devplatform.PackageIntegrityResult{Status: "Verified", Files: 2148, CheckedAt: time.Date(2026, 11, 7, 10, 0, 0, 0, time.UTC), Baseline: baseline}
						if page.name == "package-repair" {
							r.Status = "Needs repair"
							r.Issues = []string{"Changed: lib/node_modules/npm/package.json", "Missing: bin/node"}
						}
						a.dev.packageChecks = map[string]devplatform.PackageIntegrityResult{id: r}
						a.dev.detailScroll[1].Y = 380
					}
					if page.name == "quit-scope" || page.name == "quit-failure" {
						a.quit.open = true
						a.quit.stopProxy = true
						a.quit.plan = &devplatform.ShutdownPlan{Fingerprint: "example", Projects: []devplatform.ShutdownResource{{ID: "atlas", Name: "Atlas API"}, {ID: "storefront", Name: "Storefront"}}, Instances: []devplatform.ShutdownResource{{ID: "atlas-postgres", Name: "postgresql"}, {ID: "cache", Name: "redis"}}, PrivilegedProxy: true}
						if page.name == "quit-failure" {
							a.quit.plan = nil
							a.quit.err = "Could not stop Atlas API. Some services may already have stopped. Review the scope to retry, or quit keeping what remains."
						}
					}
					if page.name == "local-settings" || page.name == "local-environment" {
						p := a.dev.state.Projects["atlas"].Clone()
						p.Services[0].Command = []string{"node", "server.js", "<local argument 3>", "<local argument 4>"}
						p.Services[0].LocalArguments = []devplatform.LocalArgument{{Index: 2, Kind: "path", Label: "Local configuration file", Prefix: "--config="}, {Index: 3, Kind: "secret", Label: "Private API token", Prefix: "--token=", Value: "design-preview-token"}}
						p.Services[0].RequiredEnvironment = []string{"PORT", "API_TOKEN"}
						p.Services[0].Environment = map[string]string{"PORT": "49172"}
						a.dev.state.Projects["atlas"] = p
						a.dev.selectProject(p)
						a.dev.projectTab = page.projectTab
						a.dev.statuses["project:atlas"] = "Stopped"
						if page.name == "local-settings" {
							a.dev.detailScroll[0].Y = 530
						} else {
							a.dev.detailScroll[0].Y = 300
						}
					}
					if page.name == "export-recovery" || page.name == "export-conflict" {
						a.dev.importing = true
						a.dev.directory = a.dev.draft.Directory
						a.dev.exportRecoveryDirectory = a.dev.directory
						a.dev.exportRecovery = &devplatform.ExportRecovery{OperationID: "6c2e45bd701a893f", ProjectName: "Atlas API", Services: 1, Databases: 1, Sites: 1, PackageIDs: []string{"node@22.14.0-arm64", "postgresql@17-2.9.6-universal"}, Directory: a.dev.directory, CreatedAt: time.Date(2026, 11, 7, 11, 0, 0, 0, time.UTC), Phase: "Partially saved", CanResume: true, Fingerprint: "preview", Files: []devplatform.ExportFileState{{Name: "devmanager.lock.json", Status: "New file saved"}, {Name: "devmanager.json", Status: "Previous file"}}}
						if page.name == "export-conflict" {
							a.dev.exportRecovery.CanResume = false
							a.dev.exportRecovery.Phase = "Review file changes"
							a.dev.exportRecovery.Files[1].Status = "Changed after export began"
						}
						a.dev.detailScroll[0].Y = 450
					}
					if page.name == "health-settings" {
						p := a.dev.state.Projects["atlas"].Clone()
						p.Services[0].Restart = true
						p.Services[0].ReadinessTimeoutSeconds = 15
						a.dev.state.Projects["atlas"] = p
						a.dev.selectProject(p)
						a.dev.projectTab = 1
						a.dev.detailScroll[0].Y = 650
						a.dev.statuses["project:atlas"] = "Not ready"
						a.dev.serviceStatuses[devplatform.ServiceStatusKey("atlas", "api")] = devplatform.ServiceStatus{Status: "Not ready", PID: 456, Launches: 1, Message: "Process is running but the readiness check failed."}
					}
					if page.name == "diagnostics" {
						a.showDevDiagnostics(devplatform.DiagnosticReport{SchemaVersion: 1, GeneratedAt: time.Date(2026, 11, 7, 10, 0, 0, 0, time.UTC), Checks: []devplatform.DiagnosticCheck{
							{Layer: "DNS", Resource: "atlas.test", Status: "Failed", Summary: "Domain is unresolved or includes an address other than 127.0.0.1.", Remedy: "Use Configure local domains; remove conflicting DNS or hosts entries, then retry."},
							{Layer: "Readiness", Resource: "Project Atlas API / api", Status: "OK", Summary: "Process is running and its readiness check passed."},
							{Layer: "Proxy TLS", Resource: "atlas.test", Status: "Failed", Summary: "Local HTTPS listener or certificate could not be verified.", Remedy: "Apply proxy, verify CA trust and the HTTPS port, then run diagnostics again."},
						}})
					}
					if page.name == "proxy-settings" {
						a.dev.siteAdvanced = true
						a.dev.detailScroll[3].Y = 400
					}
					if page.name == "runtime-declarations" {
						a.dev.draft.RuntimeHints = []devplatform.RuntimeHint{{Name: "node", Version: "22.14.0", Source: ".nvmrc"}, {Name: "node", Version: "20.19.0", Source: ".node-version"}}
						saved := a.dev.state.Projects[a.dev.id]
						saved.RuntimeHints = a.dev.draft.RuntimeHints
						a.dev.state.Projects[a.dev.id] = saved
						a.dev.detailScroll[0].Y = 310
					}
					if page.name == "software-details" {
						pkg := a.dev.state.Packages["node@22.14.0-arm64"]
						pkg.URL = "https://nodejs.org/dist/v22.14.0/node-v22.14.0-darwin-arm64.tar.gz"
						pkg.SHA256 = strings.Repeat("a", 64)
						info := devplatform.PackageDetails{Package: pkg, ArchiveBytes: 30 << 20, Cached: true, Compatible: true, Installed: true, Compatibility: "Compatible architecture", Location: "~/Library/Application Support/ServiceManager/Development/packages/node@22.14.0-arm64", References: []string{"Project: Atlas API"}}
						a.dev.packagePreview = &info
						a.dev.packageDetailsText = packageDetailsText(info)
						a.dev.detailScroll[1].Y = 200
					}
					tt := ui.NewTester(a.view, size[0], size[1])
					tt.SetDark(dark)
					tt.Frame()
					if page.tab == 0 && !a.dev.importing && !a.dev.commandWorkOpen {
						r, ok := tt.Find("Save project")
						if !ok || r.X < 0 || r.Y < 0 || r.X+r.W > float32(size[0]) || r.Y+r.H > float32(size[1]) {
							t.Fatal("save action is not visible at window size", r)
						}
					}
					if a.dev.commandWorkOpen {
						if _, ok := tt.Find("Save project"); ok {
							t.Fatal("command review exposed a hidden project save")
						}
					}
					if dir := os.Getenv("MSM_DEV_DESIGN_SHOTS"); dir != "" {
						if e := os.MkdirAll(dir, 0755); e != nil {
							t.Fatal(e)
						}
						f, e := os.Create(filepath.Join(dir, name+".png"))
						if e != nil {
							t.Fatal(e)
						}
						defer f.Close()
						if e = png.Encode(f, tt.Image()); e != nil {
							t.Fatal(e)
						}
					}
				})
			}
		}
	}
}

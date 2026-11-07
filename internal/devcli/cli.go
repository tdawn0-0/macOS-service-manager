package devcli

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"macos-service-manager/internal/devplatform"
	"os"
	"path/filepath"
	"strconv"
	"time"
)

func Run(ctx context.Context, args []string, out io.Writer) error {
	m := devplatform.New(devplatform.DefaultRoot())
	m.Event = func(s string) { fmt.Fprintln(out, s) }
	need := func(n int) error {
		if len(args) < n {
			return errors.New("missing arguments; run --dev help")
		}
		return nil
	}
	if len(args) == 0 {
		args = []string{"help"}
	}
	switch args[0] {
	case "help":
		fmt.Fprintln(out, `Development commands:
 import <directory>               Import without executing scripts
 export <project-id>              Export a shareable environment and version lock
 export-recovery <directory>      Review an interrupted export and its fingerprint
 export-recover <dir> <fingerprint> Complete the exact reviewed export
 export-discard <dir> <fingerprint> Discard its record while keeping both files
 project-folder <id> <new-dir>    Preview relocation of a stopped project
 project-folder-apply <id> <new-dir> <fingerprint> Apply the reviewed folder change
 port <number>                    Inspect IPv4 loopback and listener metadata
 available-port                   Suggest an unused high local port
 db-port <instance-id> <number>    Change a stopped database port, keeping its data
 db-port-repair <instance-id>      Reconcile endpoint files with the saved port
 prepare <project-id>             Install locked software and initialize private instances
 project <id> <config.json>        Save project configuration
 install <software> <version>     PHP, Node.js, MySQL, PostgreSQL, Redis or MongoDB
 install-manifest <package.json>   Install a verified binary archive
 check-package <package-id>        Check installed files against the installation index
 repair <package-id>               Restore the same exact software version; stop its users first
 up|down|restart|remove <id>       Manage project services with launchctl
 status                           Print configuration (no stored passwords)
 exec <project-id> <command...>    Execute in the selected runtime
 project-commands                 Review unfinished finite project commands
 project-command-resolve <id> <revision> Stop the reviewed owned command and clear its private task files
 doctor [--json|--export]          Diagnose layers; anonymize JSON and exported reports
 site <site.json>                 Save a .test HTTPS reverse proxy
 proxy                            Apply proxy configuration
 hosts                            Apply managed hosts block (macOS authorization)
 trust|untrust                    Install/remove this app's CA trust
 db-work                          Review unfinished database data tasks
 db-work-resolve <id> <revision>   Stop the reviewed owned task, keeping database and output files
 db-initializations               Review unfinished database preparations
 db-initialization-resolve <id> <revision> Stop owned jobs and retain prepared data
 db-delete-preview <id>           Preview data path, references and deletion impact
 db-delete-data <id> <fingerprint> <confirm-id> Permanently delete confirmed instance data
 db-deletions                     Review interrupted data deletions
 db-deletion-finish <id> <revision> <confirm-instance> Finish a confirmed deletion
 db-deletion-restore <id> <revision> Restore data moved before removal committed
 tasks                            List private operation records
 task-retry <id> <revision>        Retry safe work after inspecting its record
 task-dismiss <id> <revision>      Dismiss a record, retaining all resource files
 ca-info                          Inspect CA material, expiry and macOS verification
 ca-prepare                       Prepare a replacement without changing active trust
 ca-trust <revision>              Trust the exact prepared CA
 ca-activate <revision>           Activate after trust, with HTTPS stopped
 ca-discard <revision>            Discard the prepared selection, keep saved material
 ca-use <id> <fingerprint> <revision> Restore a valid, trusted saved CA
 ca-saved-trust <id> <fingerprint> Trust a reviewed saved CA
 ca-saved-untrust <id> <fingerprint> Remove only the reviewed saved CA trust
 instance <instance.json>         Initialize an isolated database instance
 db <start|stop|restart|remove> <id> Manage database instance
 createdb <id> <database>          Create a SQL or MongoDB database
 backup <id> <database>            Write a private logical backup
 restore <id> <new-db> <dump>      Restore into a new database
 connection <id>                  Show endpoint and credential file
 log-rotation <on|off>            Configure the scheduled user log maintenance task
 maintain <root>                 Internal finite log rotation task
 serve <root>                     Internal launchd proxy entry point`)
		return nil
	case "node":
		return runNode(ctx, m, args[1:], out)
	case "command-worker":
		if e := need(3); e != nil {
			return e
		}
		return devplatform.New(args[1]).RunProjectCommandWorker(ctx, args[2])
	case "project-commands":
		rows, e := m.PendingProjectCommands(ctx)
		if e != nil {
			return e
		}
		return json.NewEncoder(out).Encode(rows)
	case "project-command-resolve":
		if e := need(3); e != nil {
			return e
		}
		return m.ResolveProjectCommand(ctx, args[1], args[2])
	case "database-worker":
		if e := need(3); e != nil {
			return e
		}
		return devplatform.New(args[1]).RunDatabaseWorker(ctx, args[2])
	case "db-work":
		records, e := m.PendingDatabaseWork(ctx)
		if e != nil {
			return e
		}
		return json.NewEncoder(out).Encode(records)
	case "db-work-resolve":
		if e := need(3); e != nil {
			return e
		}
		return m.ResolveDatabaseWork(ctx, args[1], args[2])
	case "db-initializations":
		records, e := m.PendingInstanceInitializations(ctx)
		if e != nil {
			return e
		}
		return json.NewEncoder(out).Encode(records)
	case "db-initialization-resolve":
		if e := need(3); e != nil {
			return e
		}
		path, e := m.ResolveInstanceInitialization(ctx, args[1], args[2])
		if e == nil {
			fmt.Fprintln(out, path)
		}
		return e
	case "db-delete-preview":
		if e := need(2); e != nil {
			return e
		}
		p, e := m.PreviewInstanceDeletion(ctx, args[1])
		if e != nil {
			return e
		}
		return json.NewEncoder(out).Encode(p)
	case "db-delete-data":
		if e := need(4); e != nil {
			return e
		}
		return m.DeleteInstanceData(ctx, args[1], args[2], args[3])
	case "db-deletions":
		records, e := m.PendingInstanceDeletions(ctx)
		if e != nil {
			return e
		}
		return json.NewEncoder(out).Encode(records)
	case "db-deletion-finish":
		if e := need(4); e != nil {
			return e
		}
		return m.FinishInstanceDeletion(ctx, args[1], args[2], args[3])
	case "db-deletion-restore":
		if e := need(3); e != nil {
			return e
		}
		return m.RestoreInstanceDeletion(ctx, args[1], args[2])
	case "tasks":
		records, e := m.Operations(ctx)
		if e != nil {
			return e
		}
		return json.NewEncoder(out).Encode(records)
	case "task-retry", "task-dismiss":
		if e := need(3); e != nil {
			return e
		}
		if args[0] == "task-retry" {
			return m.RetryOperation(ctx, args[1], args[2])
		}
		return m.DismissOperation(ctx, args[1], args[2])
	case "ca-info":
		r, e := m.AuthorityStatus(ctx)
		if e != nil {
			return e
		}
		return json.NewEncoder(out).Encode(r)
	case "ca-prepare":
		return m.PrepareAuthority(ctx)
	case "ca-trust", "ca-activate", "ca-discard":
		if e := need(2); e != nil {
			return e
		}
		if args[0] == "ca-trust" {
			return m.TrustPreparedAuthority(ctx, args[1])
		}
		if args[0] == "ca-activate" {
			return m.ActivateAuthority(ctx, args[1])
		}
		return m.DiscardPreparedAuthority(ctx, args[1])
	case "ca-use":
		if e := need(4); e != nil {
			return e
		}
		return m.RestoreAuthority(ctx, args[1], args[2], args[3])
	case "ca-saved-trust", "ca-saved-untrust":
		if e := need(3); e != nil {
			return e
		}
		return m.ConfigureSavedAuthorityTrust(ctx, args[1], args[2], args[0] == "ca-saved-untrust")
	case "port":
		if e := need(2); e != nil {
			return e
		}
		port, e := strconv.Atoi(args[1])
		if e != nil {
			return errors.New("enter a numeric TCP port")
		}
		r, e := m.InspectPort(ctx, port)
		if e != nil {
			return e
		}
		fmt.Fprintf(out, "Port %d: %s\nChecked: %s\n", r.Port, r.Status, r.CheckedAt.Format(time.RFC3339))
		for _, listener := range r.Listeners {
			fmt.Fprintf(out, "PID %d / UID %d / %s / %s\n", listener.PID, listener.UID, listener.Name, listener.Address)
		}
		fmt.Fprintln(out, r.Detail)
		return nil
	case "available-port":
		port, e := m.AvailableLocalPort(ctx)
		if e != nil {
			return e
		}
		fmt.Fprintln(out, port)
		return nil
	case "db-port-repair":
		if e := need(2); e != nil {
			return e
		}
		s, e := m.Read()
		if e != nil {
			return e
		}
		instance, ok := s.Instances[args[1]]
		if !ok {
			return errors.New("instance not found")
		}
		return m.RepairInstancePort(ctx, instance)
	case "db-port":
		if e := need(3); e != nil {
			return e
		}
		port, e := strconv.Atoi(args[2])
		if e != nil {
			return errors.New("enter a numeric database port")
		}
		s, e := m.Read()
		if e != nil {
			return e
		}
		instance, ok := s.Instances[args[1]]
		if !ok {
			return errors.New("instance not found")
		}
		return m.ChangeInstancePort(ctx, instance, port)
	case "project-folder", "project-folder-apply":
		if e := need(3); e != nil {
			return e
		}
		s, e := m.Read()
		if e != nil {
			return e
		}
		p, ok := s.Projects[args[1]]
		if !ok {
			return errors.New("project not found")
		}
		p = p.Clone()
		p.Directory = args[2]
		if args[0] == "project-folder-apply" {
			if e = need(4); e != nil {
				return e
			}
			return m.ApplyProjectFolder(ctx, p, args[3])
		}
		r, e := m.PreviewProjectFolder(ctx, p)
		if e != nil {
			return e
		}
		fmt.Fprintf(out, "Project: %s\nFrom: %s\nTo: %s\nFingerprint: %s\n", r.ProjectName, r.From, r.To, r.Fingerprint)
		for _, change := range r.Changes {
			fmt.Fprintf(out, "%s / %s: %s -> %s\n", change.Service, change.Field, change.From, change.To)
		}
		for _, warning := range r.Warnings {
			fmt.Fprintln(out, "Review:", warning)
		}
		return nil
	case "export-recovery":
		if e := need(2); e != nil {
			return e
		}
		r, e := m.InspectExportRecovery(ctx, args[1])
		if e != nil {
			return e
		}
		if r == nil {
			fmt.Fprintln(out, "No interrupted environment export.")
			return nil
		}
		fmt.Fprintf(out, "Operation: %s\nPhase: %s\nCreated: %s\nCan resume: %t\nFingerprint: %s\n", r.OperationID, r.Phase, r.CreatedAt.Format("2006-01-02T15:04:05Z07:00"), r.CanResume, r.Fingerprint)
		fmt.Fprintf(out, "Prepared snapshot: %s (%d services, %d databases, %d sites)\n", r.ProjectName, r.Services, r.Databases, r.Sites)
		for _, id := range r.PackageIDs {
			fmt.Fprintln(out, "Locked software:", id)
		}
		for _, file := range r.Files {
			fmt.Fprintf(out, "%s: %s\n", file.Name, file.Status)
		}
		return nil
	case "export-recover", "export-discard":
		if e := need(3); e != nil {
			return e
		}
		if args[0] == "export-recover" {
			return m.RecoverExport(ctx, args[1], args[2])
		}
		return m.DiscardExport(ctx, args[1], args[2])
	case "check-package":
		if e := need(2); e != nil {
			return e
		}
		r, e := m.VerifyPackage(ctx, args[1])
		if e != nil {
			return e
		}
		fmt.Fprintln(out, r.Text())
		if r.Status == "Needs repair" {
			return errors.New("package needs repair")
		}
		return nil
	case "repair":
		if e := need(2); e != nil {
			return e
		}
		return m.RepairPackage(ctx, args[1])
	case "maintain":
		if e := need(2); e != nil {
			return e
		}
		return devplatform.New(args[1]).RotateLogs(ctx)
	case "maintenance-stop":
		return m.StopLogMaintenance(ctx)
	case "log-rotation":
		if e := need(2); e != nil {
			return e
		}
		if args[1] != "on" && args[1] != "off" {
			return errors.New("use log-rotation on or off")
		}
		return m.SetLogRotation(ctx, args[1] == "on")
	case "serve":
		if e := need(2); e != nil {
			return e
		}
		if os.Getenv("MSM_DEV_SOCKET_ACTIVATION") == "1" {
			f, e := os.OpenFile(filepath.Join(args[1], "logs", "proxy-sockets.log"), os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0600)
			if e != nil {
				return e
			}
			defer f.Close()
			log.SetOutput(f)
			log.Printf("socket proxy starting as uid %d", os.Geteuid())
			e = devplatform.New(args[1]).ServeProxy(ctx)
			if e != nil {
				log.Printf("socket proxy stopped: %v", e)
			}
			return e
		}
		return devplatform.New(args[1]).ServeProxy(ctx)
	case "doctor":
		if len(args) > 2 || (len(args) == 2 && args[1] != "--json" && args[1] != "--export") {
			return errors.New("use doctor [--json|--export]")
		}
		if len(args) == 2 {
			report, err := m.Diagnose(ctx)
			if err != nil {
				return err
			}
			if args[1] == "--export" {
				path, err := m.ExportDiagnostics(ctx, report)
				if err == nil {
					fmt.Fprintln(out, path)
				}
				return err
			}
			b, err := report.RedactedJSON()
			if err == nil {
				_, err = out.Write(b)
			}
			return err
		}
		text, e := m.Doctor(ctx)
		fmt.Fprintln(out, text)
		return e
	case "export":
		if e := need(2); e != nil {
			return e
		}
		return m.ExportProject(ctx, args[1])
	case "prepare":
		if e := need(2); e != nil {
			return e
		}
		return m.PrepareEnvironment(ctx, args[1])
	case "logs":
		if e := need(2); e != nil {
			return e
		}
		if err := readLog(m, args[1], out); err != nil {
			return err
		}
		return nil
	case "status":
		s, e := m.Read()
		for id, p := range s.Projects {
			for n := range p.Services {
				p.Services[n].Environment = nil
			}
			s.Projects[id] = p
		}
		if e != nil {
			return e
		}
		b, e := json.MarshalIndent(struct {
			devplatform.State
			Runtime devplatform.RuntimeSnapshot `json:"runtime"`
		}{s, m.Snapshot(ctx, s)}, "", "  ")
		fmt.Fprintln(out, string(b))
		return e
	case "import":
		if e := need(2); e != nil {
			return e
		}
		p, e := m.Import(ctx, args[1])
		if e == nil {
			b, _ := json.MarshalIndent(p, "", "  ")
			fmt.Fprintln(out, string(b))
		}
		return e
	case "project":
		if e := need(3); e != nil {
			return e
		}
		var p devplatform.Project
		if e := read(args[2], &p); e != nil {
			return e
		}
		p.ID = args[1]
		return m.SaveProject(ctx, p)
	case "install":
		if e := need(3); e != nil {
			return e
		}
		p, e := m.ResolvePackage(ctx, args[1], args[2])
		if e != nil {
			return e
		}
		return m.Install(ctx, p)
	case "install-manifest":
		if e := need(2); e != nil {
			return e
		}
		var p devplatform.Package
		if e := read(args[1], &p); e != nil {
			return e
		}
		return m.Install(ctx, p)
	case "up", "down", "restart", "remove":
		if e := need(2); e != nil {
			return e
		}
		switch args[0] {
		case "up":
			if e := m.EnsureLogMaintenance(ctx); e != nil {
				return e
			}
			return m.Up(ctx, args[1])
		case "restart":
			if e := m.EnsureLogMaintenance(ctx); e != nil {
				return e
			}
			return m.RestartProject(ctx, args[1])
		case "down":
			return m.Down(ctx, args[1])
		default:
			return m.RemoveProject(ctx, args[1])
		}
	case "exec":
		if e := need(3); e != nil {
			return e
		}
		text, e := m.Exec(ctx, args[1], args[2:])
		fmt.Fprint(out, text)
		return e
	case "site":
		if e := need(2); e != nil {
			return e
		}
		var s devplatform.Site
		if e := read(args[1], &s); e != nil {
			return e
		}
		return m.SaveSite(ctx, s)
	case "proxy-verify":
		return m.VerifySocketLifecycle(ctx)
	case "proxy-restart":
		return m.RestartProxy(ctx)
	case "proxy-stop":
		return m.StopProxy(ctx)
	case "proxy":
		if e := m.EnsureLogMaintenance(ctx); e != nil {
			return e
		}
		return m.ApplyProxy(ctx)
	case "hosts":
		return m.ConfigureHosts(ctx)
	case "trust", "untrust":
		return m.ConfigureTrust(ctx, args[0] == "untrust")
	case "instance":
		if e := need(2); e != nil {
			return e
		}
		var i devplatform.Instance
		if e := read(args[1], &i); e != nil {
			return e
		}
		if e := m.EnsureLogMaintenance(ctx); e != nil {
			return e
		}
		return m.CreateInstance(ctx, i)
	case "db":
		if e := need(3); e != nil {
			return e
		}
		if args[1] == "start" || args[1] == "restart" {
			if e := m.EnsureLogMaintenance(ctx); e != nil {
				return e
			}
		}
		return m.InstanceAction(ctx, args[2], args[1])
	case "createuser":
		if e := need(4); e != nil {
			return e
		}
		password := os.Getenv("MSM_DB_PASSWORD")
		return m.CreateUser(ctx, args[1], args[2], password, args[3])
	case "createdb":
		if e := need(3); e != nil {
			return e
		}
		return m.CreateDatabase(ctx, args[1], args[2])
	case "backup":
		if e := need(3); e != nil {
			return e
		}
		path, e := m.Backup(ctx, args[1], args[2])
		if e == nil {
			fmt.Fprintln(out, path)
		}
		return e
	case "restore":
		if e := need(4); e != nil {
			return e
		}
		return m.Restore(ctx, args[1], args[2], args[3])
	case "connection":
		if e := need(2); e != nil {
			return e
		}
		text, e := m.Connection(args[1])
		fmt.Fprintln(out, text)
		return e
	}
	return fmt.Errorf("unknown command %q", args[0])
}
func read(path string, v any) error {
	b, e := os.ReadFile(path)
	if e != nil {
		return e
	}
	return json.Unmarshal(b, v)
}

func readLog(m *devplatform.Manager, id string, out io.Writer) error {
	text, err := m.Logs(id)
	if err == nil {
		_, err = fmt.Fprintln(out, text)
	}
	return err
}

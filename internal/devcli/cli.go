package devcli

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"devbase/internal/devplatform"
	"devbase/internal/privhelper"
	"os"
	"path/filepath"
	"strconv"
	"time"
)

// internalCommands are the only entries the shipped app answers to when it is
// started with --dev. They are what the app's own launchd jobs, workers and
// shell setup start; none of them is an interface for people. Everything a
// person does is in the window.
var internalCommands = map[string]bool{
	"serve":           true, // the HTTPS proxy
	"tunnel":          true, // the Cloudflare tunnel agent
	"maintain":        true, // log rotation
	"database-worker": true, // database work that outlives the window
	"node":            true, // the Node.js version for a directory, for the shell setup
	"helper":          true, // the launchd-managed privileged helper
}

// RunInternal is what the app executable runs for --dev.
func RunInternal(ctx context.Context, args []string, out io.Writer) error {
	if len(args) == 0 || !internalCommands[args[0]] {
		return errors.New("this program has no command line; use the window")
	}
	return Run(ctx, args, out)
}

// Run is every command, for tests and the verification scripts through
// cmd/devmgr. It is not shipped in the app and not a maintained interface.
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
 status                           Print configuration (no stored passwords)
 install <software> <version>     Node.js, Java, MySQL, PostgreSQL, Redis or MongoDB
 port <number>                    Inspect IPv4 loopback and listener metadata
 available-port                   Suggest an unused high local port
 db-port <instance-id> <number>    Change a stopped database port, keeping its data
 db-port-repair <instance-id>      Reconcile endpoint files with the saved port
 db-initializations               Review unfinished database preparations
 db-initialization-resolve <id> <revision> Stop owned jobs and retain prepared data
 db-delete-preview <id>           Preview data path, references and deletion impact
 db-delete-data <id> <fingerprint> <confirm-id> Permanently delete confirmed instance data
 db-deletions                     Review interrupted data deletions
 tasks                            List private operation records
 task-dismiss <id> <revision>      Dismiss a record, retaining all resource files
 ca-info                          Inspect CA material, expiry and macOS verification
 ca-prepare                       Prepare a replacement without changing active trust
 ca-discard <revision>            Discard the prepared selection, keep saved material
 log-rotation <on|off>            Configure the scheduled user log maintenance task
 maintenance-stop                 Stop the scheduled user log maintenance task
 maintain <root>                  Internal finite log rotation task
 serve <root>                     Internal launchd proxy entry point
 tunnel <root>                    Internal launchd Cloudflare tunnel entry point`)
		return nil
	case "node":
		return runNode(ctx, m, args[1:], out)
	case "database-worker":
		if e := need(3); e != nil {
			return e
		}
		return devplatform.New(args[1]).RunDatabaseWorker(ctx, args[2])
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
	case "tasks":
		records, e := m.Operations(ctx)
		if e != nil {
			return e
		}
		return json.NewEncoder(out).Encode(records)
	case "task-dismiss":
		if e := need(3); e != nil {
			return e
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
	case "ca-discard":
		if e := need(2); e != nil {
			return e
		}
		return m.DiscardPreparedAuthority(ctx, args[1])
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
	case "tunnel":
		if e := need(2); e != nil {
			return e
		}
		return devplatform.New(args[1]).ServeTunnel(ctx)
	case "status":
		s, e := m.Read()
		if e != nil {
			return e
		}
		b, e := json.MarshalIndent(struct {
			devplatform.State
			Runtime devplatform.RuntimeSnapshot `json:"runtime"`
		}{s, m.Snapshot(ctx, s)}, "", "  ")
		fmt.Fprintln(out, string(b))
		return e
	case "install":
		if e := need(3); e != nil {
			return e
		}
		p, e := m.ResolvePackage(ctx, args[1], args[2])
		if e != nil {
			return e
		}
		return m.Install(ctx, p)
	case "helper":
		return runHelper(ctx, args[1:])
	}
	return fmt.Errorf("unknown command %q", args[0])
}

func runHelper(ctx context.Context, args []string) error {
	ownerUID := -1
	for i := 0; i < len(args); i++ {
		if args[i] == "--owner-uid" && i+1 < len(args) {
			uid, err := strconv.Atoi(args[i+1])
			if err != nil {
				return fmt.Errorf("invalid --owner-uid %q: %w", args[i+1], err)
			}
			ownerUID = uid
			i++
		}
	}
	return privhelper.ServeActivated(ctx, ownerUID)
}

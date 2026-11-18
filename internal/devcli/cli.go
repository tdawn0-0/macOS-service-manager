package devcli

import (
	"context"
	"devbase/internal/devplatform"
	"devbase/internal/privhelper"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"os"
	"path/filepath"
	"strconv"
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
// cmd/devmgr. It preserves exact output contracts expected by
// scripts/verify-native-release.py and internal helpers.
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
 log-rotation <on|off>            Configure the scheduled user log maintenance task
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
	case "maintain":
		if e := need(2); e != nil {
			return e
		}
		return devplatform.New(args[1]).RotateLogs(ctx)
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
	default:
		env := DefaultEnv(os.Stdin, out, out)
		return RunCLIWithEnv(ctx, args, env)
	}
}

func wantsJSONFlag(args []string) bool {
	jsonOut := false
	for _, a := range args {
		if a == "--" {
			break
		}
		if a == "--json" || a == "--json=true" {
			jsonOut = true
		} else if a == "--json=false" {
			jsonOut = false
		}
	}
	return jsonOut
}

// RunCLI runs the public Devbase CLI and returns process exit code 0, 1, or 2.
func RunCLI(ctx context.Context, args []string, in io.Reader, out, errOut io.Writer) int {
	env := DefaultEnv(in, out, errOut)
	env.JSON = wantsJSONFlag(args)
	if err := RunCLIWithEnv(ctx, args, env); err != nil {
		writeActionError(env, err)
		var ec interface{ ExitCode() int }
		if errors.As(err, &ec) {
			return ec.ExitCode()
		}
		return 1
	}
	return 0
}

// RunCLIWithEnv executes the public Devbase CLI against an injectable Env.
func RunCLIWithEnv(ctx context.Context, args []string, env *Env) error {
	if env == nil {
		env = DefaultEnv(nil, nil, nil)
	}
	if wantsJSONFlag(args) {
		env.JSON = true
	}
	cleaned, jsonFlag, err := stripGlobalFlags(args)
	if jsonFlag {
		env.JSON = true
	}
	if err != nil {
		return err
	}
	if len(cleaned) == 0 {
		cleaned = []string{"help"}
	}
	cmd, rest := cleaned[0], cleaned[1:]
	switch cmd {
	case "help", "--help", "-h":
		return runHelpCommand(env, rest)
	case "version", "--version", "-v":
		return runVersionCommand(env, rest)
	case "schema":
		return runSchemaCommand(env, rest)
	case "status":
		return runStatusCommand(ctx, env, rest)
	case "diagnostics":
		return runDiagnosticsCommand(ctx, env, rest)
	case "tasks":
		return runTasksCommand(ctx, env, rest)
	case "logs":
		return runLogsCommand(ctx, env, rest)
	case "ports":
		return runPortsCommand(ctx, env, rest)
	case "shutdown":
		return runShutdownCommand(ctx, env, rest)
	case "software":
		return runSoftwareCommand(ctx, env, rest)
	case "install":
		return runSoftwareInstall(ctx, env, rest)
	case "db", "database":
		return runDatabaseCommand(ctx, env, rest)
	case "https", "proxy":
		return runHTTPSCommand(ctx, env, rest)
	case "tunnel":
		return runTunnelCommand(ctx, env, rest)
	case "ca":
		return runCACommand(ctx, env, rest)
	case "services", "service":
		return runServicesCommand(ctx, env, rest)
	case "settings":
		return runSettingsCommand(ctx, env, rest)
	case "log-rotation":
		return runSettingsLogRotation(ctx, env, rest)
	case "cli":
		return runCLICommand(ctx, env, rest)
	default:
		return usageErrorf("unknown command %q (run \"devbase help\" or \"devbase schema --json\")", cmd)
	}
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

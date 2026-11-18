package devcli

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"

	"devbase/internal/devplatform"
	"devbase/internal/logs"
	"devbase/internal/privhelper"
)

var logTargetRE = regexp.MustCompile(`^[a-zA-Z0-9][a-zA-Z0-9._-]{0,239}$`)

func runStatusCommand(ctx context.Context, env *Env, args []string) error {
	if hasHelpFlag(args) {
		printTopicHelp(env.Out, "status")
		return nil
	}
	if len(args) > 0 {
		return usageErrorf("unexpected argument %q for status", args[0])
	}
	m := env.manager()
	s, err := m.Read()
	if err != nil {
		return err
	}
	snap := m.Snapshot(ctx, s)
	pub, _ := m.ProxyPublicationState()
	if pub == "" {
		pub = "Unknown"
	}
	payload := struct {
		devplatform.State
		Runtime               devplatform.RuntimeSnapshot `json:"runtime"`
		ProxyPublicationState string                      `json:"proxyPublicationState"`
		Helper                privhelper.HelperStatus     `json:"helper"`
		CLI                   CLIInstallStatus            `json:"cli"`
		Root                  string                      `json:"root"`
	}{
		State:                 s,
		Runtime:               snap,
		ProxyPublicationState: pub,
		Helper:                env.helper().Status(),
		CLI:                   CLIStatus(env.CLIBinDir, env.AppPath, env.LoginProgram),
		Root:                  m.Root,
	}
	return writeResult(env, payload, func(w io.Writer) {
		fmt.Fprintf(w, "Workspace: %s\n", payload.Root)
		fmt.Fprintf(w, "Packages: %d\n", len(payload.Packages))
		fmt.Fprintf(w, "Instances: %d\n", len(payload.Instances))
		for _, id := range sortedMapKeys(payload.Instances) {
			inst := payload.Instances[id]
			fmt.Fprintf(w, "  - %s (%s, :%d) [%s]\n", inst.ID, inst.Kind, inst.Port, snap.States["instance:"+id])
		}
		fmt.Fprintf(w, "HTTPS Proxy: %s (%d rules, listener :%d, %s)\n", snap.States["proxy"], len(payload.ProxyRules), payload.ProxyListenerPort, pub)
		if snap.States["tunnel"] != "" || snap.States["tunnel:token"] != "" {
			fmt.Fprintf(w, "Cloudflare Tunnel: %s (token: %s)\n", snap.States["tunnel"], snap.States["tunnel:token"])
		}
		fmt.Fprintf(w, "Privileged Helper: %s\n", payload.Helper.State)
		fmt.Fprintf(w, "CLI Wrapper: installed=%t (%s)\n", payload.CLI.Installed, payload.CLI.WrapperPath)
	})
}

func runDiagnosticsCommand(ctx context.Context, env *Env, args []string) error {
	if hasHelpFlag(args) {
		printTopicHelp(env.Out, "diagnostics")
		return nil
	}
	sub := "run"
	if len(args) > 0 {
		sub = args[0]
		args = args[1:]
	}
	if len(args) > 0 {
		return usageErrorf("unexpected argument %q for diagnostics %s", args[0], sub)
	}
	m := env.manager()
	switch sub {
	case "run":
		report, err := m.Diagnose(ctx)
		if err != nil {
			return err
		}
		return writeResult(env, report, func(w io.Writer) {
			fmt.Fprintf(w, "Diagnostics (%d checks):\n", len(report.Checks))
			for _, chk := range report.Checks {
				fmt.Fprintf(w, "  [%s] %s | %s: %s\n", chk.Status, chk.Layer, chk.Resource, chk.Summary)
				if chk.Remedy != "" {
					fmt.Fprintf(w, "         Remedy: %s\n", chk.Remedy)
				}
			}
		})
	case "export":
		report, err := m.Diagnose(ctx)
		if err != nil {
			return err
		}
		path, err := m.ExportDiagnostics(ctx, report)
		if err != nil {
			return err
		}
		return writeResult(env, map[string]any{"path": path, "report": report}, func(w io.Writer) {
			fmt.Fprintf(w, "Redacted diagnostic report exported: %s\n", path)
		})
	default:
		return usageErrorf("unknown diagnostics subcommand %q (expected run or export)", sub)
	}
}

func runTasksCommand(ctx context.Context, env *Env, args []string) error {
	if hasHelpFlag(args) {
		printTopicHelp(env.Out, "tasks")
		return nil
	}
	sub := "list"
	if len(args) > 0 {
		sub = args[0]
		args = args[1:]
	}
	m := env.manager()
	switch sub {
	case "list":
		if len(args) > 0 {
			return usageErrorf("unexpected argument %q for tasks list", args[0])
		}
		ops, err := m.Operations(ctx)
		if err != nil {
			return err
		}
		type opView struct {
			devplatform.OperationRecord
			CanRetry bool `json:"canRetry"`
		}
		out := make([]opView, 0, len(ops))
		for _, op := range ops {
			out = append(out, opView{OperationRecord: op, CanRetry: devplatform.OperationCanRetry(op)})
		}
		return writeResult(env, map[string]any{"operations": out}, func(w io.Writer) {
			if len(out) == 0 {
				fmt.Fprintln(w, "No recorded operations.")
				return
			}
			for _, op := range out {
				fmt.Fprintf(w, "%s | %s | %s | %s | rev=%s | retry=%t\n", op.ID, op.Request.Kind, op.Request.Resource, op.Status, op.Revision, op.CanRetry)
				if op.Result != "" {
					fmt.Fprintf(w, "  %s\n", op.Result)
				}
			}
		})
	case "dismiss":
		if len(args) != 2 {
			return usageErrorf("usage: devbase tasks dismiss <id> <revision>")
		}
		if err := m.DismissOperation(ctx, args[0], args[1]); err != nil {
			return err
		}
		return writeResult(env, map[string]any{"dismissed": args[0], "revision": args[1]}, func(w io.Writer) {
			fmt.Fprintf(w, "Dismissed operation %s.\n", args[0])
		})
	case "retry":
		if len(args) != 2 {
			return usageErrorf("usage: devbase tasks retry <id> <revision>")
		}
		if err := m.RetryOperation(ctx, args[0], args[1]); err != nil {
			return err
		}
		return writeResult(env, map[string]any{"retried": args[0], "revision": args[1]}, func(w io.Writer) {
			fmt.Fprintf(w, "Retried operation %s.\n", args[0])
		})
	case "work":
		if len(args) > 0 {
			return usageErrorf("unexpected argument %q for tasks work", args[0])
		}
		items, err := m.PendingDatabaseWork(ctx)
		if err != nil {
			return err
		}
		return writeResult(env, map[string]any{"databaseWork": items}, func(w io.Writer) {
			if len(items) == 0 {
				fmt.Fprintln(w, "No unfinished database tasks.")
				return
			}
			for _, item := range items {
				fmt.Fprintf(w, "%s | %s | %s | %s | rev=%s\n", item.ID, item.Kind, item.InstanceID, item.Status, item.Revision)
				if item.Detail != "" {
					fmt.Fprintf(w, "  %s\n", item.Detail)
				}
			}
		})
	case "resolve-work":
		if len(args) != 2 {
			return usageErrorf("usage: devbase tasks resolve-work <id> <revision>")
		}
		if err := m.ResolveDatabaseWork(ctx, args[0], args[1]); err != nil {
			return err
		}
		return writeResult(env, map[string]any{"resolved": args[0], "revision": args[1]}, func(w io.Writer) {
			fmt.Fprintf(w, "Resolved database work %s.\n", args[0])
		})
	case "initializations":
		if len(args) > 0 {
			return usageErrorf("unexpected argument %q for tasks initializations", args[0])
		}
		items, err := m.PendingInstanceInitializations(ctx)
		if err != nil {
			return err
		}
		return writeResult(env, map[string]any{"initializations": items}, func(w io.Writer) {
			if len(items) == 0 {
				fmt.Fprintln(w, "No unfinished initializations.")
				return
			}
			for _, item := range items {
				fmt.Fprintf(w, "%s | %s | %s | %s | rev=%s\n", item.ID, item.InstanceID, item.Kind, item.Status, item.Revision)
				if item.Detail != "" {
					fmt.Fprintf(w, "  %s\n", item.Detail)
				}
			}
		})
	case "resolve-init":
		if len(args) != 2 {
			return usageErrorf("usage: devbase tasks resolve-init <id> <revision>")
		}
		archive, err := m.ResolveInstanceInitialization(ctx, args[0], args[1])
		if err != nil {
			return err
		}
		return writeResult(env, map[string]any{"resolved": args[0], "revision": args[1], "archive": archive}, func(w io.Writer) {
			fmt.Fprintf(w, "Resolved initialization %s (archive: %s).\n", args[0], archive)
		})
	case "deletions":
		if len(args) > 0 {
			return usageErrorf("unexpected argument %q for tasks deletions", args[0])
		}
		items, err := m.PendingInstanceDeletions(ctx)
		if err != nil {
			return err
		}
		return writeResult(env, map[string]any{"deletions": items}, func(w io.Writer) {
			if len(items) == 0 {
				fmt.Fprintln(w, "No pending deletions.")
				return
			}
			for _, item := range items {
				fmt.Fprintf(w, "%s | %s | %s | %s | rev=%s\n", item.ID, item.InstanceID, item.Kind, item.Status, item.Revision)
				if item.Detail != "" {
					fmt.Fprintf(w, "  %s\n", item.Detail)
				}
			}
		})
	case "finish-deletion":
		fs := newFlagSet("tasks finish-deletion")
		var confirm string
		fs.StringVar(&confirm, "confirm", "", "Exact original instance ID confirmation")
		pos, err := parseFlags(fs, args)
		if err != nil {
			return err
		}
		if len(pos) != 2 || strings.TrimSpace(confirm) == "" {
			return usageErrorf("usage: devbase tasks finish-deletion <id> <revision> --confirm <instance-id>")
		}
		if err := m.FinishInstanceDeletion(ctx, pos[0], pos[1], confirm); err != nil {
			return err
		}
		return writeResult(env, map[string]any{"finished": pos[0], "revision": pos[1]}, func(w io.Writer) {
			fmt.Fprintf(w, "Finished pending deletion %s.\n", pos[0])
		})
	case "restore-deletion":
		if len(args) != 2 {
			return usageErrorf("usage: devbase tasks restore-deletion <id> <revision>")
		}
		if err := m.RestoreInstanceDeletion(ctx, args[0], args[1]); err != nil {
			return err
		}
		return writeResult(env, map[string]any{"restored": args[0], "revision": args[1]}, func(w io.Writer) {
			fmt.Fprintf(w, "Restored pending deletion %s.\n", args[0])
		})
	default:
		return usageErrorf("unknown tasks subcommand %q", sub)
	}
}

func runLogsCommand(_ context.Context, env *Env, args []string) error {
	if hasHelpFlag(args) {
		printTopicHelp(env.Out, "logs")
		return nil
	}
	fs := newFlagSet("logs")
	var stream, filter string
	var lines int
	fs.StringVar(&stream, "stream", "", "Log stream: stderr or stdout")
	fs.IntVar(&lines, "lines", 200, "Maximum number of trailing lines")
	fs.StringVar(&filter, "filter", "", "Case-insensitive line filter")
	pos, err := parseFlags(fs, args)
	if err != nil {
		return err
	}
	if len(pos) != 1 {
		return usageErrorf("usage: devbase logs <proxy|tunnel|db-<id>|<instance-id>> [--stream stderr|stdout] [--lines N] [--filter <text>]")
	}
	if lines <= 0 {
		lines = 200
	}
	if lines > 5000 {
		lines = 5000
	}
	m := env.manager()
	target := strings.TrimSpace(pos[0])
	if target != "proxy" && target != "tunnel" && !strings.HasPrefix(target, "db-") {
		if s, err := m.Read(); err == nil {
			if _, ok := s.Instances[target]; ok {
				target = "db-" + target
			}
		}
	}
	if !logTargetRE.MatchString(target) {
		return usageErrorf("invalid log target %q", pos[0])
	}
	chosenStream := strings.ToLower(strings.TrimSpace(stream))
	if chosenStream != "" && chosenStream != "stderr" && chosenStream != "stdout" {
		return usageErrorf("--stream must be stderr or stdout")
	}
	errPath := filepath.Join(m.Root, "logs", target+".err.log")
	outPath := filepath.Join(m.Root, "logs", target+".log")
	path := errPath
	if chosenStream == "stdout" {
		path = outPath
	} else if chosenStream == "" {
		chosenStream = "stderr"
		if info, err := os.Stat(errPath); err != nil || info.Size() == 0 {
			if outInfo, outErr := os.Stat(outPath); outErr == nil && outInfo.Size() > 0 {
				path = outPath
				chosenStream = "stdout"
			}
		}
	}
	snap, err := logs.Tail(path, 256<<10, lines)
	exists := true
	if errors.Is(err, os.ErrNotExist) {
		exists = false
		snap = logs.Snapshot{Lines: []string{}}
		err = nil
	}
	if err != nil {
		return err
	}
	matched := make([]string, 0, len(snap.Lines))
	needle := strings.ToLower(strings.TrimSpace(filter))
	for _, line := range snap.Lines {
		if needle == "" || strings.Contains(strings.ToLower(line), needle) {
			matched = append(matched, line)
		}
	}
	payload := map[string]any{
		"jobId":     target,
		"stream":    chosenStream,
		"path":      path,
		"exists":    exists,
		"truncated": snap.Truncated,
		"lines":     matched,
	}
	return writeResult(env, payload, func(w io.Writer) {
		if !exists {
			fmt.Fprintf(w, "No logs yet (%s).\n", path)
			return
		}
		if len(matched) == 0 {
			fmt.Fprintln(w, "No matching log lines.")
			return
		}
		for _, line := range matched {
			fmt.Fprintln(w, line)
		}
	})
}

func runPortsCommand(ctx context.Context, env *Env, args []string) error {
	if hasHelpFlag(args) {
		printTopicHelp(env.Out, "ports")
		return nil
	}
	sub := "active"
	if len(args) > 0 {
		sub = args[0]
		args = args[1:]
	}
	m := env.manager()
	switch sub {
	case "active":
		if len(args) > 0 {
			return usageErrorf("unexpected argument %q for ports active", args[0])
		}
		ports, err := m.ListeningUserPorts(ctx)
		if err != nil {
			return err
		}
		return writeResult(env, map[string]any{"ports": ports}, func(w io.Writer) {
			if len(ports) == 0 {
				fmt.Fprintln(w, "No active listening local user ports found.")
				return
			}
			for _, p := range ports {
				if p.Process != "" {
					fmt.Fprintf(w, "%d\t%s\n", p.Port, p.Process)
				} else {
					fmt.Fprintf(w, "%d\n", p.Port)
				}
			}
		})
	case "free":
		if len(args) > 0 {
			return usageErrorf("unexpected argument %q for ports free", args[0])
		}
		port, err := m.AvailableLocalPort(ctx)
		if err != nil {
			return err
		}
		return writeResult(env, map[string]any{"port": port}, func(w io.Writer) {
			fmt.Fprintf(w, "%d\n", port)
		})
	case "check":
		if len(args) != 1 {
			return usageErrorf("usage: devbase ports check <port>")
		}
		port, err := strconv.Atoi(args[0])
		if err != nil || port < 1 || port > 65535 {
			return usageErrorf("port must be 1–65535")
		}
		info, err := m.InspectPort(ctx, port)
		if err != nil {
			return err
		}
		return writeResult(env, info, func(w io.Writer) {
			fmt.Fprintf(w, "Port %d: %s\n", info.Port, info.Status)
			if info.Detail != "" {
				fmt.Fprintln(w, info.Detail)
			}
			for _, l := range info.Listeners {
				fmt.Fprintf(w, "  PID %d (UID %d) %s %s\n", l.PID, l.UID, l.Name, l.Address)
			}
		})
	default:
		return usageErrorf("unknown ports subcommand %q (expected active, free, or check)", sub)
	}
}

func runShutdownCommand(ctx context.Context, env *Env, args []string) error {
	if hasHelpFlag(args) {
		printTopicHelp(env.Out, "shutdown")
		return nil
	}
	sub := "preview"
	if len(args) > 0 {
		sub = args[0]
		args = args[1:]
	}
	m := env.manager()
	switch sub {
	case "preview":
		if len(args) > 0 {
			return usageErrorf("unexpected argument %q for shutdown preview", args[0])
		}
		plan, err := m.PreviewShutdown(ctx)
		if err != nil {
			return err
		}
		return writeResult(env, plan, func(w io.Writer) {
			fmt.Fprintf(w, "Managed database instances in stop scope: %d (fingerprint: %s)\n", len(plan.Instances), plan.Fingerprint)
			for _, inst := range plan.Instances {
				fmt.Fprintf(w, "  - %s (%s)\n", inst.ID, inst.Name)
			}
		})
	case "stop":
		fs := newFlagSet("shutdown stop")
		var yes bool
		fs.BoolVar(&yes, "yes", false, "Confirm stopping all managed database instances")
		pos, err := parseFlags(fs, args)
		if err != nil {
			return err
		}
		if len(pos) > 0 {
			return usageErrorf("unexpected argument %q for shutdown stop", pos[0])
		}
		if !yes {
			return usageErrorf("stopping all managed database instances requires --yes")
		}
		plan, err := m.PreviewShutdown(ctx)
		if err != nil {
			return err
		}
		if err := m.Shutdown(ctx, plan, devplatform.ShutdownOptions{}); err != nil {
			return err
		}
		return writeResult(env, map[string]any{"stopped": len(plan.Instances), "instances": plan.Instances}, func(w io.Writer) {
			fmt.Fprintf(w, "Stopped %d managed database instances (HTTPS proxy and tunnel remain running).\n", len(plan.Instances))
		})
	default:
		return usageErrorf("unknown shutdown subcommand %q (expected preview or stop)", sub)
	}
}

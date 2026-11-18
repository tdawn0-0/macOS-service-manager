package devcli

import (
	"context"
	"fmt"
	"io"
	"strings"

	"devbase/internal/launchd"
	"devbase/internal/privhelper"
)

func runSettingsCommand(ctx context.Context, env *Env, args []string) error {
	if hasHelpFlag(args) && len(args) == 1 {
		printTopicHelp(env.Out, "settings")
		return nil
	}
	sub := "status"
	if len(args) > 0 {
		sub = args[0]
		args = args[1:]
	}
	if hasHelpFlag(args) {
		printTopicHelp(env.Out, "settings")
		return nil
	}
	switch sub {
	case "status":
		return runSettingsStatus(env, args)
	case "helper":
		return runSettingsHelper(ctx, env, args)
	case "cache":
		return runSettingsCache(ctx, env, args)
	case "log-rotation":
		return runSettingsLogRotation(ctx, env, args)
	case "login-item":
		return runSettingsLoginItem(env, args)
	default:
		return usageErrorf("unknown settings subcommand %q", sub)
	}
}

func runSettingsStatus(env *Env, args []string) error {
	if len(args) > 0 {
		return usageErrorf("unexpected argument %q for settings status", args[0])
	}
	m := env.manager()
	s, err := m.Read()
	if err != nil {
		return err
	}
	helperStatus := env.helper().Status()
	cacheInfo, err := m.DownloadCacheInfo()
	if err != nil {
		return err
	}
	loginEnabled := launchd.LaunchAtLoginEnabled(env.LoginAgentDir)
	cliStatus := CLIStatus(env.CLIBinDir, env.AppPath, env.LoginProgram)
	payload := map[string]any{
		"workspace":           m.Root,
		"helper":              helperStatus,
		"downloadCache":       cacheInfo,
		"logRotationEnabled":  !s.LogRotationDisabled,
		"startAtLoginEnabled": loginEnabled,
		"loginAgentPlist":     launchd.LoginAgentPlistPath(env.LoginAgentDir),
		"cli":                 cliStatus,
	}
	return writeResult(env, payload, func(w io.Writer) {
		fmt.Fprintf(w, "Workspace: %s\n", m.Root)
		fmt.Fprintf(w, "Privileged Helper: %s\n", helperStatus.State)
		if helperStatus.Message != "" {
			fmt.Fprintf(w, "  %s\n", helperStatus.Message)
		}
		fmt.Fprintf(w, "Download Cache: %d files (%d bytes) at %s\n", cacheInfo.Files, cacheInfo.Bytes, cacheInfo.Directory)
		fmt.Fprintf(w, "Automatic Log Rotation: %t\n", !s.LogRotationDisabled)
		fmt.Fprintf(w, "Start at Login (Menu Bar): %t\n", loginEnabled)
		fmt.Fprintf(w, "CLI Wrapper: installed=%t path=%s inPATH=%t\n", cliStatus.Installed, cliStatus.WrapperPath, cliStatus.InPATH)
	})
}

func runSettingsHelper(ctx context.Context, env *Env, args []string) error {
	sub := "status"
	if len(args) > 0 {
		sub = args[0]
		args = args[1:]
	}
	client := env.helper()
	switch sub {
	case "status":
		if len(args) > 0 {
			return usageErrorf("unexpected argument %q for settings helper status", args[0])
		}
		st := client.Status()
		return writeResult(env, st, func(w io.Writer) {
			fmt.Fprintf(w, "Privileged Helper: %s\n", st.State)
			if st.Message != "" {
				fmt.Fprintln(w, st.Message)
			}
			fmt.Fprintf(w, "Daemon: %s\nSocket: %s\nHelper: %s\n", privhelper.PlistPath, privhelper.SocketPath, privhelper.HelperDir)
		})
	case "install":
		if len(args) > 0 {
			return usageErrorf("unexpected argument %q for settings helper install", args[0])
		}
		if err := client.Install(ctx); err != nil {
			return err
		}
		st := client.Status()
		return writeResult(env, map[string]any{"installed": true, "status": st}, func(w io.Writer) {
			fmt.Fprintf(w, "Privileged helper installed (%s).\n", st.State)
		})
	case "uninstall":
		fs := newFlagSet("settings helper uninstall")
		var yes bool
		fs.BoolVar(&yes, "yes", false, "Confirm removing the privileged helper")
		pos, err := parseFlags(fs, args)
		if err != nil {
			return err
		}
		if len(pos) > 0 {
			return usageErrorf("unexpected argument %q for settings helper uninstall", pos[0])
		}
		if !yes {
			return usageErrorf("removing the privileged helper requires --yes")
		}
		if err := client.Uninstall(ctx); err != nil {
			return err
		}
		st := client.Status()
		return writeResult(env, map[string]any{"uninstalled": true, "status": st}, func(w io.Writer) {
			fmt.Fprintln(w, "Privileged helper removed.")
		})
	default:
		return usageErrorf("unknown settings helper subcommand %q (expected status, install, or uninstall)", sub)
	}
}

func runSettingsCache(ctx context.Context, env *Env, args []string) error {
	sub := "status"
	if len(args) > 0 {
		sub = args[0]
		args = args[1:]
	}
	m := env.manager()
	switch sub {
	case "status":
		if len(args) > 0 {
			return usageErrorf("unexpected argument %q for settings cache status", args[0])
		}
		info, err := m.DownloadCacheInfo()
		if err != nil {
			return err
		}
		return writeResult(env, info, func(w io.Writer) {
			fmt.Fprintf(w, "Download Cache: %d files (%d bytes) in %s\n", info.Files, info.Bytes, info.Directory)
		})
	case "clear":
		fs := newFlagSet("settings cache clear")
		var yes bool
		fs.BoolVar(&yes, "yes", false, "Confirm clearing cached download archives")
		pos, err := parseFlags(fs, args)
		if err != nil {
			return err
		}
		if len(pos) > 0 {
			return usageErrorf("unexpected argument %q for settings cache clear", pos[0])
		}
		if !yes {
			return usageErrorf("clearing download cache requires --yes")
		}
		removed, err := m.ClearDownloadCache(ctx)
		if err != nil {
			return err
		}
		remaining, _ := m.DownloadCacheInfo()
		return writeResult(env, map[string]any{"removed": removed, "remaining": remaining}, func(w io.Writer) {
			fmt.Fprintf(w, "Cleared %d archives (%d bytes) from download cache.\n", removed.Files, removed.Bytes)
		})
	default:
		return usageErrorf("unknown settings cache subcommand %q (expected status or clear)", sub)
	}
}

func runSettingsLogRotation(ctx context.Context, env *Env, args []string) error {
	sub := "status"
	if len(args) > 0 {
		sub = strings.ToLower(strings.TrimSpace(args[0]))
		args = args[1:]
	}
	if len(args) > 0 {
		return usageErrorf("unexpected argument %q for settings log-rotation %s", args[0], sub)
	}
	m := env.manager()
	switch sub {
	case "status":
		s, err := m.Read()
		if err != nil {
			return err
		}
		return writeResult(env, map[string]any{"enabled": !s.LogRotationDisabled}, func(w io.Writer) {
			fmt.Fprintf(w, "Automatic log rotation enabled: %t\n", !s.LogRotationDisabled)
		})
	case "on", "off":
		enabled := sub == "on"
		if err := m.SetLogRotation(ctx, enabled); err != nil {
			return err
		}
		return writeResult(env, map[string]any{"enabled": enabled}, func(w io.Writer) {
			fmt.Fprintf(w, "Automatic log rotation set to %s.\n", sub)
		})
	default:
		return usageErrorf("usage: devbase settings log-rotation [status|on|off]")
	}
}

func runSettingsLoginItem(env *Env, args []string) error {
	sub := "status"
	if len(args) > 0 {
		sub = strings.ToLower(strings.TrimSpace(args[0]))
		args = args[1:]
	}
	if len(args) > 0 {
		return usageErrorf("unexpected argument %q for settings login-item %s", args[0], sub)
	}
	switch sub {
	case "status":
		enabled := launchd.LaunchAtLoginEnabled(env.LoginAgentDir)
		plistPath := launchd.LoginAgentPlistPath(env.LoginAgentDir)
		return writeResult(env, map[string]any{"enabled": enabled, "plist": plistPath}, func(w io.Writer) {
			fmt.Fprintf(w, "Start at login in menu bar: %t (%s)\n", enabled, plistPath)
		})
	case "on":
		exe, err := launchd.ResolveLoginExecutable(env.AppPath, env.LoginProgram)
		if err != nil {
			return err
		}
		if err := launchd.EnableLaunchAtLogin(env.LoginAgentDir, exe, env.LoginRunner); err != nil {
			return err
		}
		return writeResult(env, map[string]any{
			"enabled":    true,
			"executable": exe,
			"plist":      launchd.LoginAgentPlistPath(env.LoginAgentDir),
		}, func(w io.Writer) {
			fmt.Fprintf(w, "Enabled start at login in menu bar (%s).\n", launchd.LoginAgentPlistPath(env.LoginAgentDir))
		})
	case "off":
		if err := launchd.DisableLaunchAtLogin(env.LoginAgentDir, env.LoginRunner); err != nil {
			return err
		}
		return writeResult(env, map[string]any{
			"enabled": false,
			"plist":   launchd.LoginAgentPlistPath(env.LoginAgentDir),
		}, func(w io.Writer) {
			fmt.Fprintln(w, "Disabled start at login in menu bar.")
		})
	default:
		return usageErrorf("usage: devbase settings login-item [status|on|off]")
	}
}

func runCLICommand(_ context.Context, env *Env, args []string) error {
	if hasHelpFlag(args) && len(args) == 1 {
		printTopicHelp(env.Out, "cli")
		return nil
	}
	sub := "status"
	if len(args) > 0 {
		sub = args[0]
		args = args[1:]
	}
	if hasHelpFlag(args) {
		printTopicHelp(env.Out, "cli")
		return nil
	}
	fs := newFlagSet("cli " + sub)
	var dir string
	var yes bool
	fs.StringVar(&dir, "dir", env.CLIBinDir, "Target bin directory for the devbase CLI wrapper")
	if sub == "uninstall" {
		fs.BoolVar(&yes, "yes", false, "Optional confirmation flag")
	}
	pos, err := parseFlags(fs, args)
	if err != nil {
		return err
	}
	if len(pos) > 0 {
		return usageErrorf("unexpected argument %q for cli %s", pos[0], sub)
	}
	_ = yes
	switch sub {
	case "status":
		st := CLIStatus(dir, env.AppPath, env.LoginProgram)
		return writeResult(env, st, func(w io.Writer) {
			fmt.Fprintf(w, "Installed: %t\nManaged by Devbase: %t\nIn PATH: %t\nWrapper: %s\nTarget: %s\n",
				st.Installed, st.ManagedByDevbase, st.InPATH, st.WrapperPath, st.TargetExecutable)
		})
	case "install":
		st, err := InstallCLIWrapper(dir, env.AppPath, env.LoginProgram)
		if err != nil {
			return err
		}
		return writeResult(env, st, func(w io.Writer) {
			fmt.Fprintf(w, "Installed devbase CLI wrapper at %s (-> %s)\n", st.WrapperPath, st.TargetExecutable)
			if !st.InPATH {
				fmt.Fprintf(w, "Note: %s is not currently in PATH.\n", dir)
			}
		})
	case "uninstall":
		if err := UninstallCLIWrapper(dir); err != nil {
			return err
		}
		st := CLIStatus(dir, env.AppPath, env.LoginProgram)
		return writeResult(env, map[string]any{"removed": true, "status": st}, func(w io.Writer) {
			fmt.Fprintf(w, "Removed devbase CLI wrapper from %s.\n", st.WrapperPath)
		})
	default:
		return usageErrorf("unknown cli subcommand %q (expected status, install, or uninstall)", sub)
	}
}

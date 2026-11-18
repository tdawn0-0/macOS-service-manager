package devcli

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"slices"
	"strings"

	"devbase/internal/launchd"
	"devbase/internal/service"
)

type serviceRootSaver interface {
	SaveAsRoot(s launchd.Spec, existing string, load bool) (string, error)
}

func runServicesCommand(_ context.Context, env *Env, args []string) error {
	if hasHelpFlag(args) && len(args) == 1 {
		printTopicHelp(env.Out, "services")
		return nil
	}
	sub := "list"
	if len(args) > 0 {
		sub = args[0]
		args = args[1:]
	}
	if hasHelpFlag(args) {
		printTopicHelp(env.Out, "services")
		return nil
	}
	switch sub {
	case "list":
		return runServicesList(env, args)
	case "inspect":
		return runServicesInspect(env, args)
	case "start", "stop", "restart", "load", "unload", "enable", "disable", "remove":
		return runServicesAction(env, service.Action(sub), args)
	case "add-login-item":
		return runServicesAddLoginItem(env, args)
	case "job-save":
		return runServicesJobSave(env, args)
	default:
		return usageErrorf("unknown services subcommand %q", sub)
	}
}

func parseServiceSource(s string) (service.Source, error) {
	switch strings.ToLower(strings.TrimSpace(s)) {
	case string(service.Brew):
		return service.Brew, nil
	case string(service.LoginItem):
		return service.LoginItem, nil
	case string(service.UserAgent):
		return service.UserAgent, nil
	case string(service.SystemAgent):
		return service.SystemAgent, nil
	case string(service.Daemon):
		return service.Daemon, nil
	default:
		return "", usageErrorf("unknown service source %q (expected brew, login-item, user-agent, system-agent, or daemon)", s)
	}
}

func serviceStateKey(st service.State) string {
	switch st {
	case service.StateNone:
		return "none"
	case service.StateStopped:
		return "stopped"
	case service.StateLoaded:
		return "idle"
	case service.StateRunning:
		return "running"
	case service.StateScheduled:
		return "scheduled"
	case service.StateFailed:
		return "error"
	case service.StateInvalid:
		return "invalid"
	default:
		return strings.ToLower(st.String())
	}
}

func serviceAutostartText(a service.Autostart) string {
	switch a {
	case service.AutostartOn:
		return "on"
	case service.AutostartOff:
		return "off"
	default:
		return "unknown"
	}
}

type serviceItemView struct {
	ID            string   `json:"id"`
	Source        string   `json:"source"`
	Name          string   `json:"name"`
	State         string   `json:"state"`
	PID           int      `json:"pid,omitempty"`
	Autostart     string   `json:"autostart"`
	Owner         string   `json:"owner,omitempty"`
	Path          string   `json:"path,omitempty"`
	Kind          string   `json:"kind,omitempty"`
	Admin         bool     `json:"admin,omitempty"`
	KeepAlive     bool     `json:"keepAlive,omitempty"`
	Actions       []string `json:"actions,omitempty"`
	PrimaryAction string   `json:"primaryAction,omitempty"`
	ToggleAction  string   `json:"toggleAction,omitempty"`
}

func buildServiceItemView(p service.Provider, it service.Item) serviceItemView {
	actions := p.Actions(it)
	actionNames := make([]string, 0, len(actions))
	for _, a := range actions {
		actionNames = append(actionNames, string(a))
	}
	stateText := it.State.String()
	if stateText == "" {
		stateText = "None"
	}
	return serviceItemView{
		ID:            it.ID,
		Source:        string(it.Source),
		Name:          it.Name,
		State:         stateText,
		PID:           it.PID,
		Autostart:     serviceAutostartText(it.Autostart),
		Owner:         it.Owner,
		Path:          it.Path,
		Kind:          it.Kind,
		Admin:         it.Admin,
		KeepAlive:     it.KeepAlive,
		Actions:       actionNames,
		PrimaryAction: string(service.Primary(actions)),
		ToggleAction:  string(service.Toggle(actions)),
	}
}

func collectServices(env *Env, sourceFilter string) ([]service.Item, map[service.Source]service.Provider, []map[string]string, error) {
	providers := env.providers()
	var sources []service.Source
	if sourceFilter != "" {
		src, err := parseServiceSource(sourceFilter)
		if err != nil {
			return nil, nil, nil, err
		}
		sources = []service.Source{src}
	} else {
		sources = service.Sources
	}
	var all []service.Item
	var warnings []map[string]string
	for _, src := range sources {
		p, ok := providers[src]
		if !ok || p == nil {
			continue
		}
		items, err := p.List()
		all = append(all, items...)
		if err != nil {
			warnings = append(warnings, map[string]string{"source": string(src), "error": err.Error()})
		}
		if r, ok := p.(service.Restricted); ok {
			if rest := r.Restriction(); rest != nil {
				warnings = append(warnings, map[string]string{"source": string(src), "restriction": rest.Text})
			}
		}
	}
	return all, providers, warnings, nil
}

func resolveServiceItem(env *Env, target, sourceFilter string) (service.Item, service.Provider, error) {
	items, providers, _, err := collectServices(env, sourceFilter)
	if err != nil {
		return service.Item{}, nil, err
	}
	target = strings.TrimSpace(target)
	if target == "" {
		return service.Item{}, nil, usageErrorf("service identifier is required")
	}
	var exactID []service.Item
	var byName []service.Item
	for _, it := range items {
		if it.ID == target || (it.Path != "" && it.Path == target) {
			exactID = append(exactID, it)
		} else if strings.EqualFold(it.Name, target) {
			byName = append(byName, it)
		}
	}
	matches := exactID
	if len(matches) == 0 {
		matches = byName
	}
	if len(matches) == 0 {
		return service.Item{}, nil, fmt.Errorf("service %q not found", target)
	}
	if len(matches) > 1 {
		ids := make([]string, 0, len(matches))
		for _, m := range matches {
			ids = append(ids, m.ID)
		}
		return service.Item{}, nil, fmt.Errorf("multiple services match %q; pass exact ID or --source: %s", target, strings.Join(ids, ", "))
	}
	chosen := matches[0]
	p := providers[chosen.Source]
	if p == nil {
		return service.Item{}, nil, fmt.Errorf("provider for %s is unavailable", chosen.Source)
	}
	return chosen, p, nil
}

func runServicesList(env *Env, args []string) error {
	fs := newFlagSet("services list")
	var sourceFilter, stateFilter, query string
	fs.StringVar(&sourceFilter, "source", "", "Filter by source: brew, login-item, user-agent, system-agent, daemon")
	fs.StringVar(&stateFilter, "state", "", "Filter by state: running, stopped, idle, scheduled, error, invalid, none, up, active")
	fs.StringVar(&query, "query", "", "Filter matching name, owner, or path")
	pos, err := parseFlags(fs, args)
	if err != nil {
		return err
	}
	if len(pos) > 0 {
		return usageErrorf("unexpected argument %q for services list", pos[0])
	}
	items, providers, warnings, err := collectServices(env, sourceFilter)
	if err != nil {
		return err
	}
	wantState := strings.ToLower(strings.TrimSpace(stateFilter))
	views := make([]serviceItemView, 0, len(items))
	for _, it := range items {
		if query != "" && !it.Matches(query) {
			continue
		}
		if wantState != "" {
			switch wantState {
			case "up":
				if !it.Up() {
					continue
				}
			case "active":
				if !it.Active() {
					continue
				}
			default:
				if serviceStateKey(it.State) != wantState && !strings.EqualFold(it.State.String(), wantState) {
					continue
				}
			}
		}
		p := providers[it.Source]
		if p == nil {
			continue
		}
		views = append(views, buildServiceItemView(p, it))
	}
	payload := map[string]any{"items": views}
	if len(warnings) > 0 {
		payload["warnings"] = warnings
	}
	return writeResult(env, payload, func(w io.Writer) {
		for _, warn := range warnings {
			if msg := warn["error"]; msg != "" {
				fmt.Fprintf(w, "Warning (%s): %s\n", warn["source"], msg)
			}
			if msg := warn["restriction"]; msg != "" {
				fmt.Fprintf(w, "Note (%s): %s\n", warn["source"], msg)
			}
		}
		if len(views) == 0 {
			fmt.Fprintln(w, "No matching services found.")
			return
		}
		for _, v := range views {
			pidText := ""
			if v.PID > 0 {
				pidText = fmt.Sprintf("pid=%d", v.PID)
			}
			fmt.Fprintf(w, "%s\t[%s]\t%s\t%s\t%s\tactions=%s\n", v.ID, v.Source, v.Name, v.State, pidText, strings.Join(v.Actions, ","))
		}
	})
}

func runServicesInspect(env *Env, args []string) error {
	fs := newFlagSet("services inspect")
	var sourceFilter string
	fs.StringVar(&sourceFilter, "source", "", "Restrict lookup to a specific service source")
	pos, err := parseFlags(fs, args)
	if err != nil {
		return err
	}
	if len(pos) != 1 {
		return usageErrorf("usage: devbase services inspect <id-or-name> [--source <source>]")
	}
	it, p, err := resolveServiceItem(env, pos[0], sourceFilter)
	if err != nil {
		return err
	}
	detail, err := p.Detail(it)
	if err != nil {
		return err
	}
	view := buildServiceItemView(p, it)
	payload := map[string]any{
		"item":   view,
		"detail": detail,
	}
	if (it.Source == service.UserAgent || it.Source == service.SystemAgent || it.Source == service.Daemon) && it.Path != "" {
		if spec, specErr := launchd.LoadSpec(it.Path); specErr == nil {
			payload["spec"] = spec
		}
	}
	return writeResult(env, payload, func(w io.Writer) {
		fmt.Fprintf(w, "Service: %s (%s, %s, source=%s)\n", view.Name, view.ID, view.State, view.Source)
		if view.Path != "" {
			fmt.Fprintf(w, "Path: %s\n", view.Path)
		}
		fmt.Fprintf(w, "Actions: %s\n", strings.Join(view.Actions, ", "))
		for _, row := range detail.Rows {
			fmt.Fprintf(w, "  [%s] %s: %s\n", row.Group, row.Key, row.Value)
		}
		for _, lf := range detail.Logs {
			fmt.Fprintf(w, "  Log (%s): %s\n", lf.Label, lf.Path)
		}
	})
}

func runServicesAction(env *Env, action service.Action, args []string) error {
	fs := newFlagSet("services " + string(action))
	var sourceFilter string
	var yes bool
	fs.StringVar(&sourceFilter, "source", "", "Restrict lookup to a specific service source")
	fs.BoolVar(&yes, "yes", false, "Confirm destructive or unload actions")
	pos, err := parseFlags(fs, args)
	if err != nil {
		return err
	}
	if len(pos) != 1 {
		return usageErrorf("usage: devbase services %s <id-or-name> [--source <source>] [--yes]", action)
	}
	it, p, err := resolveServiceItem(env, pos[0], sourceFilter)
	if err != nil {
		return err
	}
	allowed := p.Actions(it)
	if !service.Has(allowed, action) {
		allowedNames := make([]string, 0, len(allowed))
		for _, a := range allowed {
			allowedNames = append(allowedNames, string(a))
		}
		return fmt.Errorf("action %q is not available for %s (available: %s)", action, it.ID, strings.Join(allowedNames, ", "))
	}
	confirmMsg := p.Confirm(it, action)
	if (action == service.Remove || confirmMsg != "") && !yes {
		if confirmMsg != "" {
			return usageErrorf("%s requires --yes (%s)", action, confirmMsg)
		}
		return usageErrorf("%s requires --yes", action)
	}
	if err := p.Do(it, action); err != nil {
		return err
	}
	return writeResult(env, map[string]any{
		"id":     it.ID,
		"source": string(it.Source),
		"name":   it.Name,
		"action": string(action),
		"status": action.Past(),
	}, func(w io.Writer) {
		fmt.Fprintf(w, "%s %s (%s).\n", it.Name, action.Past(), it.ID)
	})
}

func runServicesAddLoginItem(env *Env, args []string) error {
	if len(args) != 1 || strings.TrimSpace(args[0]) == "" {
		return usageErrorf("usage: devbase services add-login-item <app-path>")
	}
	if env.LoginAdder == nil {
		return errors.New("login item adder is unavailable")
	}
	appPath := strings.TrimSpace(args[0])
	if err := env.LoginAdder.Add(appPath); err != nil {
		return err
	}
	return writeResult(env, map[string]any{"added": appPath}, func(w io.Writer) {
		fmt.Fprintf(w, "Added login item %s.\n", appPath)
	})
}

func runServicesJobSave(env *Env, args []string) error {
	fs := newFlagSet("services job-save")
	var sourceStr, existing, label, command, workingDir, stdoutPath, stderrPath, userName, groupName, rootDir string
	var runAtLoad, keepAlive, restartOnCrash, startOnMount, load, jsonStdin bool
	var startInterval int
	var argList, envPairs stringSliceFlag

	fs.StringVar(&sourceStr, "source", string(service.UserAgent), "launchd domain: user-agent, system-agent, or daemon")
	fs.StringVar(&existing, "existing", "", "Existing property list path to edit")
	fs.BoolVar(&load, "load", false, "Load or reload the saved job in launchd")
	fs.BoolVar(&jsonStdin, "json-stdin", false, "Read launchd.Spec JSON payload from stdin")
	fs.StringVar(&label, "label", "", "launchd job Label")
	fs.StringVar(&command, "command", "", "Shell command wrapped as /bin/sh -c <command>")
	fs.Var(&argList, "arg", "ProgramArguments element (repeatable: binary first, then args)")
	fs.StringVar(&workingDir, "working-dir", "", "WorkingDirectory")
	fs.BoolVar(&runAtLoad, "run-at-load", false, "RunAtLoad")
	fs.BoolVar(&keepAlive, "keep-alive", false, "KeepAlive")
	fs.BoolVar(&restartOnCrash, "restart-on-crash", false, "KeepAlive on non-zero exit + RunAtLoad")
	fs.IntVar(&startInterval, "start-interval", 0, "StartInterval in seconds")
	fs.BoolVar(&startOnMount, "start-on-mount", false, "StartOnMount")
	fs.Var(&envPairs, "env", "Environment variable KEY=VAL (repeatable)")
	fs.StringVar(&stdoutPath, "stdout", "", "StandardOutPath")
	fs.StringVar(&stderrPath, "stderr", "", "StandardErrorPath")
	fs.StringVar(&userName, "user", "", "UserName (daemon only)")
	fs.StringVar(&groupName, "group", "", "GroupName (daemon only)")
	fs.StringVar(&rootDir, "root-dir", "", "RootDirectory (daemon only)")

	pos, err := parseFlags(fs, args)
	if err != nil {
		return err
	}
	if len(pos) > 0 {
		return usageErrorf("unexpected argument %q for services job-save", pos[0])
	}
	src, err := parseServiceSource(sourceStr)
	if err != nil {
		return err
	}
	if src != service.UserAgent && src != service.SystemAgent && src != service.Daemon {
		return usageErrorf("--source for job-save must be user-agent, system-agent, or daemon")
	}

	var spec launchd.Spec
	if jsonStdin {
		raw, err := readJSONStdin(env.In, 512<<10)
		if err != nil {
			return err
		}
		if err := json.Unmarshal(raw, &spec); err != nil {
			return usageErrorf("invalid launchd spec JSON: %v", err)
		}
	} else {
		if existing != "" {
			if loaded, loadErr := launchd.LoadSpec(existing); loadErr == nil {
				spec = loaded
			}
		}
		if command != "" && len(argList) > 0 {
			return usageErrorf("--command and --arg cannot be used together")
		}
		setFlags := map[string]bool{}
		fs.Visit(func(f *flag.Flag) {
			setFlags[f.Name] = true
		})
		if setFlags["label"] {
			spec.Label = strings.TrimSpace(label)
		}
		if command != "" {
			spec.Args = launchd.ShellArgs(command)
		} else if len(argList) > 0 {
			spec.Args = slices.Clone(argList)
		}
		if setFlags["working-dir"] {
			spec.WorkingDir = workingDir
		}
		if setFlags["run-at-load"] {
			spec.RunAtLoad = runAtLoad
		}
		if setFlags["keep-alive"] {
			spec.KeepAlive = keepAlive
			spec.KeepAliveCustom = false
		}
		if setFlags["restart-on-crash"] {
			spec.RestartOnCrash = restartOnCrash
		}
		if setFlags["start-interval"] {
			spec.StartInterval = startInterval
		}
		if setFlags["start-on-mount"] {
			spec.StartOnMount = startOnMount
		}
		if setFlags["stdout"] {
			spec.StdOut = stdoutPath
		}
		if setFlags["stderr"] {
			spec.StdErr = stderrPath
		}
		if setFlags["user"] {
			spec.UserName = userName
		}
		if setFlags["group"] {
			spec.GroupName = groupName
		}
		if setFlags["root-dir"] {
			spec.RootDir = rootDir
		}
		if len(envPairs) > 0 {
			envMap, err := launchd.ParseEnv(strings.Join(envPairs, "\n"))
			if err != nil {
				return usageErrorf("invalid --env: %v", err)
			}
			spec.Env = envMap
		}
	}
	spec.Existing = existing != ""
	if err := spec.Validate(); err != nil {
		return err
	}

	providers := env.providers()
	p := providers[src]
	var savedPath string
	if src == service.UserAgent {
		dir := env.UserAgentDir
		if dir == "" {
			dir = launchd.UserAgents().Dir
		}
		savedPath, err = launchd.Save(dir, spec, existing)
		if err == nil && load {
			if p == nil {
				return errors.New("user-agent provider is unavailable to load job")
			}
			it := service.Item{Name: spec.Label, Path: savedPath, Source: service.UserAgent, Autostart: service.AutostartOn}
			if existing != "" {
				_ = p.Do(it, service.Unload)
			}
			err = p.Do(it, service.Load)
		}
	} else {
		saver, ok := p.(serviceRootSaver)
		if !ok || p == nil {
			return fmt.Errorf("%s provider does not support root save", src)
		}
		savedPath, err = saver.SaveAsRoot(spec, existing, load)
	}
	if err != nil {
		return err
	}
	return writeResult(env, map[string]any{
		"source": string(src),
		"label":  spec.Label,
		"path":   savedPath,
		"loaded": load,
	}, func(w io.Writer) {
		fmt.Fprintf(w, "Saved %s job %s at %s (loaded=%t).\n", src, spec.Label, savedPath, load)
	})
}

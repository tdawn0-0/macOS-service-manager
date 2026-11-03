// Package launchd manages launch agents and daemons.
package launchd

import (
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"

	"howett.net/plist"

	"macos-service-manager/internal/service"
	"macos-service-manager/internal/system"
)

// job is what launchd says of a loaded job.
type job struct {
	PID int // 0 if no process runs
}

// State is what launchd says of a domain: its loaded jobs, and the
// overrides that disable jobs.
type State struct {
	Jobs     map[string]job
	Disabled map[string]bool
}

// ParsePrint reads the output of `launchctl print <domain>` (the
// "services" and "disabled services" blocks of it), which an ordinary user
// can run for the system domain as well as their own. A service line is a
// process ID (0 for none), a last exit status and the label.
func ParsePrint(out string) State {
	st := State{Jobs: map[string]job{}, Disabled: map[string]bool{}}
	block := ""
	for _, line := range strings.Split(out, "\n") {
		trimmed := strings.TrimSpace(line)
		switch {
		case strings.HasPrefix(line, "\tservices = {"):
			block = "services"
			continue
		case strings.HasPrefix(line, "\tdisabled services = {"):
			block = "disabled"
			continue
		case strings.HasPrefix(line, "\t}"):
			block = ""
			continue
		}
		switch block {
		case "services":
			fields := strings.Fields(trimmed)
			if len(fields) < 3 {
				continue
			}
			pid, _ := strconv.Atoi(fields[0])
			st.Jobs[fields[len(fields)-1]] = job{PID: pid}
		case "disabled":
			label, state, ok := strings.Cut(trimmed, " => ")
			if ok {
				st.Disabled[strings.Trim(label, `"`)] = state == "disabled"
			}
		}
	}
	return st
}

// file is the part of a property list the app reads.
type file struct {
	Label             string            `plist:"Label"`
	Program           *string           `plist:"Program"`
	ProgramArguments  []string          `plist:"ProgramArguments"`
	RunAtLoad         *bool             `plist:"RunAtLoad"`
	KeepAlive         any               `plist:"KeepAlive"`
	StartInterval     *int              `plist:"StartInterval"`
	WorkingDirectory  *string           `plist:"WorkingDirectory"`
	StandardOutPath   *string           `plist:"StandardOutPath"`
	StandardErrorPath *string           `plist:"StandardErrorPath"`
	Environment       map[string]string `plist:"EnvironmentVariables"`
}

func readFile(path string) (*file, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		if os.IsNotExist(err) {
			return nil, fmt.Errorf("plist file not found: %s", path)
		}
		return nil, err
	}
	var f file
	if _, err := plist.Unmarshal(data, &f); err != nil {
		return nil, fmt.Errorf("failed to parse plist: %w", err)
	}
	return &f, nil
}

// Config says which launch agents or daemons a provider manages.
type Config struct {
	Source service.Source
	// Dir holds the property lists.
	Dir string
	// Domain is the launchd domain the jobs load into: gui/<uid> or system.
	Domain string
	// Elevated says that changing jobs takes root, which the user is asked
	// to authenticate for.
	Elevated bool
}

// Provider manages the launch agents or daemons of one directory.
type Provider struct{ Config }

// UserAgents manages ~/Library/LaunchAgents.
func UserAgents() *Provider {
	home, _ := os.UserHomeDir()
	return &Provider{Config{service.UserAgent, filepath.Join(home, "Library/LaunchAgents"), guiDomain(), false}}
}

// SystemAgents manages /Library/LaunchAgents. Its jobs load into the user's
// domain, so the user can run them, but cannot change their files.
func SystemAgents() *Provider {
	return &Provider{Config{service.SystemAgent, "/Library/LaunchAgents", guiDomain(), false}}
}

// Daemons manages /Library/LaunchDaemons, which take root to change.
func Daemons() *Provider {
	return &Provider{Config{service.Daemon, "/Library/LaunchDaemons", "system", true}}
}

func guiDomain() string { return fmt.Sprintf("gui/%d", os.Getuid()) }

func (p *Provider) Source() service.Source { return p.Config.Source }

func (p *Provider) state() (State, error) {
	out, err := system.Run("launchctl", "print", p.Domain)
	if err != nil {
		return State{}, err
	}
	return ParsePrint(out), nil
}

func (p *Provider) item(path string, f *file, st State) service.Item {
	it := service.Item{
		ID:     string(p.Config.Source) + ":" + path,
		Source: p.Config.Source,
		Name:   f.Label,
		State:  service.StateStopped,
		Path:   path,
		Owner:  p.Domain,
	}
	if j, ok := st.Jobs[f.Label]; ok {
		it.State = service.StateLoaded
		if j.PID > 0 {
			it.State, it.PID = service.StateRunning, j.PID
		}
	}
	it.Autostart = service.AutostartOn
	if st.Disabled[f.Label] {
		it.Autostart = service.AutostartOff
	}
	return it
}

func (p *Provider) List() ([]service.Item, error) {
	st, err := p.state()
	if err != nil {
		return nil, err
	}
	return p.scan(st), nil
}

// scan lists the jobs of the directory, sorted by label. Files that are not
// property lists with a label are left out.
func (p *Provider) scan(st State) []service.Item {
	items := []service.Item{}
	entries, err := os.ReadDir(p.Dir)
	if err != nil {
		return items
	}
	for _, e := range entries {
		if filepath.Ext(e.Name()) != ".plist" {
			continue
		}
		path := filepath.Join(p.Dir, e.Name())
		if f, err := readFile(path); err == nil && f.Label != "" {
			items = append(items, p.item(path, f, st))
		}
	}
	slices.SortFunc(items, func(a, b service.Item) int { return strings.Compare(a.Name, b.Name) })
	return items
}

func (p *Provider) Detail(it service.Item) (service.Detail, error) {
	f, err := readFile(it.Path)
	if err != nil {
		return service.Detail{}, err
	}
	// The state may have changed since the list.
	st, err := p.state()
	if err != nil {
		return service.Detail{}, err
	}
	return detailOf(p.item(it.Path, f, st), f), nil
}

func detailOf(it service.Item, f *file) service.Detail {
	var d service.Detail
	add := func(k, v string) { d.Rows = append(d.Rows, service.Row{Key: k, Value: v}) }
	str := func(k string, v *string) {
		if v != nil && *v != "" {
			add(k, *v)
		}
	}
	add("label", it.Name)
	add("path", it.Path)
	add("domain", it.Owner)
	add("state", map[bool]string{true: it.State.String(), false: "stopped"}[it.State != service.StateNone])
	if it.PID > 0 {
		add("pid", strconv.Itoa(it.PID))
	}
	add("starts by itself", map[service.Autostart]string{
		service.AutostartOn: "yes", service.AutostartOff: "no (disabled)",
	}[it.Autostart])
	str("program", f.Program)
	if f.ProgramArguments != nil {
		add("program arguments", strings.Join(f.ProgramArguments, " "))
	}
	if f.RunAtLoad != nil {
		add("run at load", strconv.FormatBool(*f.RunAtLoad))
	}
	switch k := f.KeepAlive.(type) {
	case bool:
		add("keep alive", strconv.FormatBool(k))
	case map[string]any:
		add("keep alive", "on conditions")
	}
	if f.StartInterval != nil {
		add("start interval", fmt.Sprintf("%d s", *f.StartInterval))
	}
	str("working directory", f.WorkingDirectory)
	keys := make([]string, 0, len(f.Environment))
	for k := range f.Environment {
		keys = append(keys, k)
	}
	slices.Sort(keys)
	for _, k := range keys {
		add("env "+k, f.Environment[k])
	}
	str("standard out", f.StandardOutPath)
	str("standard error", f.StandardErrorPath)
	if f.StandardOutPath != nil && *f.StandardOutPath != "" {
		d.Logs = append(d.Logs, service.LogFile{Label: "Output", Path: *f.StandardOutPath})
	}
	if f.StandardErrorPath != nil && *f.StandardErrorPath != "" &&
		(f.StandardOutPath == nil || *f.StandardErrorPath != *f.StandardOutPath) {
		d.Logs = append(d.Logs, service.LogFile{Label: "Errors", Path: *f.StandardErrorPath})
	}
	return d
}

func (p *Provider) Actions(it service.Item) []service.Action {
	var actions []service.Action
	switch it.State {
	case service.StateRunning:
		actions = []service.Action{service.Stop, service.Restart, service.Unload}
	case service.StateLoaded:
		actions = []service.Action{service.Start, service.Unload}
	default:
		actions = []service.Action{service.Load}
	}
	if it.Autostart == service.AutostartOff {
		actions = append(actions, service.Enable)
	} else {
		actions = append(actions, service.Disable)
	}
	if p.Config.Source == service.UserAgent {
		actions = append(actions, service.Remove)
	}
	return actions
}

func (p *Provider) Confirm(it service.Item, a service.Action) string {
	switch {
	case a == service.Remove:
		return fmt.Sprintf("Unload %s and move %s to the Trash?", it.Name, filepath.Base(it.Path))
	case p.Elevated && (a == service.Unload || a == service.Disable):
		return fmt.Sprintf("%s is a system daemon. Changing it can affect the software that depends on it. Continue?", it.Name)
	}
	return ""
}

func (p *Provider) launchctl(args ...string) error { return p.launchctls(args) }

// launchctls runs launchctl once for each list of arguments, stopping at
// the first that fails. A provider that needs root runs them all under one
// authentication.
func (p *Provider) launchctls(commands ...[]string) error {
	if !p.Elevated {
		for _, args := range commands {
			if _, err := system.Run("launchctl", args...); err != nil {
				return err
			}
		}
		return nil
	}
	if len(commands) == 1 {
		_, err := system.RunAdmin("launchctl", commands[0]...)
		return err
	}
	script := make([]string, len(commands))
	for i, args := range commands {
		quoted := make([]string, len(args)+1)
		quoted[0] = "launchctl"
		copy(quoted[1:], args)
		for j, a := range quoted {
			quoted[j] = system.ShellQuote(a)
		}
		script[i] = strings.Join(quoted, " ")
	}
	_, err := system.RunAdmin("/bin/sh", "-c", strings.Join(script, " && "))
	return err
}

func (p *Provider) Do(it service.Item, a service.Action) error {
	target := p.Domain + "/" + it.Name
	switch a {
	case service.Load:
		if it.Autostart == service.AutostartOff {
			// A disabled job cannot be loaded.
			return p.launchctls([]string{"enable", target}, []string{"bootstrap", p.Domain, it.Path})
		}
		return p.launchctl("bootstrap", p.Domain, it.Path)
	case service.Unload:
		return p.launchctl("bootout", target)
	case service.Start:
		return p.launchctl("kickstart", target)
	case service.Restart:
		return p.launchctl("kickstart", "-k", target)
	case service.Stop:
		return p.launchctl("kill", "SIGTERM", target)
	case service.Enable:
		return p.launchctl("enable", target)
	case service.Disable:
		return p.launchctl("disable", target)
	case service.Remove:
		if p.Config.Source != service.UserAgent {
			return fmt.Errorf("only user agents can be removed")
		}
		if it.State != service.StateStopped {
			// Not loaded is fine too.
			_ = p.launchctl("bootout", target)
		}
		return system.Trash(it.Path)
	}
	return fmt.Errorf("launchd: unknown action %q", a)
}

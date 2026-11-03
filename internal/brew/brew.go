// Package brew manages Homebrew services with the brew command.
package brew

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	"macos-service-manager/internal/launchd"
	"macos-service-manager/internal/service"
	"macos-service-manager/internal/system"
)

// listed is a service in the output of `brew services list --json`.
type listed struct {
	Name     string  `json:"name"`
	Status   string  `json:"status"`
	User     *string `json:"user"`
	File     string  `json:"file"`
	ExitCode *int    `json:"exit_code"`
}

// info is a service in the output of `brew services info --json`.
type info struct {
	Name         string  `json:"name"`
	ServiceName  string  `json:"service_name"`
	Running      bool    `json:"running"`
	Loaded       bool    `json:"loaded"`
	Schedulable  bool    `json:"schedulable"`
	PID          *int    `json:"pid"`
	ExitCode     *int    `json:"exit_code"`
	User         *string `json:"user"`
	Status       string  `json:"status"`
	File         *string `json:"file"`
	Command      *string `json:"command"`
	WorkingDir   *string `json:"working_dir"`
	RootDir      *string `json:"root_dir"`
	LogPath      *string `json:"log_path"`
	ErrorLogPath *string `json:"error_log_path"`
	Interval     *int    `json:"interval"`
	Cron         *string `json:"cron"`
}

// parseList reads the output of `brew services list --json`.
func parseList(data []byte) ([]listed, error) {
	var services []listed
	if err := json.Unmarshal(data, &services); err != nil {
		return nil, fmt.Errorf("parse brew services: %w", err)
	}
	return services, nil
}

func parseInfo(data []byte) (*info, error) {
	var infos []info
	if err := json.Unmarshal(data, &infos); err != nil {
		return nil, fmt.Errorf("parse brew service info: %w", err)
	}
	if len(infos) == 0 {
		return nil, fmt.Errorf("no data")
	}
	return &infos[0], nil
}

// stateOf reads a status of `brew services`: "none" is not loaded, while
// "stopped", "unknown" and "other" are loaded with no process.
func stateOf(status string) service.State {
	switch status {
	case "started":
		return service.StateRunning
	case "scheduled":
		return service.StateScheduled
	case "error":
		return service.StateFailed
	case "none", "":
		return service.StateStopped
	}
	return service.StateLoaded
}

// Provider is the Homebrew services of the machine.
type Provider struct {
	// AgentDir is where a service registered to start at login has its
	// property list, and DaemonDir where one registered as root to start at
	// boot has: a service is registered if one is there. `brew services
	// list` says only that a service is started, not whether it will be.
	AgentDir, DaemonDir string
}

// New returns the provider of the machine's Homebrew services.
func New() *Provider {
	home, _ := os.UserHomeDir()
	return &Provider{
		AgentDir:  filepath.Join(home, "Library/LaunchAgents"),
		DaemonDir: "/Library/LaunchDaemons",
	}
}

func (*Provider) Source() service.Source { return service.Brew }

// registered returns the property list in dir that registers the service, if
// there is one. Homebrew names it sh.brew.<name>.plist, or
// homebrew.mxcl.<name>.plist in older versions.
func registered(dir, name string) string {
	for _, file := range []string{"sh.brew." + name + ".plist", "homebrew.mxcl." + name + ".plist"} {
		path := filepath.Join(dir, file)
		if _, err := os.Stat(path); err == nil {
			return path
		}
	}
	return ""
}

// labelOf is the launchd label of the job a service's property list defines.
func labelOf(path string) string {
	return strings.TrimSuffix(filepath.Base(path), ".plist")
}

// items reads the services. Those registered as root are jobs of the system
// domain, of which `brew services` run as the user reports nothing: their
// state comes from launchd.
func (p *Provider) items(services []listed) []service.Item {
	var system_ *launchd.State
	items := make([]service.Item, 0, len(services))
	for _, s := range services {
		it := service.Item{
			ID:        "brew:" + s.Name,
			Source:    service.Brew,
			Name:      s.Name,
			State:     stateOf(s.Status),
			Autostart: service.AutostartOff,
			Path:      s.File,
		}
		if s.User != nil {
			it.Owner = *s.User
		}
		if plist := registered(p.AgentDir, s.Name); plist != "" {
			it.Autostart, it.Path = service.AutostartOn, plist
		} else if plist := registered(p.DaemonDir, s.Name); plist != "" {
			it.Autostart, it.Path, it.Admin, it.Owner = service.AutostartOn, plist, true, "root"
			if system_ == nil {
				system_ = &launchd.State{}
				if out, err := system.Run("launchctl", "print", "system"); err == nil {
					*system_ = launchd.ParsePrint(out)
				}
			}
			it.State, it.PID = system_.Of(labelOf(plist))
		}
		items = append(items, it)
	}
	return items
}

func (p *Provider) List() ([]service.Item, error) {
	out, err := system.Run("brew", "services", "list", "--json")
	if err != nil {
		return nil, err
	}
	services, err := parseList([]byte(out))
	if err != nil {
		return nil, err
	}
	return p.items(services), nil
}

func (p *Provider) Detail(it service.Item) (service.Detail, error) {
	out, err := system.Run("brew", "services", "info", it.Name, "--json")
	if err != nil {
		return service.Detail{}, err
	}
	i, err := parseInfo([]byte(out))
	if err != nil {
		return service.Detail{}, err
	}
	return detailOf(i, it), nil
}

func detailOf(i *info, it service.Item) service.Detail {
	var d service.Detail
	add := func(group, k, v string) {
		if v != "" {
			d.Rows = append(d.Rows, service.Row{Group: group, Key: k, Value: v})
		}
	}
	str := func(v *string) string {
		if v == nil {
			return ""
		}
		return *v
	}
	num := func(v *int) string {
		if v == nil {
			return ""
		}
		return strconv.Itoa(*v)
	}
	const status, program, files = "Status", "Program", "Files"
	switch {
	case it.PID > 0:
		add(status, "Process ID", strconv.Itoa(it.PID))
	default:
		add(status, "Process ID", num(i.PID))
	}
	add(status, "Last exit code", num(i.ExitCode))
	if it.Admin {
		add(status, "Runs as", "root")
	} else {
		add(status, "Runs as", str(i.User))
	}
	add(status, "Service name", i.ServiceName)
	add(program, "Command", str(i.Command))
	add(program, "Working directory", str(i.WorkingDir))
	add(program, "Root directory", str(i.RootDir))
	if i.Interval != nil {
		add(program, "Runs every", fmt.Sprintf("%d s", *i.Interval))
	}
	add(program, "Schedule", str(i.Cron))
	add(files, "Property list", it.Path)
	add(files, "Output log", str(i.LogPath))
	add(files, "Error log", str(i.ErrorLogPath))
	if i.LogPath != nil && *i.LogPath != "" {
		d.Logs = append(d.Logs, service.LogFile{Label: "Output", Path: *i.LogPath})
	}
	if i.ErrorLogPath != nil && *i.ErrorLogPath != "" && (i.LogPath == nil || *i.ErrorLogPath != *i.LogPath) {
		d.Logs = append(d.Logs, service.LogFile{Label: "Errors", Path: *i.ErrorLogPath})
	}
	return d
}

func (*Provider) Actions(it service.Item) []service.Action {
	var actions []service.Action
	if it.Active() {
		actions = []service.Action{service.Stop, service.Restart}
	} else {
		actions = []service.Action{service.Start}
	}
	switch {
	case it.Autostart == service.AutostartOn:
		return append(actions, service.Disable)
	case it.Admin:
		return actions
	}
	return append(actions, service.Enable)
}

func (*Provider) Confirm(it service.Item, a service.Action) string {
	if it.Admin && a == service.Disable {
		return fmt.Sprintf("%s runs as root, at boot. This stops it and deletes %s.", it.Name, filepath.Base(it.Path))
	}
	return ""
}

// Do runs an action. Starting and stopping leave whether the service starts
// at login as it is, and turning that on or off leaves the service running or
// stopped: brew's own words mix the two, `start` and `stop` also registering
// and unregistering.
func (p *Provider) Do(it service.Item, a service.Action) error {
	if it.Admin {
		return doAsRoot(it, a)
	}
	services := func(args ...string) error {
		_, err := system.Run("brew", append(append([]string{"services"}, args...), it.Name)...)
		return err
	}
	registeredNow := it.Autostart == service.AutostartOn
	switch a {
	case service.Start:
		switch {
		case it.State == service.StateLoaded:
			// Both refuse a service that is loaded.
			return services("restart")
		case registeredNow:
			return services("start")
		}
		return services("run")
	case service.Stop:
		if registeredNow {
			return services("stop", "--keep")
		}
		return services("stop")
	case service.Restart:
		return services("restart")
	case service.Enable:
		return p.register(it)
	case service.Disable:
		return p.unregister(it)
	}
	return fmt.Errorf("brew: unknown action %q", a)
}

// register makes the service start at login, as `brew services start` does
// but for starting it now: it puts the service's property list where launchd
// looks at login.
func (p *Provider) register(it service.Item) error {
	data, err := os.ReadFile(it.Path)
	if err != nil {
		return fmt.Errorf("read the service file: %w", err)
	}
	if err := os.MkdirAll(p.AgentDir, 0o755); err != nil {
		return err
	}
	return os.WriteFile(filepath.Join(p.AgentDir, filepath.Base(it.Path)), data, 0o644)
}

// unregister keeps the service from starting at login, as `brew services
// stop` does but for stopping it now.
func (p *Provider) unregister(it service.Item) error {
	path := registered(p.AgentDir, it.Name)
	if path == "" {
		return nil
	}
	return os.Remove(path)
}

// doAsRoot changes a service registered as root. Homebrew's commands, run as
// the user, cannot reach it, and launchctl is all they would run.
func doAsRoot(it service.Item, a service.Action) error {
	target := "system/" + labelOf(it.Path)
	admin := func(args ...string) error {
		_, err := system.RunAdmin("launchctl", args...)
		return err
	}
	switch a {
	case service.Start:
		if it.State == service.StateStopped {
			return admin("bootstrap", "system", it.Path)
		}
		return admin("kickstart", target)
	case service.Stop:
		return admin("bootout", target)
	case service.Restart:
		return admin("kickstart", "-k", target)
	case service.Disable:
		// Not loaded is fine too.
		script := "launchctl bootout " + system.ShellQuote(target) + "; rm -f " + system.ShellQuote(it.Path)
		_, err := system.RunAdmin("/bin/sh", "-c", script)
		return err
	}
	return fmt.Errorf("brew: %s does not apply to a service of root", a)
}

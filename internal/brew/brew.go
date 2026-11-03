// Package brew manages Homebrew services with the brew command.
package brew

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strconv"

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

// ParseList reads the output of `brew services list --json`.
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

func stateOf(status string) service.State {
	switch status {
	case "started":
		return service.StateRunning
	case "scheduled":
		return service.StateScheduled
	case "error":
		return service.StateFailed
	}
	return service.StateStopped
}

// Provider is the Homebrew services of the machine.
type Provider struct {
	// Registered lists the directories where `brew services start`
	// installs the property list of a service it registers to start by
	// itself: a service is registered if one is there. `brew services
	// list` says only that a service is started, not whether it will be at
	// login, and its file field is the same either way.
	Registered []string
}

// New returns the provider of the machine's Homebrew services.
func New() *Provider {
	p := &Provider{}
	if home, err := os.UserHomeDir(); err == nil {
		p.Registered = append(p.Registered, filepath.Join(home, "Library/LaunchAgents"))
	}
	p.Registered = append(p.Registered, "/Library/LaunchDaemons")
	return p
}

func (*Provider) Source() service.Source { return service.Brew }

// registeredPlist returns the property list that registers the service, if
// there is one. Homebrew names it sh.brew.<name>.plist, or
// homebrew.mxcl.<name>.plist in older versions.
func (p *Provider) registeredPlist(name string, dirEntries map[string][]string) string {
	for _, dir := range p.Registered {
		for _, file := range dirEntries[dir] {
			if file == "sh.brew."+name+".plist" || file == "homebrew.mxcl."+name+".plist" {
				return filepath.Join(dir, file)
			}
		}
	}
	return ""
}

func (p *Provider) readDirs() map[string][]string {
	entries := map[string][]string{}
	for _, dir := range p.Registered {
		files, err := os.ReadDir(dir)
		if err != nil {
			continue
		}
		for _, f := range files {
			entries[dir] = append(entries[dir], f.Name())
		}
	}
	return entries
}

func (p *Provider) items(services []listed) []service.Item {
	dirs := p.readDirs()
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
		if plist := p.registeredPlist(s.Name, dirs); plist != "" {
			it.Autostart = service.AutostartOn
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
	add := func(k, v string) { d.Rows = append(d.Rows, service.Row{Key: k, Value: v}) }
	str := func(k string, v *string) {
		if v != nil && *v != "" {
			add(k, *v)
		}
	}
	num := func(k string, v *int) {
		if v != nil {
			add(k, strconv.Itoa(*v))
		}
	}
	add("name", i.Name)
	add("service name", i.ServiceName)
	add("status", i.Status)
	add("running", strconv.FormatBool(i.Running))
	add("loaded", strconv.FormatBool(i.Loaded))
	add("starts at login", map[service.Autostart]string{
		service.AutostartOn: "yes", service.AutostartOff: "no", service.AutostartUnknown: "unknown",
	}[it.Autostart])
	num("pid", i.PID)
	num("exit code", i.ExitCode)
	str("user", i.User)
	str("file", i.File)
	str("command", i.Command)
	str("working dir", i.WorkingDir)
	str("root dir", i.RootDir)
	str("log", i.LogPath)
	str("error log", i.ErrorLogPath)
	num("interval", i.Interval)
	str("cron", i.Cron)
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
		actions = []service.Action{service.Stop, service.Restart, service.Kill}
	} else {
		actions = []service.Action{service.Run, service.Start}
	}
	if it.Autostart == service.AutostartOn {
		return append(actions, service.Disable)
	}
	return append(actions, service.Enable)
}

func (*Provider) Confirm(it service.Item, a service.Action) string {
	if (a == service.Enable || a == service.Disable) && it.Active() {
		return fmt.Sprintf("Homebrew cannot change whether a service starts at login while it runs, "+
			"so %s will be stopped and started again. It is interrupted for a few seconds.", it.Name)
	}
	return ""
}

func (*Provider) Do(it service.Item, a service.Action) error {
	services := func(command, name string) error {
		_, err := system.Run("brew", "services", command, name)
		return err
	}
	switch a {
	case service.Run, service.Start, service.Stop, service.Restart, service.Kill:
		return services(string(a), it.Name)
	case service.Enable:
		// start refuses a service that runs, so stop it first.
		if it.Active() {
			if err := services("stop", it.Name); err != nil {
				return err
			}
		}
		return services("start", it.Name)
	case service.Disable:
		// stop unregisters the service as it stops it: run it again if it ran.
		if err := services("stop", it.Name); err != nil {
			return err
		}
		if it.Active() {
			return services("run", it.Name)
		}
		return nil
	}
	return fmt.Errorf("brew: unknown action %q", a)
}

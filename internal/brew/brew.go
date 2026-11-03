// Package brew manages Homebrew services with the brew command.
package brew

import (
	"encoding/json"
	"fmt"
	"strconv"

	"macos-service-manager/internal/detail"
	"macos-service-manager/internal/system"
)

// Command is an action of `brew services`.
type Command string

const (
	Start   Command = "start"
	Stop    Command = "stop"
	Restart Command = "restart"
	Run     Command = "run"
	Kill    Command = "kill"
)

// Service is a Homebrew service in the list.
type Service struct {
	Name     string  `json:"name"`
	Status   string  `json:"status"`
	User     *string `json:"user"`
	File     string  `json:"file"`
	ExitCode *int    `json:"exit_code"`
}

// Active reports whether the service runs, or is scheduled to, or failed
// while running: the states in which the list offers to stop it.
func (s Service) Active() bool {
	switch s.Status {
	case "started", "scheduled", "error":
		return true
	}
	return false
}

// Info is the detail of a Homebrew service.
type Info struct {
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

// Rows lists the values of the info to show, in order.
func (i Info) Rows() []detail.Row {
	var r detail.Rows
	r.Add("name", i.Name)
	r.Add("service_name", i.ServiceName)
	r.Add("running", strconv.FormatBool(i.Running))
	r.Add("loaded", strconv.FormatBool(i.Loaded))
	r.Add("schedulable", strconv.FormatBool(i.Schedulable))
	r.AddInt("pid", i.PID)
	r.AddInt("exit_code", i.ExitCode)
	r.AddString("user", i.User)
	r.Add("status", i.Status)
	r.AddString("file", i.File)
	r.AddString("command", i.Command)
	r.AddString("working_dir", i.WorkingDir)
	r.AddString("root_dir", i.RootDir)
	r.AddLog("log_path", i.LogPath)
	r.AddLog("error_log_path", i.ErrorLogPath)
	r.AddInt("interval", i.Interval)
	r.AddString("cron", i.Cron)
	return r
}

// ParseList reads the output of `brew services list --json`.
func ParseList(data []byte) ([]Service, error) {
	var services []Service
	if err := json.Unmarshal(data, &services); err != nil {
		return nil, fmt.Errorf("parse brew services: %w", err)
	}
	return services, nil
}

// ParseInfo reads the output of `brew services info <name> --json`.
func ParseInfo(data []byte) (*Info, error) {
	var infos []Info
	if err := json.Unmarshal(data, &infos); err != nil {
		return nil, fmt.Errorf("parse brew service info: %w", err)
	}
	if len(infos) == 0 {
		return nil, fmt.Errorf("no data")
	}
	return &infos[0], nil
}

// CLI runs the real brew command.
type CLI struct{}

// List returns the services Homebrew knows.
func (CLI) List() ([]Service, error) {
	out, err := system.Run("brew", "services", "list", "--json")
	if err != nil {
		return nil, err
	}
	return ParseList([]byte(out))
}

// Info returns the detail of a service.
func (CLI) Info(name string) (*Info, error) {
	out, err := system.Run("brew", "services", "info", name, "--json")
	if err != nil {
		return nil, err
	}
	return ParseInfo([]byte(out))
}

// Manage runs a command on a service.
func (CLI) Manage(name string, command Command) error {
	_, err := system.Run("brew", "services", string(command), name)
	return err
}

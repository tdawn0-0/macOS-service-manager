// Package launchd manages the launch agents of the user and the system.
package launchd

import (
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"

	"howett.net/plist"

	"macos-service-manager/internal/detail"
	"macos-service-manager/internal/system"
)

// The domains say whose launch agents a service belongs to.
const (
	DomainUser   = "user"
	DomainSystem = "system"
)

// Command is an action on a launch agent.
type Command string

const (
	Load   Command = "load"
	Unload Command = "unload"
	Start  Command = "start"
	Stop   Command = "stop"
)

// Service is a launch agent in the list.
type Service struct {
	Label   string
	Path    string
	Domain  string
	Loaded  bool
	Running bool
	PID     *int
}

// Info is the detail of a launch agent.
type Info struct {
	Service
	Program           *string
	ProgramArguments  []string
	RunAtLoad         *bool
	KeepAlive         *bool
	WorkingDirectory  *string
	StandardOutPath   *string
	StandardErrorPath *string
}

// Rows lists the values of the info to show, in order.
func (i Info) Rows() []detail.Row {
	var r detail.Rows
	r.Add("label", i.Label)
	r.Add("path", i.Path)
	r.Add("domain", i.Domain)
	r.Add("loaded", strconv.FormatBool(i.Loaded))
	r.Add("running", strconv.FormatBool(i.Running))
	r.AddInt("pid", i.PID)
	r.AddString("program", i.Program)
	if i.ProgramArguments != nil {
		r.Add("program_arguments", strings.Join(i.ProgramArguments, " "))
	}
	r.AddBool("run_at_load", i.RunAtLoad)
	r.AddBool("keep_alive", i.KeepAlive)
	r.AddString("working_directory", i.WorkingDirectory)
	r.AddLog("standard_out_path", i.StandardOutPath)
	r.AddLog("standard_error_path", i.StandardErrorPath)
	return r
}

// file is the part of a property list the app reads.
type file struct {
	Label             string   `plist:"Label"`
	Program           *string  `plist:"Program"`
	ProgramArguments  []string `plist:"ProgramArguments"`
	RunAtLoad         *bool    `plist:"RunAtLoad"`
	KeepAlive         any      `plist:"KeepAlive"`
	WorkingDirectory  *string  `plist:"WorkingDirectory"`
	StandardOutPath   *string  `plist:"StandardOutPath"`
	StandardErrorPath *string  `plist:"StandardErrorPath"`
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

// Jobs maps the labels of the loaded jobs to their process IDs, nil for a
// job that does not run.
type Jobs map[string]*int

// ParseJobs reads the output of `launchctl list`: a header line, then a
// line of PID, last exit status and label for each job.
func ParseJobs(output string) Jobs {
	jobs := Jobs{}
	for i, line := range strings.Split(output, "\n") {
		fields := strings.Fields(line)
		if i == 0 || len(fields) < 3 {
			continue
		}
		var pid *int
		if n, err := strconv.Atoi(fields[0]); err == nil {
			pid = &n
		}
		jobs[fields[2]] = pid
	}
	return jobs
}

func (j Jobs) apply(s *Service) {
	pid, loaded := j[s.Label]
	s.Loaded = loaded
	s.PID = pid
	s.Running = pid != nil
}

// Dir is a directory of launch agents.
type Dir struct {
	Path   string
	Domain string
}

// Dirs returns the directories of launch agents that exist.
func Dirs() []Dir {
	var dirs []Dir
	if home, err := os.UserHomeDir(); err == nil {
		dirs = append(dirs, Dir{filepath.Join(home, "Library/LaunchAgents"), DomainUser})
	}
	dirs = append(dirs, Dir{"/Library/LaunchAgents", DomainSystem})
	return slices.DeleteFunc(dirs, func(d Dir) bool {
		info, err := os.Stat(d.Path)
		return err != nil || !info.IsDir()
	})
}

// Scan lists the launch agents in directories, sorted by label. Files that
// are not property lists with a label are left out.
func Scan(dirs []Dir, jobs Jobs) []Service {
	services := []Service{}
	for _, dir := range dirs {
		entries, err := os.ReadDir(dir.Path)
		if err != nil {
			continue
		}
		for _, entry := range entries {
			if filepath.Ext(entry.Name()) != ".plist" {
				continue
			}
			path := filepath.Join(dir.Path, entry.Name())
			f, err := readFile(path)
			if err != nil || f.Label == "" {
				continue
			}
			s := Service{Label: f.Label, Path: path, Domain: dir.Domain}
			jobs.apply(&s)
			services = append(services, s)
		}
	}
	slices.SortFunc(services, func(a, b Service) int { return strings.Compare(a.Label, b.Label) })
	return services
}

// DomainOf tells whose launch agent the file at path is.
func DomainOf(path string) string {
	if strings.HasPrefix(path, "/Library/") {
		return DomainSystem
	}
	return DomainUser
}

func buildInfo(path string, f *file, jobs Jobs) (*Info, error) {
	if f.Label == "" {
		return nil, fmt.Errorf("missing Label in plist")
	}
	info := &Info{
		Service:           Service{Label: f.Label, Path: path, Domain: DomainOf(path)},
		Program:           f.Program,
		ProgramArguments:  f.ProgramArguments,
		RunAtLoad:         f.RunAtLoad,
		WorkingDirectory:  f.WorkingDirectory,
		StandardOutPath:   f.StandardOutPath,
		StandardErrorPath: f.StandardErrorPath,
	}
	// KeepAlive may be a dictionary of conditions, which has no single value.
	if keepAlive, ok := f.KeepAlive.(bool); ok {
		info.KeepAlive = &keepAlive
	}
	jobs.apply(&info.Service)
	return info, nil
}

// Launchctl talks to launchctl.
type Launchctl struct{}

func (Launchctl) jobs() (Jobs, error) {
	out, err := system.Run("launchctl", "list")
	if err != nil {
		return nil, err
	}
	return ParseJobs(out), nil
}

// List returns the launch agents of the user and the system.
func (s Launchctl) List() ([]Service, error) {
	jobs, err := s.jobs()
	if err != nil {
		return nil, err
	}
	return Scan(Dirs(), jobs), nil
}

// Info returns the detail of the launch agent whose file is at path.
func (s Launchctl) Info(path string) (*Info, error) {
	f, err := readFile(path)
	if err != nil {
		return nil, err
	}
	jobs, err := s.jobs()
	if err != nil {
		return nil, err
	}
	return buildInfo(path, f, jobs)
}

// Manage runs a command on a launch agent in the user's GUI domain.
func (Launchctl) Manage(label, path string, command Command) error {
	domain := fmt.Sprintf("gui/%d", os.Getuid())
	target := domain + "/" + label
	var args []string
	switch command {
	case Load:
		args = []string{"bootstrap", domain, path}
	case Unload:
		args = []string{"bootout", target}
	case Start:
		args = []string{"kickstart", "-k", target}
	case Stop:
		args = []string{"kill", "SIGTERM", target}
	default:
		return fmt.Errorf("unknown command %q", command)
	}
	_, err := system.Run("launchctl", args...)
	return err
}

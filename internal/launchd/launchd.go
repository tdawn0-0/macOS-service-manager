// Package launchd manages launch agents and daemons.
package launchd

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"

	"howett.net/plist"

	"devbase/internal/privhelper"
	"devbase/internal/service"
	"devbase/internal/system"
)

// Job is what launchd says of a loaded job.
type Job struct {
	PID int // 0 if no process runs
	// Exit is its last exit status: negative for the signal that ended it,
	// 0 if it never ran or launchd prints no number.
	Exit int
}

// State is what launchd says of a domain: its loaded jobs, and the
// overrides that disable jobs.
type State struct {
	Jobs     map[string]Job
	Disabled map[string]bool
}

// Of returns the state of the job with the label, and its process ID. A job
// ended by a signal did not fail: that is how a job is stopped.
func (st State) Of(label string) (service.State, int) {
	j, ok := st.Jobs[label]
	switch {
	case !ok:
		return service.StateStopped, 0
	case j.PID > 0:
		return service.StateRunning, j.PID
	case j.Exit > 0:
		return service.StateFailed, 0
	}
	return service.StateLoaded, 0
}

// ParsePrint reads the output of `launchctl print <domain>` (the
// "services" and "disabled services" blocks of it), which an ordinary user
// can run for the system domain as well as their own. A service line is a
// process ID (0 for none), a last exit status and the label.
func ParsePrint(out string) State {
	st := State{Jobs: map[string]Job{}, Disabled: map[string]bool{}}
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
			exit, _ := strconv.Atoi(fields[1])
			st.Jobs[fields[len(fields)-1]] = Job{PID: pid, Exit: exit}
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
	Label                 string            `plist:"Label"`
	Disabled              *bool             `plist:"Disabled"`
	Program               *string           `plist:"Program"`
	ProgramArguments      []string          `plist:"ProgramArguments"`
	RunAtLoad             *bool             `plist:"RunAtLoad"`
	KeepAlive             any               `plist:"KeepAlive"`
	StartInterval         *int              `plist:"StartInterval"`
	StartCalendarInterval any               `plist:"StartCalendarInterval"`
	WorkingDirectory      *string           `plist:"WorkingDirectory"`
	StandardOutPath       *string           `plist:"StandardOutPath"`
	StandardErrorPath     *string           `plist:"StandardErrorPath"`
	Environment           map[string]string `plist:"EnvironmentVariables"`
	StartOnMount          *bool             `plist:"StartOnMount"`
	QueueDirectories      []string          `plist:"QueueDirectories"`
	UserName              *string           `plist:"UserName"`
	GroupName             *string           `plist:"GroupName"`
	RootDirectory         *string           `plist:"RootDirectory"`
}

// scheduled reports whether the job runs at times of its own.
func (f *file) scheduled() bool { return f.StartInterval != nil || f.StartCalendarInterval != nil }

// keepAlive reports whether launchd may start the job again when its process
// ends. Conditions count too: whether a signal meets them depends on the
// condition, so only unloading the job is sure to stop it.
func (f *file) keepAlive() bool {
	switch k := f.KeepAlive.(type) {
	case nil:
		return false
	case bool:
		return k
	}
	return true
}

// disabledIn reports whether launchd keeps the job from loading: as the
// domain's overrides say, or else as its file does.
func (f *file) disabledIn(st State) bool {
	if off, ok := st.Disabled[f.Label]; ok {
		return off
	}
	return f.Disabled != nil && *f.Disabled
}

// runs says in words when the job runs, once loaded.
func (f *file) runs() string {
	var when []string
	if f.RunAtLoad != nil && *f.RunAtLoad {
		when = append(when, "when loaded")
	}
	switch k := f.KeepAlive.(type) {
	case bool:
		if k {
			when = append(when, "kept alive")
		}
	case nil:
	case map[string]any:
		if success, ok := k["SuccessfulExit"].(bool); ok && !success && len(k) == 1 {
			when = append(when, "restarted after a crash")
		} else {
			when = append(when, "kept alive on conditions")
		}
	default:
		when = append(when, "kept alive on conditions")
	}
	if f.StartInterval != nil {
		when = append(when, fmt.Sprintf("every %d s", *f.StartInterval))
	}
	if f.StartCalendarInterval != nil {
		when = append(when, "on a schedule")
	}
	if f.StartOnMount != nil && *f.StartOnMount {
		when = append(when, "when a disk mounts")
	}
	if len(f.QueueDirectories) > 0 {
		when = append(when, "while a queue has files")
	}
	if len(when) == 0 {
		return "On demand"
	}
	s := strings.Join(when, ", ")
	return strings.ToUpper(s[:1]) + s[1:]
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
	// Elevated says that changing jobs takes root, which runs through the
	// privileged helper.
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
		Path:   path,
		Owner:  p.Domain,
		Admin:  p.Elevated,
	}
	it.State, it.PID = st.Of(f.Label)
	if it.State == service.StateLoaded && f.scheduled() {
		it.State = service.StateScheduled
	}
	it.KeepAlive = f.keepAlive()
	it.Autostart = service.AutostartOn
	if f.disabledIn(st) {
		it.Autostart = service.AutostartOff
	}
	return it
}

// invalid is the item of a property list launchd cannot load.
func (p *Provider) invalid(path string) service.Item {
	return service.Item{
		ID:        string(p.Config.Source) + ":" + path,
		Source:    p.Config.Source,
		Name:      strings.TrimSuffix(filepath.Base(path), ".plist"),
		State:     service.StateInvalid,
		Path:      path,
		Owner:     p.Domain,
		Admin:     p.Elevated,
		Kind:      invalidKind,
		Autostart: service.AutostartUnknown,
	}
}

// invalidKind is what an invalid property list is.
const invalidKind = "Invalid property list"

// problem says why launchd cannot load the file at path, or "" if it can.
func problem(path string) string {
	f, err := readFile(path)
	switch {
	case err != nil:
		return err.Error()
	case f.Label == "":
		return "it has no Label, so launchd cannot load it"
	}
	return ""
}

func (p *Provider) List() ([]service.Item, error) {
	st, err := p.state()
	if err != nil {
		return nil, err
	}
	return p.scan(st), nil
}

// scan lists the jobs of the directory, sorted by label. A .plist file that
// is not a property list with a label is listed as invalid, by its file name:
// launchd cannot load it, which is what someone looking here wants to know.
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
		} else {
			items = append(items, p.invalid(path))
		}
	}
	slices.SortFunc(items, func(a, b service.Item) int { return strings.Compare(a.Name, b.Name) })
	return items
}

func (p *Provider) Detail(it service.Item) (service.Detail, error) {
	if it.State == service.StateInvalid {
		if _, err := os.Stat(it.Path); err != nil {
			return service.Detail{}, fmt.Errorf("plist file not found: %s", it.Path)
		}
		return service.Detail{Rows: []service.Row{
			{Group: "Status", Key: "Problem", Value: problem(it.Path)},
			{Group: "Files", Key: "Property list", Value: it.Path},
		}}, nil
	}
	f, err := readFile(it.Path)
	if err != nil {
		return service.Detail{}, err
	}
	// The state may have changed since the list.
	st, err := p.state()
	if err != nil {
		return service.Detail{}, err
	}
	return detailOf(p.item(it.Path, f, st), f, st), nil
}

func detailOf(it service.Item, f *file, st State) service.Detail {
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
	const status, program, environment, files = "Status", "Program", "Environment", "Files"
	if it.PID > 0 {
		add(status, "Process ID", strconv.Itoa(it.PID))
	}
	if j := st.Jobs[it.Name]; j.Exit != 0 {
		add(status, "Last exit code", strconv.Itoa(j.Exit))
	}
	add(status, "Runs", f.runs())
	add(status, "Label", it.Name)
	add(status, "Domain", it.Owner)
	command := str(f.Program)
	if f.ProgramArguments != nil {
		command = strings.Join(f.ProgramArguments, " ")
	}
	add(program, "Command", command)
	add(program, "Working directory", str(f.WorkingDirectory))
	keys := make([]string, 0, len(f.Environment))
	for k := range f.Environment {
		keys = append(keys, k)
	}
	slices.Sort(keys)
	for _, k := range keys {
		add(environment, k, f.Environment[k])
	}
	add(files, "Property list", it.Path)
	add(files, "Output log", str(f.StandardOutPath))
	add(files, "Error log", str(f.StandardErrorPath))
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
	if it.State == service.StateInvalid {
		// Its label is unknown, so nothing can address a job of it. Only a
		// file of the user's own can go, without asking launchd anything.
		if p.Config.Source == service.UserAgent {
			return []service.Action{service.Remove}
		}
		return nil
	}
	var actions []service.Action
	switch {
	case it.State == service.StateRunning && it.KeepAlive:
		// Stopping it unloads it: Unload would do the same.
		actions = []service.Action{service.Stop, service.Restart}
	case it.State == service.StateRunning:
		actions = []service.Action{service.Stop, service.Restart, service.Unload}
	case it.State == service.StateLoaded, it.State == service.StateScheduled, it.State == service.StateFailed:
		actions = []service.Action{service.Start, service.Unload}
	default:
		actions = []service.Action{service.Load}
	}
	if it.Autostart == service.AutostartOff {
		actions = append(actions, service.Enable)
	} else {
		actions = append(actions, service.Disable)
	}
	return append(actions, service.Remove)
}

func (p *Provider) Confirm(it service.Item, a service.Action) string {
	switch {
	case a == service.Remove && it.State == service.StateInvalid:
		return fmt.Sprintf("Move %s to the Trash? launchd cannot load it as it is.", filepath.Base(it.Path))
	case a == service.Remove && p.Config.Source == service.UserAgent:
		return fmt.Sprintf("Unload %s and move %s to the Trash?", it.Name, filepath.Base(it.Path))
	case a == service.Remove:
		return fmt.Sprintf("This unloads the job and moves its file to the Trash, which is not an undo: Put Back cannot return it to the %s folder. To get it back, reinstall the software it came from, or install the file again as root. Until then that software may stop working.", filepath.Base(filepath.Dir(it.Path)))
	case p.Elevated && (a == service.Unload || a == service.Disable):
		return fmt.Sprintf("%s is a system daemon. Changing it can affect the software that depends on it. Continue?", it.Name)
	}
	return ""
}

func (p *Provider) launchctl(args ...string) error { return p.launchctls(args) }

// launchctls runs launchctl once for each list of arguments, stopping at
// the first that fails. A provider that needs root sends them all to the
// privileged helper in one request.
func (p *Provider) launchctls(commands ...[]string) error {
	if !p.Elevated {
		for _, args := range commands {
			if _, err := system.Run("launchctl", args...); err != nil {
				return err
			}
		}
		return nil
	}
	hc, err := privhelper.Require()
	if err != nil {
		return err
	}
	return hc.LaunchdControl(context.Background(), commands)
}

// commands returns what launchctl runs for an action on an item.
func (p *Provider) commands(it service.Item, a service.Action) ([][]string, error) {
	if it.State == service.StateInvalid {
		return nil, fmt.Errorf("%s is not a job launchd can load", filepath.Base(it.Path))
	}
	target := p.Domain + "/" + it.Name
	switch a {
	case service.Load:
		if it.Autostart == service.AutostartOff {
			// A disabled job cannot be loaded.
			return [][]string{{"enable", target}, {"bootstrap", p.Domain, it.Path}}, nil
		}
		return [][]string{{"bootstrap", p.Domain, it.Path}}, nil
	case service.Unload:
		return [][]string{{"bootout", target}}, nil
	case service.Start:
		return [][]string{{"kickstart", target}}, nil
	case service.Restart:
		return [][]string{{"kickstart", "-k", target}}, nil
	case service.Stop:
		if it.KeepAlive {
			// launchd would start it again at once.
			return [][]string{{"bootout", target}}, nil
		}
		return [][]string{{"kill", "SIGTERM", target}}, nil
	case service.Enable:
		return [][]string{{"enable", target}}, nil
	case service.Disable:
		return [][]string{{"disable", target}}, nil
	}
	return nil, fmt.Errorf("launchd: unknown action %q", a)
}

func (p *Provider) Do(it service.Item, a service.Action) error {
	if it.State == service.StateInvalid && !service.Has(p.Actions(it), a) {
		return fmt.Errorf("%s is not a job launchd can load, so %s does not apply", filepath.Base(it.Path), a.Label())
	}
	if a == service.Remove {
		if p.Config.Source != service.UserAgent {
			return p.removeAsRoot(it)
		}
		if it.State != service.StateStopped && it.State != service.StateInvalid {
			// Not loaded is fine too.
			_ = p.launchctl("bootout", p.Domain+"/"+it.Name)
		}
		return system.Trash(it.Path)
	}
	commands, err := p.commands(it, a)
	if err != nil {
		return err
	}
	return p.launchctls(commands...)
}

// DoAll runs an action on several items, in one request to the privileged
// helper where they need root.
func (p *Provider) DoAll(items []service.Item, a service.Action) error {
	var all [][]string
	for _, it := range items {
		commands, err := p.commands(it, a)
		if err != nil {
			return err
		}
		all = append(all, commands...)
	}
	return p.launchctls(all...)
}

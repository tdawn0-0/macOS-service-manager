package main

import (
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"

	"devbase/internal/launchd"
	"devbase/internal/service"
)

// editorState is the dialog that makes and edits launch agents.
type editorState struct {
	open       bool
	existing   string // the file being edited, "" for a new agent
	label      string
	args       string // one per line
	workDir    string
	interval   string
	env        string // KEY=value per line
	stdout     string
	stderr     string
	runAtLoad  bool
	keepCustom bool
	apply      bool // load the agent, or load it again, once saved
	logDefault bool // write output to a log of the label's name, if no file is set
	err        string
	saving     bool

	kind    int    // of jobKinds: where a new job goes
	shell   string // how the file being edited names the shell, which stays
	command int    // of the commandShell kinds: what args holds
	when    int    // of the whenService kinds: what starts the job
	// whenLoaded is when as the file says it, for telling what the user changed.
	whenLoaded int
	restart    int // of the restartNever kinds
	mount      bool
	queue      string // directories, separated by commas
	// The fields of the calendar, and that the file has one they cannot hold.
	minute, hour, day, month, weekday string
	calendarCustom                    bool
	// Who a daemon runs as.
	user, group, rootDir string
}

// jobKinds are the sources a new job can go to.
var jobKinds = []service.Source{service.UserAgent, service.SystemAgent, service.Daemon}

// What editorState.command, when and restart hold.
const (
	commandShell = iota
	commandProgram
)

const (
	whenService = iota
	whenInterval
	whenCalendar
)

const (
	restartNever = iota
	restartOnCrash
	restartAlways
)

// source returns the source the job goes to.
func (e *editorState) source() service.Source { return jobKinds[e.kind] }

// logDefaultPath returns the log of the label's name, or "" if the job has no
// one place for it: a system agent runs as every user, whose home launchd does
// not expand, and a user may not write where a daemon logs.
func (e *editorState) logDefaultPath(label string) string {
	switch {
	case e.source() == service.UserAgent:
		home, _ := os.UserHomeDir()
		return filepath.Join(home, "Library/Logs", label+".log")
	case e.source() == service.Daemon && strings.TrimSpace(e.user) == "":
		return filepath.Join("/Library/Logs", label+".log")
	}
	return ""
}

// whenChanged keeps what the user no longer sees from being saved unseen: a job
// on a schedule does not stay alive by itself, which would make it pointless.
func (e *editorState) whenChanged() {
	if e.when == whenService {
		e.runAtLoad = true
		return
	}
	e.runAtLoad = false
	if e.whenLoaded != e.when {
		e.mount, e.queue = false, ""
		if !e.keepCustom {
			e.restart = restartNever
		}
	}
}

// newJob opens the form for a new job, of the kind of the list shown.
func (a *app) newJob() {
	kind := max(slices.Index(jobKinds, service.Source(a.nav)), 0)
	a.editor = editorState{open: true, kind: kind, runAtLoad: true, apply: true, logDefault: true}
}

// editJob opens the form on a job's file.
func (a *app) editJob(it service.Item) {
	spec, err := launchd.LoadSpec(it.Path)
	if err != nil {
		a.toast("%s", err)
		return
	}
	e := editorState{
		open:       true,
		kind:       max(slices.Index(jobKinds, it.Source), 0),
		existing:   it.Path,
		label:      spec.Label,
		args:       strings.Join(spec.Args, "\n"),
		workDir:    spec.WorkingDir,
		env:        launchd.FormatEnv(spec.Env),
		stdout:     spec.StdOut,
		stderr:     spec.StdErr,
		runAtLoad:  spec.RunAtLoad,
		keepCustom: spec.KeepAliveCustom,
		apply:      it.State != service.StateStopped,
		command:    commandProgram,
		mount:      spec.StartOnMount,
		queue:      strings.Join(spec.QueueDirs, ", "),
		minute:     spec.Calendar.Minute,
		hour:       spec.Calendar.Hour,
		day:        spec.Calendar.Day,
		month:      spec.Calendar.Month,
		weekday:    spec.Calendar.Weekday,

		calendarCustom: spec.CalendarCustom,
		user:           spec.UserName,
		group:          spec.GroupName,
		rootDir:        spec.RootDir,
	}
	if command, ok := launchd.ShellCommand(spec.Args); ok {
		e.command, e.args, e.shell = commandShell, command, spec.Args[0]
	}
	switch {
	case spec.KeepAlive:
		e.restart = restartAlways
	case spec.RestartOnCrash:
		e.restart = restartOnCrash
	}
	switch {
	case spec.StartInterval > 0:
		e.when = whenInterval
		e.interval = strconv.Itoa(spec.StartInterval)
	case spec.CalendarCustom || !spec.Calendar.Empty():
		e.when = whenCalendar
	}
	e.whenLoaded = e.when
	a.editor = e
}

// spec reads the form.
func (e *editorState) spec() (launchd.Spec, error) {
	spec := launchd.Spec{
		Label:           strings.TrimSpace(e.label),
		Existing:        e.existing != "",
		WorkingDir:      strings.TrimSpace(e.workDir),
		RunAtLoad:       e.runAtLoad,
		KeepAlive:       e.restart == restartAlways || e.keepCustom,
		KeepAliveCustom: e.keepCustom,
		RestartOnCrash:  e.restart == restartOnCrash,
		CalendarCustom:  e.calendarCustom,
		StartOnMount:    e.mount,
		QueueDirs:       launchd.ParseList(e.queue),
		UserName:        strings.TrimSpace(e.user),
		GroupName:       strings.TrimSpace(e.group),
		RootDir:         strings.TrimSpace(e.rootDir),
		StdOut:          strings.TrimSpace(e.stdout),
		StdErr:          strings.TrimSpace(e.stderr),
	}
	// The form says something, and the label's checks read better first.
	if spec.Label != "" {
		switch e.command {
		case commandShell:
			command := strings.TrimSpace(e.args)
			if command == "" {
				return spec, fmt.Errorf("give the command to run")
			}
			spec.Args = launchd.ShellArgs(command)
			if e.shell != "" {
				spec.Args[0] = e.shell
			}
		default:
			spec.Args = launchd.ParseLines(e.args)
		}
	}
	switch e.when {
	case whenInterval:
		n, err := strconv.Atoi(strings.TrimSpace(e.interval))
		if err != nil || n <= 0 {
			return spec, fmt.Errorf("the interval is a number of seconds")
		}
		spec.StartInterval = n
	case whenCalendar:
		spec.Calendar = launchd.Calendar{Minute: e.minute, Hour: e.hour, Day: e.day, Month: e.month, Weekday: e.weekday}
		if spec.Calendar.Empty() && !e.calendarCustom {
			return spec, fmt.Errorf("give at least one field of the calendar")
		}
	}
	env, err := launchd.ParseEnv(e.env)
	if err != nil {
		return spec, err
	}
	spec.Env = env
	if e.logDefault && e.existing == "" && spec.StdOut == "" && spec.StdErr == "" && spec.Label != "" {
		spec.StdOut = e.logDefaultPath(spec.Label)
		spec.StdErr = spec.StdOut
	}
	return spec, spec.Validate()
}

// rootSaver is a provider of jobs that only root can write.
type rootSaver interface {
	SaveAsRoot(s launchd.Spec, existing string, load bool) (string, error)
}

// saveJob saves the form. Creating or changing a system job only root can write
// asks for in-app confirmation first, and editing asks nothing if nothing changes.
func (a *app) saveJob() {
	e := &a.editor
	spec, err := e.spec()
	if err != nil {
		e.err = err.Error()
		return
	}
	e.err = ""
	src := e.source()
	if src == service.UserAgent {
		a.writeJob(spec)
		return
	}
	if e.existing == "" {
		a.ask = askState{
			open:    true,
			title:   fmt.Sprintf("Create system job %s?", spec.Label),
			message: createWarning(src, e.apply),
			button:  "Create Job",
			run:     func() { a.writeJob(spec) },
		}
		return
	}
	current, err := os.ReadFile(e.existing)
	if err != nil {
		e.err = err.Error()
		return
	}
	changes, err := spec.Changes(current)
	if err != nil {
		e.err = err.Error()
		return
	}
	if len(changes) == 0 {
		e.open = false
		a.toast("%s has no changes to save", spec.Label)
		return
	}
	a.ask = askState{
		open:    true,
		title:   fmt.Sprintf("Save changes to %s?", spec.Label),
		message: changeWarning(changes, e.apply),
		button:  "Save Changes",
		run:     func() { a.writeJob(spec) },
	}
}

func createWarning(src service.Source, load bool) string {
	target := "/Library/LaunchAgents"
	if src == service.Daemon {
		target = "/Library/LaunchDaemons"
	}
	msg := fmt.Sprintf("This creates a root-owned plist file in %s. ", target)
	if load {
		msg += "It also loads the job now. "
	}
	return msg + "The privileged helper will save it without a macOS password prompt."
}

// changeWarning says what saving an edit of a job only root can write does.
func changeWarning(changes []string, reload bool) string {
	msg := "This changes " + strings.Join(changes, ", ") + ". "
	if reload {
		msg += "The job loads again, which restarts it if it runs. "
	}
	msg += "The software that installed it may depend on what it says, or undo this when it updates. "
	return msg + "The privileged helper will save it without a macOS password prompt."
}

// writeJob writes the job, then loads it, or loads it again.
func (a *app) writeJob(spec launchd.Spec) {
	e := &a.editor
	e.saving = true
	existing, apply := e.existing, e.apply
	src := e.source()
	p := a.providers[src]
	dir := a.agentDir
	a.spawn(func() {
		var path string
		var err error
		if src != service.UserAgent {
			saver, ok := p.(rootSaver)
			if !ok {
				err = fmt.Errorf("%s cannot be saved", src.Title())
			} else {
				path, err = saver.SaveAsRoot(spec, existing, apply)
			}
		} else if path, err = launchd.Save(dir, spec, existing); err == nil && apply {
			it := service.Item{Name: spec.Label, Path: path, Source: service.UserAgent, Autostart: service.AutostartOn}
			if existing != "" {
				// Loaded agents keep what they were loaded with: load it again.
				_ = p.Do(it, service.Unload)
			}
			err = p.Do(it, service.Load)
		}
		a.post(func() {
			a.editor.saving = false
			if err != nil {
				a.editor.err = err.Error()
				if path != "" {
					// The file was saved; only loading failed. Saving again edits it.
					a.editor.existing = path
					a.refresh(src)
				} else {
					a.helperProblem(err, func() { a.writeJob(spec) })
				}
				return
			}
			a.editor.open = false
			a.toast("%s saved", spec.Label)
			a.nav = string(src)
			a.chosen.Clear()
			a.chosen.Add(string(src) + ":" + path)
			a.refresh(src)
			// The file changed under the details shown, if the job was chosen.
			if a.detail.id == string(src)+":"+path {
				a.loadDetail()
			}
		})
		// A job loaded again takes a moment to run.
		a.settleRead(src, func() {
			if a.detail.id == string(src)+":"+path {
				a.loadDetail()
			}
		})
	})
}

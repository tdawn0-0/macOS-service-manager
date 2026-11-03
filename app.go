package main

import (
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"time"

	"github.com/egoist/mygo/ui"

	"macos-service-manager/internal/brew"
	"macos-service-manager/internal/launchd"
	"macos-service-manager/internal/loginitem"
	"macos-service-manager/internal/logs"
	"macos-service-manager/internal/service"
	"macos-service-manager/internal/system"
)

// sourceState is what the app knows of one source.
type sourceState struct {
	items   []service.Item
	loading bool
	again   bool // a refresh was asked for while one was running
	loaded  bool
	err     string
}

// detailState is the details of the item chosen.
type detailState struct {
	id      string
	loading bool
	err     string
	rows    []service.Row
	logs    []service.LogFile
	seq     int // tells the answer to the latest request from older ones
	tab     int // 0 info, 1 logs
}

// logState is the log file shown in the logs tab.
type logState struct {
	file      int
	path      string
	lines     []string
	truncated bool
	size      int64
	mod       time.Time
	err       string
	last      time.Time // when the file was last looked at
	reading   bool
	list      ui.ListState
}

// askState is a question before an action with consequences.
type askState struct {
	open    bool
	title   string
	message string
	button  string
	run     func()
}

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
	keepAlive  bool
	keepCustom bool
	apply      bool // load the agent, or load it again, once saved
	logDefault bool // write output to ~/Library/Logs/<label>.log, if no file is set
	err        string
	saving     bool
}

// Navigation ids of the sidebar besides the sources'.
const navAll = "all"

// app is the state of the window. Everything in it is read and written on
// the main thread: background work hands its results over through post.
type app struct {
	providers map[service.Source]service.Provider
	adder     interface{ Add(path string) error }
	// agentDir is where new launch agents are written.
	agentDir string

	// post runs a function on the main thread (Window.Update), spawn runs
	// background work, copy puts text on the clipboard, and chooseApp asks
	// the user for an app. Tests replace them.
	post      func(func())
	spawn     func(func())
	copy      func(string)
	reveal    func(path string)
	chooseApp func() (string, bool)
	// settle is how long after a command the lists are read once more:
	// stopping a process takes a moment.
	settle time.Duration
	// changed is called when a list has changed.
	changed func()

	sources map[service.Source]*sourceState
	busy    map[string]bool // ids of the items a command runs on
	toasts  []string        // messages the next frame shows

	nav     string
	query   string
	filter  int // 0 all, 1 running, 2 not running
	list    ui.ListState
	chosen  ui.Selection[string]
	visible []service.Item // the items listed, built for each frame

	detail detailState
	log    logState
	ask    askState
	editor editorState
}

func newApp() *app {
	login := loginitem.New()
	return newAppWith(map[service.Source]service.Provider{
		service.Brew:        brew.New(),
		service.UserAgent:   launchd.UserAgents(),
		service.SystemAgent: launchd.SystemAgents(),
		service.Daemon:      launchd.Daemons(),
		service.LoginItem:   login,
	}, login, launchd.UserAgents().Dir)
}

func newAppWith(providers map[service.Source]service.Provider, adder interface{ Add(string) error }, agentDir string) *app {
	a := &app{
		providers: providers,
		adder:     adder,
		agentDir:  agentDir,
		spawn:     func(f func()) { go f() },
		settle:    1200 * time.Millisecond,
		sources:   map[service.Source]*sourceState{},
		busy:      map[string]bool{},
		nav:       navAll,
	}
	for src := range providers {
		a.sources[src] = &sourceState{}
	}
	a.list.Key = func(i int) any { return a.visible[i].ID }
	a.list.Label = func(i int) string { return a.visible[i].Name }
	a.list.Selection = &a.chosen
	a.log.list.FollowEnd = true
	return a
}

func (a *app) toast(format string, args ...any) {
	a.toasts = append(a.toasts, fmt.Sprintf(format, args...))
}

// --- Lists ---

// refresh reads a source's items in the background. A refresh asked for
// while one runs makes it run once more as it ends, so no change is missed.
func (a *app) refresh(src service.Source) {
	st, p := a.sources[src], a.providers[src]
	if st == nil || p == nil {
		return
	}
	if st.loading {
		st.again = true
		return
	}
	st.loading = true
	a.spawn(func() {
		items, err := p.List()
		a.post(func() {
			st.loading, st.loaded = false, true
			if err != nil {
				st.items, st.err = nil, err.Error()
			} else {
				slices.SortStableFunc(items, func(x, y service.Item) int {
					return strings.Compare(strings.ToLower(x.Name), strings.ToLower(y.Name))
				})
				st.items, st.err = items, ""
			}
			if st.again {
				st.again = false
				a.refresh(src)
			}
			if a.changed != nil {
				a.changed()
			}
		})
	})
}

func (a *app) refreshAll() {
	for _, src := range service.Sources {
		a.refresh(src)
	}
}

func (a *app) loading() bool {
	for _, st := range a.sources {
		if st.loading {
			return true
		}
	}
	return false
}

// find returns the item with the id.
func (a *app) find(id string) (service.Item, bool) {
	for _, st := range a.sources {
		for _, it := range st.items {
			if it.ID == id {
				return it, true
			}
		}
	}
	return service.Item{}, false
}

// build lists the items to show: those of the source chosen in the
// sidebar that the search and the filter let through.
func (a *app) build() {
	a.visible = a.visible[:0]
	for _, src := range service.Sources {
		if a.nav != navAll && a.nav != string(src) {
			continue
		}
		st := a.sources[src]
		if st == nil {
			continue
		}
		for _, it := range st.items {
			if !it.Matches(a.query) {
				continue
			}
			switch a.filter {
			case 1:
				if !it.Active() {
					continue
				}
			case 2:
				// Login items have no process, so they are never "not running".
				if it.Active() || it.State == service.StateNone {
					continue
				}
			}
			a.visible = append(a.visible, it)
		}
	}
}

// counts returns how many items a navigation entry has, and how many run.
func (a *app) counts(nav string) (total, running int) {
	for src, st := range a.sources {
		if nav != navAll && nav != string(src) {
			continue
		}
		for _, it := range st.items {
			total++
			if it.Active() {
				running++
			}
		}
	}
	return
}

// errors returns what failed to load, for the sources of the navigation.
func (a *app) errors() []string {
	var errs []string
	for _, src := range service.Sources {
		if a.nav != navAll && a.nav != string(src) {
			continue
		}
		if st := a.sources[src]; st != nil && st.err != "" {
			errs = append(errs, src.Title()+": "+st.err)
		}
	}
	return errs
}

// --- The item chosen ---

// target returns the item the details show: the one item chosen.
func (a *app) target() (service.Item, bool) {
	if a.chosen.Len() != 1 {
		return service.Item{}, false
	}
	for id := range a.chosen.All() {
		return a.find(id)
	}
	return service.Item{}, false
}

// syncDetail loads the details of the item chosen when it changes.
func (a *app) syncDetail() {
	id := ""
	if it, ok := a.target(); ok {
		id = it.ID
	}
	if id == a.detail.id {
		return
	}
	a.detail = detailState{id: id, seq: a.detail.seq}
	a.log = logState{list: a.log.list}
	if id != "" {
		a.loadDetail()
	}
}

// loadDetail reads the details of the item chosen. Rows already shown stay
// until the new ones arrive.
func (a *app) loadDetail() {
	it, ok := a.find(a.detail.id)
	if !ok {
		return
	}
	d := &a.detail
	d.seq++
	seq := d.seq
	d.loading, d.err = true, ""
	p := a.providers[it.Source]
	a.spawn(func() {
		detail, err := p.Detail(it)
		a.post(func() {
			if a.detail.seq != seq {
				return
			}
			a.detail.loading = false
			if err != nil {
				a.detail.err, a.detail.rows, a.detail.logs = err.Error(), nil, nil
				return
			}
			a.detail.rows, a.detail.logs = detail.Rows, detail.Logs
			if a.log.file >= len(detail.Logs) {
				a.log.file = 0
			}
		})
	})
}

// --- Actions ---

// request runs an action, after asking if it has consequences the user may
// not expect.
func (a *app) request(it service.Item, act service.Action) {
	p := a.providers[it.Source]
	if msg := p.Confirm(it, act); msg != "" {
		a.ask = askState{
			open:    true,
			title:   fmt.Sprintf("%s %s?", act.Label(), it.Name),
			message: msg,
			button:  act.Label(),
			run:     func() { a.do(it, act) },
		}
		return
	}
	a.do(it, act)
}

// do runs an action in the background, then reports how it went and reads
// the source again.
func (a *app) do(it service.Item, act service.Action) {
	if a.busy[it.ID] {
		return
	}
	a.busy[it.ID] = true
	p := a.providers[it.Source]
	a.spawn(func() {
		err := p.Do(it, act)
		a.post(func() {
			delete(a.busy, it.ID)
			if err != nil {
				a.toast("%s: %s", it.Name, err)
			} else {
				a.toast("%s %s", it.Name, act.Past())
			}
			if err == nil && act == service.Remove {
				a.chosen.Remove(it.ID)
			}
			a.refresh(it.Source)
			if a.detail.id == it.ID && act != service.Remove {
				a.loadDetail()
			}
		})
		if a.settle > 0 {
			time.Sleep(a.settle)
			a.post(func() {
				a.refresh(it.Source)
				if a.detail.id == it.ID && act != service.Remove {
					a.loadDetail()
				}
			})
		}
	})
}

// bulk starts or stops the items chosen that can be.
func (a *app) bulk(start bool) {
	for id := range a.chosen.All() {
		it, ok := a.find(id)
		if !ok {
			continue
		}
		actions := a.providers[it.Source].Actions(it)
		switch {
		case start && !it.Active() && it.State != service.StateNone:
			if act := service.Primary(actions); act != "" {
				a.request(it, act)
			}
		case !start && it.Active() && service.Has(actions, service.Stop):
			a.request(it, service.Stop)
		}
	}
}

// bulkCounts returns how many of the items chosen a bulk start and a bulk
// stop would act on.
func (a *app) bulkCounts() (start, stop int) {
	for id := range a.chosen.All() {
		it, ok := a.find(id)
		if !ok {
			continue
		}
		actions := a.providers[it.Source].Actions(it)
		if !it.Active() && it.State != service.StateNone && service.Primary(actions) != "" {
			start++
		}
		if it.Active() && service.Has(actions, service.Stop) {
			stop++
		}
	}
	return
}

func (a *app) addLoginItem() {
	path, ok := a.chooseApp()
	if !ok {
		return
	}
	a.spawn(func() {
		err := a.adder.Add(path)
		a.post(func() {
			if err != nil {
				a.toast("%s", err)
				return
			}
			a.toast("%s will open at login", strings.TrimSuffix(filepath.Base(path), ".app"))
			a.refresh(service.LoginItem)
		})
	})
}

func (a *app) openLog(path string) {
	a.spawn(func() {
		if err := system.OpenInConsole(path); err != nil {
			a.post(func() { a.toast("%s", err) })
		}
	})
}

func (a *app) openLoginSettings() {
	a.spawn(func() {
		if err := system.OpenLoginItemsSettings(); err != nil {
			a.post(func() { a.toast("%s", err) })
		}
	})
}

// --- Logs ---

const (
	logBytes = 256 << 10
	logLines = 1000
	logEvery = time.Second
)

// pollLog reads the end of the log file shown, at most once a second, and
// only if the file changed.
func (a *app) pollLog() {
	l := &a.log
	if a.detail.tab != 1 || l.file >= len(a.detail.logs) || l.reading || time.Since(l.last) < logEvery*9/10 {
		return
	}
	path := a.detail.logs[l.file].Path
	if path != l.path {
		*l = logState{file: l.file, path: path, list: l.list}
	}
	l.reading, l.last = true, time.Now()
	size, mod := l.size, l.mod
	known := l.err == "" && l.lines != nil
	a.spawn(func() {
		var snap logs.Snapshot
		var err error
		if s, m, statErr := logs.Stat(path); statErr == nil && known && s == size && m.Equal(mod) {
			snap.Size, snap.ModTime = s, m
			err = errUnchanged
		} else {
			snap, err = logs.Tail(path, logBytes, logLines)
		}
		a.post(func() {
			if a.log.path != path {
				return
			}
			a.log.reading = false
			switch {
			case err == errUnchanged:
			case err != nil:
				a.log.err = err.Error()
			default:
				a.log.err, a.log.lines, a.log.truncated = "", snap.Lines, snap.Truncated
				if a.log.lines == nil {
					a.log.lines = []string{}
				}
				a.log.size, a.log.mod = snap.Size, snap.ModTime
			}
		})
	})
}

var errUnchanged = fmt.Errorf("unchanged")

// --- The launch agent editor ---

func (a *app) newAgent() {
	a.editor = editorState{open: true, runAtLoad: true, apply: true, logDefault: true}
}

func (a *app) editAgent(it service.Item) {
	spec, err := launchd.LoadSpec(it.Path)
	if err != nil {
		a.toast("%s", err)
		return
	}
	e := editorState{
		open:       true,
		existing:   it.Path,
		label:      spec.Label,
		args:       strings.Join(spec.Args, "\n"),
		workDir:    spec.WorkingDir,
		env:        launchd.FormatEnv(spec.Env),
		stdout:     spec.StdOut,
		stderr:     spec.StdErr,
		runAtLoad:  spec.RunAtLoad,
		keepAlive:  spec.KeepAlive,
		keepCustom: spec.KeepAliveCustom,
		apply:      it.State != service.StateStopped,
	}
	if spec.StartInterval > 0 {
		e.interval = strconv.Itoa(spec.StartInterval)
	}
	a.editor = e
}

// spec reads the form.
func (e *editorState) spec() (launchd.Spec, error) {
	spec := launchd.Spec{
		Label:           strings.TrimSpace(e.label),
		Args:            launchd.ParseLines(e.args),
		WorkingDir:      strings.TrimSpace(e.workDir),
		RunAtLoad:       e.runAtLoad,
		KeepAlive:       e.keepAlive,
		KeepAliveCustom: e.keepCustom,
		StdOut:          strings.TrimSpace(e.stdout),
		StdErr:          strings.TrimSpace(e.stderr),
	}
	if s := strings.TrimSpace(e.interval); s != "" {
		n, err := strconv.Atoi(s)
		if err != nil || n < 0 {
			return spec, fmt.Errorf("the interval is a number of seconds")
		}
		spec.StartInterval = n
	}
	env, err := launchd.ParseEnv(e.env)
	if err != nil {
		return spec, err
	}
	spec.Env = env
	if e.logDefault && spec.StdOut == "" && spec.StdErr == "" && spec.Label != "" {
		spec.StdOut = defaultLogPath(spec.Label)
		spec.StdErr = spec.StdOut
	}
	return spec, spec.Validate()
}

// saveAgent writes the launch agent, then loads it, or loads it again.
func (a *app) saveAgent() {
	e := &a.editor
	spec, err := e.spec()
	if err != nil {
		e.err = err.Error()
		return
	}
	e.err, e.saving = "", true
	existing, apply := e.existing, e.apply
	p := a.providers[service.UserAgent]
	dir := a.agentDir
	a.spawn(func() {
		path, err := launchd.Save(dir, spec, existing)
		if err == nil && apply {
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
					// The file was saved; only loading failed.
					a.refresh(service.UserAgent)
				}
				return
			}
			a.editor.open = false
			a.toast("%s saved", spec.Label)
			a.nav = string(service.UserAgent)
			a.chosen.Clear()
			a.chosen.Add(string(service.UserAgent) + ":" + path)
			a.refresh(service.UserAgent)
		})
	})
}

func defaultLogPath(label string) string {
	home, _ := os.UserHomeDir()
	return filepath.Join(home, "Library/Logs", label+".log")
}

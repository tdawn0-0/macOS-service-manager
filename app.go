package main

import (
	"fmt"
	"image"
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
	// limit is what macOS kept the last read from listing, and reading says
	// the user asked for it, which macOS is authenticating.
	limit   *service.Restriction
	reading bool
}

// guideState is the page that leads the user to allow what macOS restricts.
type guideState struct {
	open bool
	src  service.Source
	// welcome says it opened by itself at launch, not at the user's asking.
	welcome bool
	checked time.Time // when it last checked whether access was allowed
	// allowedAt is when access was allowed, which the page says for a moment.
	allowedAt time.Time
	// joined says System Settings was opened: the page is a small one beside
	// its window now, where settings says that window is.
	joined       bool
	settings     system.Rect
	settingsOpen bool
	// windowAt and overlayAt are where the page's window and the overlay's are
	// on screen, and icon is the center of the icon in the page.
	windowAt, overlayAt image.Point
	icon                image.Point
	// flightStart is when the icon started showing how to drag it.
	flightStart time.Time
	// dragging says the user is dragging the icon, and dragged that they did.
	dragging, dragged bool
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

// Navigation ids of the sidebar besides the sources'.
const navAll = "all"

// listRow is a row of the list: an item, or the header of a source's items.
type listRow struct {
	header bool
	src    service.Source
	count  int
	item   service.Item
}

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
	// showGuide, hideGuide and joinGuide show the guide's window, close it, and
	// make it follow System Settings. redrawGuide redraws its windows.
	showGuide, hideGuide, joinGuide, redrawGuide func()
	// appPath is the app's bundle, which the guide offers to drag, and
	// welcomedPath the file that says it opened at launch already.
	appPath, welcomedPath string
	// settle is how long after a command the lists are read once more:
	// stopping a process takes a moment.
	settle time.Duration

	guide   guideState
	sources map[service.Source]*sourceState
	// brewNames holds the names of the Homebrew services: launchd lists their
	// jobs too, which are the same services.
	brewNames map[string]bool
	busy      map[string]bool // ids of the items a command runs on
	gone      map[string]bool // ids of items removed that a read may still return
	toasts    []string        // messages the next frame shows
	panelOpen bool            // the menu bar's panel shows, and the toasts with it

	nav     string
	query   string
	filter  int // 0 all, 1 running, 2 not running
	list    ui.ListState
	chosen  ui.Selection[string]
	visible []service.Item // the items listed, built for each frame
	rows    []listRow      // the rows of the list: the items, under headers

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
		providers:   providers,
		adder:       adder,
		agentDir:    agentDir,
		spawn:       func(f func()) { go f() },
		showGuide:   func() {},
		hideGuide:   func() {},
		joinGuide:   func() {},
		redrawGuide: func() {},
		settle:      1200 * time.Millisecond,
		sources:     map[service.Source]*sourceState{},
		busy:        map[string]bool{},
		gone:        map[string]bool{},
		nav:         navAll,
	}
	for src := range providers {
		a.sources[src] = &sourceState{}
	}
	a.list.Key = func(i int) any {
		if e := a.rows[i]; e.header {
			return "header:" + string(e.src)
		}
		return a.rows[i].item.ID
	}
	a.list.Label = func(i int) string {
		if e := a.rows[i]; e.header {
			return e.src.Title()
		}
		return a.rows[i].item.Name
	}
	a.list.Header = func(i int) bool { return a.rows[i].header }
	a.list.Selection = &a.chosen
	// The sources load one after the other: the top of the list stays the top
	// as those above what shows arrive.
	a.list.KeepOffset = true
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
	// Reading a restricted source may ask for a permission that the guide
	// makes needless. Closing the guide reads it.
	if a.guide.open && a.guide.src == src {
		return
	}
	if st.loading {
		st.again = true
		return
	}
	st.loading = true
	a.spawn(func() {
		items, err := p.List()
		var limit *service.Restriction
		if r, ok := p.(service.Restricted); ok {
			limit = r.Restriction()
		}
		a.post(func() {
			st.loading, st.loaded = false, true
			st.limit = limit
			st.err = ""
			if err != nil {
				st.err = err.Error()
			}
			// A failure leaves what was read last time, unless it came with
			// items of its own.
			if err == nil || items != nil {
				slices.SortStableFunc(items, func(x, y service.Item) int {
					return strings.Compare(strings.ToLower(x.Name), strings.ToLower(y.Name))
				})
				st.items = items
				if src == service.Brew {
					a.brewNames = make(map[string]bool, len(items))
					for _, it := range items {
						a.brewNames[it.Name] = true
					}
				}
			}
			if st.again {
				st.again = false
				a.refresh(src)
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

// shown reports whether an item is listed: launchd's job of a Homebrew service
// is the service, listed under Homebrew. A job of a formula Homebrew no longer
// has is listed, as nothing else shows it.
func (a *app) shown(it service.Item) bool {
	if a.gone[it.ID] {
		return false
	}
	if it.Source == service.Brew || it.Source == service.LoginItem {
		return true
	}
	for _, prefix := range []string{"sh.brew.", "homebrew.mxcl."} {
		if name, ok := strings.CutPrefix(it.Name, prefix); ok {
			return !a.brewNames[name]
		}
	}
	return true
}

// build lists the items to show: those of the source chosen in the
// sidebar that the search and the filter let through, under the headers of
// their sources if there are several.
func (a *app) build() {
	a.visible = a.visible[:0]
	a.rows = a.rows[:0]
	for _, src := range service.Sources {
		if a.nav != navAll && a.nav != string(src) {
			continue
		}
		st := a.sources[src]
		if st == nil {
			continue
		}
		first := len(a.visible)
		for _, it := range st.items {
			if !a.shown(it) || !it.Matches(a.query) {
				continue
			}
			if (a.filter == 1 && !it.Up()) || (a.filter == 2 && it.Up()) {
				continue
			}
			a.visible = append(a.visible, it)
		}
		section := a.visible[first:]
		if a.nav == navAll && len(section) > 0 {
			a.rows = append(a.rows, listRow{header: true, src: src, count: len(section)})
		}
		for _, it := range section {
			a.rows = append(a.rows, listRow{src: src, item: it})
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
			if !a.shown(it) {
				continue
			}
			total++
			if it.State == service.StateRunning {
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

// pruneChosen forgets the items chosen that are gone, once the lists are read.
func (a *app) pruneChosen() {
	if a.loading() || a.chosen.Len() == 0 {
		return
	}
	var gone []string
	for id := range a.chosen.All() {
		if _, ok := a.find(id); !ok {
			gone = append(gone, id)
		}
	}
	for _, id := range gone {
		a.chosen.Remove(id)
	}
}

// syncDetail loads the details of the item chosen when it changes. The tab
// stays as it is, to read the logs of one item after the other.
func (a *app) syncDetail() {
	id := ""
	if it, ok := a.target(); ok {
		id = it.ID
	}
	if id == a.detail.id {
		return
	}
	a.detail = detailState{id: id, seq: a.detail.seq, tab: a.detail.tab}
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
				a.gone[it.ID] = true
			}
			a.refresh(it.Source)
			if a.detail.id == it.ID && act != service.Remove {
				a.loadDetail()
			}
		})
		a.settleRead(it.Source, func() {
			if a.detail.id == it.ID && act != service.Remove {
				a.loadDetail()
			}
		})
		// What a read returns now is true, even if the item is still there.
		a.post(func() { delete(a.gone, it.ID) })
	})
}

// settleRead reads a source again as what was done takes effect, calling also
// after each read. It waits, so it runs in the background: stopping a process
// takes a moment, and System Settings' database changes seconds after a login
// item does.
func (a *app) settleRead(src service.Source, also func()) {
	if a.settle <= 0 {
		return
	}
	waits := []time.Duration{a.settle}
	if src == service.LoginItem {
		waits = append(waits, 2*a.settle, 2*a.settle)
	}
	for _, wait := range waits {
		time.Sleep(wait)
		a.post(func() {
			a.refresh(src)
			if also != nil {
				also()
			}
		})
	}
}

// bulkAction returns what a bulk start or stop does to an item, if anything:
// its main action, when that is starting or stopping it.
func (a *app) bulkAction(it service.Item, start bool) service.Action {
	switch act := service.Primary(a.providers[it.Source].Actions(it)); {
	case start && (act == service.Start || act == service.Load):
		return act
	case !start && act == service.Stop:
		return act
	}
	return ""
}

// bulk starts or stops the items chosen that can be. Those that take an
// administrator go together, so that macOS asks once rather than for each.
func (a *app) bulk(start bool) {
	type group struct {
		src service.Source
		act service.Action
	}
	batches := map[group][]service.Item{}
	for id := range a.chosen.All() {
		it, ok := a.find(id)
		if !ok {
			continue
		}
		act := a.bulkAction(it, start)
		switch _, batches_ := a.providers[it.Source].(service.Batcher); {
		case act == "":
		case it.Admin && batches_:
			batches[group{it.Source, act}] = append(batches[group{it.Source, act}], it)
		default:
			a.do(it, act)
		}
	}
	for g, items := range batches {
		if len(items) == 1 {
			a.do(items[0], g.act)
		} else {
			a.doAll(g.src, items, g.act)
		}
	}
}

// doAll runs an action on several items of a source at once.
func (a *app) doAll(src service.Source, items []service.Item, act service.Action) {
	for _, it := range items {
		a.busy[it.ID] = true
	}
	batcher := a.providers[src].(service.Batcher)
	a.spawn(func() {
		err := batcher.DoAll(items, act)
		a.post(func() {
			for _, it := range items {
				delete(a.busy, it.ID)
			}
			if err != nil {
				a.toast("%s", err)
			} else {
				a.toast("%d items %s", len(items), act.Past())
			}
			a.refresh(src)
		})
		a.settleRead(src, nil)
	})
}

// bulkCounts returns how many of the items chosen a bulk start and a bulk
// stop would act on.
func (a *app) bulkCounts() (start, stop int) {
	for id := range a.chosen.All() {
		it, ok := a.find(id)
		if !ok {
			continue
		}
		if a.bulkAction(it, true) != "" {
			start++
		}
		if a.bulkAction(it, false) != "" {
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
		if err == nil {
			a.settleRead(service.LoginItem, nil)
		}
	})
}

func (a *app) openLog(path string) {
	a.spawn(func() {
		if err := system.OpenInConsole(path); err != nil {
			a.post(func() { a.toast("%s", err) })
		}
	})
}

// openRestrictionSettings opens the setting that lifts a source's restriction.
func (a *app) openRestrictionSettings(src service.Source) {
	r, ok := a.providers[src].(service.Restricted)
	if !ok {
		return
	}
	a.guide.joined = true
	a.joinGuide()
	a.spawn(func() {
		if err := r.OpenSettings(); err != nil {
			a.post(func() { a.toast("%s", err) })
		}
	})
}

// welcome opens the guide at launch, once, if macOS restricts a source.
func (a *app) welcome() {
	if a.welcomedPath == "" {
		return
	}
	if _, err := os.Stat(a.welcomedPath); err == nil {
		return
	}
	for _, src := range service.Sources {
		if r, ok := a.providers[src].(service.Restricted); ok && r.NeedsAccess() {
			a.openGuide(src, true)
			return
		}
	}
}

func (a *app) openGuide(src service.Source, welcome bool) {
	if !a.guide.open {
		a.guide = guideState{open: true, src: src, welcome: welcome}
	}
	a.showGuide()
}

// closeGuide ends the guide, for good as far as launches go, and reads the
// source it held back. Closing its window comes here too.
func (a *app) closeGuide() {
	if !a.guide.open {
		return
	}
	src := a.guide.src
	a.guide.open = false
	if a.welcomedPath != "" {
		if os.MkdirAll(filepath.Dir(a.welcomedPath), 0o755) == nil {
			_ = os.WriteFile(a.welcomedPath, nil, 0o644)
		}
	}
	a.hideGuide()
	a.refresh(src)
}

// checkGuide closes the guide once the user has allowed access. Dragging the
// app into System Settings is the user's to do, and nothing tells the app.
func (a *app) checkGuide() {
	r, ok := a.providers[a.guide.src].(service.Restricted)
	if !ok || time.Since(a.guide.checked) < time.Second {
		return
	}
	a.guide.checked = time.Now()
	a.spawn(func() {
		needs := r.NeedsAccess()
		a.post(func() {
			if a.guide.open && !needs && a.guide.allowedAt.IsZero() {
				a.guide.allowedAt = time.Now()
				a.redrawGuide()
			}
		})
	})
}

// readRestricted reads what a source's restriction leaves out, at the user's
// asking: macOS asks them to authenticate.
func (a *app) readRestricted(src service.Source) {
	st, p := a.sources[src], a.providers[src]
	r, ok := p.(service.Restricted)
	if st == nil || !ok || st.reading {
		return
	}
	st.reading = true
	a.spawn(func() {
		err := r.ReadOnce()
		a.post(func() {
			st.reading = false
			if err != nil {
				a.toast("%s", err)
				return
			}
			a.refresh(src)
		})
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

// saveJob saves the form. Changing a job only root can write asks first, saying
// what changes, and asks nothing at all if nothing does.
func (a *app) saveJob() {
	e := &a.editor
	spec, err := e.spec()
	if err != nil {
		e.err = err.Error()
		return
	}
	e.err = ""
	if e.existing == "" || e.source() == service.UserAgent {
		a.writeJob(spec)
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

// changeWarning says what saving an edit of a job only root can write does.
func changeWarning(changes []string, reload bool) string {
	msg := "This changes " + strings.Join(changes, ", ") + ". "
	if reload {
		msg += "The job loads again, which restarts it if it runs. "
	}
	return msg + "The software that installed it may depend on what it says, or undo this when it updates. macOS will ask for your password."
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

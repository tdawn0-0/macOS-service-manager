package main

import (
	"fmt"

	"github.com/egoist/mygo/ui"

	"macos-service-manager/internal/brew"
	"macos-service-manager/internal/detail"
	"macos-service-manager/internal/launchd"
	"macos-service-manager/internal/system"
)

// brewBackend is what the app needs of Homebrew.
type brewBackend interface {
	List() ([]brew.Service, error)
	Info(name string) (*brew.Info, error)
	Manage(name string, command brew.Command) error
}

// launchdBackend is what the app needs of launchd.
type launchdBackend interface {
	List() ([]launchd.Service, error)
	Info(path string) (*launchd.Info, error)
	Manage(label, path string, command launchd.Command) error
}

// listTab is the state of a tab listing services.
type listTab[T any] struct {
	items   []T
	loading bool
	again   bool // a refresh was asked for while one was running
	err     string
	busy    map[string]bool // keys of the services a command runs on
	list    ui.ListState
}

func newListTab[T any]() listTab[T] { return listTab[T]{busy: map[string]bool{}} }

// Pages of the detail dialog.
type detailKind int

const (
	detailBrew detailKind = iota
	detailLaunchd
)

// detailState is the state of the dialog showing a service in detail.
type detailState struct {
	open    bool
	kind    detailKind
	key     string // the name of a Homebrew service, the path of a launch agent
	title   string
	loading bool
	err     string
	rows    []detail.Row
	seq     int // tells the answer to the latest request from older ones
}

// app is the state of the window. Everything in it is read and written on
// the main thread: background work hands its results over through post.
type app struct {
	brew    brewBackend
	launchd launchdBackend

	// post runs a function on the main thread (Window.Update), spawn runs
	// background work, and copy puts text on the clipboard. Tests replace
	// them.
	post  func(func())
	spawn func(func())
	copy  func(string)

	tab     int
	brewTab listTab[brew.Service]
	agents  listTab[launchd.Service]
	detail  detailState
	toasts  []string // messages the next frame shows
}

func newApp() *app {
	return &app{
		brew:    brew.CLI{},
		launchd: launchd.Launchctl{},
		spawn:   func(f func()) { go f() },
		brewTab: newListTab[brew.Service](),
		agents:  newListTab[launchd.Service](),
	}
}

func (a *app) toast(format string, args ...any) {
	a.toasts = append(a.toasts, fmt.Sprintf(format, args...))
}

// refresh loads a tab's services in the background. A refresh asked for
// while one runs makes it run once more as it ends, so no change is missed.
func refresh[T any](a *app, tab *listTab[T], fetch func() ([]T, error)) {
	if tab.loading {
		tab.again = true
		return
	}
	tab.loading = true
	a.spawn(func() {
		items, err := fetch()
		a.post(func() {
			tab.loading = false
			if err != nil {
				tab.items, tab.err = nil, err.Error()
			} else {
				tab.items, tab.err = items, ""
			}
			if tab.again {
				tab.again = false
				refresh(a, tab, fetch)
			}
		})
	})
}

func (a *app) refreshBrew()   { refresh(a, &a.brewTab, a.brew.List) }
func (a *app) refreshAgents() { refresh(a, &a.agents, a.launchd.List) }

func (a *app) refreshAll() {
	a.refreshBrew()
	a.refreshAgents()
}

// perform runs a command in the background on the service with the key,
// then reports how it went and calls after.
func (a *app) perform(busy map[string]bool, key, done string, work func() error, after func()) {
	if busy[key] {
		return
	}
	busy[key] = true
	a.spawn(func() {
		err := work()
		a.post(func() {
			delete(busy, key)
			if err != nil {
				a.toast("%s", err)
			} else {
				a.toast("%s", done)
			}
			after()
		})
	})
}

func (a *app) manageBrew(name string, command brew.Command) {
	a.perform(a.brewTab.busy, name, fmt.Sprintf("%s: %s", name, command),
		func() error { return a.brew.Manage(name, command) },
		func() {
			a.refreshBrew()
			if a.detail.open && a.detail.kind == detailBrew && a.detail.key == name {
				a.loadDetail(false)
			}
		})
}

func (a *app) manageAgent(s launchd.Service, command launchd.Command) {
	a.perform(a.agents.busy, s.Path, fmt.Sprintf("%s: %s", s.Label, command),
		func() error { return a.launchd.Manage(s.Label, s.Path, command) },
		func() {
			a.refreshAgents()
			if a.detail.open && a.detail.kind == detailLaunchd && a.detail.key == s.Path {
				a.loadDetail(false)
			}
		})
}

func (a *app) openBrewDetail(name string) {
	a.detail = detailState{open: true, kind: detailBrew, key: name, title: name, seq: a.detail.seq}
	a.loadDetail(true)
}

func (a *app) openAgentDetail(s launchd.Service) {
	a.detail = detailState{open: true, kind: detailLaunchd, key: s.Path, title: s.Label, seq: a.detail.seq}
	a.loadDetail(true)
}

// loadDetail fetches the rows of the service the dialog shows. A reload
// after a command keeps the old rows on screen until the new ones arrive.
func (a *app) loadDetail(clear bool) {
	d := &a.detail
	d.seq++
	seq, kind, key := d.seq, d.kind, d.key
	d.loading, d.err = true, ""
	if clear {
		d.rows = nil
	}
	a.spawn(func() {
		var rows []detail.Row
		var err error
		switch kind {
		case detailBrew:
			var info *brew.Info
			if info, err = a.brew.Info(key); err == nil {
				rows = info.Rows()
			}
		case detailLaunchd:
			var info *launchd.Info
			if info, err = a.launchd.Info(key); err == nil {
				rows = info.Rows()
			}
		}
		a.post(func() {
			if a.detail.seq != seq {
				return
			}
			a.detail.loading = false
			if err != nil {
				a.detail.err, a.detail.rows = err.Error(), nil
				return
			}
			a.detail.rows = rows
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

// agent returns the launch agent whose file is at path.
func (a *app) agent(path string) (launchd.Service, bool) {
	for _, s := range a.agents.items {
		if s.Path == path {
			return s, true
		}
	}
	return launchd.Service{}, false
}

func (a *app) viewContent() *ui.Content { return ui.View(a.view) }

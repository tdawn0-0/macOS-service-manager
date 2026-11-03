package main

import (
	"errors"
	"fmt"
	"image/png"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/egoist/mygo/ui"

	"macos-service-manager/internal/brew"
	"macos-service-manager/internal/launchd"
	"macos-service-manager/internal/service"
	"macos-service-manager/internal/system"
)

// fake is a source of items that remembers what was done to them.
type fake struct {
	src     service.Source
	items   []service.Item
	detail  service.Detail
	listErr error
	calls   []string
}

func (f *fake) Source() service.Source { return f.src }

func (f *fake) List() ([]service.Item, error) { return slices.Clone(f.items), f.listErr }

func (f *fake) Detail(service.Item) (service.Detail, error) { return f.detail, nil }

func (f *fake) Actions(it service.Item) []service.Action {
	switch {
	case it.Source == service.LoginItem:
		return []service.Action{service.Remove}
	case it.Active():
		return []service.Action{service.Stop, service.Restart, service.Unload, service.Disable}
	case it.State == service.StateLoaded:
		return []service.Action{service.Start, service.Unload, service.Disable}
	}
	return []service.Action{service.Load, service.Enable}
}

func (f *fake) Confirm(it service.Item, a service.Action) string {
	switch {
	case a == service.Remove:
		return "Really remove " + it.Name + "?"
	case it.Source == service.Brew && it.Active() && (a == service.Enable || a == service.Disable):
		return "Homebrew cannot change whether a service starts at login while it runs."
	}
	return ""
}

func (f *fake) Do(it service.Item, a service.Action) error {
	f.calls = append(f.calls, fmt.Sprintf("%s %s", a, it.Name))
	for i := range f.items {
		if f.items[i].ID != it.ID {
			continue
		}
		switch a {
		case service.Stop, service.Unload:
			f.items[i].State, f.items[i].PID = service.StateStopped, 0
		case service.Start, service.Load, service.Run:
			f.items[i].State = service.StateRunning
		case service.Remove:
			f.items = slices.Delete(f.items, i, i+1)
		}
		break
	}
	return nil
}

func item(src service.Source, name string, state service.State) service.Item {
	it := service.Item{ID: string(src) + ":" + name, Source: src, Name: name, State: state, Autostart: service.AutostartOn}
	if state == service.StateRunning {
		it.PID = 100
	}
	return it
}

type loginFake struct {
	fake
	added []string
}

func (l *loginFake) Add(path string) error {
	l.added = append(l.added, path)
	return nil
}

// harness is an app on fake sources, whose background work runs at once,
// with the tester rendering it.
type harness struct {
	t      *testing.T
	a      *app
	tt     *ui.Tester
	brew   *fake
	agents *fake
	login  *loginFake
	copied []string
}

func newHarness(t *testing.T) *harness {
	t.Helper()
	h := &harness{t: t,
		brew: &fake{src: service.Brew, items: []service.Item{
			item(service.Brew, "postgresql", service.StateRunning),
			item(service.Brew, "redis", service.StateStopped),
		}},
		agents: &fake{src: service.UserAgent, items: []service.Item{
			item(service.UserAgent, "com.example.sync", service.StateLoaded),
		}},
		login: &loginFake{fake: fake{src: service.LoginItem, items: []service.Item{
			{ID: "login-item:1", Source: service.LoginItem, Name: "CleanShot X", Path: "/Applications/CleanShot X.app", Autostart: service.AutostartOn},
		}}},
	}
	h.start(map[service.Source]service.Provider{
		service.Brew:        h.brew,
		service.UserAgent:   h.agents,
		service.SystemAgent: &fake{src: service.SystemAgent},
		service.Daemon:      &fake{src: service.Daemon},
		service.LoginItem:   h.login,
	})
	return h
}

func (h *harness) start(providers map[service.Source]service.Provider) {
	h.a = newAppWith(providers, h.login, h.t.TempDir())
	h.a.post = func(f func()) { f() }
	h.a.spawn = func(f func()) { f() }
	h.a.settle = 0
	h.a.copy = func(s string) { h.copied = append(h.copied, s) }
	h.a.reveal = func(string) {}
	h.a.chooseApp = func() (string, bool) { return "/Applications/Vitals.app", true }
	h.a.refreshAll()
	h.tt = ui.NewTester(h.a.view, 1080, 680)
	h.tt.Frame()
}

func (h *harness) click(s string) {
	h.t.Helper()
	if err := h.tt.Click(s); err != nil {
		h.t.Fatalf("click %q: %v\ntexts: %q", s, err, h.tt.Texts())
	}
	h.tt.Frame()
}

func (h *harness) wantText(s ...string) {
	h.t.Helper()
	for _, w := range s {
		if !h.tt.HasText(w) {
			h.t.Errorf("missing %q in %q", w, h.tt.Texts())
		}
	}
}

func (h *harness) wantNoText(s ...string) {
	h.t.Helper()
	for _, w := range s {
		if h.tt.HasText(w) {
			h.t.Errorf("unexpected %q in %q", w, h.tt.Texts())
		}
	}
}

func TestAllItemsListsEverySource(t *testing.T) {
	h := newHarness(t)
	h.wantText("All Items", "postgresql", "redis", "com.example.sync", "CleanShot X", "brew", "user agent", "login item")
	if got := len(h.a.visible); got != 4 {
		t.Errorf("visible = %d, want 4", got)
	}
	total, running := h.a.counts(navAll)
	if total != 4 || running != 1 {
		t.Errorf("counts = %d, %d", total, running)
	}
	h.wantText("4 items, 1 running")
}

func TestSidebarAndFilter(t *testing.T) {
	h := newHarness(t)
	h.click("Homebrew")
	h.wantText("Homebrew", "postgresql", "redis")
	h.wantNoText("com.example.sync", "CleanShot X")

	h.click("Running")
	h.wantText("postgresql")
	h.wantNoText("redis")
	h.click("Stopped")
	h.wantText("redis")
	h.wantNoText("postgresql")
	h.click("All")

	h.click("All Items")
	if err := h.tt.Click("Search"); err != nil {
		t.Fatal(err)
	}
	h.tt.Type("sync")
	h.tt.Frame()
	h.wantText("com.example.sync")
	h.wantNoText("postgresql", "redis")
	h.tt.Type("zzz")
	h.tt.Frame()
	h.wantText("Nothing matches")
	h.click("Clear search and filter")
	h.wantText("postgresql", "redis")
}

func TestPrimaryActionFromTheRow(t *testing.T) {
	h := newHarness(t)
	h.click("Homebrew")
	h.click("Stop") // postgresql runs: its row offers Stop
	if !slices.Equal(h.brew.calls, []string{"stop postgresql"}) {
		t.Fatalf("calls = %v", h.brew.calls)
	}
	h.wantText("postgresql stopped")
	if len(h.a.busy) != 0 {
		t.Errorf("busy = %v", h.a.busy)
	}
}

func TestDetailOfTheItemChosen(t *testing.T) {
	h := newHarness(t)
	h.brew.detail = service.Detail{
		Rows: []service.Row{{Key: "service name", Value: "sh.brew.postgresql"}, {Key: "pid", Value: "100"}},
		Logs: []service.LogFile{{Label: "Output", Path: "/nonexistent/pg.log"}},
	}
	h.click("Homebrew")
	h.wantText("Select an item")
	h.click("postgresql")
	h.wantText("service name", "sh.brew.postgresql", "Info", "Logs", "Restart")
	h.wantNoText("Select an item")

	h.click("Copy")
	if !slices.Equal(h.copied, []string{"sh.brew.postgresql"}) {
		t.Errorf("copied = %v", h.copied)
	}

	h.click("Logs")
	h.wantText("/nonexistent/pg.log", "Console")
	h.a.log.last = h.a.log.last.Add(-10 * 1e9)
	h.tt.Frame()
	if h.a.log.err == "" {
		t.Errorf("a missing log file should say so; log = %+v", h.a.log)
	}
}

func TestLogTabShowsTheEndOfTheFile(t *testing.T) {
	h := newHarness(t)
	path := filepath.Join(t.TempDir(), "x.log")
	if err := os.WriteFile(path, []byte("first line\nsecond line\nthird line\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	h.brew.detail = service.Detail{Logs: []service.LogFile{{Label: "Output", Path: path}}}
	h.click("Homebrew")
	h.click("postgresql")
	h.click("Logs")
	h.a.log.last = h.a.log.last.Add(-10 * 1e9)
	h.tt.Frame()
	h.tt.Frame()
	h.wantText("third line")
	if !slices.Equal(h.a.log.lines, []string{"first line", "second line", "third line"}) {
		t.Errorf("lines = %q", h.a.log.lines)
	}
	// The file grows: the next poll picks it up.
	f, _ := os.OpenFile(path, os.O_APPEND|os.O_WRONLY, 0)
	f.WriteString("fourth line\n")
	f.Close()
	h.a.log.last = h.a.log.last.Add(-10 * 1e9)
	h.tt.Frame()
	h.tt.Frame()
	h.wantText("fourth line")
}

func TestRemoveAsksFirst(t *testing.T) {
	h := newHarness(t)
	h.click("Login Items")
	h.click("CleanShot X")
	h.click("Remove…")
	h.wantText("Remove CleanShot X?", "Really remove CleanShot X?")
	if len(h.login.calls) != 0 {
		t.Fatalf("nothing happens before the answer: %v", h.login.calls)
	}
	h.click("Cancel")
	if len(h.login.calls) != 0 {
		t.Fatalf("Cancel must not run it: %v", h.login.calls)
	}
	h.click("Remove…")
	h.click("Remove")
	if !slices.Equal(h.login.calls, []string{"remove CleanShot X"}) {
		t.Errorf("calls = %v", h.login.calls)
	}
	if slices.Contains(h.tt.Texts(), "CleanShot X") {
		t.Errorf("the item is still listed: %q", h.tt.Texts())
	}
	h.wantText("CleanShot X removed")
	if h.a.chosen.Len() != 0 {
		t.Error("a removed item is not chosen any more")
	}
}

func TestBulkStartAndStop(t *testing.T) {
	h := newHarness(t)
	h.click("Homebrew")
	h.click("postgresql")
	if err := h.tt.ClickWith(ui.Cmd, "redis"); err != nil {
		t.Fatal(err)
	}
	h.tt.Frame()
	h.wantText("2 items selected", "Start 1", "Stop 1")
	h.click("Stop 1")
	if !slices.Equal(h.brew.calls, []string{"stop postgresql"}) {
		t.Errorf("calls = %v", h.brew.calls)
	}
}

func TestSourceErrorShowsAndOthersLoad(t *testing.T) {
	h := newHarness(t)
	h.brew.listErr = errors.New("brew: command not found")
	h.a.refreshAll()
	h.tt.Frame()
	h.wantText("Homebrew: brew: command not found", "com.example.sync")
}

func TestAddLoginItem(t *testing.T) {
	h := newHarness(t)
	h.click("Login Items")
	h.click("Add Login Item…")
	if !slices.Equal(h.login.added, []string{"/Applications/Vitals.app"}) {
		t.Errorf("added = %v", h.login.added)
	}
}

// The real Homebrew provider on a stubbed brew: changing whether a running
// service starts at login restarts it, so the app asks first.
func TestBrewAutostartAsksBeforeRestartingARunningService(t *testing.T) {
	old := system.Run
	defer func() { system.Run = old }()
	var calls []string
	running := true
	system.Run = func(name string, args ...string) (string, error) {
		calls = append(calls, strings.Join(args, " "))
		switch args[1] {
		case "list":
			status := "none"
			if running {
				status = "started"
			}
			return fmt.Sprintf(`[{"name":"postgresql","status":%q,"user":"jyeu","file":"/p.plist","exit_code":null}]`, status), nil
		case "stop":
			running = false
		case "start", "run":
			running = true
		}
		return "[]", nil
	}

	h := &harness{t: t, login: &loginFake{}}
	h.start(map[service.Source]service.Provider{service.Brew: &brew.Provider{Registered: []string{t.TempDir()}}})
	calls = nil
	h.click("Homebrew")
	h.click("postgresql")

	// Turning it on while it runs: asks, and says why.
	h.click("Start at login")
	h.wantText("Enable postgresql?", "stopped and started again", "interrupted for a few seconds")
	for _, c := range calls {
		if strings.Contains(c, "stop") || strings.Contains(c, "start") {
			t.Fatalf("nothing may run before the answer: %v", calls)
		}
	}

	// Cancel: nothing happens.
	h.click("Cancel")
	if slices.ContainsFunc(calls, func(c string) bool { return strings.Contains(c, "stop") }) {
		t.Fatalf("Cancel must not stop the service: %v", calls)
	}
	h.wantNoText("stopped and started again")

	// Continue: it is stopped, then started registered.
	h.click("Start at login")
	h.click("Enable")
	var steps []string
	for _, c := range calls {
		if !strings.HasPrefix(c, "services list") && !strings.HasPrefix(c, "services info") {
			steps = append(steps, c)
		}
	}
	if !slices.Equal(steps, []string{"services stop postgresql", "services start postgresql"}) {
		t.Errorf("steps = %v", steps)
	}

	// A stopped service has nothing to interrupt: no question.
	running = false
	h.a.refreshAll()
	h.tt.Frame()
	calls = nil
	h.click("Start at login")
	h.wantNoText("stopped and started again")
	if !slices.ContainsFunc(calls, func(c string) bool { return c == "services start postgresql" }) {
		t.Errorf("enabling a stopped service starts it at once: %v", calls)
	}
}

func TestNewLaunchAgent(t *testing.T) {
	old := system.Run
	defer func() { system.Run = old }()
	system.Run = func(string, ...string) (string, error) { return "", nil } // plutil -lint

	h := newHarness(t)
	h.click("User Agents")
	h.click("New Agent…")
	h.wantText("New Launch Agent", "Label", "Command", "Save")

	// Invalid: the error shows, and nothing is written.
	h.click("Save")
	h.wantText("the label may hold letters")
	if entries, _ := os.ReadDir(h.a.agentDir); len(entries) != 0 {
		t.Fatalf("an invalid agent was written: %v", entries)
	}

	if err := h.tt.Click("Label"); err != nil {
		t.Fatal(err)
	}
	h.tt.Type("com.example.job")
	if err := h.tt.Click("Command"); err != nil {
		t.Fatal(err)
	}
	h.tt.Type("/bin/sleep")
	h.tt.Frame()
	h.click("Save")

	path := filepath.Join(h.a.agentDir, "com.example.job.plist")
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("the agent was not written: %v\ntexts %q", err, h.tt.Texts())
	}
	spec, err := launchd.LoadSpec(path)
	if err != nil {
		t.Fatal(err)
	}
	if spec.Label != "com.example.job" || !slices.Equal(spec.Args, []string{"/bin/sleep"}) || !spec.RunAtLoad {
		t.Errorf("spec = %+v\n%s", spec, data)
	}
	if !strings.HasSuffix(spec.StdOut, "/Library/Logs/com.example.job.log") {
		t.Errorf("output goes to the default log: %q", spec.StdOut)
	}
	if h.a.editor.open {
		t.Errorf("the dialog should close; error: %q", h.a.editor.err)
	}
	if !slices.Contains(h.agents.calls, "load com.example.job") {
		t.Errorf("a new agent is loaded: %v", h.agents.calls)
	}
}

func TestMenubarPanelTogglesBrewServices(t *testing.T) {
	h := newHarness(t)
	opened := 0
	panel := ui.NewTester(func(c *ui.Context) { h.a.menubarView(c, func() { opened++ }) }, 320, 400)
	panel.Frame()
	for _, w := range []string{"Homebrew Services", "1 running", "postgresql", "redis", "Open Service Manager"} {
		if !panel.HasText(w) {
			t.Errorf("panel lacks %q: %q", w, panel.Texts())
		}
	}
	if panel.HasText("com.example.sync") {
		t.Error("the panel lists Homebrew services only")
	}
	if err := panel.Click("Open Service Manager"); err != nil || opened != 1 {
		t.Errorf("open: %v %d", err, opened)
	}
	rect, ok := panel.Find("redis")
	if !ok {
		t.Fatal("no redis row")
	}
	// The switch is at the right end of the row.
	panel.ClickAt(300, rect.Y+rect.H/2)
	panel.Frame()
	if !slices.Equal(h.brew.calls, []string{"start redis"}) && !slices.Equal(h.brew.calls, []string{"run redis"}) {
		t.Errorf("a switch turned on runs the service: calls = %v", h.brew.calls)
	}
}

func TestShots(t *testing.T) {
	dir := os.Getenv("MSM_SHOTS")
	if dir == "" {
		t.Skip("set MSM_SHOTS to a directory to write screenshots")
	}
	h := newHarness(t)
	h.brew.items = []service.Item{
		{ID: "brew:postgresql@16", Source: service.Brew, Name: "postgresql@16", State: service.StateRunning, PID: 4242, Autostart: service.AutostartOn, Owner: "jyeu", Path: "/opt/homebrew/opt/postgresql@16/sh.brew.postgresql@16.plist"},
		{ID: "brew:redis", Source: service.Brew, Name: "redis", State: service.StateStopped, Autostart: service.AutostartOff},
		{ID: "brew:nginx", Source: service.Brew, Name: "nginx", State: service.StateFailed, Autostart: service.AutostartOff},
		{ID: "brew:mysql", Source: service.Brew, Name: "mysql", State: service.StateScheduled, Autostart: service.AutostartOn},
	}
	h.agents.items = []service.Item{
		{ID: "user-agent:a", Source: service.UserAgent, Name: "io.mutagen.mutagen", State: service.StateRunning, PID: 777, Autostart: service.AutostartOn},
		{ID: "user-agent:b", Source: service.UserAgent, Name: "com.google.GoogleUpdater.wake", State: service.StateStopped, Autostart: service.AutostartOff},
	}
	h.brew.detail = service.Detail{
		Rows: []service.Row{
			{Key: "name", Value: "postgresql@16"}, {Key: "service name", Value: "sh.brew.postgresql@16"},
			{Key: "status", Value: "started"}, {Key: "starts at login", Value: "yes"}, {Key: "pid", Value: "4242"},
			{Key: "command", Value: "/opt/homebrew/opt/postgresql@16/bin/postgres -D /opt/homebrew/var/postgresql@16"},
			{Key: "log", Value: "/opt/homebrew/var/log/postgresql@16.log"},
		},
		Logs: []service.LogFile{{Label: "Output", Path: filepath.Join(dir, "pg.log")}},
	}
	_ = os.WriteFile(filepath.Join(dir, "pg.log"), []byte(
		"2026-11-03 16:01:12.101 CST [4242] LOG:  database system is ready to accept connections\n"+
			"2026-11-03 16:03:44.230 CST [4300] LOG:  checkpoint starting: time\n"+
			"2026-11-03 16:03:44.790 CST [4300] LOG:  checkpoint complete: wrote 3 buffers (0.0%)\n"), 0o644)
	h.a.refreshAll()
	shot := func(name string) {
		h.tt.Frame()
		f, err := os.Create(filepath.Join(dir, name+".png"))
		if err != nil {
			t.Fatal(err)
		}
		defer f.Close()
		if err := png.Encode(f, h.tt.Image()); err != nil {
			t.Fatal(err)
		}
	}
	shot("1-all")
	h.click("Homebrew")
	h.click("postgresql@16")
	shot("2-brew-detail")
	h.click("Logs")
	h.a.log.last = h.a.log.last.Add(-10 * 1e9)
	h.tt.Frame()
	shot("3-logs")
	h.click("Info")
	h.click("Start at login")
	shot("4-confirm")
	h.click("Cancel")
	h.click("User Agents")
	h.click("New Agent…")
	shot("5-editor")
}

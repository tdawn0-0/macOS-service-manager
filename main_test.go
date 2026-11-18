package main

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/egoist/mygo/ui"

	"devbase/internal/brew"
	"devbase/internal/service"
	"devbase/internal/system"
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
		case service.Start, service.Load:
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
	h.wantText("All Items", "postgresql", "redis", "com.example.sync", "CleanShot X", "Homebrew", "User Agents", "Login Items")
	if got := len(h.a.visible); got != 4 {
		t.Errorf("visible = %d, want 4", got)
	}
	total, running := h.a.counts(navAll)
	if total != 4 || running != 1 {
		t.Errorf("counts = %d, %d", total, running)
	}
	h.wantText("1/4")
	h.wantNoText("4 items · 1 running")
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
	if slices.Contains(h.tt.Texts(), "Stop") { // the buttons show on a row chosen or under the pointer
		t.Errorf("a button shows at rest: %q", h.tt.Texts())
	}
	h.click("postgresql")
	h.click("Stop") // it runs: its row offers Stop
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
		Rows: []service.Row{{Group: "Status", Key: "Service name", Value: "sh.brew.postgresql"}, {Group: "Status", Key: "Process ID", Value: "100"}},
		Logs: []service.LogFile{{Label: "Output", Path: "/nonexistent/pg.log"}},
	}
	h.click("Homebrew")
	h.wantNoText("Close details") // the details take room only once there is something to show
	if h.a.chosen.Len() != 0 {
		t.Fatal("nothing is chosen yet")
	}
	h.click("postgresql")
	h.wantText("Service name", "sh.brew.postgresql", "Status", "Info", "Logs", "Restart")

	// The button to copy a value shows under the pointer.
	rect, ok := h.tt.Find("Service name")
	if !ok {
		t.Fatal("no row")
	}
	h.tt.Move(rect.X+rect.W/2, rect.Y+rect.H/2)
	h.tt.Frame()
	h.click("Copy Service name")
	if !slices.Equal(h.copied, []string{"sh.brew.postgresql"}) {
		t.Errorf("copied = %v", h.copied)
	}

	h.click("Logs")
	h.wantText("/nonexistent/pg.log")
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
	// What was read last time stays until a read succeeds.
	h.wantText("postgresql")
}

func TestDetailsCloseAndGoneItemsAreForgotten(t *testing.T) {
	h := newHarness(t)
	h.click("Homebrew")
	h.click("postgresql")
	h.click("Close details")
	if h.a.chosen.Len() != 0 {
		t.Error("closing the details clears the choice")
	}
	h.click("postgresql")
	h.brew.items = h.brew.items[1:]
	h.a.refreshAll()
	h.tt.Frame()
	h.tt.Frame()
	if h.a.chosen.Len() != 0 {
		t.Error("an item that is gone is not chosen any more")
	}
}

func TestAHomebrewJobIsListedOnce(t *testing.T) {
	h := newHarness(t)
	h.agents.items = append(h.agents.items,
		item(service.UserAgent, "homebrew.mxcl.redis", service.StateLoaded),
		item(service.UserAgent, "sh.brew.postgresql", service.StateRunning),
		item(service.UserAgent, "sh.brew.uninstalled", service.StateLoaded))
	h.a.refreshAll()
	h.tt.Frame()
	h.wantText("sh.brew.uninstalled", "redis", "postgresql")
	h.wantNoText("homebrew.mxcl.redis", "sh.brew.postgresql")
	if total, running := h.a.counts(navAll); total != 5 || running != 1 {
		t.Errorf("counts = %d, %d", total, running)
	}
}

// batchFake is a source that runs an action on several items at once.
type batchFake struct {
	fake
	batches []string
}

func (f *batchFake) DoAll(items []service.Item, a service.Action) error {
	var names []string
	for _, it := range items {
		names = append(names, it.Name)
	}
	f.batches = append(f.batches, string(a)+" "+strings.Join(names, " "))
	return nil
}

func TestBulkOnItemsThatTakeAnAdminAsksOnce(t *testing.T) {
	daemons := &batchFake{fake: fake{src: service.Daemon}}
	for _, name := range []string{"a", "b", "c"} {
		it := item(service.Daemon, name, service.StateLoaded)
		it.Admin = true
		daemons.items = append(daemons.items, it)
	}
	h := &harness{t: t, login: &loginFake{}}
	h.start(map[service.Source]service.Provider{service.Daemon: daemons})
	for _, it := range daemons.items {
		h.a.chosen.Add(it.ID)
	}
	h.a.bulk(true)
	if len(daemons.batches) != 1 || len(daemons.calls) != 0 {
		t.Fatalf("batches %v, calls %v", daemons.batches, daemons.calls)
	}
	for _, name := range []string{"a", "b", "c"} {
		if !strings.Contains(daemons.batches[0], name) {
			t.Errorf("batch %q lacks %s", daemons.batches[0], name)
		}
	}
}

// staleSource is a source whose reads lag behind what was done to it, as System
// Settings' database does.
type staleSource struct{ fake }

func (f *staleSource) Do(it service.Item, a service.Action) error {
	f.calls = append(f.calls, fmt.Sprintf("%s %s", a, it.Name))
	return nil // the item stays in what List returns
}

func TestARemovedItemIsNotListedWhileReadsLagBehind(t *testing.T) {
	src := &staleSource{fake{src: service.LoginItem, items: []service.Item{
		{ID: "login-item:1", Source: service.LoginItem, Name: "CleanShot X", Path: "/Applications/CleanShot X.app", Autostart: service.AutostartOn},
	}}}
	h := &harness{t: t, login: &loginFake{}}
	h.start(map[service.Source]service.Provider{service.LoginItem: src})
	// Hold back what the background work hands over, to run it in turn.
	var queued []func()
	h.a.post = func(f func()) { queued = append(queued, f) }
	h.a.do(src.items[0], service.Remove)
	if len(queued) != 2 {
		t.Fatalf("queued %d", len(queued))
	}
	report, settled := queued[0], queued[1]
	queued = nil
	report() // it reports, and reads the source
	if len(queued) != 1 {
		t.Fatalf("the report reads the source: queued %d", len(queued))
	}
	queued[0]() // what that read returned, still with the item
	h.tt.Frame()
	if len(h.a.visible) != 0 {
		t.Errorf("a removed item is listed: %+v", h.a.visible)
	}
	settled()
	h.tt.Frame()
	// In the end the source decides: it did not go, so it shows again.
	if len(h.a.visible) != 1 {
		t.Errorf("after the reads settle, visible = %+v", h.a.visible)
	}
}

func TestSettleReadsALoginItemUntilItsDatabaseChanges(t *testing.T) {
	h := newHarness(t)
	h.a.settle = time.Millisecond
	reads := 0
	h.a.post = func(f func()) { reads++; f() }
	h.a.settleRead(service.LoginItem, nil)
	login := reads
	reads = 0
	h.a.settleRead(service.Brew, nil)
	if login != 3*reads || reads == 0 {
		t.Errorf("login items are read %d times as they settle, others %d", login, reads)
	}
}

func TestAPartialListKeepsItsItems(t *testing.T) {
	h := newHarness(t)
	h.login.listErr = errors.New("background helpers are not listed")
	h.a.refreshAll()
	h.tt.Frame()
	h.wantText("Login Items: background helpers are not listed", "CleanShot X")
}

func TestAddLoginItem(t *testing.T) {
	h := newHarness(t)
	h.click("Login Items")
	h.click("Add Login Item…") // the plus button is named so here
	if !slices.Equal(h.login.added, []string{"/Applications/Vitals.app"}) {
		t.Errorf("added = %v", h.login.added)
	}
}

// The real Homebrew provider on a stubbed brew: changing whether a running
// service starts at login leaves it running, with no question and no command.
func TestBrewAutostartDoesNotInterruptARunningService(t *testing.T) {
	old := system.Run
	defer func() { system.Run = old }()
	source := filepath.Join(t.TempDir(), "sh.brew.postgresql.plist")
	if err := os.WriteFile(source, []byte("plist"), 0o644); err != nil {
		t.Fatal(err)
	}
	var calls []string
	system.Run = func(name string, args ...string) (string, error) {
		calls = append(calls, strings.Join(args, " "))
		if args[1] == "list" {
			return fmt.Sprintf(`[{"name":"postgresql","status":"started","user":"jyeu","file":%q,"exit_code":null}]`, source), nil
		}
		return "[]", nil
	}
	agents := t.TempDir()
	h := &harness{t: t, login: &loginFake{}}
	h.start(map[service.Source]service.Provider{service.Brew: &brew.Provider{AgentDir: agents, DaemonDir: t.TempDir()}})
	h.click("Homebrew")
	h.click("postgresql")
	calls = nil

	dest := filepath.Join(agents, "sh.brew.postgresql.plist")
	h.click("Start at login")
	if _, err := os.Stat(dest); err != nil {
		t.Fatalf("not registered: %v", err)
	}
	h.tt.Frame()
	h.click("Start at login")
	if _, err := os.Stat(dest); !os.IsNotExist(err) {
		t.Fatalf("still registered: %v", err)
	}
	for _, c := range calls {
		if !strings.HasPrefix(c, "services list") && !strings.HasPrefix(c, "services info") {
			t.Errorf("the service was touched: %v", calls)
		}
	}
	h.wantNoText("Cancel")
}

// The Dock icon goes and comes back with the window. Only transitions reach
// the app: restoring an icon that shows activates the app every time.
func TestShowDockIconMakesChangesOnly(t *testing.T) {
	h := newHarness(t)
	var changed []bool
	h.a.dockOn = func(on bool) { changed = append(changed, on) }
	h.a.showDockIcon(true)  // the window is open already
	h.a.showDockIcon(false) // the window closed
	h.a.showDockIcon(false)
	h.a.showDockIcon(true) // the menu bar opened it again
	if !slices.Equal(changed, []bool{false, true}) {
		t.Errorf("Dock changes = %v, want [false true]", changed)
	}
}

// Coming back to the window reads the lists again, so what it shows and the
// actions it offers follow what changed meanwhile.
func TestTheWindowReadsAgainWhenItComesBack(t *testing.T) {
	h := newHarness(t)
	state := func() service.State {
		for _, it := range h.a.sources[service.Brew].items {
			if it.Name == "redis" {
				return it.State
			}
		}
		t.Fatal("redis is not listed")
		return 0
	}
	h.brew.items[1].State = service.StateRunning // started elsewhere
	h.a.windowShown()
	if state() != service.StateRunning {
		t.Fatal("the window showed what it read before it went away")
	}
	// Activations close together read once.
	h.brew.items[1].State = service.StateStopped
	h.a.windowShown()
	if state() != service.StateRunning {
		t.Fatal("a second activation at once read again")
	}
	h.a.shownAt = h.a.shownAt.Add(-3 * time.Second)
	h.a.windowShown()
	if state() != service.StateStopped {
		t.Fatal("a later activation did not read again")
	}
	// The development workspace reads its own state.
	h.a.nav = navDevelopment
	h.a.shownAt = time.Time{}
	h.brew.items[1].State = service.StateRunning
	h.a.windowShown()
	if state() != service.StateStopped {
		t.Fatal("the development workspace read the inspector's lists")
	}
}

// A property list launchd cannot load is listed as itself, even where a
// Homebrew service of the same name hides its valid job, and it is not running.
func TestABrokenJobFileIsListedAsInvalid(t *testing.T) {
	h := newHarness(t)
	h.agents.items = append(h.agents.items, item(service.UserAgent, "sh.brew.postgresql", service.StateInvalid))
	h.a.refreshAll()
	h.tt.Frame()
	h.wantText("sh.brew.postgresql", "Invalid")
	if _, running := h.a.counts(string(service.UserAgent)); running != 0 {
		t.Errorf("an invalid file counts as running: %d", running)
	}
}

func TestSidebarInspectorSectionAndKeyboardShortcuts(t *testing.T) {
	h := newHarness(t)
	h.a.dev.loaded = true
	h.wantText("Inspector", "All Items", "Workspace", "Software", "Databases", "HTTPS")

	// Selecting an item opens the inspector; Cmd+1/2/3 clears it and switches workspace tabs.
	h.click("postgresql")
	if h.a.chosen.Len() != 1 {
		t.Fatalf("expected item selected, got %d", h.a.chosen.Len())
	}

	h.a.dev.diagnosticsOpen = true
	h.a.dev.operationsOpen = true
	h.tt.Key(ui.Cmd, ui.Key2)
	h.tt.Frame()
	if h.a.nav != navDevelopment || h.a.dev.tab != devTabDatabases || h.a.chosen.Len() != 0 || h.a.dev.diagnosticsOpen || h.a.dev.operationsOpen {
		t.Fatalf("Cmd+2 state mismatch: nav=%q tab=%d chosen=%d diag=%v ops=%v", h.a.nav, h.a.dev.tab, h.a.chosen.Len(), h.a.dev.diagnosticsOpen, h.a.dev.operationsOpen)
	}

	h.tt.Key(ui.Cmd, ui.Key3)
	h.tt.Frame()
	if h.a.nav != navDevelopment || h.a.dev.tab != devTabHTTPS {
		t.Fatalf("Cmd+3 state mismatch: nav=%q tab=%d", h.a.nav, h.a.dev.tab)
	}

	h.tt.Key(ui.Cmd, ui.Key1)
	h.tt.Frame()
	if h.a.nav != navDevelopment || h.a.dev.tab != devTabSoftware {
		t.Fatalf("Cmd+1 state mismatch: nav=%q tab=%d", h.a.nav, h.a.dev.tab)
	}

	// Cmd+, opens Settings and refreshes settings state.
	h.a.settingsLoaded = false
	h.tt.Key(ui.Cmd, ui.KeyComma)
	h.tt.Frame()
	if h.a.nav != navSettings || !h.a.settingsLoaded {
		t.Fatalf("Cmd+, state mismatch: nav=%q settingsLoaded=%v", h.a.nav, h.a.settingsLoaded)
	}
}

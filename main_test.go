package main

import (
	"bytes"
	"errors"
	"fmt"
	"image"
	"image/png"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

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
	h.wantText("4 items · 1 running")
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

func TestNewLaunchAgent(t *testing.T) {
	old := system.Run
	defer func() { system.Run = old }()
	system.Run = func(string, ...string) (string, error) { return "", nil } // plutil -lint

	h := newHarness(t)
	h.click("User Agents")
	h.click("New Job…")
	h.wantText("New Job", "Label", "Command", "Save", "User Agent", "System Agent", "Daemon", "As a service")

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
	h.tt.Type("sleep 1 && echo done")
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
	if spec.Label != "com.example.job" || !slices.Equal(spec.Args, []string{"/bin/sh", "-c", "sleep 1 && echo done"}) || !spec.RunAtLoad {
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

func TestMenubarPanelTogglesServices(t *testing.T) {
	h := newHarness(t)
	opened := 0
	panel := ui.NewTester(func(c *ui.Context) { h.a.menubarView(c, func() { opened++ }) }, 320, 400)
	panel.Frame()
	for _, w := range []string{"Service Manager", "1 running", "Homebrew", "postgresql", "redis", "User Agents", "com.example.sync", "Open Service Manager"} {
		if !panel.HasText(w) {
			t.Errorf("panel lacks %q: %q", w, panel.Texts())
		}
	}
	if panel.HasText("CleanShot X") {
		t.Error("login items have no switch to show in the panel")
	}
	if err := panel.Click("Open Service Manager"); err != nil || opened != 1 {
		t.Errorf("open: %v %d", err, opened)
	}
	rect, ok := panel.Find("redis")
	if !ok {
		t.Fatal("no redis row")
	}
	// The switch is at the right end of the row.
	panel.ClickAt(290, rect.Y+rect.H/2)
	panel.Frame()
	if !slices.Equal(h.brew.calls, []string{"load redis"}) {
		t.Errorf("a switch turned on starts the service: calls = %v", h.brew.calls)
	}
}

func TestARowsMenuListsWhatCanBeDone(t *testing.T) {
	h := newHarness(t)
	h.a.providers[service.UserAgent] = launchd.UserAgents()
	path := filepath.Join(t.TempDir(), "com.example.sync.plist")
	h.agents.items[0].Path = path
	h.a.sources[service.UserAgent].items = h.agents.items
	h.click("User Agents")
	if err := h.tt.RightClick("com.example.sync"); err != nil {
		t.Fatal(err)
	}
	h.tt.Frame()
	menu := fmt.Sprint(h.tt.Menu())
	for _, want := range []string{"Start", "Unload", "Edit…", "Reveal in Finder", "Copy Path", "Remove…"} {
		if !strings.Contains(menu, want) {
			t.Errorf("the menu lacks %q: %s", want, menu)
		}
	}
	if err := h.tt.ChooseMenuItem("Copy Path"); err != nil {
		t.Fatal(err)
	}
	h.tt.Frame()
	if !slices.Equal(h.copied, []string{path}) {
		t.Errorf("copied = %v", h.copied)
	}
}

func TestSwitchAction(t *testing.T) {
	for _, c := range []struct {
		actions []service.Action
		on      bool
		want    service.Action
	}{
		{[]service.Action{service.Start}, true, service.Start},
		{[]service.Action{service.Load}, true, service.Load},
		{[]service.Action{service.Stop, service.Restart}, true, service.Restart}, // a service in error
		{[]service.Action{service.Stop, service.Restart}, false, service.Stop},
		{[]service.Action{service.Start, service.Unload}, false, service.Unload}, // a scheduled job
		{nil, true, ""},
	} {
		if got := switchAction(c.actions, c.on); got != c.want {
			t.Errorf("switchAction(%v, %v) = %q, want %q", c.actions, c.on, got, c.want)
		}
	}
}

func TestShots(t *testing.T) {
	dir := os.Getenv("MSM_SHOTS")
	if dir == "" {
		t.Skip("set MSM_SHOTS to a directory to write screenshots")
	}
	for _, look := range []struct {
		name   string
		dark   bool
		width  int
		height int
	}{
		{"light", false, 1120, 720},
		{"dark", true, 1120, 720},
		{"min", false, 900, 520},
	} {
		t.Run(look.name, func(t *testing.T) { shots(t, dir, look.name, look.dark, look.width, look.height) })
	}
}

func shots(t *testing.T, dir, name string, dark bool, width, height int) {
	h := newHarness(t)
	h.a.toasts = nil
	h.brew.items = []service.Item{
		{ID: "brew:caddy", Source: service.Brew, Name: "caddy", State: service.StateStopped, Autostart: service.AutostartOff},
		{ID: "brew:nginx", Source: service.Brew, Name: "nginx", State: service.StateFailed, Autostart: service.AutostartOff},
		{ID: "brew:php", Source: service.Brew, Name: "php", State: service.StateRunning, PID: 407, Autostart: service.AutostartOn, Admin: true, Owner: "root", Path: "/Library/LaunchDaemons/homebrew.mxcl.php.plist"},
		{ID: "brew:postgresql@17", Source: service.Brew, Name: "postgresql@17", State: service.StateRunning, PID: 4242, Autostart: service.AutostartOn, Owner: "jyeu", Path: "/Users/jyeu/Library/LaunchAgents/sh.brew.postgresql@17.plist"},
		{ID: "brew:redis", Source: service.Brew, Name: "redis", State: service.StateStopped, Autostart: service.AutostartOff},
		{ID: "brew:xray", Source: service.Brew, Name: "xray", State: service.StateScheduled, Autostart: service.AutostartOn},
	}
	h.agents.items = []service.Item{
		{ID: "user-agent:a", Source: service.UserAgent, Name: "io.mutagen.mutagen", State: service.StateRunning, PID: 47313, Autostart: service.AutostartOn, Path: "/Users/jyeu/Library/LaunchAgents/io.mutagen.mutagen.plist"},
		{ID: "user-agent:b", Source: service.UserAgent, Name: "com.google.GoogleUpdater.wake", State: service.StateStopped, Autostart: service.AutostartOff},
		{ID: "user-agent:c", Source: service.UserAgent, Name: "com.a.very.long.bundle.identifier.that.keeps.going.UninstallPKGDeleteHelper.agent", State: service.StateLoaded, Autostart: service.AutostartOn},
	}
	h.login.items = []service.Item{
		{ID: "login-item:1", Source: service.LoginItem, Name: "CleanShot X", Owner: "MacKed", Kind: "App", State: service.StateRunning, PID: 1368, Path: "/Applications/CleanShot X.app", Autostart: service.AutostartOn},
		{ID: "login-item:2", Source: service.LoginItem, Name: "Mac Mouse Fix Helper", Owner: "Mac Mouse Fix", Kind: "Background helper", State: service.StateRunning, PID: 2173, Path: "/Applications/Mac Mouse Fix.app/Contents/Library/LoginItems/Mac Mouse Fix Helper.app", Autostart: service.AutostartOn},
		{ID: "login-item:3", Source: service.LoginItem, Name: "GOG Galaxy Auto Launch Helper", Owner: "GOG Galaxy", Kind: "Background helper", Path: "/Applications/GOG Galaxy.app/Contents/Library/LoginItems/GOG Galaxy Auto Launch Helper.app", Autostart: service.AutostartOff},
	}
	logPath := filepath.Join(t.TempDir(), "postgresql@17.log")
	h.brew.detail = service.Detail{
		Rows: []service.Row{
			{Group: "Status", Key: "Process ID", Value: "4242"},
			{Group: "Status", Key: "Runs as", Value: "jyeu"},
			{Group: "Status", Key: "Service name", Value: "sh.brew.postgresql@17"},
			{Group: "Program", Key: "Command", Value: "/opt/homebrew/opt/postgresql@17/bin/postgres -D /opt/homebrew/var/postgresql@17"},
			{Group: "Program", Key: "Working directory", Value: "/opt/homebrew"},
			{Group: "Files", Key: "Property list", Value: "/Users/jyeu/Library/LaunchAgents/sh.brew.postgresql@17.plist"},
			{Group: "Files", Key: "Output log", Value: logPath},
		},
		Logs: []service.LogFile{{Label: "Output", Path: logPath}, {Label: "Errors", Path: logPath}},
	}
	_ = os.WriteFile(logPath, []byte(
		"2026-11-03 16:01:12.101 CST [4242] LOG:  database system is ready to accept connections\n"+
			"2026-11-03 16:03:44.230 CST [4300] LOG:  checkpoint starting: time\n"+
			"2026-11-03 16:03:44.790 CST [4300] LOG:  checkpoint complete: wrote 3 buffers (0.0%)\n"), 0o644)
	h.a.refreshAll()
	h.tt.SetDark(dark)
	h.tt.SetSize(width, height)
	shot := func(tt *ui.Tester, page string) {
		tt.Frame()
		f, err := os.Create(filepath.Join(dir, name+"-"+page+".png"))
		if err != nil {
			t.Fatal(err)
		}
		defer f.Close()
		if err := png.Encode(f, tt.Image()); err != nil {
			t.Fatal(err)
		}
	}
	shot(h.tt, "1-all")
	h.click("Homebrew")
	h.click("postgresql@17")
	shot(h.tt, "2-detail")
	h.click("Logs")
	h.a.log.last = h.a.log.last.Add(-10 * 1e9)
	h.tt.Frame()
	h.tt.Frame()
	shot(h.tt, "3-logs")
	h.click("Info")
	h.click("User Agents")
	h.click("io.mutagen.mutagen")
	shot(h.tt, "4-agent")
	if err := h.tt.ClickWith(ui.Cmd, "com.google.GoogleUpdater.wake"); err != nil {
		t.Fatal(err)
	}
	shot(h.tt, "5-bulk")
	h.click("New Job…")
	shot(h.tt, "6-editor")
	h.a.editor.kind, h.a.editor.when = 2, whenCalendar
	h.a.editor.minute, h.a.editor.hour, h.a.editor.weekday = "0,30", "9-17", "1-5"
	shot(h.tt, "6-editor-daemon-calendar")
	h.a.editor.kind, h.a.editor.when = 0, whenService
	h.a.editor.minute, h.a.editor.hour, h.a.editor.weekday = "", "", ""
	h.click("Cancel")
	h.click("Login Items")
	h.click("Mac Mouse Fix Helper")
	shot(h.tt, "7-login")
	h.a.providers[service.LoginItem] = &restrictedLogin{loginFake: h.login, limit: &service.Restriction{
		Text:     "Helpers that apps registered are not listed: macOS lets only apps with Full Disk Access read them.",
		Settings: true,
	}}
	h.a.refresh(service.LoginItem)
	shot(h.tt, "7-login-restricted")
	h.a.appPath = "/Applications/Service Manager.app"
	guide := ui.NewTester(h.a.guideView, guideWidth, guideHeight)
	guide.SetDark(dark)
	for _, welcome := range []bool{true, false} {
		h.a.guide = guideState{open: true, src: service.LoginItem, welcome: welcome, checked: time.Now()}
		shot(guide, map[bool]string{true: "10-welcome", false: "11-guide"}[welcome])
	}
	h.a.guide = guideState{open: true, src: service.LoginItem, welcome: true, joined: true, settingsOpen: true, checked: time.Now()}
	guide.SetSize(sidecarWidth, sidecarHeight)
	shot(guide, "12-sidecar")
	h.a.guide.allowedAt = time.Now()
	shot(guide, "13-done")
	overlay := ui.NewTester(h.a.overlayView, 1400, 900)
	overlay.SetDark(dark)
	h.a.guide = guideState{open: true, src: service.LoginItem, joined: true, settingsOpen: true, settings: system.Rect{X: 200, Y: 120, Width: 740, Height: 625},
		windowAt: image.Pt(954, 120), icon: image.Pt(144, 106), flightStart: time.Now().Add(-flightTime / 2)}
	overlay.Frame()
	shot(overlay, "14-overlay-flight")
	h.a.guide.dragging, h.a.guide.dragged = true, true
	overlay.Frame()
	overlay.Advance(time.Second)
	shot(overlay, "15-overlay-drag")
	h.a.guide = guideState{}
	h.a.providers[service.LoginItem] = h.login
	h.a.refresh(service.LoginItem)
	h.a.query = "zzz"
	h.a.chosen.Clear()
	shot(h.tt, "8-empty")
	h.a.query = ""

	panel := ui.NewTester(func(c *ui.Context) { h.a.menubarView(c, func() {}) }, 320, h.a.panelHeight())
	panel.SetDark(dark)
	shot(panel, "9-panel")
}

// macOS tints a template image by its alpha, at the height of the menu bar.
func TestMenubarIconIsATemplate(t *testing.T) {
	img, err := png.Decode(bytes.NewReader(menubarIcon))
	if err != nil {
		t.Fatal(err)
	}
	if b := img.Bounds(); b.Dx() != 36 || b.Dy() != 36 {
		t.Errorf("size %v, want 36×36", b.Size())
	}
	drawn := 0
	for y := range img.Bounds().Dy() {
		for x := range img.Bounds().Dx() {
			r, g, b, a := img.At(x, y).RGBA()
			if r|g|b != 0 {
				t.Fatalf("(%d, %d) is not black", x, y)
			}
			if a > 0 {
				drawn++
			}
		}
	}
	if drawn == 0 {
		t.Error("the icon is empty")
	}
}

// restrictedLogin is a login source that macOS lets read only in part.
type restrictedLogin struct {
	*loginFake
	limit  *service.Restriction
	needs  bool
	lists  int
	opened int
	reads  int
}

func (r *restrictedLogin) List() ([]service.Item, error) {
	r.lists++
	return r.loginFake.List()
}

func (r *restrictedLogin) Restriction() *service.Restriction { return r.limit }

func (r *restrictedLogin) NeedsAccess() bool { return r.needs }

func (r *restrictedLogin) OpenSettings() error {
	r.opened++
	return nil
}

func (r *restrictedLogin) ReadOnce() error {
	r.reads++
	r.limit = nil
	return nil
}

// restrict makes the harness's login source one that macOS restricts, and
// counts how the guide's window is shown, closed and moved.
func (h *harness) restrict() (login *restrictedLogin, shown, hidden, joined *int) {
	login = &restrictedLogin{loginFake: h.login, needs: true, limit: &service.Restriction{Text: "Helpers are not listed", Settings: true}}
	h.a.providers[service.LoginItem] = login
	shown, hidden, joined = new(int), new(int), new(int)
	h.a.showGuide = func() { *shown++ }
	h.a.hideGuide = func() { *hidden++ }
	h.a.joinGuide = func() { *joined++ }
	h.a.welcomedPath = filepath.Join(h.t.TempDir(), "state", "welcomed")
	return login, shown, hidden, joined
}

func TestARestrictedSourceOffersWaysToListTheRest(t *testing.T) {
	h := newHarness(t)
	login, shown, _, _ := h.restrict()
	h.a.refresh(service.LoginItem)
	h.tt.Frame()
	h.wantNoText("Helpers are not listed")

	h.click("Login Items")
	h.wantText("Helpers are not listed", "Allow Full Disk Access…", "Read Once with Password", "CleanShot X")
	if login.reads != 0 {
		t.Errorf("read %d times before the user asked", login.reads)
	}

	h.click("Allow Full Disk Access…")
	if *shown != 1 || !h.a.guide.open || h.a.guide.welcome {
		t.Errorf("the guide: shown %d, %+v", *shown, h.a.guide)
	}
	h.a.closeGuide()
	h.tt.Frame()
	h.click("Read Once with Password")
	if login.reads != 1 {
		t.Errorf("read %d times", login.reads)
	}
	h.wantNoText("Helpers are not listed", "Read Once with Password")
}

func TestARestrictionWithoutASettingOffersOnlyThePassword(t *testing.T) {
	h := newHarness(t)
	h.a.providers[service.LoginItem] = &restrictedLogin{loginFake: h.login, limit: &service.Restriction{Text: "Helpers are not listed"}}
	h.a.refresh(service.LoginItem)
	h.click("Login Items")
	h.wantText("Read Once with Password")
	h.wantNoText("Allow Full Disk Access…")
}

func TestTheWelcomeOpensOnceAndCanBeSkipped(t *testing.T) {
	h := newHarness(t)
	login, shown, hidden, _ := h.restrict()
	h.a.welcome()
	if *shown != 1 || !h.a.guide.open || !h.a.guide.welcome || h.a.guide.src != service.LoginItem {
		t.Fatalf("shown %d, %+v", *shown, h.a.guide)
	}

	// What may ask for a permission waits for the user's answer to this page.
	h.a.refreshAll()
	if login.lists != 0 {
		t.Errorf("listed %d times with the welcome open", login.lists)
	}

	page := ui.NewTester(h.a.guideView, guideWidth, guideHeight)
	page.Frame()
	for _, want := range []string{"Welcome to Service Manager", "Full Disk Access", "Allow…", "Skip for Now", "Without it, some login items may not show. Everything else works, and you can do this later from Login Items."} {
		if !page.HasText(want) {
			t.Errorf("missing %q in %q", want, page.Texts())
		}
	}
	if err := page.Click("Skip for Now"); err != nil {
		t.Fatal(err)
	}
	if h.a.guide.open || *hidden != 1 {
		t.Errorf("skipping: open %v, hidden %d", h.a.guide.open, *hidden)
	}
	if login.lists != 1 {
		t.Errorf("skipping reads the source: %d", login.lists)
	}

	// It does not come back at the next launch, which still may read.
	h.a.welcome()
	if *shown != 1 {
		t.Errorf("shown %d times", *shown)
	}
}

func TestNoWelcomeWithAccess(t *testing.T) {
	h := newHarness(t)
	login, shown, _, _ := h.restrict()
	login.needs = false
	h.a.welcome()
	if *shown != 0 || h.a.guide.open {
		t.Errorf("shown %d, %+v", *shown, h.a.guide)
	}
}

func TestTheGuideLeadsToSettingsAndClosesOnceAllowed(t *testing.T) {
	h := newHarness(t)
	login, _, hidden, joined := h.restrict()
	h.a.appPath = "/Applications/Service Manager.app"
	h.a.welcome()
	page := ui.NewTester(h.a.guideView, guideWidth, guideHeight)
	page.Frame()

	if err := page.Click("Allow…"); err != nil {
		t.Fatal(err)
	}
	if login.opened != 1 || *joined != 1 || !h.a.guide.joined {
		t.Errorf("settings opened %d times, joined %d, %+v", login.opened, *joined, h.a.guide)
	}
	page.SetSize(sidecarWidth, sidecarHeight)
	page.Frame()
	if !h.a.guide.open {
		t.Error("the guide stays open for the drag")
	}
	for _, want := range []string{"Drag into the list", "Open Settings", "Skip for Now"} {
		if !page.HasText(want) {
			t.Errorf("missing %q in %q", want, page.Texts())
		}
	}
	if _, ok := page.Find("Drag Service Manager into the list"); !ok {
		t.Errorf("no icon to drag in %q", page.Texts())
	}

	// System Settings shows.
	h.a.guide.settingsOpen = true
	page.Frame()
	if !page.HasText("Waiting for access") || page.HasText("Open Settings") {
		t.Errorf("with settings open: %q", page.Texts())
	}

	// macOS allows it while the page is open: the page says so, and closes.
	login.needs = false
	h.a.guide.checked = time.Time{}
	page.Frame()
	page.Frame()
	if !page.HasText("Full Disk Access is on") || !h.a.guide.open {
		t.Errorf("once allowed: %q, open %v", page.Texts(), h.a.guide.open)
	}
	h.a.guide.allowedAt = time.Now().Add(-2 * guideDoneAfter)
	page.Frame()
	if h.a.guide.open || *hidden != 1 || login.lists != 1 {
		t.Errorf("closing: open %v, hidden %d, lists %d", h.a.guide.open, *hidden, login.lists)
	}
	if _, err := os.Stat(h.a.welcomedPath); err != nil {
		t.Error("the welcome is done:", err)
	}
}

func TestTheIconDragsAsTheAppsBundle(t *testing.T) {
	h := newHarness(t)
	h.a.appPath = "/Applications/Service Manager.app"
	data, err := h.a.guideDragData()
	if err != nil {
		t.Fatal(err)
	}
	if files, err := data.Files(); err != nil || !slices.Equal(files, []string{h.a.appPath}) {
		t.Errorf("files = %v, %v", files, err)
	}
	if p := dragPreview(); p == nil || p.Bounds().Dx() != 72 {
		t.Errorf("preview = %v", p)
	}
	if guideIcon() == nil {
		t.Error("no icon")
	}

	// A build outside a bundle has nothing to drag.
	h.restrict()
	h.a.welcome()
	h.a.guide.joined = true
	h.a.appPath = ""
	page := ui.NewTester(h.a.guideView, sidecarWidth, sidecarHeight)
	page.Frame()
	if _, ok := page.Find("Drag Service Manager into the list"); ok || !page.HasText("Add this app to the list") {
		t.Errorf("without a bundle: %q", page.Texts())
	}
}

func TestTheOverlayShowsTheWayThenTheWindowToDropOn(t *testing.T) {
	h := newHarness(t)
	h.a.guide = guideState{open: true, joined: true, src: service.LoginItem, settingsOpen: true, settings: system.Rect{X: 100, Y: 100, Width: 700, Height: 600}}
	page := ui.NewTester(h.a.overlayView, 1400, 900)
	page.Frame()
	if !page.HasText("Drop here") {
		t.Errorf("before the drag: %q", page.Texts())
	}
	h.a.guide.dragging, h.a.guide.dragged = true, true
	page.Frame()
	page.Advance(time.Second)
	page.Frame()
	if !page.HasText("Drop it in the list to allow access") || page.HasText("Drop here") {
		t.Errorf("during the drag: %q", page.Texts())
	}
	h.a.guide.settingsOpen = false
	page.Frame()
	if page.HasText("Drop it in the list to allow access") {
		t.Errorf("without the window: %q", page.Texts())
	}
}

func TestAFlightArcsToTheDropAndStops(t *testing.T) {
	from, to := image.Pt(1000, 200), image.Pt(400, 500)
	start := flightAt(from, to, 0)
	if !start.flying || start.x != 1000 || start.y != 200 || start.opacity != 0 {
		t.Errorf("start = %+v", start)
	}
	mid := flightAt(from, to, flightTime/2)
	if mid.y >= 350 {
		t.Errorf("the middle of the way is lifted: %+v", mid)
	}
	if mid.opacity < 0.9 || mid.scale >= 1 || mid.scale <= 0.8 {
		t.Errorf("mid = %+v", mid)
	}
	landed := flightAt(from, to, flightTime+rippleTime/2)
	if landed.flying || landed.ripple <= 0 || landed.ripple >= 1 || landed.x != 400 {
		t.Errorf("landed = %+v", landed)
	}
	if again := flightAt(from, to, flightTime+flightPause); !again.flying {
		t.Errorf("a next flight: %+v", again)
	}
	if end := flightAt(from, to, flightsShown*(flightTime+flightPause)); !end.done || end.flying {
		t.Errorf("end = %+v", end)
	}
}

func TestSidecarOrigin(t *testing.T) {
	area := system.Rect{X: 0, Y: 30, Width: 1920, Height: 1000}
	for _, test := range []struct {
		name string
		s    system.Rect
		x, y int
	}{
		{"at its right", system.Rect{X: 500, Y: 200, Width: 700, Height: 600}, 1214, 200},
		{"at its left", system.Rect{X: 1000, Y: 200, Width: 700, Height: 600}, 698, 200},
		{"over its corner", system.Rect{X: 100, Y: 200, Width: 1700, Height: 600}, 1618, 200},
		{"on the screen, above", system.Rect{X: 500, Y: -40, Width: 700, Height: 600}, 1214, 30},
		{"on the screen, below", system.Rect{X: 500, Y: 900, Width: 700, Height: 600}, 1214, 700},
	} {
		if x, y := sidecarOrigin(test.s, area, sidecarWidth, sidecarHeight); x != test.x || y != test.y {
			t.Errorf("%s: (%d, %d), want (%d, %d)", test.name, x, y, test.x, test.y)
		}
	}
}

func TestBundleOf(t *testing.T) {
	for exe, want := range map[string]string{
		"/Applications/Service Manager.app/Contents/MacOS/Service Manager": "/Applications/Service Manager.app",
		"/Users/me/go/bin/msm": "",
		"/tmp/x.app/msm":       "",
	} {
		if got := bundleOf(exe); got != want {
			t.Errorf("bundleOf(%q) = %q, want %q", exe, got, want)
		}
	}
}

// rootJobs is a source of jobs that only root can write, which remembers them.
type rootJobs struct {
	fake
	made   []launchd.Spec
	loads  []bool
	edited []string
	dir    string
	err    error
}

func (r *rootJobs) SaveAsRoot(s launchd.Spec, existing string, load bool) (string, error) {
	r.made, r.loads, r.edited = append(r.made, s), append(r.loads, load), append(r.edited, existing)
	if r.err != nil {
		return "", r.err
	}
	return filepath.Join(r.dir, s.Label+".plist"), nil
}

func (h *harness) typeInto(label, text string) {
	h.t.Helper()
	if err := h.tt.Click(label); err != nil {
		h.t.Fatalf("click %q: %v\ntexts: %q", label, err, h.tt.Texts())
	}
	h.tt.Type(text)
	h.tt.Frame()
}

func TestANewDaemonGoesToRootWithItsUser(t *testing.T) {
	old := system.Run
	defer func() { system.Run = old }()
	system.Run = func(string, ...string) (string, error) { return "", nil }

	h := newHarness(t)
	h.tt.SetSize(1080, 1500)
	jobs := &rootJobs{fake: fake{src: service.Daemon}, dir: "/Library/LaunchDaemons"}
	h.a.providers[service.Daemon] = jobs
	h.click("Daemons")
	h.click("New Job…")
	if h.a.editor.source() != service.Daemon {
		t.Fatalf("the form opens on the list's kind: %v", h.a.editor.source())
	}
	h.wantText("Starts at boot", "Runs as", "User", "Group", "Root directory")

	h.typeInto("Label", "com.example.nightly")
	h.typeInto("Command", "/usr/local/bin/backup")
	h.typeInto("User", "_backup")
	h.click("Save")
	if len(jobs.made) != 1 || !jobs.loads[0] {
		t.Fatalf("made %+v, loads %v, error %q", jobs.made, jobs.loads, h.a.editor.err)
	}
	spec := jobs.made[0]
	if spec.UserName != "_backup" || spec.Label != "com.example.nightly" {
		t.Errorf("spec = %+v", spec)
	}
	// A log root could write, but this user may not.
	if spec.StdOut != "" {
		t.Errorf("a daemon with a user has no default log: %q", spec.StdOut)
	}
	if h.a.editor.open || h.a.nav != string(service.Daemon) {
		t.Errorf("open %v, nav %q", h.a.editor.open, h.a.nav)
	}
	if entries, _ := os.ReadDir(h.a.agentDir); len(entries) != 0 {
		t.Errorf("nothing goes to the user's agents: %v", entries)
	}

	// As root, its log goes where root logs.
	h.a.newJob()
	h.a.editor.label, h.a.editor.args = "com.example.root", "/bin/date"
	got, err := h.a.editor.spec()
	if err != nil || got.StdOut != "/Library/Logs/com.example.root.log" {
		t.Errorf("spec = %+v, %v", got, err)
	}
}

func TestASystemAgentHasNoUserAndNoDefaultLog(t *testing.T) {
	h := newHarness(t)
	jobs := &rootJobs{fake: fake{src: service.SystemAgent}, dir: "/Library/LaunchAgents"}
	h.a.providers[service.SystemAgent] = jobs
	h.click("System Agents")
	h.click("New Job…")
	h.wantText("Starts at login", "Saving asks for your password")
	h.wantNoText("Runs as", "Root directory", "if no log file is set")
	h.a.editor.label, h.a.editor.args = "com.example.all", "/bin/date"
	got, err := h.a.editor.spec()
	if err != nil || got.StdOut != "" {
		t.Errorf("spec = %+v, %v", got, err)
	}
}

func TestAFailureToCreateARootJobKeepsTheForm(t *testing.T) {
	h := newHarness(t)
	h.a.providers[service.Daemon] = &rootJobs{fake: fake{src: service.Daemon}, err: errors.New("authorization canceled")}
	h.click("Daemons")
	h.click("New Job…")
	h.typeInto("Label", "com.example.nightly")
	h.typeInto("Command", "/bin/date")
	h.click("Save")
	if !h.a.editor.open || !h.tt.HasText("authorization canceled") {
		t.Errorf("open %v, %q", h.a.editor.open, h.tt.Texts())
	}
}

func TestTheFormsScheduleModes(t *testing.T) {
	h := newHarness(t)
	h.tt.SetSize(1080, 1500)
	h.click("User Agents")
	h.click("New Job…")
	e := &h.a.editor
	e.label, e.args = "com.example.job", "/bin/date"

	// As a service: restarting after a crash runs it at load, whatever the switch.
	h.click("After a crash")
	spec, err := e.spec()
	if err != nil || !spec.RestartOnCrash || spec.KeepAlive {
		t.Errorf("spec = %+v, %v", spec, err)
	}
	h.tt.Frame()
	e.mount, e.queue = true, "/tmp/a, /tmp/b"

	// A schedule drops what only a service does, which it no longer shows.
	h.click("At an interval")
	h.wantText("Run every (seconds)", "Also starts at login")
	h.wantNoText("Queue directories", "Restarts")
	if e.restart != restartNever || e.mount || e.queue != "" || e.runAtLoad {
		t.Errorf("after choosing an interval: %+v", e)
	}
	if _, err := e.spec(); err == nil {
		t.Error("an interval needs its seconds")
	}
	e.interval = "90"
	if spec, err := e.spec(); err != nil || spec.StartInterval != 90 {
		t.Errorf("spec = %+v, %v", spec, err)
	}

	h.click("On a calendar")
	h.wantText("Minute", "Hour", "Day", "Month", "Weekday")
	if _, err := e.spec(); err == nil {
		t.Error("a calendar needs a field")
	}
	e.hour, e.weekday = "9-17", "1-5"
	h.tt.Frame()
	spec, err = e.spec()
	if err != nil || spec.StartInterval != 0 || spec.Calendar.Hour != "9-17" || spec.Calendar.Weekday != "1-5" {
		t.Errorf("spec = %+v, %v", spec, err)
	}
	e.minute = "61"
	if _, err := e.spec(); err == nil || !strings.Contains(err.Error(), "0-59") {
		t.Errorf("err = %v", err)
	}
}

func TestEditingKeepsWhatTheFormDoesNotShow(t *testing.T) {
	old := system.Run
	defer func() { system.Run = old }()
	system.Run = func(string, ...string) (string, error) { return "", nil }

	h := newHarness(t)
	path := filepath.Join(h.a.agentDir, "com.example.mixed.plist")
	if err := os.WriteFile(path, []byte(`<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>Label</key><string>com.example.mixed</string>
<key>ProgramArguments</key><array><string>/bin/sh</string><string>-c</string><string>date</string></array>
<key>StartInterval</key><integer>60</integer>
<key>KeepAlive</key><true/>
<key>StartOnMount</key><true/>
<key>QueueDirectories</key><array><string>/tmp/q</string></array>
<key>Nice</key><integer>5</integer>
</dict></plist>`), 0o644); err != nil {
		t.Fatal(err)
	}
	h.a.editJob(service.Item{Name: "com.example.mixed", Path: path, Source: service.UserAgent})
	e := &h.a.editor
	if e.command != commandShell || e.args != "date" || e.when != whenInterval || e.restart != restartAlways || !e.mount || e.queue != "/tmp/q" {
		t.Fatalf("loaded: %+v", e)
	}
	spec, err := e.spec()
	if err != nil {
		t.Fatal(err)
	}
	if !spec.KeepAlive || !spec.StartOnMount || len(spec.QueueDirs) != 1 || spec.StartInterval != 60 {
		t.Errorf("an edit without a change drops nothing: %+v", spec)
	}
}

// rootJobFile writes a daemon's file, as its installer might have.
func rootJobFile(t *testing.T) (dir, path string) {
	t.Helper()
	dir = t.TempDir()
	path = filepath.Join(dir, "com.vendor.helper.plist")
	if err := os.WriteFile(path, []byte(`<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>Label</key><string>com.vendor.helper</string>
<key>Program</key><string>/opt/vendor/helper</string>
<key>UserName</key><string>_vendor</string>
<key>MachServices</key><dict><key>com.vendor.helper</key><true/></dict>
</dict></plist>`), 0o644); err != nil {
		t.Fatal(err)
	}
	return dir, path
}

func TestEditingARootJobSaysWhatChangesBeforeThePassword(t *testing.T) {
	old := system.Run
	defer func() { system.Run = old }()
	system.Run = func(string, ...string) (string, error) { return "", nil }

	dir, path := rootJobFile(t)
	h := newHarness(t)
	h.tt.SetSize(1080, 1500)
	jobs := &rootJobs{fake: fake{src: service.Daemon}, dir: dir}
	h.a.providers[service.Daemon] = jobs
	h.a.editJob(service.Item{Name: "com.vendor.helper", Path: path, Source: service.Daemon, State: service.StateRunning})
	h.tt.Frame()
	h.wantText("Edit Job", "Saving asks for your password", "Runs as", "Reload it after saving")
	if got := h.a.editor.user; got != "_vendor" {
		t.Errorf("user = %q", got)
	}

	// Nothing to save: no question, no password, and no restart.
	h.click("Save")
	if h.a.editor.open || h.a.ask.open || len(jobs.made) != 0 {
		t.Fatalf("open %v, ask %v, made %v", h.a.editor.open, h.a.ask.open, jobs.made)
	}

	h.a.editJob(service.Item{Name: "com.vendor.helper", Path: path, Source: service.Daemon, State: service.StateRunning})
	h.tt.Frame()
	h.a.editor.user = "_other"
	h.a.editor.runAtLoad = true
	h.click("Save")
	h.wantText("Save changes to com.vendor.helper?", "This changes RunAtLoad, UserName.", "restarts it if it runs", "macOS will ask for your password")
	if len(jobs.made) != 0 {
		t.Fatal("saved before the answer")
	}
	// Cancel, left of the answer and above the form's own, goes back to the form
	// with what was typed.
	answer, ok := h.tt.Find("Save Changes")
	if !ok {
		t.Fatal("no answer")
	}
	h.tt.ClickAt(answer.X-20, answer.Y+answer.H/2)
	h.tt.Frame()
	if h.a.ask.open || !h.a.editor.open || h.a.editor.user != "_other" || len(jobs.made) != 0 {
		t.Fatalf("ask %v, form %v, made %v", h.a.ask.open, h.a.editor.open, jobs.made)
	}

	h.click("Save")
	h.click("Save Changes")
	if len(jobs.made) != 1 || jobs.edited[0] != path || !jobs.loads[0] || jobs.made[0].UserName != "_other" {
		t.Fatalf("made %+v, edited %v, loads %v, error %q", jobs.made, jobs.edited, jobs.loads, h.a.editor.err)
	}
	if h.a.editor.open {
		t.Error("the form stays open")
	}
}

func TestAJobSavedButNotLoadedIsEditedNext(t *testing.T) {
	old := system.Run
	defer func() { system.Run = old }()
	system.Run = func(string, ...string) (string, error) { return "", nil }

	h := newHarness(t)
	jobs := &savedNotLoaded{rootJobs{fake: fake{src: service.Daemon}, dir: "/Library/LaunchDaemons"}}
	h.a.providers[service.Daemon] = jobs
	h.click("Daemons")
	h.click("New Job…")
	h.a.editor.label, h.a.editor.args = "com.example.d", "/bin/date"
	h.tt.Frame()
	h.click("Save")
	h.wantText("saved, but it did not load")
	if want := "/Library/LaunchDaemons/com.example.d.plist"; h.a.editor.existing != want {
		t.Errorf("existing = %q, want %q", h.a.editor.existing, want)
	}
}

// savedNotLoaded saves a job and fails to load it.
type savedNotLoaded struct{ rootJobs }

func (s *savedNotLoaded) SaveAsRoot(spec launchd.Spec, existing string, load bool) (string, error) {
	path, _ := s.rootJobs.SaveAsRoot(spec, existing, load)
	return path, errors.New("saved, but it did not load: Input/output error")
}

func TestAnyKindOfJobCanBeEditedAndRemoved(t *testing.T) {
	for _, c := range []struct {
		src  service.Source
		nav  string
		list func() *launchd.Provider
	}{
		{service.SystemAgent, "System Agents", launchd.SystemAgents},
		{service.Daemon, "Daemons", launchd.Daemons},
	} {
		h := newHarness(t)
		item := service.Item{ID: string(c.src) + ":/x/com.example.job.plist", Source: c.src, Name: "com.example.job", Path: "/x/com.example.job.plist", State: service.StateLoaded}
		h.a.providers[c.src] = c.list()
		h.a.sources[c.src].items = []service.Item{item}
		h.click(c.nav)
		if err := h.tt.RightClick("com.example.job"); err != nil {
			t.Fatal(err)
		}
		h.tt.Frame()
		menu := fmt.Sprint(h.tt.Menu())
		for _, want := range []string{"Edit…", "Remove…"} {
			if !strings.Contains(menu, want) {
				t.Errorf("%s: the menu lacks %q: %s", c.src, want, menu)
			}
		}
		if err := h.tt.ChooseMenuItem("Remove…"); err != nil {
			t.Fatal(err)
		}
		h.tt.Frame()
		h.wantText("Remove com.example.job?", "not an undo", "may stop working", "macOS will ask for your password")
		h.click("Cancel")
		if h.a.ask.open {
			t.Errorf("%s: the question stays open", c.src)
		}
	}
}

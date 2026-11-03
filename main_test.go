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
	h.click("New Launch Agent…")
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
	h.click("New Launch Agent…")
	shot(h.tt, "6-editor")
	h.click("Cancel")
	h.click("Login Items")
	h.click("Mac Mouse Fix Helper")
	shot(h.tt, "7-login")
	h.a.query = "zzz"
	h.a.chosen.Clear()
	shot(h.tt, "8-empty")
	h.a.query = ""

	panel := ui.NewTester(func(c *ui.Context) { h.a.menubarView(c, func() {}) }, 320, h.a.panelHeight())
	panel.SetDark(dark)
	shot(panel, "9-panel")
}

package main

import (
	"fmt"
	"strings"
	"testing"

	"devbase/internal/devplatform"
	"github.com/egoist/mygo/ui"
)

func TestUIIndependentResourceLifecycle(t *testing.T) {
	t.Run("menu", func(t *testing.T) {
		a := designFixture(t)
		a.dev.state.Instances["one"] = devplatform.Instance{ID: "one", Kind: "redis"}
		a.dev.state.Instances["two"] = devplatform.Instance{ID: "two", Kind: "redis"}
		a.dev.state.ProxyRules = []devplatform.ProxyRule{{Domain: "one.devbase.localhost", LocalPort: 3000}, {Domain: "two.devbase.localhost", LocalPort: 4000}}
		a.dev.statuses = map[string]string{"instance:one": "Running", "instance:two": "Unknown", "proxy": "Running"}
		opened := 0
		panel := ui.NewTester(func(c *ui.Context) { a.menubarView(c, func() { opened++ }) }, 320, a.panelHeight())
		panel.Frame()
		shown := strings.Join(panel.Texts(), " ")
		for _, want := range []string{"2 running", "Databases", "Stop one", "Unknown · Redis", "Proxy · Running", "HTTPS settings"} {
			if !strings.Contains(shown, want) {
				t.Fatalf("missing %q: %s", want, shown)
			}
		}
		if strings.Contains(shown, "one.devbase.localhost") || strings.Contains(shown, "two.devbase.localhost") {
			t.Fatal("proxy rules became menu resources")
		}
		if err := panel.Click("HTTPS settings"); err != nil || opened != 1 || a.nav != "development" || a.dev.tab != devTabHTTPS {
			t.Fatal("HTTPS settings did not open", err, opened, a.nav, a.dev.tab)
		}
		if a.panelRunning() != 2 {
			t.Fatal("unknown resource counted as running")
		}
	})
	t.Run("quit", func(t *testing.T) {
		a := designFixture(t)
		a.quit.open = true
		a.quit.plan = &devplatform.ShutdownPlan{Instances: []devplatform.ShutdownResource{{ID: "db", Name: "redis"}}}
		a.dev.proxyDraftLoaded = true
		a.dev.proxyDraft = []proxyRuleDraft{{Domain: "unsaved.devbase.localhost", LocalPort: "3000"}}
		quitCount := 0
		a.quitApp = func() { quitCount++ }
		tt := ui.NewTester(a.view, 900, 520)
		tt.Frame()
		shown := strings.Join(tt.Texts(), " ")
		for _, want := range []string{"Stop scope: 1 managed databases", "Unsaved edits will be discarded", "Stop managed databases and quit"} {
			if !strings.Contains(shown, want) {
				t.Fatalf("missing %q: %s", want, shown)
			}
		}
		if strings.Contains(shown, "Stop local HTTPS proxy") {
			t.Fatalf("quit dialog should not ask about HTTPS proxy: %s", shown)
		}
		if err := tt.Click("Cancel quit"); err != nil || a.quit.open || quitCount != 0 {
			t.Fatal("cancel quit mutated state", err)
		}
		a.quit.open = true
		tt.Frame()
		if err := tt.Click("Keep services and quit"); err != nil || quitCount != 1 {
			t.Fatal("keep quit failed", err, quitCount)
		}
	})
	t.Run("layouts", func(t *testing.T) {
		for _, tc := range []struct {
			dark          bool
			width, height int
		}{{false, 900, 520}, {true, 900, 520}, {false, 1120, 720}, {true, 1120, 720}} {
			a := designFixture(t)
			a.dev.state.Instances["long-database-instance-name"] = devplatform.Instance{ID: "long-database-instance-name", Kind: "redis"}
			a.dev.state.ProxyRules = []devplatform.ProxyRule{{Domain: "one.devbase.localhost", LocalPort: 3000}, {Domain: "two.devbase.localhost", LocalPort: 4000}}
			a.dev.statuses = map[string]string{"instance:long-database-instance-name": "Stopped", "proxy": "Running"}
			panel := ui.NewTester(func(c *ui.Context) { a.menubarView(c, func() {}) }, 320, a.panelHeight())
			panel.SetDark(tc.dark)
			panel.Frame()
			if _, ok := panel.Find("HTTPS settings"); !ok {
				t.Fatal("HTTPS settings inaccessible")
			}
			if shotsWanted() {
				// Capture the menu content at the required layout viewport.
				full := ui.NewTester(func(c *ui.Context) { a.menubarView(c, func() {}) }, tc.width, tc.height)
				full.SetDark(tc.dark)
				full.Frame()
				saveShot(t, fmt.Sprintf("%s-%dx%d", map[bool]string{false: "light", true: "dark"}[tc.dark], tc.width, tc.height), full.Image())
			}
		}
	})
}

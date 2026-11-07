package main

import (
	"fmt"
	"image/png"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/egoist/mygo/ui"
	"macos-service-manager/internal/devplatform"
)

func TestUIIndependentResourceLifecycle(t *testing.T) {
	t.Run("menu", func(t *testing.T) {
		a := designFixture(t)
		a.dev.state.Instances["one"] = devplatform.Instance{ID: "one", Kind: "redis"}
		a.dev.state.Instances["two"] = devplatform.Instance{ID: "two", Kind: "redis"}
		a.dev.state.ProxyRules = []devplatform.ProxyRule{{Domain: "one.test", LocalPort: 3000}, {Domain: "two.test", LocalPort: 4000}}
		a.dev.statuses = map[string]string{"instance:one": "Running", "instance:two": "Unknown", "proxy": "Running"}
		opened := 0
		panel := ui.NewTester(func(c *ui.Context) { a.menubarView(c, func() { opened++ }) }, 320, a.panelHeight())
		panel.Frame()
		shown := strings.Join(panel.Texts(), " ")
		for _, want := range []string{"2 running", "Databases", "one · Running", "two · Unknown", "Proxy · Running", "HTTPS settings"} {
			if !strings.Contains(shown, want) {
				t.Fatalf("missing %q: %s", want, shown)
			}
		}
		if strings.Contains(shown, "one.test") || strings.Contains(shown, "two.test") {
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
		a.quit.plan = &devplatform.ShutdownPlan{Instances: []devplatform.ShutdownResource{{ID: "db", Name: "redis"}}, PrivilegedProxy: true}
		a.dev.proxyDraftLoaded = true
		a.dev.proxyDraft = []proxyRuleDraft{{Domain: "unsaved.test", LocalPort: "3000"}}
		quitCount := 0
		a.quitApp = func() { quitCount++ }
		tt := ui.NewTester(a.view, 900, 520)
		tt.Frame()
		shown := strings.Join(tt.Texts(), " ")
		for _, want := range []string{"Stop scope: 1 managed databases", "Unsaved edits will be discarded", "Stop local HTTPS proxy", "Stop managed databases and quit"} {
			if !strings.Contains(shown, want) {
				t.Fatalf("missing %q: %s", want, shown)
			}
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
			a.dev.state.ProxyRules = []devplatform.ProxyRule{{Domain: "one.test", LocalPort: 3000}, {Domain: "two.test", LocalPort: 4000}}
			a.dev.statuses = map[string]string{"instance:long-database-instance-name": "Stopped", "proxy": "Running"}
			panel := ui.NewTester(func(c *ui.Context) { a.menubarView(c, func() {}) }, 320, a.panelHeight())
			panel.SetDark(tc.dark)
			panel.Frame()
			if _, ok := panel.Find("HTTPS settings"); !ok {
				t.Fatal("HTTPS settings inaccessible")
			}
			if dir := os.Getenv("MSM_RESOURCE_SHOTS"); dir != "" {
				if err := os.MkdirAll(dir, 0755); err != nil {
					t.Fatal(err)
				}
				theme := "light"
				if tc.dark {
					theme = "dark"
				}
				name := fmt.Sprintf("%s-%dx%d.png", theme, tc.width, tc.height)
				file, err := os.Create(filepath.Join(dir, name))
				if err != nil {
					t.Fatal(err)
				}
				// Capture the menu content at the required layout viewport.
				full := ui.NewTester(func(c *ui.Context) { a.menubarView(c, func() {}) }, tc.width, tc.height)
				full.SetDark(tc.dark)
				full.Frame()
				if err := png.Encode(file, full.Image()); err != nil {
					file.Close()
					t.Fatal(err)
				}
				if err := file.Close(); err != nil {
					t.Fatal(err)
				}
			}
		}
	})
}

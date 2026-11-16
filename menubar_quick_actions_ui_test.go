package main

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"devbase/internal/devplatform"
	"github.com/egoist/mygo/ui"
)

func TestUIMenuBarQuickActions(t *testing.T) {
	setup := func(t *testing.T) (*app, string, *[]string) {
		a := designFixture(t)
		root := t.TempDir()
		var ran []string
		a.spawn = func(f func()) { f() }
		a.post = func(f func()) { f() }
		a.dev.managerFactory = func() *devplatform.Manager {
			m := devplatform.New(root)
			m.Run = func(_ context.Context, name string, args ...string) (string, error) {
				ran = append(ran, strings.Join(append([]string{name}, args...), " "))
				return "", nil
			}
			return m
		}
		return a, root, &ran
	}
	t.Run("proxy-url", func(t *testing.T) {
		a := designFixture(t)
		a.dev.state.ProxyRules = []devplatform.ProxyRule{{Domain: "app.devbase.localhost", LocalPort: 3000}}
		a.dev.statuses = map[string]string{"proxy": "Running"}
		a.dev.state.ProxyListenerPort = 443
		if got := a.panelProxyURL(); got != "https://app.devbase.localhost" {
			t.Fatal(got)
		}
		a.dev.state.ProxyListenerPort = 8443
		if got := a.panelProxyURL(); got != "https://app.devbase.localhost:8443" {
			t.Fatal(got)
		}
		a.dev.statuses["proxy"] = "Stopped"
		if got := a.panelProxyURL(); got != "" {
			t.Fatal(got)
		}
		a.dev.statuses["proxy"] = "Running"
		a.dev.state.ProxyRules = append(a.dev.state.ProxyRules, devplatform.ProxyRule{Domain: "b.devbase.localhost", LocalPort: 3001})
		if got := a.panelProxyURL(); got != "" {
			t.Fatal(got)
		}
	})
	t.Run("logs-console", func(t *testing.T) {
		a, root, ran := setup(t)
		path := filepath.Join(root, "logs", "db-one.err.log")
		if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte("boom\n"), 0o600); err != nil {
			t.Fatal(err)
		}
		opened := 0
		a.openInstanceLogs("one", func() { opened++ })
		if len(*ran) != 1 || (*ran)[0] != "/usr/bin/open -a Console "+path || opened != 0 {
			t.Fatal(*ran, opened)
		}
	})
	t.Run("logs-fallback", func(t *testing.T) {
		a, _, ran := setup(t)
		opened := 0
		a.openInstanceLogs("one", func() { opened++ })
		if len(*ran) != 0 || opened != 1 || a.nav != navDevelopment || a.dev.tab != devTabDatabases || !strings.Contains(a.dev.output, "No logs yet") {
			t.Fatal(*ran, opened, a.nav, a.dev.tab, a.dev.output)
		}
	})
	t.Run("panel-text", func(t *testing.T) {
		a := designFixture(t)
		a.dev.state.ProxyRules = []devplatform.ProxyRule{{Domain: "secret-app.devbase.localhost", LocalPort: 3000}}
		a.dev.statuses = map[string]string{"proxy": "Running"}
		panel := ui.NewTester(func(c *ui.Context) { a.menubarView(c, func() {}) }, 320, a.panelHeight())
		panel.Frame()
		if shown := strings.Join(panel.Texts(), " "); strings.Contains(shown, "secret-app.devbase.localhost") {
			t.Fatal(shown)
		}
	})
}

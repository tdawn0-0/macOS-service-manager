package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
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
		if !a.devLog.open || a.devLog.jobID != "db-one" {
			t.Fatalf("expected fallback to open live log drawer for db-one, got %+v", a.devLog)
		}
	})
	t.Run("stop-all-databases", func(t *testing.T) {
		a, root, _ := setup(t)
		a.dev.state = devplatform.State{
			SchemaVersion:     devplatform.Schema,
			Packages:          map[string]devplatform.Package{},
			ProxyRules:        []devplatform.ProxyRule{},
			ProxyListenerPort: 443,
			Instances: map[string]devplatform.Instance{
				"pg-one":   {ID: "pg-one", Kind: "postgresql", Package: "postgresql@17.11_1-arm64", Port: 5432},
				"pg-two":   {ID: "pg-two", Kind: "postgresql", Package: "postgresql@17.11_1-arm64", Port: 5433},
				"pg-three": {ID: "pg-three", Kind: "postgresql", Package: "postgresql@17.11_1-arm64", Port: 5434},
			},
		}
		raw, err := json.MarshalIndent(a.dev.state, "", "  ")
		if err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(root, "state.json"), raw, 0o600); err != nil {
			t.Fatal(err)
		}
		a.dev.statuses = map[string]string{
			"instance:pg-one":   "Running",
			"instance:pg-two":   "Healthy",
			"instance:pg-three": "Stopped",
		}
		stopped := map[string]bool{}
		var bootedOut []string
		a.dev.managerFactory = func() *devplatform.Manager {
			m := devplatform.New(root)
			m.Run = func(_ context.Context, name string, args ...string) (string, error) {
				if name == "launchctl" && len(args) >= 2 {
					target := args[1]
					switch args[0] {
					case "bootout":
						stopped[target] = true
						bootedOut = append(bootedOut, target)
						return "", nil
					case "print":
						if stopped[target] || strings.Contains(target, "pg-three") {
							return "", errors.New("Could not find service")
						}
						return "state = running\npid = 0\n", nil
					}
				}
				return "", errors.New("Could not find service")
			}
			return m
		}

		panel := ui.NewTester(func(c *ui.Context) { a.menubarView(c, func() {}) }, 340, a.panelHeight())
		panel.Frame()
		if !panel.HasText("Stop all") {
			t.Fatalf("expected Stop all button when databases are running: %v", panel.Texts())
		}
		if err := panel.Click("Stop all"); err != nil {
			t.Fatal(err)
		}
		panel.Frame()
		if len(bootedOut) != 2 {
			t.Fatalf("expected 2 bootout calls for running instances, got %v (out=%q)", bootedOut, a.dev.output)
		}
		if panel.HasText("Stop all") {
			t.Fatalf("expected Stop all hidden once 0 instances running: %v", panel.Texts())
		}
	})
	t.Run("failure-badge-and-no-domains", func(t *testing.T) {
		a := designFixture(t)
		a.dev.state.Instances = map[string]devplatform.Instance{
			"db1": {ID: "db1", Kind: "postgresql", Port: 5432},
			"db2": {ID: "db2", Kind: "redis", Port: 6379},
		}
		a.dev.state.ProxyRules = []devplatform.ProxyRule{{Domain: "hidden-domain.devbase.localhost", LocalPort: 3000}}
		a.dev.statuses = map[string]string{
			"instance:db1": "Failed",
			"instance:db2": "Running",
			"proxy":        "Failed",
			"tunnel":       "Running",
		}
		if got := a.panelFailed(); got != 2 {
			t.Fatalf("expected 2 failed resources, got %d", got)
		}
		panel := ui.NewTester(func(c *ui.Context) { a.menubarView(c, func() {}) }, 340, a.panelHeight())
		panel.Frame()
		if !panel.HasText("2 running · 2 failed") {
			t.Fatalf("expected header failure badge, got %v", panel.Texts())
		}
		if shown := strings.Join(panel.Texts(), " "); strings.Contains(shown, "hidden-domain.devbase.localhost") {
			t.Fatalf("rule domain leaked into menubar panel text: %s", shown)
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
	t.Run("hover-single-and-multi-rule-actions", func(t *testing.T) {
		a, _, ran := setup(t)
		var copied []string
		a.copy = func(v string) { copied = append(copied, v) }
		a.dev.state.ProxyListenerPort = 443
		a.dev.state.ProxyRules = []devplatform.ProxyRule{
			{Domain: "one.devbase.localhost", LocalPort: 3000, Tunnel: true, TunnelURL: "https://devbase-one.demo.workers.dev"},
		}
		a.dev.statuses = map[string]string{"proxy": "Running", "tunnel": "Running", "tunnel:token": "Configured"}
		panel := ui.NewTester(func(c *ui.Context) { a.menubarView(c, func() {}) }, 340, a.panelHeight())
		panel.Frame()

		// Hover Proxy row (1 rule): shows Copy HTTPS URL & Open HTTPS URL.
		proxyRect, ok := panel.Find("Proxy · Running")
		if !ok {
			t.Fatal("missing Proxy · Running row")
		}
		panel.Move(proxyRect.X+proxyRect.W/2, proxyRect.Y+proxyRect.H/2)
		panel.Frame()
		if err := panel.Click("Copy HTTPS URL"); err != nil {
			t.Fatal(err)
		}
		panel.Frame()
		if len(copied) == 0 || copied[len(copied)-1] != "https://one.devbase.localhost" {
			t.Fatalf("expected single HTTPS URL copied, got %v", copied)
		}

		// Hover Tunnel row (1 rule): shows Copy tunnel URL & Open tunnel URL.
		tunnelRect, ok := panel.Find("Tunnel · Running")
		if !ok {
			t.Fatal("missing Tunnel · Running row")
		}
		panel.Move(tunnelRect.X+tunnelRect.W/2, tunnelRect.Y+tunnelRect.H/2)
		panel.Frame()
		if err := panel.Click("Copy tunnel URL"); err != nil {
			t.Fatal(err)
		}
		panel.Frame()
		if copied[len(copied)-1] != "https://devbase-one.demo.workers.dev" {
			t.Fatalf("expected single tunnel URL copied, got %v", copied)
		}

		// Multi-rule mode: domains stay out of panel text until menu opens.
		a.dev.state.ProxyRules = append(a.dev.state.ProxyRules, devplatform.ProxyRule{
			Domain:    "two.devbase.localhost",
			LocalPort: 4000,
			Tunnel:    true,
			TunnelURL: "https://devbase-two.demo.workers.dev",
		})
		panel.Frame()
		if shown := strings.Join(panel.Texts(), " "); strings.Contains(shown, "two.devbase.localhost") {
			t.Fatalf("domain leaked into menubar panel text: %s", shown)
		}
		panel.Move(proxyRect.X+proxyRect.W/2, proxyRect.Y+proxyRect.H/2)
		panel.Frame()
		if err := panel.Click("Open HTTPS URL"); err != nil {
			t.Fatal(err)
		}
		panel.Frame()
		menuText := fmt.Sprint(panel.Menu())
		for _, want := range []string{"Open https://one.devbase.localhost", "Copy https://two.devbase.localhost"} {
			if !strings.Contains(menuText, want) {
				t.Fatalf("multi-rule proxy menu lacks %q: %s", want, menuText)
			}
		}
		if err := panel.ChooseMenuItem("Copy https://two.devbase.localhost"); err != nil {
			t.Fatal(err)
		}
		panel.Frame()
		if copied[len(copied)-1] != "https://two.devbase.localhost" {
			t.Fatalf("expected second rule copied from menu, got %v", copied)
		}

		panel.Move(tunnelRect.X+tunnelRect.W/2, tunnelRect.Y+tunnelRect.H/2)
		panel.Frame()
		if err := panel.Click("Open tunnel URL"); err != nil {
			t.Fatal(err)
		}
		panel.Frame()
		if err := panel.ChooseMenuItem("Open https://devbase-two.demo.workers.dev"); err != nil {
			t.Fatal(err)
		}
		panel.Frame()
		if len(*ran) == 0 || (*ran)[len(*ran)-1] != "/usr/bin/open https://devbase-two.demo.workers.dev" {
			t.Fatalf("expected tunnel URL opened from multi-rule menu, got %v", *ran)
		}
	})
}

package main

import (
	"fmt"
	"github.com/egoist/mygo/ui"
	"image/png"
	"devbase/internal/devplatform"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestUIHTTPSActivation(t *testing.T) {
	t.Run("navigation-and-stale", func(t *testing.T) {
		a := designFixture(t)
		a.dev.tab = devTabHTTPS
		a.dev.state.ProxyRules = []devplatform.ProxyRule{{Domain: "rule.test", LocalPort: 3000}}
		a.dev.state.ProxyRulesRevision = 2
		a.dev.proxyDraft = []proxyRuleDraft{{Domain: "rule.test", LocalPort: "3000"}}
		a.dev.proxyDraftLoaded = true
		a.dev.proxyDraftRevision = 2
		a.dev.proxyChecks = map[string]devplatform.ProxyHTTPResult{"rule.test": {Domain: "rule.test", Revision: 1, Fingerprint: "old", StatusCode: 404, CheckedAt: time.Now()}}
		tt := ui.NewTester(a.view, 1120, 720)
		tt.Frame()
		if strings.Contains(strings.Join(tt.Texts(), "\n"), "HTTP response: 404") {
			t.Fatal("stale response shown")
		}
		a.dev.proxyDraft[0].LocalPort = "3001"
		a.openDiagnosticTarget(devplatform.DiagnosticTarget{Kind: "proxy", ID: "rule.test"}, false)
		if a.dev.proxyDraft[0].LocalPort != "3001" || a.dev.tab != devTabHTTPS {
			t.Fatal("proxy draft lost on navigation")
		}
		a.dev.diagnosticsOpen = true
		a.openDiagnosticTarget(devplatform.DiagnosticTarget{Kind: "proxy", ID: "deleted.test"}, false)
		if !a.dev.diagnosticsOpen {
			t.Fatal("missing target navigated")
		}
		a.dev.dbID = "draft"
		a.dev.dbNewDraft = true
		a.dev.dbPort = "55111"
		a.dev.state.Instances["db1"] = devplatform.Instance{ID: "db1", Kind: "redis", Port: 6379}
		a.openDiagnosticTarget(devplatform.DiagnosticTarget{Kind: "database", ID: "db1"}, false)
		if a.dev.diagnosticPendingTarget == nil || a.dev.dbID != "draft" {
			t.Fatal("dirty database draft discarded")
		}
		a.dev.diagnosticPendingTarget = nil
		if a.dev.dbID != "draft" {
			t.Fatal("cancel navigation changed draft")
		}
		a.openDiagnosticTarget(devplatform.DiagnosticTarget{Kind: "database", ID: "db1"}, true)
		if a.dev.dbID != "db1" || a.dev.tab != devTabDatabases {
			t.Fatal("explicit discard did not navigate")
		}
	})
	t.Run("out-of-order-checks", func(t *testing.T) {
		d := developmentState{proxyCheckGeneration: 2, proxyChecking: map[string]bool{"new.test": true}, proxyChecks: map[string]devplatform.ProxyHTTPResult{}}
		old := devplatform.ProxyHTTPResult{Domain: "old.test", Revision: 1, Fingerprint: "old", StatusCode: 200, CheckedAt: time.Now()}
		d.acceptProxyHTTPResult("old.test", 1, 1, "old", old, nil, "old", 1, nil)
		if len(d.proxyChecks) != 0 || !d.proxyChecking["new.test"] {
			t.Fatal("old callback overwrote new check")
		}
		fresh := devplatform.ProxyHTTPResult{Domain: "new.test", Revision: 2, Fingerprint: "new", StatusCode: 404, CheckedAt: time.Now()}
		d.acceptProxyHTTPResult("new.test", 2, 2, "new", fresh, nil, "new", 2, nil)
		if d.proxyChecks["new.test"].StatusCode != 404 || d.proxyChecking["new.test"] {
			t.Fatal("new check was lost")
		}
	})
	t.Run("layouts", func(t *testing.T) {
		for _, tc := range []struct {
			dark bool
			w, h int
		}{{false, 900, 520}, {true, 900, 520}, {false, 1120, 720}, {true, 1120, 720}} {
			a := designFixture(t)
			a.dev.tab = devTabHTTPS
			a.dev.state.ProxyRules = []devplatform.ProxyRule{{Domain: "some-long-local-name.test", LocalPort: 3000}}
			a.dev.state.ProxyRulesRevision = 2
			a.dev.proxyDraft = []proxyRuleDraft{{Domain: "some-long-local-name.test", LocalPort: "3000"}}
			a.dev.proxyDraftRevision = 2
			a.dev.proxyDraftLoaded = true
			tt := ui.NewTester(a.view, tc.w, tc.h)
			tt.SetDark(tc.dark)
			tt.Frame()
			if _, ok := tt.Find("Check HTTPS response"); !ok {
				t.Fatal("check action absent")
			}
			if dir := os.Getenv("MSM_RESOURCE_SHOTS"); dir != "" {
				if err := os.MkdirAll(dir, 0755); err != nil {
					t.Fatal(err)
				}
				theme := "light"
				if tc.dark {
					theme = "dark"
				}
				f, err := os.Create(filepath.Join(dir, fmt.Sprintf("%s-%dx%d.png", theme, tc.w, tc.h)))
				if err != nil {
					t.Fatal(err)
				}
				if err := png.Encode(f, tt.Image()); err != nil {
					f.Close()
					t.Fatal(err)
				}
				if err := f.Close(); err != nil {
					t.Fatal(err)
				}
			}
		}
	})
}

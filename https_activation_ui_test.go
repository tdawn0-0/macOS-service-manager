package main

import (
	"devbase/internal/devplatform"
	"fmt"
	"github.com/egoist/mygo/ui"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

func TestUIHTTPSActivation(t *testing.T) {
	t.Run("navigation-and-stale", func(t *testing.T) {
		a := designFixture(t)
		a.dev.tab = devTabHTTPS
		a.dev.state.ProxyRules = []devplatform.ProxyRule{{Domain: "rule.devbase.localhost", LocalPort: 3000}}
		a.dev.state.ProxyRulesRevision = 2
		a.dev.proxyDraft = []proxyRuleDraft{{Domain: "rule.devbase.localhost", LocalPort: "3000"}}
		a.dev.proxyDraftLoaded = true
		a.dev.proxyDraftRevision = 2
		a.dev.proxyChecks = map[string]devplatform.ProxyHTTPResult{"rule.devbase.localhost": {Domain: "rule.devbase.localhost", Revision: 1, Fingerprint: "old", StatusCode: 404, CheckedAt: time.Now()}}
		tt := ui.NewTester(a.view, 1120, 720)
		tt.Frame()
		if strings.Contains(strings.Join(tt.Texts(), "\n"), "HTTP response: 404") {
			t.Fatal("stale response shown")
		}
		a.dev.proxyDraft[0].LocalPort = "3001"
		a.openDiagnosticTarget(devplatform.DiagnosticTarget{Kind: "proxy", ID: "rule.devbase.localhost"}, false)
		if a.dev.proxyDraft[0].LocalPort != "3001" || a.dev.tab != devTabHTTPS {
			t.Fatal("proxy draft lost on navigation")
		}
		a.dev.diagnosticsOpen = true
		a.openDiagnosticTarget(devplatform.DiagnosticTarget{Kind: "proxy", ID: "deleted.devbase.localhost"}, false)
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
		d := developmentState{proxyCheckGeneration: 2, proxyChecking: map[string]bool{"new.devbase.localhost": true}, proxyChecks: map[string]devplatform.ProxyHTTPResult{}}
		old := devplatform.ProxyHTTPResult{Domain: "old.devbase.localhost", Revision: 1, Fingerprint: "old", StatusCode: 200, CheckedAt: time.Now()}
		d.acceptProxyHTTPResult("old.devbase.localhost", 1, 1, "old", old, nil, "old", 1, nil)
		if len(d.proxyChecks) != 0 || !d.proxyChecking["new.devbase.localhost"] {
			t.Fatal("old callback overwrote new check")
		}
		fresh := devplatform.ProxyHTTPResult{Domain: "new.devbase.localhost", Revision: 2, Fingerprint: "new", StatusCode: 404, CheckedAt: time.Now()}
		d.acceptProxyHTTPResult("new.devbase.localhost", 2, 2, "new", fresh, nil, "new", 2, nil)
		if d.proxyChecks["new.devbase.localhost"].StatusCode != 404 || d.proxyChecking["new.devbase.localhost"] {
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
			a.dev.state.ProxyRules = []devplatform.ProxyRule{{Domain: "some-long-local-name.devbase.localhost", LocalPort: 3000}}
			a.dev.state.ProxyRulesRevision = 2
			a.dev.proxyDraft = []proxyRuleDraft{{Domain: "some-long-local-name.devbase.localhost", LocalPort: "3000"}}
			a.dev.proxyDraftRevision = 2
			a.dev.proxyDraftLoaded = true
			tt := ui.NewTester(a.view, tc.w, tc.h)
			tt.SetDark(tc.dark)
			tt.Frame()
			if _, ok := tt.Find("Check HTTPS response"); !ok {
				t.Fatal("check action absent")
			}
			saveShot(t, fmt.Sprintf("%s-%dx%d", map[bool]string{false: "light", true: "dark"}[tc.dark], tc.w, tc.h), tt.Image())
		}
	})
	t.Run("tunnel-check-and-out-of-order", func(t *testing.T) {
		d := developmentState{tunnelCheckGeneration: 2, tunnelChecking: map[string]bool{"new.devbase.localhost": true}, tunnelChecks: map[string]devplatform.ProxyHTTPResult{}}
		old := devplatform.ProxyHTTPResult{Domain: "old.devbase.localhost", Revision: 1, Fingerprint: "old", StatusCode: 200, CheckedAt: time.Now()}
		d.acceptTunnelHTTPResult("old.devbase.localhost", 1, 1, "old", old, nil, "old", 1, nil)
		if len(d.tunnelChecks) != 0 || !d.tunnelChecking["new.devbase.localhost"] {
			t.Fatal("old tunnel callback overwrote new check")
		}
		fresh := devplatform.ProxyHTTPResult{Domain: "new.devbase.localhost", Revision: 2, Fingerprint: "new", StatusCode: 204, CheckedAt: time.Now()}
		d.acceptTunnelHTTPResult("new.devbase.localhost", 2, 2, "new", fresh, nil, "new", 2, nil)
		if d.tunnelChecks["new.devbase.localhost"].StatusCode != 204 || d.tunnelChecking["new.devbase.localhost"] {
			t.Fatal("new tunnel check was lost")
		}

		srv := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.WriteHeader(http.StatusOK)
		}))
		defer srv.Close()
		h := newDevUIHarnessSize(t, 1120, 720, false)
		h.a.dev.tab = devTabHTTPS
		h.a.dev.state.ProxyRules = []devplatform.ProxyRule{{
			Domain:    "edge.devbase.localhost",
			LocalPort: 3000,
			Tunnel:    true,
			TunnelURL: srv.URL,
		}}
		h.a.dev.state.ProxyListenerPort = 443
		h.a.dev.state.ProxyRulesRevision = 1
		h.a.dev.loadProxyDraft(h.a.dev.state)
		h.a.dev.tunnelChecks = map[string]devplatform.ProxyHTTPResult{
			"edge.devbase.localhost": {Domain: "edge.devbase.localhost", Revision: 1, Fingerprint: "fp", StatusCode: 200, CheckedAt: time.Now()},
		}
		h.ui.Frame()
		if _, ok := h.ui.Find("Check tunnel response"); !ok {
			t.Fatalf("Check tunnel response button missing: %v", h.ui.Texts())
		}
	})
	t.Run("live-tunnel-status-tags", func(t *testing.T) {
		a := designFixture(t)
		a.dev.tab = devTabHTTPS
		a.dev.state.ProxyRules = []devplatform.ProxyRule{
			{Domain: "ok.devbase.localhost", LocalPort: 3000, Tunnel: true},
			{Domain: "dup.devbase.localhost", LocalPort: 3001, Tunnel: true},
			{Domain: "wait.devbase.localhost", LocalPort: 3002, Tunnel: true},
			{Domain: "local.devbase.localhost", LocalPort: 3003, Tunnel: false},
		}
		a.dev.state.ProxyRulesRevision = 1
		a.dev.loadProxyDraft(a.dev.state)
		a.dev.statuses = map[string]string{
			"tunnel":                     "Running",
			"tunnel:worker:devbase-ok":   "Connected",
			"tunnel:worker:devbase-dup":  "Conflict",
			"tunnel:worker:devbase-wait": "Reconnecting",
		}
		tt := ui.NewTester(a.view, 1120, 900)
		tt.Frame()
		texts := strings.Join(tt.Texts(), "\n")
		for _, want := range []string{"Tunnel connected", "Tunnel conflict", "Tunnel connecting"} {
			if !strings.Contains(texts, want) {
				t.Fatalf("missing %q in %s", want, texts)
			}
		}

		a.dev.statuses["tunnel"] = "Stopped"
		tt.Frame()
		texts = strings.Join(tt.Texts(), "\n")
		for _, unwanted := range []string{"Tunnel connected", "Tunnel conflict", "Tunnel connecting"} {
			if strings.Contains(texts, unwanted) {
				t.Fatalf("unexpected %q when tunnel is stopped in %s", unwanted, texts)
			}
		}
	})
}

package main

import (
	"context"
	"devbase/internal/devplatform"
	"encoding/json"
	"fmt"
	"github.com/egoist/mygo/ui"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
)

func TestUIHTTPSProxyRules(t *testing.T) {
	t.Run("whole-table", func(t *testing.T) {
		h := newDevUIHarness(t)
		h.click("HTTPS")
		h.click("Add rule")
		h.fill("Domain", " app.devbase.localhost ")
		h.fill("Local port", "3000")
		if _, ok := h.ui.Find("Open HTTPS URL"); ok {
			t.Fatal("opened unsaved rule")
		}
		h.click("Discard changes")
		if len(h.a.dev.proxyDraft) != 0 {
			t.Fatal("discard retained row")
		}
		port := freeUIPort(t)
		h.fill("HTTPS listener port", fmt.Sprint(port))
		h.click("Add rule")
		h.fill("Domain", "app.devbase.localhost")
		h.fill("Local port", "3000")
		h.click("Save HTTPS rules")
		if h.a.dev.proxyPublicationState != "Unknown" {
			t.Fatalf("expected Unknown publication state before initial publish, got %q", h.a.dev.proxyPublicationState)
		}
		state, err := devplatform.New(h.root).Read()
		if err != nil || state.ProxyListenerPort != port || len(state.ProxyRules) != 1 || state.ProxyRules[0].Domain != "app.devbase.localhost" {
			t.Fatal(state, err)
		}
		defer devplatform.New(h.root).StopProxy(context.Background())
		h.click("Apply proxy")
		if h.a.dev.proxyPublicationState != "Published" {
			t.Fatalf("expected Published publication state after Apply proxy, got %q (output: %q)", h.a.dev.proxyPublicationState, h.a.dev.output)
		}
		h.click("Open HTTPS URL")
		expectedURL := fmt.Sprintf("https://app.devbase.localhost:%d", port)
		if len(h.opened) != 1 || h.opened[0] != expectedURL {
			t.Fatal("wrong saved URL", h.opened, "want", expectedURL)
		}
		h.click("Add rule")
		h.a.dev.proxyDraft[1] = proxyRuleDraft{Domain: "APP.devbase.localhost", LocalPort: "4000"}
		h.ui.Frame()
		h.click("Save HTTPS rules")
		state, err = devplatform.New(h.root).Read()
		if err != nil || len(state.ProxyRules) != 1 || !strings.Contains(h.a.dev.output, "duplicate") {
			t.Fatal("partial save or hidden validation error", state, h.a.dev.output, err)
		}
		h.click("Databases")
		h.click("HTTPS")
		if len(h.a.dev.proxyDraft) != 2 {
			t.Fatal("draft lost on navigation")
		}
		h.click("Discard changes")
		if len(h.a.dev.proxyDraft) != 1 {
			t.Fatal("discard failed")
		}
		if _, ok := h.ui.Find("Open HTTPS URL"); !ok {
			t.Fatal("saved rule cannot open")
		}
		h.fill("HTTPS listener port", "9443")
		h.click("Save HTTPS rules")
		if h.a.dev.proxyPublicationState != "Pending" {
			t.Fatalf("expected Pending publication state after modifying rule, got %q", h.a.dev.proxyPublicationState)
		}
		// Toggle HTTPS proxy service switch on/off.
		h.fill("HTTPS listener port", fmt.Sprint(port))
		h.click("Save HTTPS rules")
		if err := devplatform.New(h.root).StopProxy(context.Background()); err != nil {
			t.Fatal(err)
		}
		h.click("Refresh")
		h.click("HTTPS proxy service")
		if h.a.dev.statuses["proxy"] != "Running" {
			t.Fatalf("HTTPS proxy service switch did not start proxy: %q (output: %q)", h.a.dev.statuses["proxy"], h.a.dev.output)
		}
		h.click("HTTPS proxy service")
		if h.a.dev.statuses["proxy"] != "Stopped" {
			t.Fatalf("HTTPS proxy service switch did not stop proxy: %q (output: %q)", h.a.dev.statuses["proxy"], h.a.dev.output)
		}
	})
	t.Run("tunnel-controls", func(t *testing.T) {
		var mu sync.Mutex
		deployed := map[string]bool{}
		cfSrv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.Header().Set("Content-Type", "application/json")
			switch {
			case r.Method == http.MethodGet && r.URL.Path == "/accounts":
				_ = json.NewEncoder(w).Encode(map[string]any{"success": true, "result": []map[string]string{{"id": "acc-ui-123", "name": "Test"}}})
			case r.Method == http.MethodGet && r.URL.Path == "/accounts/acc-ui-123/workers/subdomain":
				_ = json.NewEncoder(w).Encode(map[string]any{"success": true, "result": map[string]string{"subdomain": "ui-test"}})
			case r.Method == http.MethodGet && r.URL.Path == "/zones":
				_ = json.NewEncoder(w).Encode(map[string]any{"success": true, "result": []map[string]string{{"id": "zone-1", "name": "example.com"}}})
			case r.Method == http.MethodGet && r.URL.Path == "/zones/zone-1/dns_records":
				_ = json.NewEncoder(w).Encode(map[string]any{"success": true, "result": []map[string]string{{"id": "rec-1", "name": "api.example.com", "type": "CNAME"}}})
			case r.Method == http.MethodGet && strings.HasPrefix(r.URL.Path, "/accounts/acc-ui-123/workers/services/"):
				name := strings.TrimPrefix(r.URL.Path, "/accounts/acc-ui-123/workers/services/")
				mu.Lock()
				exists := deployed[name]
				mu.Unlock()
				if !exists {
					w.WriteHeader(http.StatusNotFound)
					_ = json.NewEncoder(w).Encode(map[string]any{"success": false, "errors": []map[string]any{{"code": 10007, "message": "not found"}}})
					return
				}
				_ = json.NewEncoder(w).Encode(map[string]any{"success": true, "result": map[string]any{"default_environment": map[string]any{"script": map[string]string{"migration_tag": "v1"}}}})
			case r.Method == http.MethodPut && strings.HasPrefix(r.URL.Path, "/accounts/acc-ui-123/workers/scripts/") && !strings.HasSuffix(r.URL.Path, "/domains/records"):
				name := strings.TrimPrefix(r.URL.Path, "/accounts/acc-ui-123/workers/scripts/")
				mu.Lock()
				deployed[name] = true
				mu.Unlock()
				_ = json.NewEncoder(w).Encode(map[string]any{"success": true, "result": map[string]any{}})
			case r.Method == http.MethodPost && strings.HasSuffix(r.URL.Path, "/subdomain"):
				_ = json.NewEncoder(w).Encode(map[string]any{"success": true, "result": map[string]any{"enabled": true}})
			case r.Method == http.MethodPut && strings.HasSuffix(r.URL.Path, "/domains/records"):
				_ = json.NewEncoder(w).Encode(map[string]any{"success": true, "result": []any{}})
			case r.Method == http.MethodDelete && strings.HasPrefix(r.URL.Path, "/accounts/acc-ui-123/workers/scripts/"):
				name := strings.TrimPrefix(r.URL.Path, "/accounts/acc-ui-123/workers/scripts/")
				mu.Lock()
				delete(deployed, name)
				mu.Unlock()
				_ = json.NewEncoder(w).Encode(map[string]any{"success": true, "result": map[string]any{}})
			default:
				w.WriteHeader(http.StatusNotFound)
			}
		}))
		defer cfSrv.Close()

		h := newDevUIHarness(t)
		origFactory := h.a.dev.managerFactory
		h.a.dev.managerFactory = func() *devplatform.Manager {
			m := origFactory()
			m.CloudflareAPIBase = cfSrv.URL
			return m
		}

		h.click("HTTPS")
		h.fill("Cloudflare API token", "cf-token-ui-0123456789")
		h.fill("Client ID", "mac-ui")
		h.click("Save Cloudflare settings")
		if h.a.dev.statuses["tunnel:token"] != "Configured" {
			t.Fatalf("token status not updated: %v output=%q", h.a.dev.statuses, h.a.dev.output)
		}
		state, err := devplatform.New(h.root).Read()
		if err != nil || state.TunnelClientID != "mac-ui" {
			t.Fatalf("Cloudflare settings not saved: %+v err=%v output=%q", state, err, h.a.dev.output)
		}

		h.click("Refresh zones")
		state, err = devplatform.New(h.root).Read()
		if err != nil || state.CloudflareAccountID != "acc-ui-123" || state.CloudflareSubdomain != "ui-test" || len(state.CloudflareZones) != 1 {
			t.Fatalf("Cloudflare metadata not refreshed: %+v err=%v output=%q", state, err, h.a.dev.output)
		}

		h.click("Add rule")
		h.fill("Domain", "api.app.devbase.localhost")
		h.fill("Local port", "3000")
		h.click("Tunnel rule 1")
		h.click("Save HTTPS rules")
		state, err = devplatform.New(h.root).Read()
		if err != nil || len(state.ProxyRules) != 1 || !state.ProxyRules[0].Tunnel || state.ProxyRules[0].TunnelURL != "https://devbase-api-app.ui-test.workers.dev" {
			t.Fatalf("tunnel rule not synced: %+v err=%v output=%q", state.ProxyRules, err, h.a.dev.output)
		}
		h.click("Copy tunnel URL")
		if len(h.copied) == 0 || h.copied[len(h.copied)-1] != "https://devbase-api-app.ui-test.workers.dev" {
			t.Fatalf("wrong copied tunnel URL: %v", h.copied)
		}

		// Disabling tunnel and saving removes orphaned Worker cleanly.
		h.click("Tunnel rule 1")
		h.click("Save HTTPS rules")
		mu.Lock()
		stillDeployed := deployed["devbase-api-app"]
		mu.Unlock()
		if stillDeployed {
			t.Fatalf("expected orphaned devbase-api-app worker to be deleted, output=%q", h.a.dev.output)
		}
		if _, ok := h.ui.Find("Cloudflare tunnel service"); !ok {
			t.Fatal("Cloudflare tunnel service switch missing")
		}
	})
	t.Run("revision-conflict", func(t *testing.T) {
		h := newDevUIHarness(t)
		h.click("HTTPS")
		h.click("Add rule")
		h.fill("Domain", "draft.devbase.localhost")
		h.fill("Local port", "3000")
		m := devplatform.New(h.root)
		if err := m.ReplaceProxyRules(m.Context, 0, 8443, []devplatform.ProxyRule{{Domain: "external.devbase.localhost", LocalPort: 4000}}); err != nil {
			t.Fatal(err)
		}
		h.click("Save HTTPS rules")
		state, err := m.Read()
		if err != nil || state.ProxyRulesRevision != 1 || state.ProxyRules[0].Domain != "external.devbase.localhost" || h.a.dev.proxyDraft[0].Domain != "draft.devbase.localhost" || !strings.Contains(h.a.dev.output, "changed") {
			t.Fatal("conflict lost", state, h.a.dev.output, err)
		}
	})
	t.Run("layouts", func(t *testing.T) {
		for _, tc := range []struct {
			dark          bool
			width, height int
		}{{false, 900, 520}, {true, 900, 520}, {false, 1120, 720}, {true, 1120, 720}} {
			a := designFixture(t)
			a.dev.tab = devTabHTTPS
			a.dev.proxyDraftLoaded = true
			a.dev.proxyListenerPort = "8443"
			a.dev.proxyDraft = []proxyRuleDraft{
				{Domain: "a-very-long-but-valid-local-development-name.devbase.localhost", LocalPort: "12345", Tunnel: true, TunnelMode: tunnelModeCustomDomain, TunnelDomain: "long-app.example.com"},
				{Domain: "another-long-local-development-name.devbase.localhost", LocalPort: "54321", TunnelMode: tunnelModeWorkersDev},
			}
			tester := ui.NewTester(a.view, tc.width, tc.height)
			tester.SetDark(tc.dark)
			tester.Frame()
			for _, label := range []string{"HTTPS proxy rules", "Domain", "Local port", "Save HTTPS rules", "Discard changes", "HTTPS proxy service", "Cloudflare tunnel service", "Cloudflare API token", "Save Cloudflare settings", "Deploy Workers", "Refresh zones"} {
				if _, ok := tester.Find(label); !ok {
					t.Fatalf("%dx%d %s missing", tc.width, tc.height, label)
				}
			}
			saveShot(t, fmt.Sprintf("%s-%dx%d", map[bool]string{false: "light", true: "dark"}[tc.dark], tc.width, tc.height), tester.Image())
		}
	})
}

func TestUIHTTPSRuleUpstreamHost(t *testing.T) {
	h := newDevUIHarness(t)
	h.click("HTTPS")
	h.click("Add rule")
	h.fill("Domain", "lan.devbase.localhost")
	h.fill("Upstream host", " 192.168.1.20 ")
	h.fill("Local port", "8080")
	h.click("Save HTTPS rules")
	state, err := devplatform.New(h.root).Read()
	if err != nil || len(state.ProxyRules) != 1 || state.ProxyRules[0].UpstreamHost != "192.168.1.20" || state.ProxyRules[0].LocalPort != 8080 {
		t.Fatal(state.ProxyRules, err, h.a.dev.output)
	}
	if h.a.dev.proxyDraftDirty() {
		t.Fatal("a saved upstream host left the table dirty")
	}
	h.fill("Upstream host", "not a host")
	h.click("Save HTTPS rules")
	state, _ = devplatform.New(h.root).Read()
	if state.ProxyRules[0].UpstreamHost != "192.168.1.20" || !strings.Contains(h.a.dev.output, "Error:") {
		t.Fatal("an invalid upstream was saved or went unreported", state.ProxyRules, h.a.dev.output)
	}
}

func TestUIHTTPSSaveAndApplyCopyProtectAndLogs(t *testing.T) {
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte("ok"))
	}))
	defer upstream.Close()
	upPort := upstream.Listener.Addr().(*net.TCPAddr).Port

	h := newDevUIHarnessSize(t, 1120, 780, false)
	defer devplatform.New(h.root).StopProxy(context.Background())

	port := freeUIPort(t)
	h.click("HTTPS")
	h.fill("HTTPS listener port", fmt.Sprint(port))
	h.click("Add rule")
	h.fill("Domain", "quick.devbase.localhost")
	h.fill("Local port", fmt.Sprint(upPort))
	h.click("Tunnel rule 1")
	h.click("Protect tunnel 1")
	h.click("Save & apply")

	if h.a.dev.proxyPublicationState != "Published" || h.a.dev.statuses["proxy"] != "Running" {
		t.Fatalf("Save & apply did not publish and start proxy: pub=%q status=%q out=%q", h.a.dev.proxyPublicationState, h.a.dev.statuses["proxy"], h.a.dev.output)
	}
	wantURL := fmt.Sprintf("https://quick.devbase.localhost:%d", port)
	h.click("Copy HTTPS URL")
	if len(h.copied) == 0 || h.copied[len(h.copied)-1] != wantURL {
		t.Fatalf("expected copied %q, got %v", wantURL, h.copied)
	}

	h.click("Copy tunnel token")
	if len(h.copied) < 2 || len(h.copied[len(h.copied)-1]) < 16 {
		t.Fatalf("expected copied tunnel access token, got %v", h.copied)
	}
	firstToken := h.copied[len(h.copied)-1]
	h.click("Rotate tunnel token")
	rotatedToken, err := devplatform.New(h.root).TunnelAccessToken("quick.devbase.localhost")
	if err != nil || rotatedToken == "" || rotatedToken == firstToken {
		t.Fatalf("expected rotated token, got %q (prev %q) err=%v", rotatedToken, firstToken, err)
	}

	// Logs actions: Read proxy logs & Read tunnel logs & Open in Console.
	_ = devplatform.New(h.root).StopProxy(context.Background())
	logsDir := filepath.Join(h.root, "logs")
	if err := os.MkdirAll(logsDir, 0o700); err != nil {
		t.Fatal(err)
	}
	_ = os.Remove(filepath.Join(logsDir, "proxy.err.log"))
	proxyLog := filepath.Join(logsDir, "proxy.log")
	tunnelLog := filepath.Join(logsDir, "tunnel.err.log")
	_ = os.WriteFile(proxyLog, []byte("proxy-log-line\n"), 0o600)
	_ = os.WriteFile(tunnelLog, []byte("tunnel-log-line\n"), 0o600)

	h.click("Read proxy logs")
	if !strings.Contains(h.a.dev.output, "proxy-log-line") || !h.a.dev.showResult {
		t.Fatalf("Read proxy logs failed: %q", h.a.dev.output)
	}
	h.click("Read tunnel logs")
	if !strings.Contains(h.a.dev.output, "tunnel-log-line") || !h.a.dev.showResult {
		t.Fatalf("Read tunnel logs failed: %q", h.a.dev.output)
	}
	var consoleCalls []string
	origFactory := h.a.dev.managerFactory
	h.a.dev.managerFactory = func() *devplatform.Manager {
		m := origFactory()
		prevRun := m.Run
		m.Run = func(ctx context.Context, name string, args ...string) (string, error) {
			if name == "/usr/bin/open" {
				consoleCalls = append(consoleCalls, strings.Join(append([]string{name}, args...), " "))
			}
			return prevRun(ctx, name, args...)
		}
		return m
	}

	h.click("Open proxy logs in Console")
	h.click("Open tunnel logs in Console")
	if len(consoleCalls) != 2 || consoleCalls[0] != "/usr/bin/open -a Console "+proxyLog || consoleCalls[1] != "/usr/bin/open -a Console "+tunnelLog {
		t.Fatalf("unexpected Console open targets: %v", consoleCalls)
	}
}

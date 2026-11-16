package main

import (
	"context"
	"encoding/json"
	"fmt"
	"github.com/egoist/mygo/ui"
	"image/png"
	"devbase/internal/devplatform"
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
		h.fill("Domain", " app.test ")
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
		h.fill("Domain", "app.test")
		h.fill("Local port", "3000")
		h.click("Save HTTPS rules")
		if h.a.dev.proxyPublicationState != "Unknown" {
			t.Fatalf("expected Unknown publication state before initial publish, got %q", h.a.dev.proxyPublicationState)
		}
		state, err := devplatform.New(h.root).Read()
		if err != nil || state.ProxyListenerPort != port || len(state.ProxyRules) != 1 || state.ProxyRules[0].Domain != "app.test" {
			t.Fatal(state, err)
		}
		defer devplatform.New(h.root).StopProxy(context.Background())
		h.click("Apply proxy")
		if h.a.dev.proxyPublicationState != "Published" {
			t.Fatalf("expected Published publication state after Apply proxy, got %q (output: %q)", h.a.dev.proxyPublicationState, h.a.dev.output)
		}
		h.click("Open HTTPS URL")
		expectedURL := fmt.Sprintf("https://app.test:%d", port)
		if len(h.opened) != 1 || h.opened[0] != expectedURL {
			t.Fatal("wrong saved URL", h.opened, "want", expectedURL)
		}
		h.click("Add rule")
		h.a.dev.proxyDraft[1] = proxyRuleDraft{Domain: "APP.test", LocalPort: "4000"}
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
		h.fill("Domain", "api.app.test")
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
		h.fill("Domain", "draft.test")
		h.fill("Local port", "3000")
		m := devplatform.New(h.root)
		if err := m.ReplaceProxyRules(m.Context, 0, 8443, []devplatform.ProxyRule{{Domain: "external.test", LocalPort: 4000}}); err != nil {
			t.Fatal(err)
		}
		h.click("Save HTTPS rules")
		state, err := m.Read()
		if err != nil || state.ProxyRulesRevision != 1 || state.ProxyRules[0].Domain != "external.test" || h.a.dev.proxyDraft[0].Domain != "draft.test" || !strings.Contains(h.a.dev.output, "changed") {
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
				{Domain: "a-very-long-but-valid-local-development-name.test", LocalPort: "12345", Tunnel: true, TunnelMode: tunnelModeCustomDomain, TunnelDomain: "long-app.example.com"},
				{Domain: "another-long-local-development-name.test", LocalPort: "54321", TunnelMode: tunnelModeWorkersDev},
			}
			tester := ui.NewTester(a.view, tc.width, tc.height)
			tester.SetDark(tc.dark)
			tester.Frame()
			for _, label := range []string{"HTTPS proxy rules", "Domain", "Local port", "Save HTTPS rules", "Discard changes", "HTTPS proxy service", "Cloudflare tunnel service", "Cloudflare API token", "Save Cloudflare settings", "Deploy Workers", "Refresh zones"} {
				if _, ok := tester.Find(label); !ok {
					t.Fatalf("%dx%d %s missing", tc.width, tc.height, label)
				}
			}
			if dir := os.Getenv("MSM_RESOURCE_SHOTS"); dir != "" {
				if err := os.MkdirAll(dir, 0755); err != nil {
					t.Fatal(err)
				}
				theme := "light"
				if tc.dark {
					theme = "dark"
				}
				f, err := os.Create(filepath.Join(dir, fmt.Sprintf("%s-%dx%d.png", theme, tc.width, tc.height)))
				if err != nil {
					t.Fatal(err)
				}
				if err := png.Encode(f, tester.Image()); err != nil {
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

func TestUIHTTPSRuleUpstreamHost(t *testing.T) {
	h := newDevUIHarness(t)
	h.click("HTTPS")
	h.click("Add rule")
	h.fill("Domain", "lan.test")
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

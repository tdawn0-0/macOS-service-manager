package main

import (
	"context"
	"fmt"
	"github.com/egoist/mygo/ui"
	"image/png"
	"devbase/internal/devplatform"
	"os"
	"path/filepath"
	"strings"
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
			a.dev.proxyDraft = []proxyRuleDraft{{Domain: "a-very-long-but-valid-local-development-name.test", LocalPort: "12345"}, {Domain: "another-long-local-development-name.test", LocalPort: "54321"}}
			tester := ui.NewTester(a.view, tc.width, tc.height)
			tester.SetDark(tc.dark)
			tester.Frame()
			for _, label := range []string{"HTTPS proxy rules", "Domain", "Local port", "Save HTTPS rules", "Apply proxy"} {
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

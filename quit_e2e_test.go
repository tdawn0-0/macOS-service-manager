package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"devbase/internal/devplatform"
	"github.com/egoist/mygo/ui"
)

func TestUIQuitKeepsServicesOrStopsOwnedScopeAndRetriesFailure(t *testing.T) {
	h := newDevUIHarnessSize(t, 900, 520, true)
	external := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(204) }))
	defer external.Close()
	port := external.Listener.Addr().(*net.TCPAddr).Port
	httpsPort := freeUIPort(t)
	m := devplatform.New(h.root)
	defer m.StopProxy(context.Background())
	h.click("HTTPS")
	h.fill("HTTPS listener port", fmt.Sprint(httpsPort))
	h.click("Add rule")
	h.fill("Domain", "quit.devbase.localhost")
	h.fill("Local port", fmt.Sprint(port))
	h.click("Save HTTPS rules")
	h.click("Apply proxy")
	if strings.Contains(h.a.dev.output, "Error:") {
		t.Fatal(h.a.dev.output)
	}
	assertListening := func(p int) {
		t.Helper()
		c, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", p), time.Second)
		if err != nil {
			t.Fatal("expected service listening", p, err)
		}
		c.Close()
	}

	quitCalls, showCalls := 0, 0
	h.a.quitApp = func() { quitCalls++ }
	h.a.showQuit = func() { showCalls++ }
	triggerQuit := func() {
		t.Helper()
		panel := ui.NewTester(func(c *ui.Context) { h.a.menubarView(c, func() {}) }, 320, h.a.panelHeight())
		panel.Frame()
		if err := panel.Click("Quit"); err != nil {
			t.Fatal(err)
		}
		panel.Frame()
		h.waitIdle()
		h.ui.Frame()
	}

	// With no databases and no unsaved edits, quit completes immediately and keeps background HTTPS proxy running.
	triggerQuit()
	if h.a.quit.open || quitCalls != 1 || !h.a.quit.allow {
		t.Fatalf("clean quit with only background proxy did not exit immediately: open=%v calls=%d allow=%v err=%s", h.a.quit.open, quitCalls, h.a.quit.allow, h.a.quit.err)
	}
	assertListening(port)
	assertListening(httpsPort)

	// Re-enter and add unsaved edits: quit prompts once about unsaved edits.
	h.a.quit.allow = false
	h.fill("Domain", "unsaved-quit.devbase.localhost")
	triggerQuit()
	if !h.a.quit.open || quitCalls != 1 {
		t.Fatal("unsaved edits did not open quit confirmation")
	}
	if !strings.Contains(strings.Join(h.ui.Texts(), " "), "Unsaved edits will be discarded") {
		t.Fatal("quit hid unsaved changes")
	}
	h.click("Cancel quit")
	if h.a.quit.open || quitCalls != 1 {
		t.Fatal("cancel quit exited the application")
	}

	// Add a managed database instance to verify database stop/keep/retry scope,
	// while ensuring the background HTTPS proxy is never stopped on quit.
	state, err := m.Read()
	if err != nil {
		t.Fatal(err)
	}
	state.Instances["quitdb"] = devplatform.Instance{ID: "quitdb", Kind: "redis", Port: freeUIPort(t)}
	rawState, err := json.MarshalIndent(state, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(h.root, "state.json"), rawState, 0600); err != nil {
		t.Fatal(err)
	}

	dbLoaded := true
	refuseDBStop := true
	originalFactory := h.a.dev.managerFactory
	h.a.dev.managerFactory = func() *devplatform.Manager {
		mgr := originalFactory()
		run := mgr.Run
		mgr.Run = func(ctx context.Context, name string, args ...string) (string, error) {
			if name == "launchctl" && len(args) >= 2 && strings.HasSuffix(args[len(args)-1], ".db-quitdb") {
				switch args[0] {
				case "print":
					if dbLoaded {
						return "state = running\npid = 0", nil
					}
					return "", errors.New("Could not find service")
				case "bootout":
					if refuseDBStop {
						return "", errors.New("simulated stop refusal")
					}
					dbLoaded = false
					return "", nil
				}
			}
			return run(ctx, name, args...)
		}
		return mgr
	}

	triggerQuit()
	if !h.a.quit.open || h.a.quit.plan == nil || len(h.a.quit.plan.Instances) != 1 {
		t.Fatalf("quit did not open database scope: open=%v plan=%+v err=%s", h.a.quit.open, h.a.quit.plan, h.a.quit.err)
	}
	if strings.Contains(strings.Join(h.ui.Texts(), " "), "Stop local HTTPS proxy") {
		t.Fatal("quit dialog unexpectedly offered HTTPS proxy checkbox")
	}

	h.click("Keep services and quit")
	if quitCalls != 2 || !h.a.quit.allow {
		t.Fatal("keep did not finish quit")
	}
	assertListening(port)
	assertListening(httpsPort)
	state, err = m.Read()
	if err != nil || len(state.ProxyRules) != 1 || state.ProxyRules[0].Domain != "quit.devbase.localhost" {
		t.Fatal("quit saved an unsaved form", err)
	}

	h.a.quit.allow = false
	triggerQuit()
	h.click("Stop managed databases and quit")
	if !h.a.quit.open || quitCalls != 2 || !strings.Contains(h.a.quit.err, "simulated stop refusal") {
		t.Fatal("failed stop quit anyway", h.a.quit.err, quitCalls)
	}
	assertListening(port)
	assertListening(httpsPort)

	refuseDBStop = false
	h.click("Review updated quit scope")
	h.click("Stop managed databases and quit")
	if quitCalls != 3 || h.a.quit.open || showCalls != 4 {
		t.Fatal("retry did not complete reviewed quit", quitCalls, showCalls, h.a.quit.err)
	}
	if dbLoaded {
		t.Fatal("database job remained loaded after stop and quit")
	}
	// Background HTTPS proxy and external upstream remain untouched.
	assertListening(httpsPort)
	res, err := http.Get(external.URL)
	if err != nil {
		t.Fatal("stopped an unmanaged external process", err)
	}
	res.Body.Close()
	if res.StatusCode != 204 {
		t.Fatal(res.StatusCode)
	}
	if _, err = os.Stat(filepath.Join(h.root, "proxy", "ca.pem")); err != nil {
		t.Fatal("quit deleted CA data", err)
	}
}

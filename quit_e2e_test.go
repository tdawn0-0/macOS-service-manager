package main

import (
	"context"
	"errors"
	"fmt"
	"github.com/egoist/mygo/ui"
	"macos-service-manager/internal/devplatform"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
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
	h.fill("Domain", "quit.test")
	h.fill("Local port", fmt.Sprint(port))
	h.click("Save HTTPS rules")
	h.click("Apply proxy")
	if strings.Contains(h.a.dev.output, "Error:") {
		t.Fatal(h.a.dev.output)
	}
	h.fill("Domain", "unsaved-quit.test")
	quitCalls, showCalls := 0, 0
	h.a.quitApp = func() { quitCalls++ }
	h.a.showQuit = func() { showCalls++ }
	request := func() {
		t.Helper()
		panel := ui.NewTester(func(c *ui.Context) { h.a.menubarView(c, func() {}) }, 320, h.a.panelHeight())
		panel.Frame()
		if err := panel.Click("Quit"); err != nil {
			t.Fatal(err)
		}
		panel.Frame()
		h.waitIdle()
		h.ui.Frame()
		if !h.a.quit.open || h.a.quit.plan == nil {
			t.Fatal("quit did not open a reviewed scope", h.a.quit.err)
		}
	}
	assertListening := func(port int) {
		t.Helper()
		c, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", port), time.Second)
		if err != nil {
			t.Fatal("expected service stopped", port, err)
		}
		c.Close()
	}
	request()
	if !strings.Contains(strings.Join(h.ui.Texts(), " "), "Unsaved edits will be discarded") {
		t.Fatal("quit hid unsaved changes")
	}
	h.click("Cancel quit")
	if h.a.quit.open || quitCalls != 0 {
		t.Fatal("cancel quit exited the application")
	}
	assertListening(port)
	request()
	h.click("Keep services and quit")
	if quitCalls != 1 || !h.a.quit.allow {
		t.Fatal("keep did not finish quit")
	}
	assertListening(port)
	assertListening(httpsPort)
	state, err := m.Read()
	if err != nil || len(state.ProxyRules) != 1 || state.ProxyRules[0].Domain != "quit.test" {
		t.Fatal("quit saved an unsaved form", err)
	}
	// Re-enter as if the app was reopened, while launchd keeps the service.
	h.a.quit.allow = false
	originalFactory := h.a.dev.managerFactory
	h.a.dev.managerFactory = func() *devplatform.Manager {
		m := originalFactory()
		run := m.Run
		m.Run = func(ctx context.Context, name string, args ...string) (string, error) {
			if len(args) > 1 && args[0] == "bootout" && strings.HasSuffix(args[1], ".proxy") {
				return "", errors.New("simulated stop refusal")
			}
			return run(ctx, name, args...)
		}
		return m
	}
	request()
	h.click("Stop managed databases and quit")
	if !h.a.quit.open || quitCalls != 1 || !strings.Contains(h.a.quit.err, "simulated stop refusal") {
		t.Fatal("failed stop quit anyway", h.a.quit.err, quitCalls)
	}
	assertListening(port)
	h.a.dev.managerFactory = originalFactory
	h.click("Review updated quit scope")
	h.click("Stop managed databases and quit")
	if quitCalls != 2 || h.a.quit.open || showCalls != 3 {
		t.Fatal("retry did not complete reviewed quit", quitCalls, showCalls, h.a.quit.err)
	}
	for _, p := range []int{httpsPort} {
		if c, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", p), time.Second); err == nil {
			c.Close()
			t.Fatal("stop and quit left a listener", p)
		}
	}
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

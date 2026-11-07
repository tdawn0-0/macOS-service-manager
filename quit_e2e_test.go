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
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestUIQuitKeepsServicesOrStopsOwnedScopeAndRetriesFailure(t *testing.T) {
	h := newDevUIHarnessSize(t, 900, 520, true)
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "index.html"), []byte("quit fixture"), 0600); err != nil {
		t.Fatal(err)
	}
	python, err := exec.LookPath("python3")
	if err != nil {
		t.Fatal(err)
	}
	port, httpsPort := freeUIPort(t), freeUIPort(t)
	external := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(204) }))
	defer external.Close()
	h.fill("Project folder", dir)
	h.click("Import project")
	id := h.a.dev.id
	m := devplatform.New(h.root)
	defer func() { _ = m.Down(context.Background(), id); _ = m.StopProxy(context.Background()) }()
	h.fill("Project name", "Quit fixture")
	h.click("Services")
	h.click("Add service")
	h.fill("Service name", "web")
	h.fill("Program and arguments", fmt.Sprintf("%s\n-m\nhttp.server\n%d\n--bind\n127.0.0.1", python, port))
	h.fill("Readiness check", fmt.Sprintf("http://127.0.0.1:%d/", port))
	h.click("Save project")
	h.click("HTTPS")
	h.fill("Site name", "quit-site")
	h.fill("Local domain", "quit.test")
	h.fill("Upstream address", fmt.Sprintf("http://127.0.0.1:%d", port))
	h.fill("HTTPS port", fmt.Sprint(httpsPort))
	h.click("Save HTTPS site")
	h.click("Projects")
	h.click("Start project")
	if strings.Contains(h.a.dev.output, "Error:") {
		t.Fatal(h.a.dev.output)
	}
	h.click("Overview")
	h.fill("Project name", "Unsaved quit draft")
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
	if err != nil || state.Projects[id].Name != "Quit fixture" {
		t.Fatal("quit saved an unsaved form", err)
	}
	// Re-enter as if the app was reopened, while launchd keeps the service.
	h.a.quit.allow = false
	originalFactory := h.a.dev.managerFactory
	h.a.dev.managerFactory = func() *devplatform.Manager {
		m := originalFactory()
		run := m.Run
		m.Run = func(ctx context.Context, name string, args ...string) (string, error) {
			if len(args) > 1 && args[0] == "bootout" && strings.Contains(args[1], ".p-"+id+"-web") {
				return "", errors.New("simulated stop refusal")
			}
			return run(ctx, name, args...)
		}
		return m
	}
	request()
	h.click("Stop services and quit")
	if !h.a.quit.open || quitCalls != 1 || !strings.Contains(h.a.quit.err, "simulated stop refusal") {
		t.Fatal("failed stop quit anyway", h.a.quit.err, quitCalls)
	}
	assertListening(port)
	h.a.dev.managerFactory = originalFactory
	h.click("Review updated quit scope")
	h.click("Stop services and quit")
	if quitCalls != 2 || h.a.quit.open || showCalls != 3 {
		t.Fatal("retry did not complete reviewed quit", quitCalls, showCalls, h.a.quit.err)
	}
	for _, p := range []int{port, httpsPort} {
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

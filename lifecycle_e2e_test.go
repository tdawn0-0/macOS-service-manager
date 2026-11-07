package main

import (
	"context"
	"fmt"
	"macos-service-manager/internal/devplatform"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestUIReadinessTimeoutRestartPolicyAndObservedHealth(t *testing.T) {
	h := newDevUIHarnessSize(t, 900, 520, true)
	dir := t.TempDir()
	python, err := exec.LookPath("python3")
	if err != nil {
		t.Fatal(err)
	}
	port := freeUIPort(t)
	script := `import http.server,pathlib,sys
class Handler(http.server.BaseHTTPRequestHandler):
 def do_GET(self):
  self.send_response(200 if pathlib.Path('ready').exists() else 503); self.end_headers()
http.server.HTTPServer(('127.0.0.1',int(sys.argv[1])),Handler).serve_forever()
`
	if err = os.WriteFile(filepath.Join(dir, "server.py"), []byte(script), 0600); err != nil {
		t.Fatal(err)
	}
	h.fill("Project folder", dir)
	h.click("Import project")
	id := h.a.dev.id
	m := devplatform.New(h.root)
	defer func() { _ = m.Down(context.Background(), id) }()
	h.click("Services")
	h.click("Add service")
	h.fill("Service name", "api")
	h.fill("Program and arguments", fmt.Sprintf("%s\nserver.py\n%d", python, port))
	h.fill("Readiness check", fmt.Sprintf("http://127.0.0.1:%d/", port))
	h.fill("Readiness timeout (seconds)", "invalid")
	h.click("Save project")
	if !strings.Contains(h.a.dev.output, "Error:") || len(h.a.dev.state.Projects[id].Services) != 0 {
		t.Fatal("invalid policy was saved", h.a.dev.output)
	}
	h.fill("Readiness timeout (seconds)", "1")
	h.click("Restart this service after it exits")
	h.fill("Restart throttle (seconds)", "3")
	h.click("Save project")
	saved := h.a.dev.state.Projects[id].Services
	if len(saved) != 1 || saved[0].ReadinessTimeoutSeconds != 1 || saved[0].RestartThrottleSeconds != 3 || !saved[0].Restart {
		t.Fatal("UI did not persist service policy", saved, h.a.dev.output)
	}
	h.click("Overview")
	if err = h.ui.Click("Start project"); err != nil {
		t.Fatal(err)
	}
	h.ui.Frame()
	if !h.a.projectTaskRunning(id) || h.a.dev.statuses["project:"+id] != "Starting" {
		t.Fatal("UI hid the starting phase")
	}
	h.waitIdle()
	if !strings.Contains(h.a.dev.output, "Error:") || !strings.Contains(h.a.dev.output, "did not become ready") {
		t.Fatal("UI claimed an unready project started", h.a.dev.output)
	}
	if err = os.WriteFile(filepath.Join(dir, "ready"), nil, 0600); err != nil {
		t.Fatal(err)
	}
	h.click("Start project")
	if got := h.a.dev.statuses["project:"+id]; got != "Healthy" {
		t.Fatal("ready service did not become healthy", got, h.a.dev.output)
	}
	if err = os.Remove(filepath.Join(dir, "ready")); err != nil {
		t.Fatal(err)
	}
	// Simulate the next timer-rendered frame without sleeping for the full
	// polling interval. The actual status observation still runs asynchronously.
	h.a.dev.statusLast = time.Now().Add(-6 * time.Second)
	h.ui.Frame()
	h.waitIdle()
	if got := h.a.dev.statuses["project:"+id]; got != "Not ready" {
		t.Fatal("UI confused running and ready", got)
	}
	h.click("Services")
	status, ok := h.a.dev.serviceStatuses[devplatform.ServiceStatusKey(id, "api")]
	if !ok || status.Status != "Not ready" || status.PID <= 0 {
		t.Fatal("UI lost the observed process details", status)
	}
	h.click("Overview")
	if err = h.ui.Click("Stop project"); err != nil {
		t.Fatal(err)
	}
	h.ui.Frame()
	if !h.a.projectTaskRunning(id) || h.a.dev.statuses["project:"+id] != "Stopping" {
		t.Fatal("UI hid the stopping phase")
	}
	h.waitIdle()
	if got := h.a.dev.statuses["project:"+id]; got != "Stopped" {
		t.Fatal("UI did not finish stopping", got, h.a.dev.output)
	}
}

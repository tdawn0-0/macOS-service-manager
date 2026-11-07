package main

import (
	"context"
	"crypto/tls"
	"crypto/x509"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"testing"
	"time"
)

func createUIWaitingProject(t *testing.T, h *devUIHarness, name string) (string, string, int) {
	t.Helper()
	dir := t.TempDir()
	port := freeUIPort(t)
	script := `import http.server,pathlib,sys
class Handler(http.server.BaseHTTPRequestHandler):
 def do_GET(self):
  self.send_response(200 if pathlib.Path('ready').exists() else 503);self.end_headers();self.wfile.write(pathlib.Path('payload').read_bytes())
http.server.HTTPServer(('127.0.0.1',int(sys.argv[1])),Handler).serve_forever()
`
	if e := os.WriteFile(filepath.Join(dir, "server.py"), []byte(script), 0600); e != nil {
		t.Fatal(e)
	}
	if e := os.WriteFile(filepath.Join(dir, "payload"), []byte(name), 0600); e != nil {
		t.Fatal(e)
	}
	h.click("Projects")
	h.click("Import a project")
	h.fill("Project folder", dir)
	h.click("Import project")
	id := h.a.dev.id
	h.fill("Project name", name)
	h.click("Services")
	h.click("Add service")
	h.fill("Service name", "web")
	h.fill("Program and arguments", fmt.Sprintf("/usr/bin/python3\nserver.py\n%d", port))
	h.fill("Readiness check", fmt.Sprintf("http://127.0.0.1:%d/", port))
	h.fill("Readiness timeout (seconds)", "20")
	h.click("Save project")
	h.click("Overview")
	return id, dir, port
}
func waitUIProjectHTTP(t *testing.T, port, status int) {
	t.Helper()
	client := &http.Client{Timeout: time.Second}
	defer client.CloseIdleConnections()
	deadline := time.Now().Add(6 * time.Second)
	for time.Now().Before(deadline) {
		response, e := client.Get(fmt.Sprintf("http://127.0.0.1:%d/", port))
		if e == nil {
			response.Body.Close()
			if response.StatusCode == status {
				return
			}
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatal("project did not reach HTTP readiness stage", port, status)
}
func TestUIIndependentProjectsStartConcurrentlyKeepDraftsAndLatestHTTPS(t *testing.T) {
	for _, quitCancel := range []bool{false, true} {
		t.Run(fmt.Sprintf("quit-cancel-%t", quitCancel), func(t *testing.T) {
			h := newDevUIHarnessSize(t, 900, 520, quitCancel)
			m := h.a.dev.managerFactory()
			a, _, portA := createUIWaitingProject(t, h, "Alpha")
			b, dirB, portB := createUIWaitingProject(t, h, "Beta")
			t.Cleanup(func() {
				h.a.cancelBackgroundTasks()
				h.foregroundOnly = false
				h.waitIdle()
				_ = m.Down(context.Background(), a)
				_ = m.Down(context.Background(), b)
				_ = m.StopProxy(context.Background())
			})
			h.foregroundOnly = true
			h.click("Project Alpha")
			h.click("Start project")
			waitUIProjectHTTP(t, portA, 503)
			h.click("Project Beta")
			h.click("Start project")
			waitUIProjectHTTP(t, portB, 503)
			if h.a.dev.busy || !h.a.projectTaskRunning(a) || !h.a.projectTaskRunning(b) {
				t.Fatal("independent project starts did not overlap")
			}
			cancelA, cancelB := h.a.dev.projectTasks[a].cancel, h.a.dev.projectTasks[b].cancel
			h.click("Import a project")
			h.fill("Project folder", t.TempDir())
			h.click("Import project")
			independent := h.a.dev.id
			h.fill("Project name", "Saved during two starts")
			timer := time.AfterFunc(4*time.Second, func() { cancelA(); cancelB() })
			h.click("Save project")
			timer.Stop()
			state, e := m.Read()
			if e != nil || state.Projects[independent].Name != "Saved during two starts" || !h.a.projectTaskRunning(a) || !h.a.projectTaskRunning(b) {
				t.Fatal("project readiness blocked independent configuration", e)
			}
			h.a.dev.statusLast = time.Now().Add(-6 * time.Second)
			h.ui.Frame()
			h.waitIdle()
			if h.a.dev.statuses["project:"+a] != "Starting" || h.a.dev.statuses["project:"+b] != "Starting" {
				t.Fatal("poll or refresh replaced project phases", h.a.dev.statuses)
			}
			h.fill("Project name", "Unsaved concurrent draft")
			h.click("HTTPS")
			tlsPort := freeUIPort(t)
			h.fill("Site name", "concurrent")
			h.fill("Local domain", "concurrent-projects.test")
			h.fill("Upstream address", fmt.Sprintf("http://127.0.0.1:%d", portB))
			h.fill("HTTPS port", fmt.Sprint(tlsPort))
			h.click("Save HTTPS site")
			if quitCancel {
				calls := 0
				h.a.quitApp = func() { calls++ }
				h.a.requestQuit()
				h.ui.Frame()
				if !h.a.quitOperationBusy() {
					t.Fatal("quit ignored background projects")
				}
				if e := h.ui.Click("Keep services and quit"); e != nil {
					t.Fatal(e)
				}
				h.ui.Frame()
				if calls != 0 {
					t.Fatal("quit abandoned project startup")
				}
				h.click("Cancel background tasks")
				waitUIPackageCondition(t, h, func() bool { return !h.a.projectTasksRunning() && !h.a.dev.busy && !h.a.dev.projectNeedsRefresh })
				h.click("Cancel quit")
			} else {
				h.click("Project tasks")
				h.click("Cancel project operation " + a)
				waitUIPackageCondition(t, h, func() bool { return !h.a.projectTaskRunning(a) && !h.a.dev.busy && !h.a.dev.projectNeedsRefresh })
				if !h.a.projectTaskRunning(b) {
					t.Fatal("individual cancellation affected the other project")
				}
			}
			if h.a.dev.draft.Name != "Unsaved concurrent draft" {
				t.Fatal("project completion replaced another draft")
			}
			if e = os.WriteFile(filepath.Join(dirB, "ready"), nil, 0600); e != nil {
				t.Fatal(e)
			}
			h.foregroundOnly = false
			if quitCancel {
				h.click("Projects")
				h.click("Project Beta")
				if !h.a.ask.open || h.a.ask.button != "Discard changes" || h.a.dev.draft.Name != "Unsaved concurrent draft" {
					t.Fatal("dirty navigation did not preserve draft pending confirmation")
				}
				h.click("Discard changes")
				h.click("Start project")
			} else {
				h.waitIdle()
			}
			if h.a.dev.projectTasks[b].err != nil {
				t.Fatal("ready project failed", h.a.dev.projectTasks[b].Result)
			}
			ca, e := os.ReadFile(filepath.Join(h.root, "proxy", "ca.pem"))
			if e != nil {
				t.Fatal(e)
			}
			pool := x509.NewCertPool()
			pool.AppendCertsFromPEM(ca)
			transport := &http.Transport{TLSClientConfig: &tls.Config{RootCAs: pool, ServerName: "concurrent-projects.test"}}
			defer transport.CloseIdleConnections()
			client := &http.Client{Transport: transport, Timeout: 3 * time.Second}
			req, _ := http.NewRequest("GET", fmt.Sprintf("https://127.0.0.1:%d/", tlsPort), nil)
			req.Host = "concurrent-projects.test"
			response, e := client.Do(req)
			if e != nil {
				t.Fatal(e)
			}
			body, _ := io.ReadAll(response.Body)
			response.Body.Close()
			if response.StatusCode != 200 || string(body) != "Beta" {
				t.Fatal("project startup used stale HTTPS settings", response.StatusCode, string(body))
			}
			h.click("Projects")
			h.click("Project Beta")
			h.click("Stop project")
		})
	}
}

func TestUIProjectCompletionRefreshesWithoutAnotherMainViewFrame(t *testing.T) {
	h := newDevUIHarnessSize(t, 900, 520, true)
	m := h.a.dev.managerFactory()
	id, dir, port := createUIWaitingProject(t, h, "Hidden completion")
	t.Cleanup(func() {
		h.a.cancelBackgroundTasks()
		h.foregroundOnly = false
		h.waitIdle()
		_ = m.Down(context.Background(), id)
	})
	if e := os.WriteFile(filepath.Join(dir, "ready"), nil, 0600); e != nil {
		t.Fatal(e)
	}
	if e := h.ui.Click("Start project"); e != nil {
		t.Fatal(e)
	}
	h.ui.Frame()
	drainWithoutFrames := func() {
		t.Helper()
		timer := time.NewTimer(8 * time.Second)
		defer timer.Stop()
		for h.a.projectTaskRunning(id) || h.a.dev.busy || h.a.dev.projectNeedsRefresh {
			select {
			case f := <-h.posted:
				f()
			case <-timer.C:
				t.Fatal("completion depended on rendering the main view", h.a.dev.projectTasks[id].Result)
			}
		}
	}
	drainWithoutFrames()
	if h.a.dev.statuses["project:"+id] != "Healthy" {
		t.Fatal("hidden-view completion did not refresh state", h.a.dev.statuses)
	}
	waitUIProjectHTTP(t, port, 200)
	h.a.runProjectAction(h.a.dev.state.Projects[id], "down")
	drainWithoutFrames()
	if h.a.dev.statuses["project:"+id] != "Stopped" {
		t.Fatal("hidden-view stop did not refresh state", h.a.dev.statuses)
	}
}

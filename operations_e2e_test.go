package main

import (
	"context"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"macos-service-manager/internal/devplatform"
)

func TestUITaskHistoryCrashRetryReconcilesLiveServiceAndKeepsFiles(t *testing.T) {
	for _, dark := range []bool{false, true} {
		t.Run(fmt.Sprintf("minimum-dark-%t", dark), func(t *testing.T) {
			h := newDevUIHarnessSize(t, 900, 520, dark)
			m := h.a.dev.managerFactory()
			ctx := context.Background()
			t.Cleanup(func() {
				if err := m.StopLogMaintenance(context.Background()); err != nil {
					t.Error("remove CLI test maintenance task:", err)
				}
			})
			var ready atomic.Bool
			upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				if !ready.Load() {
					w.WriteHeader(503)
					return
				}
				w.WriteHeader(204)
			}))
			defer upstream.Close()
			p := devplatform.Project{SchemaVersion: 1, ID: "interrupted", Name: "Interrupted launch", Directory: t.TempDir(), Services: []devplatform.Service{{ID: "worker", Command: []string{"/bin/sleep", "90"}, Health: upstream.URL, ReadinessTimeoutSeconds: 60}}}
			if e := m.SaveProject(ctx, p); e != nil {
				t.Fatal(e)
			}
			defer m.Down(ctx, p.ID)
			c := exec.Command(m.Program, "--dev", "up", p.ID)
			c.Env = append(os.Environ(), "MSM_DEV_HOME="+h.root)
			if e := c.Start(); e != nil {
				t.Fatal(e)
			}
			defer func() { _ = c.Process.Kill(); _ = c.Wait() }()
			var pid int
			state, _ := m.Read()
			deadline := time.Now().Add(10 * time.Second)
			for time.Now().Before(deadline) {
				snapshot := m.Snapshot(ctx, state)
				pid = snapshot.Services[devplatform.ServiceStatusKey(p.ID, "worker")].PID
				if pid > 0 {
					break
				}
				time.Sleep(20 * time.Millisecond)
			}
			if pid == 0 {
				t.Fatal("managed service did not start")
			}
			if e := c.Process.Kill(); e != nil {
				t.Fatal(e)
			}
			_ = c.Wait()
			records, e := m.Operations(ctx)
			// macOS may report child exit before its file leases finish closing.
			// Keep treating the operation as live until the real lease is free.
			deadline = time.Now().Add(5 * time.Second)
			for e == nil && len(records) == 1 && records[0].Busy && time.Now().Before(deadline) {
				time.Sleep(20 * time.Millisecond)
				records, e = m.Operations(ctx)
			}
			if e != nil || len(records) != 1 || records[0].Status != "Interrupted" {
				t.Fatal(records, e)
			}
			original := records[0]
			h.click("Refresh")
			h.click("Tasks")
			h.click("Task " + original.ID)
			if !strings.Contains(strings.Join(h.ui.Texts(), "\n"), "Interrupted") {
				t.Fatal("interrupted result missing", h.ui.Texts())
			}
			ready.Store(true)
			h.click("Retry operation")
			records, e = m.Operations(ctx)
			if e != nil || len(records) != 2 {
				t.Fatal(records, e)
			}
			var follow devplatform.OperationRecord
			for _, r := range records {
				if r.RetryOf == original.ID {
					follow = r
				}
			}
			if follow.Status != "Succeeded" {
				t.Fatal(records, h.a.dev.output)
			}
			if current := m.Snapshot(ctx, state).Services[devplatform.ServiceStatusKey(p.ID, "worker")].PID; current != pid {
				t.Fatal("retry replaced an already running managed service", pid, current)
			}
			h.click("Task " + follow.ID)
			h.click("Dismiss record (keep all files)")
			if current := m.Snapshot(ctx, state).Services[devplatform.ServiceStatusKey(p.ID, "worker")].PID; current != pid {
				t.Fatal("dismiss stopped a live service")
			}
			if _, e := os.Stat(p.Directory); e != nil {
				t.Fatal(e)
			}
			resp, e := http.Get(upstream.URL)
			if e != nil {
				t.Fatal(e)
			}
			resp.Body.Close()
			if resp.StatusCode != 204 {
				t.Fatal(resp.StatusCode)
			}
			h.click("Back to workspace")
			if h.a.dev.operationsOpen {
				t.Fatal("did not return to workspace")
			}
		})
	}
}
func TestUITaskHistoryInitializationRequiresNewTargetReview(t *testing.T) {
	h := newDevUIHarnessSize(t, 900, 520, true)
	m := h.a.dev.managerFactory()
	if e := m.CreateInstance(context.Background(), devplatform.Instance{ID: "old-target", Kind: "redis", Package: "redis@7.4.7-arm64", Port: 0}); e == nil {
		t.Fatal("expected fixture initialization validation failure")
	}
	h.click("Tasks")
	records, e := m.Operations(context.Background())
	if e != nil || len(records) != 1 {
		t.Fatal(records, e)
	}
	h.click("Task " + records[0].ID)
	h.click("Retry operation")
	again, _ := m.Operations(context.Background())
	if len(again) != 1 {
		t.Fatal("initialization replayed automatically")
	}
	h.click("Review resource settings")
	if h.a.dev.operationsOpen || h.a.dev.tab != 2 || h.a.dev.dbID != "" || h.a.dev.dbKind != "redis" {
		t.Fatal("new instance review missing", h.a.dev.dbID, h.a.dev.dbKind)
	}
}

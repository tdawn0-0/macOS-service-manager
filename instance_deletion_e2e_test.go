package main

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"macos-service-manager/internal/devplatform"
)

func newUIDeletionInstance(t *testing.T, dark bool) (*devUIHarness, *devplatform.Manager, devplatform.Package, string, func(...string) string) {
	t.Helper()
	if os.Getenv("MSM_UI_E2E_PACKAGE_CACHE") == "" {
		t.Skip("set MSM_UI_E2E_PACKAGE_CACHE to verified Redis archive")
	}
	h := newDevUIHarnessSize(t, 900, 520, dark)
	m := h.a.dev.managerFactory()
	pkg, e := devplatform.Resolve(context.Background(), "redis", "7.4.7")
	if e != nil {
		t.Fatal(e)
	}
	seedRuntimeCache(t, h.root, pkg)
	h.click("Software")
	h.click("Software redis")
	h.click("Review package")
	h.click("Download and install")
	h.click("Databases")
	h.click("New database instance")
	h.fill("Instance name", "deletion-test")
	h.choose("Database engine", "redis")
	h.fill("Local port", fmt.Sprint(freeUIPort(t)))
	h.click("Create instance")
	id := "deletion-test"
	if _, ok := h.a.dev.state.Instances[id]; !ok {
		t.Fatal(h.a.dev.output)
	}
	t.Cleanup(func() { _ = m.InstanceAction(context.Background(), id, "remove") })
	h.click("Start instance")
	instance := h.a.dev.state.Instances[id]
	pw, e := os.ReadFile(filepath.Join(h.root, "instances", id, "password"))
	if e != nil {
		t.Fatal(e)
	}
	command := func(args ...string) string {
		t.Helper()
		base := []string{"-h", "127.0.0.1", "-p", fmt.Sprint(instance.Port)}
		c := exec.Command(filepath.Join(h.root, "packages", pkg.ID(), pkg.Binaries["redis-cli"]), append(base, args...)...)
		c.Env = append(os.Environ(), "REDISCLI_AUTH="+string(pw))
		b, e := c.CombinedOutput()
		if e != nil {
			t.Fatal("Redis fixture:", e, string(b))
		}
		return strings.TrimSpace(string(b))
	}
	if v := command("SET", "value", "42"); v != "OK" {
		t.Fatal(v)
	}
	return h, m, pkg, id, command
}
func TestUIInstanceDeletionPreviewConfirmationAndPreservedBackup(t *testing.T) {
	for _, dark := range []bool{false, true} {
		t.Run(fmt.Sprintf("minimum-dark-%t", dark), func(t *testing.T) {
			h, m, pkg, id, redis := newUIDeletionInstance(t, dark)
			h.fill("Database number (0–15)", "0")
			h.click("Back up database")
			backup := h.a.dev.output
			if _, e := os.Stat(backup); e != nil {
				t.Fatal(backup, e)
			}
			h.click("Review permanent data deletion")
			if h.a.dev.instanceDeletionPreview == nil || h.a.dev.instanceDeletionPreview.CanDelete {
				t.Fatal("running instance was deletable")
			}
			h.fill("Confirm instance ID", id)
			h.click("Permanently delete instance data")
			if v := redis("GET", "value"); v != "42" {
				t.Fatal(v)
			}
			h.click("Cancel deletion preview")
			h.click("Stop instance")
			h.click("Review permanent data deletion")
			dir := filepath.Join(h.root, "instances", id)
			if p := h.a.dev.instanceDeletionPreview; p == nil || !p.CanDelete || p.Path != dir {
				t.Fatal(p)
			}
			h.fill("Confirm instance ID", "wrong")
			h.click("Permanently delete instance data")
			if _, e := os.Stat(dir); e != nil {
				t.Fatal("wrong confirmation removed data", e)
			}
			h.click("Cancel deletion preview")
			h.click("Review permanent data deletion")
			h.fill("Confirm instance ID", id)
			outside := filepath.Join(t.TempDir(), "sentinel")
			os.WriteFile(outside, []byte("outside"), 0600)
			os.Symlink(filepath.Dir(outside), filepath.Join(dir, "late-link"))
			h.click("Permanently delete instance data")
			if !strings.Contains(h.a.dev.output, "refresh") {
				t.Fatal("old preview was not rejected", h.a.dev.output)
			}
			h.click("Refresh deletion preview")
			h.fill("Confirm instance ID", id)
			h.click("Permanently delete instance data")
			if _, ok := h.a.dev.state.Instances[id]; ok {
				t.Fatal("instance record remains", h.a.dev.output)
			}
			if _, e := os.Stat(dir); !errors.Is(e, os.ErrNotExist) {
				t.Fatal("instance files remain", e)
			}
			if _, ok := h.a.dev.state.Packages[pkg.ID()]; !ok {
				t.Fatal("runtime was deleted")
			}
			if _, e := os.Stat(backup); e != nil {
				t.Fatal("backup was deleted", e)
			}
			if b, e := os.ReadFile(outside); e != nil || string(b) != "outside" {
				t.Fatal("followed outside link", e)
			}
			if pending, e := m.PendingInstanceDeletions(context.Background()); e != nil || len(pending) > 0 {
				t.Fatal(pending, e)
			}
			// Restore the retained real backup into a newly created real instance.
			h.click("New database instance")
			h.fill("Instance name", "restored-after-delete")
			h.choose("Database engine", "redis")
			h.fill("Local port", fmt.Sprint(freeUIPort(t)))
			h.click("Create instance")
			t.Cleanup(func() { _ = m.InstanceAction(context.Background(), "restored-after-delete", "remove") })
			h.click("Start instance")
			h.fill("Database number (0–15)", "0")
			h.fill("Backup file", backup)
			h.click("Restore into new database")
			if strings.Contains(h.a.dev.output, "Error:") {
				t.Fatal(h.a.dev.output)
			}
			i := h.a.dev.state.Instances["restored-after-delete"]
			pw, _ := os.ReadFile(filepath.Join(h.root, "instances", i.ID, "password"))
			c := exec.Command(filepath.Join(h.root, "packages", pkg.ID(), pkg.Binaries["redis-cli"]), "-p", fmt.Sprint(i.Port), "GET", "value")
			c.Env = append(os.Environ(), "REDISCLI_AUTH="+string(pw))
			if b, e := c.CombinedOutput(); e != nil || strings.TrimSpace(string(b)) != "42" {
				t.Fatal("retained backup did not restore real data", string(b), e)
			}
			h.click("Stop instance")
		})
	}
}
func TestUIInstanceDeletionCrashHelper(t *testing.T) {
	root := os.Getenv("MSM_UI_DELETION_CRASH_ROOT")
	if root == "" {
		return
	}
	m := devplatform.New(root)
	p, e := m.PreviewInstanceDeletion(context.Background(), "deletion-test")
	if e != nil {
		os.Exit(25)
	}
	m.Event = func(s string) {
		if s == os.Getenv("MSM_UI_DELETION_CRASH_POINT") {
			os.Exit(23)
		}
	}
	_ = m.DeleteInstanceData(context.Background(), p.InstanceID, p.Fingerprint, p.InstanceID)
	os.Exit(26)
}
func TestUIInstanceDeletionActualCrashRestoresOrFinishes(t *testing.T) {
	for n, point := range []string{"Database data staged for deletion", "Database deletion committed"} {
		t.Run(point, func(t *testing.T) {
			h, m, _, id, redis := newUIDeletionInstance(t, n == 1)
			h.click("Stop instance")
			outside := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(204) }))
			defer outside.Close()
			c := exec.Command(os.Args[0], "-test.run=^TestUIInstanceDeletionCrashHelper$")
			c.Env = append(os.Environ(), "MSM_UI_DELETION_CRASH_ROOT="+h.root, "MSM_UI_DELETION_CRASH_POINT="+point)
			var exit *exec.ExitError
			if e := c.Run(); !errors.As(e, &exit) || exit.ExitCode() != 23 {
				t.Fatal(e)
			}
			var rows []devplatform.InstanceDeletionInfo
			deadline := time.Now().Add(5 * time.Second)
			for {
				var e error
				rows, e = m.PendingInstanceDeletions(context.Background())
				if e != nil || len(rows) != 1 {
					t.Fatal(rows, e)
				}
				if !rows[0].Busy || time.Now().After(deadline) {
					break
				}
				time.Sleep(20 * time.Millisecond)
			}
			h.click("Refresh")
			h.click("Databases")
			h.click("Review pending data deletions")
			r := rows[0]
			prefix := r.ID[:8]
			if n == 0 {
				if !r.CanRestore || r.CanErase {
					t.Fatal(r)
				}
				h.click("Restore original data " + prefix)
				h.click("Back to database settings")
				h.click("Instance " + id)
				h.click("Start instance")
				if value := redis("GET", "value"); value != "42" {
					t.Fatal("restored data folder lost value", value)
				}
				h.click("Stop instance")
			} else {
				if !r.CanErase || r.CanRestore {
					t.Fatal(r)
				}
				h.fill("Confirm deletion of "+id, "wrong")
				h.click("Finish permanent deletion " + prefix)
				if _, e := os.Stat(r.StagedPath); e != nil {
					t.Fatal("wrong confirmation deleted staged data", e)
				}
				h.fill("Confirm deletion of "+id, id)
				h.click("Finish permanent deletion " + prefix)
				if _, e := os.Stat(r.StagedPath); !errors.Is(e, os.ErrNotExist) {
					t.Fatal(e)
				}
			}
			pending, e := m.PendingInstanceDeletions(context.Background())
			if e != nil || len(pending) > 0 {
				t.Fatal(pending, e)
			}
			resp, e := http.Get(outside.URL)
			if e != nil {
				t.Fatal(e)
			}
			resp.Body.Close()
			if resp.StatusCode != 204 {
				t.Fatal("external server affected")
			}
		})
	}
}

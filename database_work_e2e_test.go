package main

import (
	"context"
	"devbase/internal/devplatform"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestUIDatabaseWorkCallerCrashHelper(t *testing.T) {
	root := os.Getenv("MSM_UI_DB_WORK_ROOT")
	if root == "" {
		return
	}
	m := devplatform.New(root)
	m.Event = func(s string) {
		if strings.HasPrefix(s, "Running database task ") {
			os.Exit(23)
		}
	}
	id, name := os.Getenv("MSM_UI_DB_WORK_ID"), os.Getenv("MSM_UI_DB_WORK_DATABASE")
	if id == "" {
		id = "deletion-test"
	}
	if name == "" {
		name = "0"
	}
	_, _ = m.Backup(context.Background(), id, name)
	os.Exit(26)
}
func TestUIDatabaseWorkCallerDeathReviewKeepsRealRedisAndBackup(t *testing.T) {
	for _, dark := range []bool{false, true} {
		t.Run(fmt.Sprintf("minimum-dark-%t", dark), func(t *testing.T) {
			h, m, _, id, redis := newUIDeletionInstance(t, dark)
			ctx := context.Background()
			c := exec.Command(os.Args[0], "-test.run=^TestUIDatabaseWorkCallerCrashHelper$")
			c.Env = append(os.Environ(), "MSM_UI_DB_WORK_ROOT="+h.root)
			err := c.Run()
			var exit *exec.ExitError
			if !errors.As(err, &exit) || exit.ExitCode() != 23 {
				t.Fatal(err)
			}
			t.Cleanup(func() {
				records, _ := m.PendingDatabaseWork(ctx)
				for _, r := range records {
					_ = m.ResolveDatabaseWork(ctx, r.ID, r.Revision)
				}
			})
			h.click("Unfinished database tasks")
			if !h.a.dev.databaseWorkOpen || len(h.a.dev.databaseWork) != 1 {
				t.Fatal(h.a.dev.output, h.a.dev.databaseWork)
			}
			deadline := time.Now().Add(10 * time.Second)
			for time.Now().Before(deadline) {
				h.click("Refresh database tasks")
				if h.a.dev.databaseWork[0].Status == "Succeeded" {
					break
				}
				time.Sleep(25 * time.Millisecond)
			}
			row := h.a.dev.databaseWork[0]
			if row.Status != "Succeeded" || row.CallerActive || !row.CanResolve {
				t.Fatal(row)
			}
			backup := row.Output
			if !strings.HasSuffix(backup, ".dump") {
				t.Fatal(row)
			}
			h.click("Finish database task " + row.ID[:8])
			if len(h.a.dev.databaseWork) != 0 {
				t.Fatal("UI did not finish task", h.a.dev.output)
			}
			if redis("GET", "value") != "42" {
				t.Fatal("review changed running source")
			}
			if _, e := os.Stat(backup); e != nil {
				t.Fatal("review deleted actual backup", e)
			}
			h.click("Back to database settings")
			h.fill("Database number (0–15)", "1")
			h.fill("Backup file", backup)
			h.click("Restore into new database")
			if redis("-n", "1", "GET", "value") != "42" {
				t.Fatal("reviewed backup did not restore")
			}
			records, e := m.Operations(ctx)
			if e != nil {
				t.Fatal(e)
			}
			interrupted := false
			for _, r := range records {
				if r.Request.Kind == "backup" && r.Request.Resource == id && r.Status == "Interrupted" {
					interrupted = true
				}
			}
			if !interrupted {
				t.Fatal("caller crash was rewritten as success", records)
			}
			if _, e = os.Stat(filepath.Join(h.root, "instances", id, "password")); e != nil {
				t.Fatal(e)
			}
		})
	}
}

func TestUIDatabaseWorkStopsOrphanToolAndRejectsChangedOwnership(t *testing.T) {
	for _, dark := range []bool{false, true} {
		t.Run(fmt.Sprintf("minimum-dark-%t", dark), func(t *testing.T) {
			h := newDevUIHarnessSize(t, 900, 520, dark)
			m := h.a.dev.managerFactory()
			ctx := context.Background()
			state, e := m.Read()
			if e != nil {
				t.Fatal(e)
			}
			pkg := devplatform.Package{Name: "database-tool-fixture", Version: "controlled-tool", Arch: "universal", Binaries: map[string]string{"pg_dump": "pg_dump"}}
			dir := filepath.Join(h.root, "packages", pkg.ID())
			if e = os.MkdirAll(dir, 0700); e != nil {
				t.Fatal(e)
			}
			script := fmt.Sprintf("#!/usr/bin/python3\nimport pathlib,sys,time\nroot=pathlib.Path(%q)\np=pathlib.Path(sys.argv[sys.argv.index('-f')+1]);p.write_bytes(b'partial')\n(root/'slow-entered').write_text('started')\nwhile not (root/'slow-release').exists():time.sleep(.02)\np.write_bytes(b'complete')\n", h.root)
			if e = os.WriteFile(filepath.Join(dir, "pg_dump"), []byte(script), 0700); e != nil {
				t.Fatal(e)
			}
			instance := devplatform.Instance{ID: "slow", Kind: "postgresql", Package: pkg.ID(), Port: freeUIPort(t)}
			instanceDir := filepath.Join(h.root, "instances", instance.ID)
			if e = os.MkdirAll(instanceDir, 0700); e != nil {
				t.Fatal(e)
			}
			if e = os.WriteFile(filepath.Join(instanceDir, "password"), []byte("fixture-credential"), 0600); e != nil {
				t.Fatal(e)
			}
			state.Packages[pkg.ID()] = pkg
			state.Instances[instance.ID] = instance
			b, _ := json.Marshal(state)
			if e = os.WriteFile(filepath.Join(h.root, "state.json"), b, 0600); e != nil {
				t.Fatal(e)
			}
			c := exec.Command(os.Args[0], "-test.run=^TestUIDatabaseWorkCallerCrashHelper$")
			c.Env = append(os.Environ(), "MSM_UI_DB_WORK_ROOT="+h.root, "MSM_UI_DB_WORK_ID=slow", "MSM_UI_DB_WORK_DATABASE=source")
			e = c.Run()
			var exit *exec.ExitError
			if !errors.As(e, &exit) || exit.ExitCode() != 23 {
				t.Fatal(e)
			}
			t.Cleanup(func() {
				rows, _ := m.PendingDatabaseWork(ctx)
				for _, r := range rows {
					_ = m.ResolveDatabaseWork(ctx, r.ID, r.Revision)
				}
			})
			deadline := time.Now().Add(8 * time.Second)
			for time.Now().Before(deadline) {
				if _, e = os.Stat(filepath.Join(h.root, "slow-entered")); e == nil {
					break
				}
				time.Sleep(20 * time.Millisecond)
			}
			if _, e = os.Stat(filepath.Join(h.root, "slow-entered")); e != nil {
				t.Fatal("tool never started", e)
			}
			h.click("Refresh")
			h.click("Databases")
			h.click("Unfinished database tasks")
			if len(h.a.dev.databaseWork) != 1 {
				t.Fatal(h.a.dev.databaseWork)
			}
			r := h.a.dev.databaseWork[0]
			if !r.Running || !r.CanResolve || r.CallerActive {
				t.Fatal(r)
			}
			paths, _ := filepath.Glob(filepath.Join(h.root, "jobs", "*dbwork-"+r.ID+".plist"))
			if len(paths) != 1 {
				t.Fatal(paths)
			}
			original, e := os.ReadFile(paths[0])
			if e != nil {
				t.Fatal(e)
			}
			t.Cleanup(func() { _ = os.WriteFile(paths[0], original, 0600) })
			if e = os.WriteFile(paths[0], append(append([]byte{}, original...), []byte("\n<!--changed-->\n")...), 0600); e != nil {
				t.Fatal(e)
			}
			h.click("Refresh database tasks")
			if h.a.dev.databaseWork[0].CanResolve {
				t.Fatal("UI allowed stop of unknown job")
			}
			if e = os.WriteFile(paths[0], original, 0600); e != nil {
				t.Fatal(e)
			}
			h.click("Refresh database tasks")
			h.click("Stop database task " + r.ID[:8])
			if len(h.a.dev.databaseWork) != 0 {
				t.Fatal("UI did not stop worker", h.a.dev.output)
			}
			if _, e = os.Stat(filepath.Join(instanceDir, "password")); e != nil {
				t.Fatal("UI erased instance", e)
			}
			if _, e = os.Stat(r.Output); e != nil {
				t.Fatal("UI erased partial output", r.Output, e)
			}
			h.click("Instance slow")
			if h.a.dev.databaseWorkOpen {
				t.Fatal("instance selection stayed in review")
			}
		})
	}
}

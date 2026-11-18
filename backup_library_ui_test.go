package main

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"testing"
	"time"

	"devbase/internal/devplatform"
	"github.com/egoist/mygo/ui"
)

func TestUIBackupLibrary(t *testing.T) {
	setup := func(t *testing.T) (*app, *ui.Tester, string, string, string) {
		a := designFixture(t)
		a.spawn = func(f func()) { f() }
		a.post = func(f func()) { f() }
		m := devplatform.New(t.TempDir())
		state := a.dev.state
		state.Instances = map[string]devplatform.Instance{"atlas-postgres": {ID: "atlas-postgres", Kind: "postgresql", Port: 5432}}
		raw, e := json.Marshal(state)
		if e != nil {
			t.Fatal(e)
		}
		if e = os.WriteFile(filepath.Join(m.Root, "state.json"), raw, 0600); e != nil {
			t.Fatal(e)
		}
		a.dev.managerFactory = func() *devplatform.Manager { return m }
		dir := filepath.Join(m.Root, "backups")
		os.MkdirAll(dir, 0700)
		first := filepath.Join(dir, "atlas-postgres-alphadb-0123456789abcdef.dump")
		second := filepath.Join(dir, "atlas-postgres-betadb-0123456789abcdef.dump")
		os.WriteFile(first, []byte("a"), 0600)
		os.WriteFile(second, []byte("b"), 0600)
		now := time.Now()
		os.Chtimes(first, now, now.Add(-time.Hour))
		a.openInstance("atlas-postgres")
		tt := ui.NewTester(a.view, 1120, 1600)
		tt.Frame()
		tt.Frame()
		return a, tt, dir, first, second
	}
	exists := func(p string) bool { _, e := os.Lstat(p); return e == nil }
	t.Run("lists", func(t *testing.T) {
		_, tt, _, _, _ := setup(t)
		for _, s := range []string{"alphadb", "betadb", "Use for restore"} {
			if !tt.HasText(s) {
				t.Fatalf("missing %q", s)
			}
		}
	})
	t.Run("delete-asks-first", func(t *testing.T) {
		a, tt, _, first, second := setup(t)
		for i := 0; i < 2; i++ {
			if e := tt.Click("Delete"); e != nil {
				t.Fatal(e)
			}
			tt.Frame()
			if !a.ask.open || a.ask.button != "Delete Backup" || !exists(first) || !exists(second) {
				t.Fatalf("not asking first: %+v", a.ask)
			}
			if i == 0 {
				if e := tt.Click("Cancel"); e != nil {
					t.Fatal(e)
				}
				tt.Frame()
				if a.ask.open || !exists(first) || !exists(second) {
					t.Fatal("cancel changed something")
				}
			}
		}
		a.ask.run()
		if a.dev.busy {
			t.Fatal("task still running")
		}
		tt.Frame()
		tt.Frame()
		gone := 0
		for _, p := range []string{first, second} {
			if !exists(p) {
				gone++
			}
		}
		if gone != 1 {
			t.Fatalf("expected exactly one backup deleted, got %d", gone)
		}
	})
	t.Run("use-for-restore", func(t *testing.T) {
		a, tt, _, first, second := setup(t)
		if !tt.HasText("Restore target database") {
			t.Fatal("missing Restore target database field")
		}
		if e := tt.Click("Use for restore"); e != nil {
			t.Fatal(e)
		}
		if p := a.dev.backupPath; p != first && p != second {
			t.Fatalf("backupPath %q", p)
		}
		wantRestore := "betadb_restored"
		if a.dev.backupPath == first {
			wantRestore = "alphadb_restored"
		}
		if a.dev.restoreDB != wantRestore {
			t.Fatalf("restoreDB = %q, want %q", a.dev.restoreDB, wantRestore)
		}
		// Switching to another instance or new draft resets restoreDB.
		a.dev.state.Instances["cache"] = devplatform.Instance{ID: "cache", Kind: "redis", Port: 6379}
		a.openInstance("cache")
		if a.dev.restoreDB != "" {
			t.Fatalf("switching instance kept restoreDB %q", a.dev.restoreDB)
		}
		a.dev.restoreDB = "1"
		a.newDatabaseDraft()
		if a.dev.restoreDB != "" {
			t.Fatalf("newDatabaseDraft kept restoreDB %q", a.dev.restoreDB)
		}
	})
	t.Run("redis-restore-prefill-and-fallback", func(t *testing.T) {
		a := designFixture(t)
		a.spawn = func(f func()) { f() }
		a.post = func(f func()) { f() }
		m := devplatform.New(t.TempDir())
		m.Run = func(context.Context, string, ...string) (string, error) {
			return "", errors.New("mocked command")
		}
		state := a.dev.state
		state.Instances = map[string]devplatform.Instance{
			"cache": {ID: "cache", Kind: "redis", Port: 6379},
		}
		raw, e := json.Marshal(state)
		if e != nil {
			t.Fatal(e)
		}
		if e = os.WriteFile(filepath.Join(m.Root, "state.json"), raw, 0600); e != nil {
			t.Fatal(e)
		}
		a.dev.state = state
		a.dev.managerFactory = func() *devplatform.Manager { return m }
		dir := filepath.Join(m.Root, "backups")
		os.MkdirAll(dir, 0700)
		b0 := filepath.Join(dir, "cache-0-0123456789abcdef.dump")
		b2 := filepath.Join(dir, "cache-2-0123456789abcdef.dump")
		os.WriteFile(b0, []byte("0"), 0600)
		os.WriteFile(b2, []byte("2"), 0600)
		now := time.Now()
		os.Chtimes(b2, now, now.Add(-time.Hour))
		a.openInstance("cache")
		tt := ui.NewTester(a.view, 1120, 1600)
		tt.Frame()
		tt.Frame()
		if !tt.HasText("Restore Redis database (0–15)") {
			t.Fatal("missing Restore Redis database (0–15) field")
		}
		if e := tt.Click("Use for restore"); e != nil {
			t.Fatal(e)
		}
		if a.dev.backupPath != b0 || a.dev.restoreDB != "1" {
			t.Fatalf("redis db 0 prefill: path=%q restoreDB=%q", a.dev.backupPath, a.dev.restoreDB)
		}
		// Clicking Restore into new database uses restoreDB when set, or falls back to dbName when empty.
		a.dev.dbName = "5"
		if e := tt.Click("Restore into new database"); e != nil {
			t.Fatal(e)
		}
		ops, err := m.Operations(context.Background())
		if err != nil || len(ops) == 0 || ops[0].Request.Database != "1" {
			t.Fatalf("expected restore target '1' from restoreDB, ops=%+v err=%v", ops, err)
		}
		a.dev.databaseTasks = nil
		a.dev.restoreDB = ""
		tt.Frame()
		if e := tt.Click("Restore into new database"); e != nil {
			t.Fatal(e)
		}
		ops, err = m.Operations(context.Background())
		if err != nil || len(ops) < 2 || ops[0].Request.Database != "5" {
			t.Fatalf("expected fallback restore target '5' from dbName, ops=%+v err=%v", ops, err)
		}
		// Non-0 Redis backup prefills "0".
		if e := os.Remove(b0); e != nil {
			t.Fatal(e)
		}
		a.reloadBackups()
		tt.Frame()
		tt.Frame()
		if e := tt.Click("Use for restore"); e != nil {
			t.Fatal(e)
		}
		if a.dev.backupPath != b2 || a.dev.restoreDB != "0" {
			t.Fatalf("redis db 2 prefill: path=%q restoreDB=%q", a.dev.backupPath, a.dev.restoreDB)
		}
	})
}

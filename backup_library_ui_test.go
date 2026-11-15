package main

import (
	"encoding/json"
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
		if e := tt.Click("Use for restore"); e != nil {
			t.Fatal(e)
		}
		if p := a.dev.backupPath; p != first && p != second {
			t.Fatalf("backupPath %q", p)
		}
	})
}

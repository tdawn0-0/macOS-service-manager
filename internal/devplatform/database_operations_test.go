package devplatform

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func databaseToolFixture(t *testing.T) (*Manager, Package) {
	t.Helper()
	m := testManager(t)
	t.Cleanup(func() {
		rows, _ := m.PendingDatabaseWork(context.Background())
		for _, r := range rows {
			if r.CanResolve {
				if e := m.ResolveDatabaseWork(context.Background(), r.ID, r.Revision); e != nil {
					t.Error("cleanup database task:", e)
				}
			}
		}
	})
	p := Package{Name: "postgresql", Version: "operation-test", Arch: "universal", Binaries: map[string]string{"pg_dump": "pg_dump", "createdb": "createdb", "pg_restore": "pg_restore", "psql": "psql"}}
	script := fmt.Sprintf("#!/usr/bin/python3\nimport pathlib,sys,time,os\nroot=pathlib.Path(%q)\nname=sys.argv[-1]\n(root/('entered-'+name)).write_text('started')\n(root/('pid-'+name)).write_text(str(os.getpid()))\noutput=None\nif '-f' in sys.argv:\n output=pathlib.Path(sys.argv[sys.argv.index('-f')+1]); output.write_bytes(b'partial')\nwhile not (root/'release').exists(): time.sleep(.02)\nif output: output.write_bytes(b'completed dump')\n", m.Root)
	if e := os.MkdirAll(m.packageDir(p), 0700); e != nil {
		t.Fatal(e)
	}
	for _, rel := range p.Binaries {
		if e := os.WriteFile(filepath.Join(m.packageDir(p), rel), []byte(script), 0700); e != nil {
			t.Fatal(e)
		}
	}
	for _, id := range []string{"first", "second"} {
		dir := filepath.Join(m.Root, "instances", id)
		if e := os.MkdirAll(dir, 0700); e != nil {
			t.Fatal(e)
		}
		if e := os.WriteFile(filepath.Join(dir, "password"), []byte("fixture-private-value"), 0600); e != nil {
			t.Fatal(e)
		}
	}
	if e := m.withState(context.Background(), func(s *State) error {
		s.Packages[p.ID()] = p
		for _, id := range []string{"first", "second"} {
			s.Instances[id] = Instance{ID: id, Kind: "postgresql", Package: p.ID(), Port: integrationPort(t)}
		}
		s.Projects["linked"] = Project{ID: "linked", SchemaVersion: Schema, Name: "Linked", Directory: t.TempDir(), Instances: []string{"first"}}
		return nil
	}); e != nil {
		t.Fatal(e)
	}
	return m, p
}
func waitDatabaseTool(t *testing.T, m *Manager, name string) {
	t.Helper()
	deadline := time.Now().Add(8 * time.Second)
	for time.Now().Before(deadline) {
		if _, e := os.Stat(filepath.Join(m.Root, "entered-"+name)); e == nil {
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatal("database tool did not execute", name)
}
func TestDatabaseToolsOverlapWithoutBlockingConfigurationAndReserveLifecycle(t *testing.T) {
	m, p := databaseToolFixture(t)
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	type result struct {
		path string
		err  error
	}
	done := make(chan result, 2)
	remaining := 2
	defer func() {
		cancel()
		for remaining > 0 {
			select {
			case <-done:
				remaining--
			case <-time.After(5 * time.Second):
				return
			}
		}
	}()
	for _, id := range []string{"first", "second"} {
		go func(id string) { path, e := New(m.Root).Backup(ctx, id, id); done <- result{path, e} }(id)
	}
	waitDatabaseTool(t, m, "first")
	waitDatabaseTool(t, m, "second")
	short, stop := context.WithTimeout(ctx, 2*time.Second)
	defer stop()
	saved := Project{SchemaVersion: Schema, ID: "unrelated", Name: "Saved during backups", Directory: t.TempDir()}
	if e := New(m.Root).SaveProject(short, saved); e != nil {
		t.Fatal("backup blocked workspace configuration", e)
	}
	assertBusy := func(name string, e error) {
		t.Helper()
		var busy *ResourceBusyError
		if !errors.As(e, &busy) || busy.ID != "first" {
			t.Fatal(name, "did not reserve the active instance", e)
		}
	}
	assertBusy("create database", m.CreateDatabase(short, "first", "another"))
	assertBusy("create user", m.CreateUser(short, "first", "newuser", "private", "first"))
	assertBusy("restore", m.Restore(short, "first", "another", "unused.dump"))
	for _, action := range []string{"start", "stop", "restart", "remove"} {
		assertBusy(action, m.InstanceAction(short, "first", action))
	}
	s, _ := m.Read()
	assertBusy("change port", m.ChangeInstancePort(short, s.Instances["first"], integrationPort(t)))
	assertBusy("delete data", m.DeleteInstanceData(short, "first", "unused", "first"))
	for name, action := range map[string]func(context.Context, string) error{"up": m.Up, "down": m.Down, "restart": m.RestartProject} {
		assertBusy(name, action(short, "linked"))
	}
	plan, e := m.PreviewShutdown(short)
	if e != nil {
		t.Fatal(e)
	}
	assertBusy("shutdown", m.Shutdown(short, plan, ShutdownOptions{}))
	if e = m.RepairPackage(short, p.ID()); e == nil {
		t.Fatal("replaced a package executing a database tool")
	}
	if e = m.Uninstall(short, p.ID()); e == nil {
		t.Fatal("uninstalled a package executing a database tool")
	}
	if e = os.WriteFile(filepath.Join(m.Root, "release"), nil, 0600); e != nil {
		t.Fatal(e)
	}
	for remaining > 0 {
		r := <-done
		remaining--
		if r.err != nil {
			t.Fatal(r.err)
		}
		b, e := os.ReadFile(r.path)
		if e != nil || string(b) != "completed dump" {
			t.Fatal(string(b), e)
		}
	}
	s, e = m.Read()
	if e != nil || s.Projects[saved.ID].Name != saved.Name {
		t.Fatal("tool completion replaced a newer configuration", e)
	}
}
func TestDatabaseToolCancellationDoesNotCancelIndependentBackup(t *testing.T) {
	m, _ := databaseToolFixture(t)
	firstCtx, cancelFirst := context.WithCancel(context.Background())
	defer cancelFirst()
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	type result struct {
		path string
		err  error
	}
	first, second := make(chan result, 1), make(chan result, 1)
	go func() { path, e := New(m.Root).Backup(firstCtx, "first", "first"); first <- result{path, e} }()
	go func() { path, e := New(m.Root).Backup(ctx, "second", "second"); second <- result{path, e} }()
	defer func() {
		cancelFirst()
		cancel()
		for _, done := range []chan result{first, second} {
			select {
			case <-done:
			case <-time.After(2 * time.Second):
			}
		}
	}()
	waitDatabaseTool(t, m, "first")
	waitDatabaseTool(t, m, "second")
	cancelFirst()
	r := <-first
	first <- r
	if r.err == nil || r.path != "" {
		t.Fatal("cancelled backup was published", r)
	}
	files, _ := filepath.Glob(filepath.Join(m.Root, "backups", "first-*.dump*"))
	if len(files) != 0 {
		t.Fatal("cancelled backup left a published or partial output", files)
	}
	select {
	case r = <-second:
		second <- r
		t.Fatal("independent backup completed before release", r)
	default:
	}
	saved := Project{SchemaVersion: Schema, ID: "during-cancel", Name: "Saved", Directory: t.TempDir()}
	if e := m.SaveProject(ctx, saved); e != nil {
		t.Fatal(e)
	}
	if e := os.WriteFile(filepath.Join(m.Root, "release"), nil, 0600); e != nil {
		t.Fatal(e)
	}
	r = <-second
	second <- r
	if r.err != nil || !strings.HasSuffix(r.path, ".dump") {
		t.Fatal(r)
	}
	if e := m.CreateDatabase(ctx, "first", "after-cancel"); e != nil {
		t.Fatal("cancelled task retained the instance lease", e)
	}
}

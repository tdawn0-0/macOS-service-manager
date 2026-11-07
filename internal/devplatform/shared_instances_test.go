package devplatform

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

func TestSharedDatabaseOnlyProjectOwnership(t *testing.T) {
	cache := os.Getenv("MSM_SIX_INTEGRATION_ROOT")
	if cache == "" {
		t.Skip("set MSM_SIX_INTEGRATION_ROOT to verified software storage")
	}
	source := New(cache)
	state, e := source.Read()
	if e != nil {
		t.Fatal(e)
	}
	pkg, ok := state.Packages["redis@7.4.7-"+runtime.GOARCH]
	if !ok {
		t.Fatal("install the supported Redis package before this integration test")
	}
	m := testManager(t)
	// Reuse verified, read-only installed binaries. All state and data are private.
	if e = os.Symlink(source.packageDir(pkg), m.packageDir(pkg)); e != nil {
		t.Fatal(e)
	}
	ctx := context.Background()
	if e = m.withState(ctx, func(s *State) error { s.Packages[pkg.ID()] = pkg; return nil }); e != nil {
		t.Fatal(e)
	}
	i := Instance{ID: "shared-cache", Kind: "redis", Package: pkg.ID(), Port: integrationPort(t), Persistent: true}
	if e = m.CreateInstance(ctx, i); e != nil {
		t.Fatal(e)
	}
	a := Project{SchemaVersion: Schema, ID: "first", Name: "First", Directory: t.TempDir(), Instances: []string{i.ID}}
	b := Project{SchemaVersion: Schema, ID: "second", Name: "Second", Directory: t.TempDir(), Instances: []string{i.ID}}
	for _, p := range []Project{a, b} {
		if e = m.SaveProject(ctx, p); e != nil {
			t.Fatal(e)
		}
	}
	t.Cleanup(func() {
		_ = m.Down(ctx, a.ID)
		_ = m.Down(ctx, b.ID)
		_ = m.RemoveProject(ctx, a.ID)
		_ = m.RemoveProject(ctx, b.ID)
		_ = m.InstanceAction(ctx, i.ID, "remove")
	})
	if e = m.InstanceAction(ctx, i.ID, "start"); e != nil {
		t.Fatal(e)
	}
	if e = m.Down(ctx, a.ID); e != nil {
		t.Fatal(e)
	}
	if !m.loaded(ctx, "db-"+i.ID) {
		t.Fatal("stopping an inactive project stopped a standalone instance")
	}
	if e = m.InstanceAction(ctx, i.ID, "stop"); e != nil {
		t.Fatal(e)
	}
	if e = m.Up(ctx, a.ID); e != nil {
		t.Fatal(e)
	}
	state, _ = m.Read()
	statuses := m.Statuses(ctx, state)
	if statuses["project:first"] != "Running" || statuses["project:second"] != "Stopped" {
		t.Fatal("shared dependency was mistaken for project ownership", statuses)
	}
	if e = m.Up(ctx, b.ID); e != nil {
		t.Fatal(e)
	}
	failed := Project{SchemaVersion: Schema, ID: "failing", Name: "Failing dependent", Directory: t.TempDir(), Instances: []string{i.ID}, Services: []Service{{ID: "bad", Command: []string{"/usr/bin/false"}, ReadinessTimeoutSeconds: 2}}}
	if e = m.SaveProject(ctx, failed); e != nil {
		t.Fatal(e)
	}
	t.Cleanup(func() { _ = m.RemoveProject(context.Background(), failed.ID) })
	if e = m.Up(ctx, failed.ID); e == nil {
		t.Fatal("failing dependent was reported started")
	}
	if !m.loaded(ctx, "db-"+i.ID) || m.loaded(ctx, "p-failing-bad") || m.loaded(ctx, "project-failing") {
		t.Fatal("failed project stopped an existing shared database or retained its new resources")
	}
	if e = m.Down(ctx, a.ID); e != nil {
		t.Fatal(e)
	}
	if !m.loaded(ctx, "db-"+i.ID) || !m.loaded(ctx, "project-"+b.ID) || m.loaded(ctx, "project-"+a.ID) {
		t.Fatal("stopping the first project released shared resources")
	}
	for _, action := range []string{"stop", "restart"} {
		if e = m.InstanceAction(ctx, i.ID, action); e == nil || !strings.Contains(e.Error(), "running project") {
			t.Fatal("instance operation ignored the database-only project", action, e)
		}
	}
	changed := b.Clone()
	changed.Name = "Cannot edit an active project"
	if e = m.SaveProject(ctx, changed); e == nil {
		t.Fatal("edited an active database-only project")
	}
	cancelled, cancel := context.WithCancel(ctx)
	cancel()
	if e = m.Down(cancelled, b.ID); !errors.Is(e, context.Canceled) {
		t.Fatal("canceled stop returned success", e)
	}
	if !m.loaded(ctx, "project-"+b.ID) || !m.loaded(ctx, "db-"+i.ID) {
		t.Fatal("canceled stop altered project ownership")
	}
	if e = m.Down(ctx, b.ID); e != nil {
		t.Fatal(e)
	}
	if m.loaded(ctx, "db-"+i.ID) || m.loaded(ctx, "project-"+b.ID) {
		t.Fatal("last owner did not release the instance")
	}
	if _, e = os.Stat(filepath.Join(m.Root, "instances", i.ID, "valkey.conf")); e != nil {
		t.Fatal("stopping projects removed data", e)
	}
	if e = m.InstanceAction(ctx, i.ID, "restart"); e != nil {
		t.Fatal("standalone instance restart failed", e)
	}
	if !m.loaded(ctx, "db-"+i.ID) {
		t.Fatal("restart did not start the standalone instance")
	}
	if e = m.Up(ctx, a.ID); e != nil {
		t.Fatal(e)
	}
	if e = m.Up(ctx, b.ID); e != nil {
		t.Fatal(e)
	}
	plan, e := m.PreviewShutdown(ctx)
	if e != nil {
		t.Fatal(e)
	}
	if e = m.Shutdown(ctx, plan, ShutdownOptions{StopProxy: false}); e != nil {
		t.Fatal(e)
	}
	if m.loaded(ctx, "db-"+i.ID) || m.loaded(ctx, "project-"+a.ID) || m.loaded(ctx, "project-"+b.ID) {
		t.Fatal("application shutdown retained an owned shared database or project")
	}
	if _, e = os.Stat(filepath.Join(m.Root, "instances", i.ID, "valkey.conf")); e != nil {
		t.Fatal("application shutdown removed data", e)
	}
}

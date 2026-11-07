package devplatform

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func lifecycleFixture(t *testing.T) (*Manager, Package, Instance, Instance) {
	t.Helper()
	script := `#!/usr/bin/python3
import pathlib,socket,sys,time
config=pathlib.Path(sys.argv[1]); root=config.parents[2]; name=config.parent.name
(root/('entered-'+name)).write_text('ready to wait')
while not (root/('ready-'+name)).exists(): time.sleep(.02)
port=int(next(x.split()[1] for x in config.read_text().splitlines() if x.startswith('port ')))
s=socket.socket(); s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1); s.bind(('127.0.0.1',port)); s.listen()
while True:
 c,_=s.accept(); c.close()
`
	m, p := initializationFixture(t, script)
	a, b := initializationInstance(t, p, "first"), initializationInstance(t, p, "second")
	for a.Port == b.Port {
		b.Port = integrationPort(t)
	}
	for _, i := range []Instance{a, b} {
		if e := m.CreateInstance(context.Background(), i); e != nil {
			t.Fatal(e)
		}
	}
	t.Cleanup(func() {
		for _, i := range []Instance{a, b} {
			_ = m.InstanceAction(context.Background(), i.ID, "stop")
		}
	})
	return m, p, a, b
}
func waitLifecycleEntry(t *testing.T, m *Manager, id string) {
	t.Helper()
	deadline := time.Now().Add(8 * time.Second)
	for time.Now().Before(deadline) {
		if _, e := os.Stat(filepath.Join(m.Root, "entered-"+id)); e == nil {
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatal("managed process did not enter readiness wait", id)
}
func TestInstanceLifecycleReadinessRunsConcurrentlyAndKeepsConfigurationAvailable(t *testing.T) {
	m, p, a, b := lifecycleFixture(t)
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	secondCtx, cancelSecond := context.WithCancel(ctx)
	defer cancelSecond()
	first, second := make(chan error, 1), make(chan error, 1)
	go func() { first <- New(m.Root).InstanceAction(ctx, a.ID, "start") }()
	go func() { second <- New(m.Root).InstanceAction(secondCtx, b.ID, "start") }()
	defer func() {
		cancel()
		cancelSecond()
		for _, ch := range []chan error{first, second} {
			select {
			case <-ch:
			case <-time.After(5 * time.Second):
			}
		}
	}()
	waitLifecycleEntry(t, m, a.ID)
	waitLifecycleEntry(t, m, b.ID)
	short, stop := context.WithTimeout(ctx, 3*time.Second)
	defer stop()
	project := Project{SchemaVersion: Schema, ID: "unrelated", Name: "Saved during readiness", Directory: t.TempDir()}
	if e := m.SaveProject(short, project); e != nil {
		t.Fatal("readiness blocked unrelated configuration", e)
	}
	var busy *ResourceBusyError
	for _, action := range []string{"start", "stop", "restart", "remove"} {
		if e := m.InstanceAction(short, a.ID, action); !errors.As(e, &busy) || busy.ID != a.ID {
			t.Fatal(action, e)
		}
	}
	project.ID = "new-reference"
	project.Instances = []string{a.ID}
	if e := m.SaveProject(short, project); !errors.As(e, &busy) {
		t.Fatal("configuration attached a busy database", e)
	}
	var packageBusy *PackageBusyError
	if e := m.RepairPackage(short, p.ID()); !errors.As(e, &packageBusy) {
		t.Fatal("readiness did not protect its binary", e)
	}
	if e := os.WriteFile(filepath.Join(m.Root, "ready-"+a.ID), nil, 0600); e != nil {
		t.Fatal(e)
	}
	e := <-first
	first <- e
	if e != nil {
		t.Fatal(e)
	}
	cancelSecond()
	e = <-second
	second <- e
	if e == nil {
		t.Fatal("cancelled readiness was reported as started")
	}
	if e = m.verifyShutdownTarget(ctx, domain()+"/"+m.label("db-"+b.ID)); e != nil {
		t.Fatal("cancelled startup left its own job", e)
	}
	if !m.loaded(ctx, "db-"+a.ID) {
		t.Fatal("cancelling the other startup stopped a ready instance")
	}
	s, e := m.Read()
	if e != nil || s.Projects["unrelated"].Name != "Saved during readiness" {
		t.Fatal("lifecycle overwrote newly saved configuration", e)
	}
}
func TestInstanceLifecycleStopWaitDoesNotHoldWorkspaceLock(t *testing.T) {
	m, _, a, b := lifecycleFixture(t)
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	if e := os.Remove(filepath.Join(m.Root, "logs")); e != nil {
		t.Fatal(e)
	}
	for _, i := range []Instance{a, b} {
		if e := os.WriteFile(filepath.Join(m.Root, "ready-"+i.ID), nil, 0600); e != nil {
			t.Fatal(e)
		}
		if e := m.InstanceAction(ctx, i.ID, "start"); e != nil {
			t.Fatal(e)
		}
	}
	worker := New(m.Root)
	run := worker.Run
	entered, release := make(chan struct{}), make(chan struct{})
	worker.Run = func(ctx context.Context, name string, args ...string) (string, error) {
		if name == "launchctl" && len(args) > 1 && args[0] == "bootout" && strings.HasSuffix(args[1], worker.label("db-"+a.ID)) {
			close(entered)
			select {
			case <-release:
			case <-ctx.Done():
				return "", ctx.Err()
			}
		}
		return run(ctx, name, args...)
	}
	done := make(chan error, 1)
	go func() { done <- worker.InstanceAction(ctx, a.ID, "stop") }()
	defer func() {
		cancel()
		select {
		case <-done:
		case <-time.After(5 * time.Second):
		}
	}()
	select {
	case <-entered:
	case <-time.After(5 * time.Second):
		t.Fatal("stop did not reach the managed job")
	}
	short, stop := context.WithTimeout(ctx, 2*time.Second)
	defer stop()
	if e := m.SaveProject(short, Project{SchemaVersion: Schema, ID: "during-stop", Name: "Saved", Directory: t.TempDir()}); e != nil {
		t.Fatal("stop held the workspace lock", e)
	}
	if !m.loaded(ctx, "db-"+a.ID) || !m.loaded(ctx, "db-"+b.ID) {
		t.Fatal("unreleased stop affected a running job")
	}
	close(release)
	e := <-done
	done <- e
	if e != nil {
		t.Fatal(e)
	}
	if e = m.verifyShutdownTarget(ctx, domain()+"/"+m.label("db-"+a.ID)); e != nil {
		t.Fatal(e)
	}
	if !m.loaded(ctx, "db-"+b.ID) {
		t.Fatal("stopping one instance stopped another")
	}
}
func TestInstanceLifecycleUnknownObservationDoesNotStopOrRemove(t *testing.T) {
	m, _, i, _ := lifecycleFixture(t)
	projectUnknown := false
	m.Run = func(_ context.Context, name string, args ...string) (string, error) {
		if name != "launchctl" || len(args) == 0 || args[0] != "print" {
			t.Fatal("acted on an unknown job", name, args)
		}
		if projectUnknown && strings.HasSuffix(args[1], m.label("db-"+i.ID)) {
			t.Fatal("touched a database before confirming its project was stopped")
		}
		return "", errors.New("launchctl observation denied")
	}
	for _, action := range []string{"stop", "remove"} {
		if e := m.InstanceAction(context.Background(), i.ID, action); e == nil {
			t.Fatal("unknown job was treated as stopped", action)
		}
	}
	s, _ := m.Read()
	if _, ok := s.Instances[i.ID]; !ok {
		t.Fatal("unknown stop removed the instance record")
	}
	if e := m.SaveProject(context.Background(), Project{SchemaVersion: Schema, ID: "unknown-project", Name: "Unknown project", Directory: t.TempDir(), Instances: []string{i.ID}}); e != nil {
		t.Fatal(e)
	}
	projectUnknown = true
	for _, action := range []string{"stop", "restart"} {
		if e := m.InstanceAction(context.Background(), i.ID, action); e == nil {
			t.Fatal("an unobservable project was treated as stopped", action)
		}
	}
	// Cleanup uses the same fake observer; no fixture jobs were started.
	projectUnknown = false
}
func TestInstanceLifecycleRemovalWriteFailureKeepsRecordAndData(t *testing.T) {
	m, p := initializationFixture(t, "#!/bin/sh\nexit 0\n")
	i := initializationInstance(t, p, "removal-failure")
	if e := m.CreateInstance(context.Background(), i); e != nil {
		t.Fatal(e)
	}
	data := filepath.Join(m.Root, "instances", i.ID, "password")
	before, e := os.ReadFile(data)
	if e != nil {
		t.Fatal(e)
	}
	t.Cleanup(func() { _ = os.Chmod(m.Root, 0700) })
	m.Event = func(s string) {
		if s == "Stopping database "+i.ID {
			if e := os.Chmod(m.Root, 0500); e != nil {
				t.Fatal(e)
			}
		}
	}
	if e = m.InstanceAction(context.Background(), i.ID, "remove"); e == nil {
		t.Fatal("failed state write was reported as removal")
	}
	if e = os.Chmod(m.Root, 0700); e != nil {
		t.Fatal(e)
	}
	m.Event = nil
	s, e := m.Read()
	if e != nil || s.Instances[i.ID] != i {
		t.Fatal("failed removal changed the record", e)
	}
	after, e := os.ReadFile(data)
	if e != nil || string(after) != string(before) {
		t.Fatal("failed removal changed database files", e)
	}
	if e = m.InstanceAction(context.Background(), i.ID, "remove"); e != nil {
		t.Fatal("failed removal could not be retried", e)
	}
}

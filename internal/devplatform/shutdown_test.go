package devplatform

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

func TestShutdownScopeIndependentResourcesAndDataRetention(t *testing.T) {
	m := testManager(t)
	ctx := context.Background()
	s := emptyState()
	s.Instances["db"] = Instance{ID: "db", Kind: "redis"}
	if err := writeJSON(filepath.Join(m.Root, "state.json"), s); err != nil {
		t.Fatal(err)
	}
	data := filepath.Join(m.Root, "instances", "db", "data", "sentinel")
	if err := atomicWrite(data, []byte("retained data"), 0600); err != nil {
		t.Fatal(err)
	}
	loaded := map[string]bool{
		m.label("db-db"):            true,
		m.label("proxy"):            true,
		m.label("tunnel"):           true,
		m.label("maintenance"):      true,
		"external.unmanaged.server": true,
	}
	m.Run = func(ctx context.Context, name string, args ...string) (string, error) {
		if err := ctx.Err(); err != nil {
			return "", err
		}
		id := strings.TrimPrefix(args[len(args)-1], domain()+"/")
		if args[0] == "print" {
			if loaded[id] {
				return "state = running\npid = 0", nil
			}
			return "", errors.New("Could not find service")
		}
		if args[0] == "bootout" {
			delete(loaded, id)
			return "", nil
		}
		t.Fatal("unexpected mutation", name, args)
		return "", nil
	}
	plan, err := m.PreviewShutdown(ctx)
	if err != nil || len(plan.Instances) != 1 {
		t.Fatal(plan, err)
	}
	if err := m.Shutdown(ctx, plan, ShutdownOptions{}); err != nil {
		t.Fatal(err)
	}
	if !loaded["external.unmanaged.server"] || !loaded[m.label("proxy")] || !loaded[m.label("tunnel")] || !loaded[m.label("maintenance")] || loaded[m.label("db-db")] {
		t.Fatal("stopped outside chosen scope or missed owned job", loaded)
	}
	after, err := m.Read()
	if err != nil || !reflect.DeepEqual(s, after) {
		t.Fatal("shutdown altered settings", err)
	}
	if b, err := os.ReadFile(data); err != nil || string(b) != "retained data" {
		t.Fatal("shutdown deleted data", err)
	}
}

func TestShutdownRejectsChangedScopeAndCancellationBeforeMutation(t *testing.T) {
	m := testManager(t)
	plan, err := m.PreviewShutdown(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if err = m.withState(context.Background(), func(s *State) error {
		s.Instances["added"] = Instance{ID: "added", Kind: "redis"}
		return nil
	}); err != nil {
		t.Fatal(err)
	}
	m.Run = func(context.Context, string, ...string) (string, error) {
		t.Fatal("mutated stale scope")
		return "", nil
	}
	if err = m.Shutdown(context.Background(), plan, ShutdownOptions{}); err == nil || !strings.Contains(err.Error(), "workspace changed") {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if err = m.Shutdown(ctx, plan, ShutdownOptions{}); !errors.Is(err, context.Canceled) {
		t.Fatal("cancelled stop was successful", err)
	}
}

func TestShutdownDoesNotTreatFailedObservationAsStopped(t *testing.T) {
	m := testManager(t)
	m.LoginDir = t.TempDir()
	if err := m.withState(context.Background(), func(s *State) error {
		s.Instances["db"] = Instance{ID: "db", Kind: "redis"}
		return nil
	}); err != nil {
		t.Fatal(err)
	}
	plan, err := m.PreviewShutdown(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	m.Run = func(context.Context, string, ...string) (string, error) {
		return "", errors.New("launchd observation unavailable")
	}
	if err = m.Shutdown(context.Background(), plan, ShutdownOptions{}); err == nil || !strings.Contains(err.Error(), "could not verify stopped service") {
		t.Fatal("quit succeeded without proof that its services stopped", err)
	}
}

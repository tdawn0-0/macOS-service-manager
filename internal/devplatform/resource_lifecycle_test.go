package devplatform

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestIndependentResourceLifecycle(t *testing.T) {
	m := testManager(t)
	s := emptyState()
	s.Instances["one"] = Instance{ID: "one", Kind: "redis"}
	s.Instances["two"] = Instance{ID: "two", Kind: "redis"}
	if err := writeJSON(filepath.Join(m.Root, "state.json"), s); err != nil {
		t.Fatal(err)
	}
	data := filepath.Join(m.Root, "instances", "one", "data", "sentinel")
	secret := filepath.Join(m.Root, "instances", "one", "private", "secret")
	for _, path := range []string{data, secret} {
		if err := atomicWrite(path, []byte("unchanged"), 0600); err != nil {
			t.Fatal(err)
		}
	}
	loaded := map[string]bool{
		m.label("db-one"):      true,
		m.label("db-two"):      true,
		m.label("proxy"):       true,
		m.label("tunnel"):      true,
		m.label("maintenance"): true,
		"external":             true,
	}
	m.Run = func(ctx context.Context, command string, args ...string) (string, error) {
		if err := ctx.Err(); err != nil {
			return "", err
		}
		if len(args) < 2 {
			return "", errors.New("unexpected command")
		}
		id := strings.TrimPrefix(args[len(args)-1], domain()+"/")
		switch args[0] {
		case "print":
			if loaded[id] {
				return "state = running\npid = 100", nil
			}
			return "", errors.New("Could not find service")
		case "bootout":
			if id == "external" {
				t.Fatal("stopped outside process")
			}
			delete(loaded, id)
			return "", nil
		}
		return "", errors.New("unexpected command")
	}
	plan, err := m.PreviewShutdown(context.Background())
	if err != nil || len(plan.Instances) != 2 {
		t.Fatal(plan, err)
	}
	if err := m.Shutdown(context.Background(), plan, ShutdownOptions{}); err != nil {
		t.Fatal(err)
	}
	if loaded[m.label("db-one")] || loaded[m.label("db-two")] || !loaded[m.label("proxy")] || !loaded[m.label("tunnel")] || !loaded[m.label("maintenance")] || !loaded["external"] {
		t.Fatal("wrong quit scope", loaded)
	}
	for _, path := range []string{data, secret} {
		b, err := os.ReadFile(path)
		if err != nil || string(b) != "unchanged" {
			t.Fatal("changed retained data", path, err)
		}
	}
}

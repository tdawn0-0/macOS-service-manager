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

func TestShutdownScopeSharedDependenciesAndDataRetention(t *testing.T) {
	for _, stopProxy := range []bool{false, true} {
		t.Run(map[bool]string{false: "retain-proxy", true: "stop-proxy"}[stopProxy], func(t *testing.T) {
			m := testManager(t)
			m.LoginDir = t.TempDir()
			ctx := context.Background()
			s := emptyState()
			s.Projects["a"] = Project{ID: "a", Name: "A", Directory: t.TempDir(), Autostart: true, Services: []Service{{ID: "base", Command: []string{"/usr/bin/true"}}, {ID: "api", Command: []string{"/usr/bin/true"}, DependsOn: []string{"base"}}}, Instances: []string{"shared"}}
			s.Projects["b"] = Project{ID: "b", Name: "B", Directory: t.TempDir(), Instances: []string{"shared"}}
			s.Instances["shared"] = Instance{ID: "shared", Kind: "redis"}
			s.Instances["standalone"] = Instance{ID: "standalone", Kind: "redis"}
			if err := writeJSON(filepath.Join(m.Root, "state.json"), s); err != nil {
				t.Fatal(err)
			}
			data := filepath.Join(m.Root, "instances", "shared", "data", "sentinel")
			if err := atomicWrite(data, []byte("retained data"), 0600); err != nil {
				t.Fatal(err)
			}
			if err := atomicWrite(m.loginPath("a"), []byte("login preference"), 0600); err != nil {
				t.Fatal(err)
			}
			if err := atomicWrite(m.maintenancePath(), []byte("scheduled helper"), 0600); err != nil {
				t.Fatal(err)
			}
			loaded := map[string]bool{}
			for _, id := range []string{"project-a", "project-b", "p-a-base", "p-a-api", "db-shared", "db-standalone", "proxy", "maintenance", "login-a"} {
				loaded[m.label(id)] = true
			}
			external := "external.unmanaged.server"
			loaded[external] = true
			var stopped []string
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
					if id == m.label("db-shared") && loaded[m.label("project-a")] && loaded[m.label("project-b")] {
						t.Fatal("stopped shared database before its last project")
					}
					stopped = append(stopped, id)
					delete(loaded, id)
					return "", nil
				}
				t.Fatal("unexpected mutation", name, args)
				return "", nil
			}
			plan, err := m.PreviewShutdown(ctx)
			if err != nil {
				t.Fatal(err)
			}
			if len(plan.Projects) != 2 || len(plan.Instances) != 2 || plan.PrivilegedProxy {
				t.Fatal(plan)
			}
			if err = m.Shutdown(ctx, plan, ShutdownOptions{StopProxy: stopProxy}); err != nil {
				t.Fatal(err)
			}
			if !loaded[external] || loaded[m.label("proxy")] == stopProxy {
				t.Fatal("stopped outside the chosen scope", loaded)
			}
			for id := range loaded {
				if id != external && id != m.label("proxy") {
					t.Fatal("owned job was not stopped", id)
				}
			}
			if len(stopped) < 3 || stopped[1] != m.label("p-a-api") || stopped[2] != m.label("p-a-base") {
				t.Fatal("did not stop in reverse dependency order", stopped)
			}
			after, err := m.Read()
			if err != nil || !reflect.DeepEqual(s, after) {
				t.Fatal("shutdown altered settings", err)
			}
			if b, err := os.ReadFile(data); err != nil || string(b) != "retained data" {
				t.Fatal("shutdown deleted data", err)
			}
			if _, err = os.Stat(m.loginPath("a")); err != nil {
				t.Fatal("shutdown removed login preference", err)
			}
		})
	}
}

func TestShutdownRejectsChangedScopeAndCancellationBeforeMutation(t *testing.T) {
	m := testManager(t)
	plan, err := m.PreviewShutdown(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if err = m.withState(context.Background(), func(s *State) error {
		s.Projects["added"] = Project{ID: "added", Name: "New project", Directory: t.TempDir()}
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

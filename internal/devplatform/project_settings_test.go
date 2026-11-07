package devplatform

import (
	"context"
	"errors"
	"macos-service-manager/internal/launchd"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestLoginProjectUsesExistingLaunchdSpec(t *testing.T) {
	m := testManager(t)
	m.LoginDir = t.TempDir()
	p := Project{SchemaVersion: 1, ID: "login-project", Name: "Login project", Directory: t.TempDir(), Autostart: true}
	if e := m.SaveProject(context.Background(), p); e != nil {
		t.Fatal(e)
	}
	spec, e := launchd.LoadSpec(m.loginPath(p.ID))
	if e != nil {
		t.Fatal(e)
	}
	if !spec.RunAtLoad || spec.KeepAlive || strings.Join(spec.Args[1:], " ") != "--dev up login-project" || spec.Env["MSM_DEV_HOME"] != m.Root {
		t.Fatal(spec)
	}
	if spec.Args[0] != filepath.Join(m.Root, "helpers", "project-launcher") {
		t.Fatal("login task depends on a movable app or build directory")
	}
	p.Autostart = false
	if e = m.SaveProject(context.Background(), p); e != nil {
		t.Fatal(e)
	}
	if _, e = os.Stat(m.loginPath(p.ID)); !os.IsNotExist(e) {
		t.Fatal("login job still exists")
	}
}
func TestEnvironmentFileDoesNotExecuteExpressions(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, ".env")
	if e := os.WriteFile(path, []byte("# comment\nexport NAME='hello world'\nTOKEN=$(touch hacked)\nEMPTY=\n"), 0600); e != nil {
		t.Fatal(e)
	}
	env, e := readEnvironment(path)
	if e != nil {
		t.Fatal(e)
	}
	if env["NAME"] != "hello world" || env["TOKEN"] != "$(touch hacked)" {
		t.Fatal(env)
	}
	if _, e = os.Stat(filepath.Join(dir, "hacked")); !os.IsNotExist(e) {
		t.Fatal("executed env data")
	}
}
func TestSaveRejectsDuplicateDomains(t *testing.T) {
	m := testManager(t)
	ctx := context.Background()
	site := Site{ID: "global", Domain: "app.test", Upstream: "http://127.0.0.1:3000", Port: 443}
	if e := m.SaveSite(ctx, site); e != nil {
		t.Fatal(e)
	}
	p := Project{SchemaVersion: 1, ID: "duplicate", Name: "Duplicate", Directory: t.TempDir(), Sites: []Site{site}}
	if e := m.SaveProject(ctx, p); e == nil {
		t.Fatal("accepted duplicate domain")
	}
	state, _ := m.Read()
	if _, ok := state.Projects[p.ID]; ok {
		t.Fatal("saved invalid project")
	}
}
func TestTrustUsesUserContextAndExactCertificate(t *testing.T) {
	m := testManager(t)
	var calls []string
	trusted := false
	m.Run = func(ctx context.Context, name string, args ...string) (string, error) {
		calls = append(calls, strings.Join(args, " "))
		if args[0] == "verify-cert" {
			if trusted {
				return "", nil
			}
			return "", errors.New("not trusted")
		}
		if args[0] == "add-trusted-cert" {
			trusted = true
		}
		if args[0] == "default-keychain" {
			return `"/Users/example/Library/Keychains/login.keychain-db"`, nil
		}
		return "", nil
	}
	if e := m.ConfigureTrust(context.Background(), false); e != nil {
		t.Fatal(e)
	}
	if len(calls) != 4 || !strings.Contains(calls[2], "add-trusted-cert -r trustRoot -k /Users/example/") {
		t.Fatal(calls)
	}
	m.Run = func(context.Context, string, ...string) (string, error) {
		return "", errors.New("authorization canceled")
	}
	if e := m.ConfigureTrust(context.Background(), false); e == nil {
		t.Fatal("hid authorization failure")
	}
}

func TestProjectSaveFailureRestoresLoginPreference(t *testing.T) {
	for _, change := range []string{"enable", "disable", "move"} {
		t.Run(change, func(t *testing.T) {
			m := testManager(t)
			m.LoginDir = t.TempDir()
			ctx := context.Background()
			p := Project{SchemaVersion: Schema, ID: "login-rollback", Name: "Saved", Directory: t.TempDir(), Autostart: change != "enable"}
			if e := m.SaveProject(ctx, p); e != nil {
				t.Fatal(e)
			}
			before, readErr := os.ReadFile(m.loginPath(p.ID))
			oldState, e := os.ReadFile(filepath.Join(m.Root, "state.json"))
			if e != nil {
				t.Fatal(e)
			}
			draft := p.Clone()
			draft.Name = "Not committed"
			if change == "move" {
				draft.Directory = t.TempDir()
			} else {
				draft.Autostart = !p.Autostart
			}
			statePath := filepath.Join(m.Root, "state.json")
			backup := filepath.Join(m.Root, "saved-state")
			injected := false
			m.Run = func(context.Context, string, ...string) (string, error) {
				if !injected {
					if e := os.Rename(statePath, backup); e != nil {
						t.Fatal(e)
					}
					if e := os.Mkdir(statePath, 0700); e != nil {
						t.Fatal(e)
					}
					injected = true
				}
				return "", errors.New("Could not find service")
			}
			if e = m.SaveProject(ctx, draft); e == nil {
				t.Fatal("failed publication accepted")
			}
			after, e := os.ReadFile(m.loginPath(p.ID))
			if errors.Is(readErr, os.ErrNotExist) {
				if !errors.Is(e, os.ErrNotExist) {
					t.Fatal("failed save enabled a login job", e)
				}
			} else if e != nil || string(after) != string(before) {
				t.Fatal("failed save changed login preference", e)
			}
			if e = os.Remove(statePath); e != nil {
				t.Fatal(e)
			}
			if e = os.Rename(backup, statePath); e != nil {
				t.Fatal(e)
			}
			persisted, e := os.ReadFile(statePath)
			if e != nil || string(persisted) != string(oldState) {
				t.Fatal("original project was not preserved", e)
			}
		})
	}
}

func projectSaveGuardFixture(t *testing.T) (*Manager, Project) {
	t.Helper()
	m := testManager(t)
	m.LoginDir = t.TempDir()
	m.Program = filepath.Join(t.TempDir(), "fixture-app")
	if e := os.WriteFile(m.Program, []byte("#!/bin/sh\n"), 0700); e != nil {
		t.Fatal(e)
	}
	p := Project{SchemaVersion: Schema, ID: "save-guard", Name: "Saved", Directory: t.TempDir(), Autostart: true, Services: []Service{{ID: "web", Command: []string{"/bin/echo", "fixture"}}}}
	if e := m.SaveProject(context.Background(), p); e != nil {
		t.Fatal(e)
	}
	return m, p
}
func TestProjectSaveRejectsUnknownState(t *testing.T) {
	for _, change := range []string{"rename-service", "remove-service", "autostart"} {
		for _, failure := range []error{errors.New("observation denied"), context.DeadlineExceeded} {
			t.Run(change+"-"+failure.Error(), func(t *testing.T) {
				m, p := projectSaveGuardFixture(t)
				statePath := filepath.Join(m.Root, "state.json")
				before, e := os.ReadFile(statePath)
				if e != nil {
					t.Fatal(e)
				}
				login, e := os.ReadFile(m.loginPath(p.ID))
				if e != nil {
					t.Fatal(e)
				}
				jobs, e := os.ReadDir(filepath.Join(m.Root, "jobs"))
				if e != nil {
					t.Fatal(e)
				}
				m.Run = func(_ context.Context, name string, args ...string) (string, error) {
					if name != "launchctl" || args[0] != "print" {
						t.Fatal("unexpected mutation", name, args)
					}
					return "", failure
				}
				draft := p.Clone()
				switch change {
				case "rename-service":
					draft.Services[0].ID = "renamed"
				case "remove-service":
					draft.Services = nil
				case "autostart":
					draft.Autostart = false
				}
				if e = m.SaveProject(context.Background(), draft); e == nil {
					t.Fatal("accepted unknown state")
				}
				after, e := os.ReadFile(statePath)
				if e != nil || string(after) != string(before) {
					t.Fatal("state changed", e)
				}
				after, e = os.ReadFile(m.loginPath(p.ID))
				if e != nil || string(after) != string(login) {
					t.Fatal("login configuration changed", e)
				}
				afterJobs, e := os.ReadDir(filepath.Join(m.Root, "jobs"))
				if e != nil || len(afterJobs) != len(jobs) {
					t.Fatal("jobs changed", e)
				}
			})
		}
	}
}
func TestProjectSaveAllowsKnownStoppedState(t *testing.T) {
	m, p := projectSaveGuardFixture(t)
	m.Run = func(_ context.Context, name string, args ...string) (string, error) {
		if name != "launchctl" || args[0] != "print" {
			t.Fatal("unexpected mutation", name, args)
		}
		return "", errors.New("Could not find service")
	}
	p.Name = "Changed"
	if e := m.SaveProject(context.Background(), p); e != nil {
		t.Fatal(e)
	}
	s, e := m.Read()
	if e != nil || s.Projects[p.ID].Name != "Changed" {
		t.Fatal("known stopped save failed", e)
	}
}
func TestProjectSaveStateGuardMatrix(t *testing.T) {
	for _, scenario := range []string{"marker-loaded", "service-loaded", "stopped", "new", "same", "cancelled"} {
		t.Run(scenario, func(t *testing.T) {
			m := testManager(t)
			m.LoginDir = t.TempDir()
			p := Project{SchemaVersion: Schema, ID: "matrix", Name: "Saved", Directory: t.TempDir(), Services: []Service{{ID: "web", Command: []string{"/bin/echo"}}}}
			if scenario != "new" {
				if e := m.SaveProject(context.Background(), p); e != nil {
					t.Fatal(e)
				}
			}
			calls := 0
			m.Run = func(_ context.Context, name string, args ...string) (string, error) {
				calls++
				if name != "launchctl" || args[0] != "print" {
					t.Fatal("unexpected mutation", name, args)
				}
				if scenario == "marker-loaded" || scenario == "service-loaded" && strings.HasSuffix(args[len(args)-1], m.label("p-"+p.ID+"-web")) {
					return "running", nil
				}
				return "", errors.New("Could not find service")
			}
			if scenario != "same" {
				p.Name = "Changed"
			}
			ctx := context.Background()
			if scenario == "cancelled" {
				var cancel context.CancelFunc
				ctx, cancel = context.WithCancel(ctx)
				cancel()
			}
			e := m.SaveProject(ctx, p)
			reject := scenario == "marker-loaded" || scenario == "service-loaded" || scenario == "cancelled"
			if (e != nil) != reject {
				t.Fatal("wrong save result", e)
			}
			if (scenario == "new" || scenario == "same" || scenario == "cancelled") && calls != 0 {
				t.Fatal("queried unnecessary state", calls)
			}
			if reject {
				s, readErr := m.Read()
				if readErr != nil || s.Projects[p.ID].Name != "Saved" {
					t.Fatal("rejected save persisted", readErr)
				}
			}
		})
	}
}

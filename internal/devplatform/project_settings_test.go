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

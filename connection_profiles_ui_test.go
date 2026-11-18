package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"devbase/internal/devplatform"
	"github.com/egoist/mygo/ui"
)

func profileUIFixture(t *testing.T) *app {
	t.Helper()
	a := designFixture(t)
	a.dev.tab = devTabDatabases
	a.dev.state.ConnectionProfiles = map[string]devplatform.ConnectionProfile{
		"one": {ID: "one", InstanceID: "atlas-postgres", Name: "Application", Database: "app", Username: "alice"},
	}
	a.dev.state.Instances["second"] = devplatform.Instance{ID: "second", Kind: "mysql", Port: 3307}
	a.loadConnectionProfileDraft("atlas-postgres", "")
	return a
}

func TestUIConnectionProfiles(t *testing.T) {
	t.Run("selection", func(t *testing.T) {
		a := profileUIFixture(t)
		a.chooseConnectionProfile("atlas-postgres", "one")
		if a.dev.profileSelected != "one" || a.dev.profileDraft.Username != "alice" {
			t.Fatal("profile not selected")
		}
		a.dev.profileDraft.Username = "changed"
		a.chooseConnectionProfile("atlas-postgres", "")
		if a.dev.profilePendingSelect != "" || a.dev.profileSelected != "one" {
			t.Fatal("dirty draft was overwritten")
		}
		a.loadConnectionProfileDraft("second", "")
		if a.dev.profileDraft.Username != "" {
			t.Fatal("other instance inherited profile")
		}
	})
	t.Run("credentials", func(t *testing.T) {
		a := profileUIFixture(t)
		a.chooseConnectionProfile("atlas-postgres", "one")
		tt := ui.NewTester(a.view, 1120, 720)
		tt.Frame()
		if _, ok := tt.Find("Copy database password"); ok {
			t.Fatal("business profile exposed admin password")
		}
		if _, ok := tt.Find("Copy connection URI"); !ok {
			t.Fatal("missing business URI")
		}
		a.loadConnectionProfileDraft("atlas-postgres", "")
		tt.Frame()
		if _, ok := tt.Find("Copy database password"); !ok {
			t.Fatal("admin option missing")
		}
	})
	t.Run("drafts", func(t *testing.T) {
		a := profileUIFixture(t)
		a.dev.profileOffer = &connectionOperationOffer{InstanceID: "atlas-postgres", Kind: "postgresql", Database: "created", Username: "created_user", OperationID: 7}
		a.dev.dbID = "second"
		tt := ui.NewTester(a.view, 1120, 720)
		tt.Frame()
		if _, ok := tt.Find("Use created database/user in connection profile"); ok {
			t.Fatal("offer leaked to other instance")
		}
		a.dev.dbID = "atlas-postgres"
		a.dev.dbKind = "postgresql"
		tt.Frame()
		if _, ok := tt.Find("Use created database/user in connection profile"); !ok {
			t.Fatal("original offer missing")
		}
		if err := tt.Click("Use created database/user in connection profile"); err != nil {
			t.Fatal(err)
		}
		if a.dev.profileDraft.Database != "created" || a.dev.profileDraft.Username != "created_user" || a.dev.profileSelected != "new" {
			t.Fatal("offer did not prefill original snapshot")
		}
		if a.dev.profileOffer != nil {
			t.Fatal("offer not consumed")
		}
	})
	t.Run("async", func(t *testing.T) {
		a := profileUIFixture(t)
		root := t.TempDir()
		state := a.dev.state
		data, err := json.Marshal(state)
		if err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(root, "state.json"), data, 0600); err != nil {
			t.Fatal(err)
		}
		// Use saved metadata without starting a database or invoking a real command.
		a.dev.managerFactory = func() *devplatform.Manager {
			x := devplatform.New(root)
			x.Run = func(context.Context, string, ...string) (string, error) { return "", errors.New("mocked command") }
			return x
		}
		posted := make(chan func(), 128)
		a.post = func(f func()) { posted <- f }
		a.spawn = func(f func()) { go f() }
		release := make(chan struct{})
		original := connectionOperationOffer{InstanceID: "atlas-postgres", Kind: "postgresql", Database: "start_db", Username: "start_user"}
		a.startDatabaseOperationWithOffer(a.dev.state.Instances["atlas-postgres"], "create user", func(*devplatform.Manager) (string, error) { <-release; return "done", nil }, &original)
		a.dev.dbID = "second"
		a.dev.dbName = "changed_db"
		a.dev.dbUser = "changed_user"
		a.dev.dbPassword = "changed_password"
		close(release)
		deadline := time.After(5 * time.Second)
		for a.dev.databaseTasks["atlas-postgres"].Running {
			select {
			case f := <-posted:
				f()
			case <-deadline:
				t.Fatal("task callback timeout")
			}
		}
		if a.dev.profileOffer == nil || a.dev.profileOffer.Database != "start_db" || a.dev.profileOffer.Username != "start_user" {
			t.Fatal("late callback lost the original operation snapshot")
		}
		if original.Database != "start_db" || original.Username != "start_user" {
			t.Fatal("operation snapshot mutated")
		}
		drain := func() {
			deadline := time.After(5 * time.Second)
			for a.dev.databaseTasks["atlas-postgres"].Running {
				select {
				case f := <-posted:
					f()
				case <-deadline:
					t.Fatal("task callback timeout")
				}
			}
		}
		a.dev.profileOffer = nil
		a.dev.busy = false
		a.startDatabaseOperationWithOffer(a.dev.state.Instances["atlas-postgres"], "create user", func(*devplatform.Manager) (string, error) {
			return "", errors.New("fixture failure")
		}, &original)
		drain()
		if a.dev.profileOffer != nil {
			t.Fatal("failed operation offered connection prefill")
		}
		cancelRelease := make(chan struct{})
		a.dev.busy = false
		a.startDatabaseOperationWithOffer(a.dev.state.Instances["atlas-postgres"], "create user", func(*devplatform.Manager) (string, error) {
			<-cancelRelease
			return "done", nil
		}, &original)
		a.dev.databaseTasks["atlas-postgres"].cancel()
		close(cancelRelease)
		drain()
		if a.dev.profileOffer != nil {
			t.Fatal("cancelled operation offered connection prefill")
		}
	})
	t.Run("layouts", func(t *testing.T) {
		for _, tc := range []struct {
			dark          bool
			width, height int
		}{{false, 900, 520}, {true, 900, 520}, {false, 1120, 720}, {true, 1120, 720}} {
			a := profileUIFixture(t)
			profile := devplatform.ConnectionProfile{ID: "one", InstanceID: "atlas-postgres", Name: "A very long application connection profile name", Database: "long database with spaces", Username: "long_user_name"}
			expectedField := "Profile database"
			if tc.dark && tc.width == 900 {
				instance := a.dev.state.Instances["atlas-postgres"]
				instance.Kind = "mongodb"
				a.dev.state.Instances[instance.ID] = instance
				a.dev.dbKind = instance.Kind
				profile.MongoAuthSource = "long database with spaces"
				expectedField = "Mongo authSource"
			} else if !tc.dark && tc.width == 1120 {
				instance := a.dev.state.Instances["atlas-postgres"]
				instance.Kind = "redis"
				a.dev.state.Instances[instance.ID] = instance
				a.dev.dbKind = instance.Kind
				profile.Database, profile.Username, profile.RedisDB = "", "", 15
				expectedField = "Profile Redis database (0–15)"
			}
			a.dev.state.ConnectionProfiles["one"] = profile
			a.loadConnectionProfileDraft("atlas-postgres", "one")
			tt := ui.NewTester(a.view, tc.width, tc.height)
			tt.SetDark(tc.dark)
			tt.Frame()
			if _, ok := tt.Find(expectedField); !ok {
				t.Fatal("profile editor inaccessible")
			}
			if tc.height == 520 {
				tt.Scroll(float32(tc.width-20), float32(tc.height/2), 0, 420)
				tt.Frame()
			}
			if _, ok := tt.Find("Save connection profile"); !ok {
				t.Fatal("save action inaccessible")
			}
			saveShot(t, fmt.Sprintf("%s-%dx%d", map[bool]string{false: "light", true: "dark"}[tc.dark], tc.width, tc.height), tt.Image())
			if strings.Contains(strings.Join(tt.Texts(), "\n"), "business-password-sentinel") {
				t.Fatal("secret shown")
			}
		}
	})
}

func TestUIInstanceInventoryBackupSelectorAndConnectionFormats(t *testing.T) {
	h := newDevUIHarnessSize(t, 1120, 780, false)
	inst := devplatform.Instance{ID: "app-pg", Kind: "postgresql", Package: "postgresql@17.11_1-arm64", Port: 55432, Persistent: true}
	m := h.a.dev.managerFactory()
	if err := os.MkdirAll(filepath.Join(h.root, "instances", inst.ID), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(h.root, "instances", inst.ID, "password"), []byte("super-secret-pg-pw"), 0o600); err != nil {
		t.Fatal(err)
	}
	state := devplatform.State{
		SchemaVersion: devplatform.Schema,
		Packages:      map[string]devplatform.Package{},
		Instances:     map[string]devplatform.Instance{inst.ID: inst},
		ConnectionProfiles: map[string]devplatform.ConnectionProfile{
			"app": {ID: "app", InstanceID: inst.ID, Name: "App", Database: "orders", Username: "svc"},
		},
		ProxyRules:        []devplatform.ProxyRule{},
		ProxyListenerPort: 443,
	}
	b, err := json.Marshal(state)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(h.root, "state.json"), b, 0o600); err != nil {
		t.Fatal(err)
	}

	origFactory := h.a.dev.managerFactory
	h.a.dev.managerFactory = func() *devplatform.Manager {
		mgr := origFactory()
		mgr.Run = func(_ context.Context, name string, args ...string) (string, error) {
			if name == "launchctl" && len(args) > 0 && args[0] == "print" {
				return "state = running\n", nil
			}
			return "", nil
		}
		mgr.InspectCatalog = func(context.Context, devplatform.Instance) (devplatform.InstanceCatalog, error) {
			return devplatform.InstanceCatalog{
				Databases: []string{"analytics", "orders", "postgres"},
				Users:     []string{"devmanager", "svc"},
			}, nil
		}
		return mgr
	}
	_ = m

	h.a.dev.state = state
	h.a.dev.statuses = map[string]string{"instance:app-pg": "Running"}
	h.a.openInstance(inst.ID)
	h.ui.Frame()
	h.drain(func() bool { return h.a.dev.dbCatalogFor == inst.ID })
	h.waitIdle()

	// Database pills, user tags, and Target database selector should render.
	for _, want := range []string{"analytics", "orders", "svc", "Target database", "Refresh databases"} {
		if !h.ui.HasText(want) {
			t.Fatalf("missing %q in running instance view: %v", want, h.ui.Texts())
		}
	}

	// Clicking a database pill selects it for DB name, .env, and JDBC URL copy.
	h.click("analytics")
	if h.a.dev.dbName != "analytics" {
		t.Fatalf("expected dbName=analytics after pill click, got %q", h.a.dev.dbName)
	}
	h.click("Copy .env")
	if len(h.copied) == 0 || h.copied[len(h.copied)-1] != "DATABASE_URL=postgresql://devmanager@127.0.0.1:55432/analytics" {
		t.Fatalf("unexpected copied .env: %v", h.copied)
	}
	h.click("Copy JDBC URL")
	if len(h.copied) == 0 || h.copied[len(h.copied)-1] != "jdbc:postgresql://127.0.0.1:55432/analytics?user=devmanager" {
		t.Fatalf("unexpected copied JDBC URL: %v", h.copied)
	}

	// Business profile uses profile DB and user without exposing admin password.
	h.click("App")
	h.click("Copy .env")
	if h.copied[len(h.copied)-1] != "DATABASE_URL=postgresql://svc@127.0.0.1:55432/orders" {
		t.Fatalf("unexpected profile .env: %v", h.copied)
	}
	h.click("Copy JDBC URL")
	if h.copied[len(h.copied)-1] != "jdbc:postgresql://127.0.0.1:55432/orders?user=svc" {
		t.Fatalf("unexpected profile JDBC URL: %v", h.copied)
	}
	for _, s := range h.copied {
		if strings.Contains(s, "super-secret-pg-pw") {
			t.Fatalf("copied string leaked password: %q", s)
		}
	}

	// Selecting from Target database updates d.dbName.
	h.choose("Target database", "orders")
	if h.a.dev.dbName != "orders" {
		t.Fatalf("expected dbName=orders after choosing Target database, got %q", h.a.dev.dbName)
	}
}

func TestUISmartEnvOpenCLIAndDropGuards(t *testing.T) {
	t.Run("smart-env-and-open-cli", func(t *testing.T) {
		for _, tc := range []struct {
			kind    string
			tool    string
			wantEnv string
			hasCLI  bool
		}{
			{kind: "postgresql", tool: "psql", wantEnv: "DATABASE_URL=postgresql://", hasCLI: true},
			{kind: "mysql", tool: "mysql", wantEnv: "DATABASE_URL=mysql://", hasCLI: true},
			{kind: "redis", tool: "redis-cli", wantEnv: "REDIS_URL=redis://", hasCLI: true},
			{kind: "mongodb", wantEnv: "MONGODB_URI=mongodb://", hasCLI: false},
		} {
			t.Run(tc.kind, func(t *testing.T) {
				h := newDevUIHarnessSize(t, 1120, 780, false)
				pkg := devplatform.Package{Name: "cli-fixture-" + tc.kind, Version: "1.0", Arch: "universal"}
				if tc.tool != "" {
					pkg.Binaries = map[string]string{tc.tool: tc.tool}
					pkgDir := filepath.Join(h.root, "packages", pkg.ID())
					if err := os.MkdirAll(pkgDir, 0o700); err != nil {
						t.Fatal(err)
					}
					if err := os.WriteFile(filepath.Join(pkgDir, tc.tool), []byte("#!/bin/sh\nexit 0\n"), 0o700); err != nil {
						t.Fatal(err)
					}
				}
				inst := devplatform.Instance{ID: "inst-" + tc.kind, Kind: tc.kind, Package: pkg.ID(), Port: 54321, Persistent: true}
				instDir := filepath.Join(h.root, "instances", inst.ID)
				if err := os.MkdirAll(instDir, 0o700); err != nil {
					t.Fatal(err)
				}
				const pw = "ui-secret-password"
				if err := os.WriteFile(filepath.Join(instDir, "password"), []byte(pw), 0o600); err != nil {
					t.Fatal(err)
				}
				if tc.kind == "mysql" {
					if err := os.WriteFile(filepath.Join(instDir, "client.cnf"), []byte("[client]\nuser=root\npassword=\"ui-secret-password\"\n"), 0o600); err != nil {
						t.Fatal(err)
					}
				}
				state := devplatform.State{
					SchemaVersion:      devplatform.Schema,
					Packages:           map[string]devplatform.Package{pkg.ID(): pkg},
					Instances:          map[string]devplatform.Instance{inst.ID: inst},
					ConnectionProfiles: map[string]devplatform.ConnectionProfile{},
					ProxyRules:         []devplatform.ProxyRule{},
					ProxyListenerPort:  443,
				}
				raw, err := json.Marshal(state)
				if err != nil {
					t.Fatal(err)
				}
				if err := os.WriteFile(filepath.Join(h.root, "state.json"), raw, 0o600); err != nil {
					t.Fatal(err)
				}

				var openCalls [][]string
				baseFactory := h.a.dev.managerFactory
				h.a.dev.managerFactory = func() *devplatform.Manager {
					m := baseFactory()
					m.Run = func(_ context.Context, name string, args ...string) (string, error) {
						if name == "/usr/bin/open" {
							openCalls = append(openCalls, append([]string{name}, args...))
							return "", nil
						}
						return "", nil
					}
					return m
				}

				h.a.dev.state = state
				h.a.openInstance(inst.ID)
				h.ui.Frame()

				h.click("Copy .env")
				if len(h.copied) == 0 || !strings.HasPrefix(h.copied[len(h.copied)-1], tc.wantEnv) {
					t.Fatalf("%s copied .env = %v, want prefix %q", tc.kind, h.copied, tc.wantEnv)
				}
				if !tc.hasCLI {
					if h.ui.HasText("Open CLI in Terminal") {
						t.Fatal("mongodb should not show Open CLI in Terminal")
					}
					return
				}
				if !h.ui.HasText("Open CLI in Terminal") {
					t.Fatalf("%s missing Open CLI in Terminal", tc.kind)
				}
				h.click("Open CLI in Terminal")
				if len(openCalls) != 1 || len(openCalls[0]) != 4 || openCalls[0][1] != "-a" || openCalls[0][2] != "Terminal" {
					t.Fatalf("unexpected open calls: %v", openCalls)
				}
				scriptBytes, err := os.ReadFile(openCalls[0][3])
				if err != nil {
					t.Fatal(err)
				}
				if strings.Contains(string(scriptBytes), pw) || strings.Contains(h.a.dev.output, pw) {
					t.Fatal("password leaked in CLI script or output")
				}
			})
		}
	})

	t.Run("drop-database-and-drop-user-dialogs-and-guards", func(t *testing.T) {
		h := newDevUIHarnessSize(t, 1120, 900, false)
		pkg := devplatform.Package{Name: "drop-fixture", Version: "1.0", Arch: "universal", Binaries: map[string]string{"psql": "psql"}}
		pkgDir := filepath.Join(h.root, "packages", pkg.ID())
		if err := os.MkdirAll(pkgDir, 0o700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(pkgDir, "psql"), []byte("#!/bin/sh\nexit 0\n"), 0o700); err != nil {
			t.Fatal(err)
		}
		inst := devplatform.Instance{ID: "app-pg", Kind: "postgresql", Package: pkg.ID(), Port: 55432, Persistent: true}
		instDir := filepath.Join(h.root, "instances", inst.ID)
		if err := os.MkdirAll(instDir, 0o700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(instDir, "password"), []byte("pg-admin-secret"), 0o600); err != nil {
			t.Fatal(err)
		}
		state := devplatform.State{
			SchemaVersion:      devplatform.Schema,
			Packages:           map[string]devplatform.Package{pkg.ID(): pkg},
			Instances:          map[string]devplatform.Instance{inst.ID: inst},
			ConnectionProfiles: map[string]devplatform.ConnectionProfile{},
			ProxyRules:         []devplatform.ProxyRule{},
			ProxyListenerPort:  443,
		}
		raw, _ := json.Marshal(state)
		if err := os.WriteFile(filepath.Join(h.root, "state.json"), raw, 0o600); err != nil {
			t.Fatal(err)
		}

		catalogLoads := 0
		baseFactory := h.a.dev.managerFactory
		h.a.dev.managerFactory = func() *devplatform.Manager {
			m := baseFactory()
			m.Run = func(_ context.Context, name string, args ...string) (string, error) {
				if name == "launchctl" && len(args) > 0 && args[0] == "print" {
					return "state = running\n", nil
				}
				return "", nil
			}
			m.InspectCatalog = func(context.Context, devplatform.Instance) (devplatform.InstanceCatalog, error) {
				catalogLoads++
				return devplatform.InstanceCatalog{
					Databases: []string{"orders", "postgres"},
					Users:     []string{"alice", "devmanager"},
				}, nil
			}
			return m
		}

		h.a.dev.state = state
		h.a.dev.statuses = map[string]string{"instance:app-pg": "Running"}
		h.a.openInstance(inst.ID)
		h.ui.Frame()
		h.drain(func() bool { return h.a.dev.dbCatalogFor == inst.ID })
		h.waitIdle()
		initialLoads := catalogLoads

		// System DB and admin user keep drop buttons disabled.
		h.a.dev.dbName = "postgres"
		h.a.dev.dbUser = "devmanager"
		h.ui.Frame()
		_ = h.ui.Click("Drop database")
		h.ui.Frame()
		if h.a.ask.open {
			t.Fatal("Drop database opened dialog for system DB postgres")
		}
		_ = h.ui.Click("Drop user")
		h.ui.Frame()
		if h.a.ask.open {
			t.Fatal("Drop user opened dialog for admin user devmanager")
		}

		// Selecting non-system DB & user enables confirmation dialog and reloads catalog after confirm.
		h.click("orders")
		h.click("Drop database")
		if !h.a.ask.open || h.a.ask.button != "Drop Database" {
			t.Fatalf("Drop database did not ask confirmation: %+v", h.a.ask)
		}
		h.click("Cancel")
		if h.a.ask.open || catalogLoads != initialLoads {
			t.Fatal("Cancel did not dismiss dialog cleanly")
		}
		h.click("Drop database")
		h.click("Drop Database")
		h.drain(func() bool { return catalogLoads > initialLoads && h.a.dev.dbCatalogFor == inst.ID })
		h.waitIdle()
		if !strings.Contains(h.a.dev.output, "Dropped database orders") {
			t.Fatalf("unexpected drop db output: %q", h.a.dev.output)
		}

		loadsBeforeUserDrop := catalogLoads
		h.click("alice")
		if h.a.dev.dbUser != "alice" {
			t.Fatalf("clicking user pill set dbUser = %q", h.a.dev.dbUser)
		}
		h.click("Drop user")
		if !h.a.ask.open || h.a.ask.button != "Drop User" {
			t.Fatalf("Drop user did not ask confirmation: %+v", h.a.ask)
		}
		h.click("Drop User")
		h.drain(func() bool { return catalogLoads > loadsBeforeUserDrop && h.a.dev.dbCatalogFor == inst.ID })
		h.waitIdle()
		if !strings.Contains(h.a.dev.output, "Dropped user alice") {
			t.Fatalf("unexpected drop user output: %q", h.a.dev.output)
		}
	})

	t.Run("viewport-reachability", func(t *testing.T) {
		for _, tc := range []struct {
			dark   bool
			w, h   int
		}{
			{false, 900, 520},
			{true, 900, 520},
			{false, 1120, 720},
			{true, 1120, 720},
		} {
			h := newDevUIHarnessSize(t, tc.w, tc.h, tc.dark)
			inst := devplatform.Instance{ID: "app-pg", Kind: "postgresql", Package: "postgresql@17.11_1-arm64", Port: 55432, Persistent: true}
			h.a.dev.state.Instances = map[string]devplatform.Instance{inst.ID: inst}
			h.a.openInstance(inst.ID)
			h.ui.Frame()
			for _, label := range []string{"Open CLI in Terminal", "Drop database", "Drop user", "Restore target database", "Restore into new database"} {
				reachable := false
				for n := 0; n < 18; n++ {
					r, ok := h.ui.Find(label)
					if ok && r.W > 0 && r.H > 0 && r.Y >= barHeight && r.Y+r.H < float32(tc.h-36) {
						reachable = true
						break
					}
					h.ui.Scroll(float32(tc.w-20), float32(tc.h/2), 0, 180)
					h.ui.Frame()
				}
				if !reachable {
					t.Fatalf("%dx%d dark=%t: control %q unreachable", tc.w, tc.h, tc.dark, label)
				}
				// Scroll back to top before searching for the next control.
				for n := 0; n < 18; n++ {
					h.ui.Scroll(float32(tc.w-20), float32(tc.h/2), 0, -180)
					h.ui.Frame()
				}
			}
		}
	})
}

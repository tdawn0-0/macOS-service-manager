package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"image/png"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/egoist/mygo/ui"
	"devbase/internal/devplatform"
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
			if dir := os.Getenv("MSM_RESOURCE_SHOTS"); dir != "" {
				if err := os.MkdirAll(dir, 0755); err != nil {
					t.Fatal(err)
				}
				theme := "light"
				if tc.dark {
					theme = "dark"
				}
				name := fmt.Sprintf("%s-%dx%d.png", theme, tc.width, tc.height)
				f, err := os.Create(filepath.Join(dir, name))
				if err != nil {
					t.Fatal(err)
				}
				if err = png.Encode(f, tt.Image()); err != nil {
					f.Close()
					t.Fatal(err)
				}
				if err = f.Close(); err != nil {
					t.Fatal(err)
				}
			}
			if strings.Contains(strings.Join(tt.Texts(), "\n"), "business-password-sentinel") {
				t.Fatal("secret shown")
			}
		}
	})
}

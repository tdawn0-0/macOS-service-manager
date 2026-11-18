package main

import (
	"context"
	"devbase/internal/devplatform"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"
)

func TestUIBackgroundDatabaseBackupKeepsConfigurationAndIndependentInstance(t *testing.T) {
	for _, cancelled := range []bool{false, true} {
		t.Run(fmt.Sprintf("cancel-%t", cancelled), func(t *testing.T) {
			h, m, _, id, redis := newUIDeletionInstance(t, cancelled)
			entered, release := make(chan struct{}), make(chan struct{})
			var once, signal sync.Once
			unblock := func() { once.Do(func() { close(release) }) }
			base := h.a.dev.managerFactory
			h.a.dev.managerFactory = func() *devplatform.Manager {
				worker := base()
				observer := worker.Event
				worker.Event = func(s string) {
					if observer != nil {
						observer(s)
					}
					if s == "Using database "+id {
						signal.Do(func() { close(entered) })
						select {
						case <-release:
						case <-worker.Context.Done():
						}
					}
				}
				return worker
			}
			t.Cleanup(func() {
				unblock()
				h.a.cancelBackgroundTasks()
				h.foregroundOnly = false
				h.waitIdle()
				_ = m.InstanceAction(context.Background(), id, "remove")
				_ = m.InstanceAction(context.Background(), "independent", "remove")
			})
			h.fill("Database number (0–15)", "0")
			h.foregroundOnly = true
			h.click("Back up database")
			select {
			case <-entered:
			case <-time.After(5 * time.Second):
				t.Fatal("backup did not begin")
			}
			if h.a.dev.busy {
				t.Fatal("backup occupied foreground UI")
			}
			configureUIInitialization(t, h, "independent", freeUIPort(t))
			h.click("Create instance")
			waitUIPackageCondition(t, h, func() bool {
				return !h.a.databaseTaskRunning("independent") && !h.a.dev.busy && !h.a.dev.databaseNeedsRefresh
			})
			h.click("Start")
			waitUIPackageCondition(t, h, func() bool {
				return !h.a.databaseTaskRunning("independent") && !h.a.dev.busy && !h.a.dev.databaseNeedsRefresh
			})
			h.fill("Database number (0–15)", "0")
			h.click("Back up database")
			waitUIPackageCondition(t, h, func() bool {
				return !h.a.databaseTaskRunning("independent") && !h.a.dev.busy && !h.a.dev.databaseNeedsRefresh
			})
			independent := h.a.dev.databaseTasks["independent"].Result
			if _, e := os.Stat(independent); e != nil {
				t.Fatal("independent backup failed", independent, e)
			}
			if !h.a.databaseTaskRunning(id) {
				t.Fatal("database backups did not overlap")
			}
			h.click("HTTPS")
			h.fill("HTTPS listener port", fmt.Sprint(freeUIPort(t)))
			h.click("Add rule")
			h.fill("Domain", "during-backup.devbase.localhost")
			h.fill("Local port", "3000")
			timer := time.AfterFunc(4*time.Second, unblock)
			h.click("Save HTTPS rules")
			timer.Stop()
			state, e := m.Read()
			if e != nil || len(state.ProxyRules) != 1 || !h.a.databaseTaskRunning(id) {
				t.Fatal("backup blocked configuration", state, e)
			}
			h.fill("Domain", "unsaved-backup.devbase.localhost")
			if cancelled {
				h.click("Database tasks")
				h.click("Cancel database operation " + id)
			} else {
				unblock()
			}
			h.foregroundOnly = false
			h.waitIdle()
			if h.a.dev.proxyDraft[0].Domain != "unsaved-backup.devbase.localhost" {
				t.Fatal("background refresh replaced draft")
			}
			if redis("GET", "value") != "42" {
				t.Fatal("backup cancellation affected source data")
			}
			h.click("Databases")
			h.click("Instance " + id)
			if cancelled {
				if !strings.Contains(h.a.dev.databaseTasks[id].Result, "did not complete") {
					t.Fatal("cancel did not report failure")
				}
				files, _ := filepath.Glob(filepath.Join(h.root, "backups", id+"-*.dump"))
				if len(files) != 0 {
					t.Fatal("cancelled backup published output", files)
				}
				unblock()
				h.click("Back up database")
			}
			backup := h.a.dev.databaseTasks[id].Result
			if _, e = os.Stat(backup); e != nil {
				t.Fatal(backup, e)
			}
			h.fill("Database number (0–15)", "1")
			h.fill("Backup file", backup)
			h.click("Restore into new database")
			if redis("-n", "1", "GET", "value") != "42" {
				t.Fatal("background backup did not restore actual data")
			}
			h.click("Stop")
			h.click("Instance independent")
			h.click("Stop")
		})
	}
}

func TestUITwoPostgreSQLInstancesKeepSameNamedDatabasesIsolated(t *testing.T) {
	if os.Getenv("MSM_UI_E2E_HOMEBREW") != "1" {
		t.Skip("set MSM_UI_E2E_HOMEBREW=1 with a preinstalled PostgreSQL Homebrew keg")
	}
	h := newDevUIHarnessSize(t, 900, 520, true)
	pkg := seedUIPackage(t, h, "postgresql", "17")
	h.click("Software")
	h.click("Software postgresql")
	h.click("Review package")
	h.click("Install with Homebrew")
	m := h.a.dev.managerFactory()
	ids := []string{"first-pg", "second-pg"}
	t.Cleanup(func() {
		h.a.cancelBackgroundTasks()
		h.foregroundOnly = false
		h.waitIdle()
		for _, id := range ids {
			_ = m.InstanceAction(context.Background(), id, "remove")
		}
	})
	ports := map[string]int{}
	passwords := map[string]string{}
	for n, id := range ids {
		port := freeUIPort(t)
		if n == 1 {
			for port == ports[ids[0]] {
				port = freeUIPort(t)
			}
		}
		ports[id] = port
		h.click("Databases")
		h.click("New database instance")
		h.fill("Instance name", id)
		h.choose("Database engine", "postgresql")
		h.fill("Local port", fmt.Sprint(port))
		h.click("Create instance")
		if _, ok := h.a.dev.state.Instances[id]; !ok {
			files, _ := filepath.Glob(filepath.Join(h.root, "logs", "*.log"))
			for _, path := range files {
				b, _ := os.ReadFile(path)
				t.Log(filepath.Base(path), string(b))
			}
			t.Fatal("PostgreSQL initialization failed", h.a.dev.output)
		}
		h.click("Start")
		h.fill("Database name", "same_db")
		h.click("Create database")
		h.fill("Database user", "app_user")
		h.fill("New user password", "isolated-e2e-user-password")
		h.click("Create database user")
		pw, e := os.ReadFile(filepath.Join(h.root, "instances", id, "password"))
		if e != nil {
			t.Fatal(e)
		}
		passwords[id] = string(pw)
	}
	if passwords[ids[0]] == passwords[ids[1]] {
		t.Fatal("instances share administrator credentials")
	}
	query := func(id, db, user, password, sql string) string {
		t.Helper()
		c := exec.Command(filepath.Join(m.PackageDirectory(pkg), pkg.Binaries["psql"]), "-h", "127.0.0.1", "-p", fmt.Sprint(ports[id]), "-U", user, "-d", db, "--set", "ON_ERROR_STOP=1", "-Atc", sql)
		c.Env = append(os.Environ(), "PGPASSWORD="+password)
		b, e := c.CombinedOutput()
		if e != nil {
			files, _ := filepath.Glob(filepath.Join(h.root, "logs", "*.log"))
			for _, path := range files {
				b, _ := os.ReadFile(path)
				t.Log(filepath.Base(path), string(b))
			}
			t.Fatal("isolated PostgreSQL query failed", e, string(b))
		}
		return strings.TrimSpace(string(b))
	}
	for n, id := range ids {
		query(id, "same_db", "app_user", "isolated-e2e-user-password", fmt.Sprintf("CREATE TABLE sample(value integer); INSERT INTO sample VALUES(%d)", 42+n*42))
	}
	value := func(id, db string) string {
		return query(id, db, "devmanager", passwords[id], "SELECT value FROM sample")
	}
	if value(ids[0], "same_db") != "42" || value(ids[1], "same_db") != "84" {
		t.Fatal("same-named databases were not isolated")
	}
	h.click("Instance " + ids[0])
	h.fill("Database name", "same_db")
	h.click("Back up database")
	backup := h.a.dev.output
	if _, e := os.Stat(backup); e != nil {
		t.Fatal("backup was not published", backup, e)
	}
	h.fill("Database name", "restored_db")
	h.fill("Backup file", backup)
	h.click("Restore into new database")
	if value(ids[0], "restored_db") != "42" || value(ids[1], "same_db") != "84" {
		t.Fatal("restore affected another instance")
	}
	h.click("Stop")
	if value(ids[1], "same_db") != "84" {
		t.Fatal("stopping one instance affected the other")
	}
	h.click("Remove instance (keep data files)")
	if _, e := os.Stat(filepath.Join(h.root, "instances", ids[0], "data")); e != nil {
		t.Fatal("removal discarded the original data", e)
	}
	h.click("Instance " + ids[1])
	h.click("Restart")
	if value(ids[1], "same_db") != "84" {
		t.Fatal("independent instance lost data on restart")
	}
	h.click("Stop")
	h.click("Remove instance (keep data files)")
}

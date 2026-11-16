package main

import (
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"devbase/internal/devplatform"
)

func installUIInitializationRedis(t *testing.T, h *devUIHarness) devplatform.Package {
	t.Helper()
	pkg := seedUIPackage(t, h, "redis", "8.2")
	h.click("Software")
	h.click("Software redis")
	h.click("Review package")
	h.click("Download and install")
	return pkg
}
func pauseUIInitialization(t *testing.T, h *devUIHarness, id string) (<-chan struct{}, func()) {
	t.Helper()
	entered, release := make(chan struct{}), make(chan struct{})
	var signal, once sync.Once
	unblock := func() { once.Do(func() { close(release) }) }
	t.Cleanup(unblock)
	base := h.a.dev.managerFactory
	h.a.dev.managerFactory = func() *devplatform.Manager {
		m := base()
		previous := m.Event
		m.Event = func(s string) {
			if previous != nil {
				previous(s)
			}
			if s == "Initializing database "+id {
				signal.Do(func() { close(entered) })
				select {
				case <-release:
				case <-m.Context.Done():
				}
			}
		}
		return m
	}
	return entered, unblock
}
func configureUIInitialization(t *testing.T, h *devUIHarness, id string, port int) {
	t.Helper()
	h.click("Databases")
	h.click("New database instance")
	h.fill("Instance name", id)
	h.choose("Database engine", "redis")
	h.fill("Local port", fmt.Sprint(port))
}
func TestUIDatabaseInitializationKeepsIndependentDrafts(t *testing.T) {
	if os.Getenv("MSM_UI_E2E_PACKAGE_CACHE") == "" {
		t.Skip("set MSM_UI_E2E_PACKAGE_CACHE to verified Redis archive")
	}
	for _, dark := range []bool{false, true} {
		t.Run(fmt.Sprintf("minimum-dark-%t", dark), func(t *testing.T) {
			h := newDevUIHarnessSize(t, 900, 520, dark)
			pkg := installUIInitializationRedis(t, h)
			m := h.a.dev.managerFactory()
			port := freeUIPort(t)
			second := freeUIPort(t)
			for second == port {
				second = freeUIPort(t)
			}
			entered, release := pauseUIInitialization(t, h, "first-bg")
			configureUIInitialization(t, h, "first-bg", port)
			h.foregroundOnly = true
			h.click("Create instance")
			select {
			case <-entered:
			case <-time.After(5 * time.Second):
				t.Fatal("initialization did not start")
			}
			if h.a.dev.busy {
				t.Fatal("initialization occupied foreground")
			}
			configureUIInitialization(t, h, "second-bg", second)
			h.click("Create instance")
			waitUIPackageCondition(t, h, func() bool {
				return !h.a.dev.databaseTasks["second-bg"].Running && !h.a.dev.busy && !h.a.dev.databaseNeedsRefresh
			})
			if !h.a.dev.databaseTasks["first-bg"].Running {
				t.Fatal("independent initialization did not overlap")
			}
			h.click("HTTPS")
			h.fill("HTTPS listener port", fmt.Sprint(freeUIPort(t)))
			h.click("Add rule")
			h.fill("Domain", "during-initialization.devbase.localhost")
			h.fill("Local port", "3000")
			timer := time.AfterFunc(4*time.Second, release)
			h.click("Save HTTPS rules")
			timer.Stop()
			state, e := m.Read()
			if e != nil || len(state.ProxyRules) != 1 || !h.a.dev.databaseTasks["first-bg"].Running {
				t.Fatal(state, e)
			}
			h.fill("Domain", "unsaved-initialization.devbase.localhost")
			release()
			h.foregroundOnly = false
			h.waitIdle()
			if h.a.dev.proxyDraft[0].Domain != "unsaved-initialization.devbase.localhost" || len(h.a.dev.state.Instances) != 2 {
				t.Fatal("completion replaced draft or state")
			}
			h.click("Databases")
			h.click("Instance first-bg")
			h.click("Start")
			i := h.a.dev.state.Instances["first-bg"]
			pw, _ := os.ReadFile(filepath.Join(h.root, "instances", i.ID, "password"))
			c := exec.Command(filepath.Join(h.root, "packages", pkg.ID(), pkg.Binaries["redis-cli"]), "-p", fmt.Sprint(i.Port), "PING")
			c.Env = append(os.Environ(), "REDISCLI_AUTH="+string(pw))
			if b, e := c.CombinedOutput(); e != nil || strings.TrimSpace(string(b)) != "PONG" {
				t.Fatal("published configuration points to staging", string(b), e)
			}
			h.click("Stop")
			for _, name := range []string{"first-bg", "second-bg"} {
				if e := m.InstanceAction(context.Background(), name, "remove"); e != nil {
					t.Fatal(e)
				}
			}
		})
	}
}
func TestUIDatabaseInitializationCallerCrashHelper(t *testing.T) {
	root := os.Getenv("MSM_UI_INITIALIZATION_ROOT")
	if root == "" {
		return
	}
	m := devplatform.New(root)
	s, _ := m.Read()
	var pkg string
	for id, p := range s.Packages {
		if p.Name == "redis" {
			pkg = id
		}
	}
	port := 0
	fmt.Sscanf(os.Getenv("MSM_UI_INITIALIZATION_PORT"), "%d", &port)
	m.Event = func(s string) {
		if s == os.Getenv("MSM_UI_INITIALIZATION_POINT") {
			os.Exit(23)
		}
	}
	if os.Getenv("MSM_UI_INITIALIZATION_RECONCILE") == "1" {
		rows, e := m.PendingInstanceInitializations(context.Background())
		if e == nil && len(rows) == 1 {
			_, _ = m.ResolveInstanceInitialization(context.Background(), rows[0].ID, rows[0].Revision)
		}
		os.Exit(26)
	}
	_ = m.CreateInstance(context.Background(), devplatform.Instance{ID: "crash-init", Kind: "redis", Package: pkg, Port: port, Persistent: true})
	os.Exit(26)
}
func TestUIDatabaseInitializationActualCrashArchiveOrFinish(t *testing.T) {
	if os.Getenv("MSM_UI_E2E_PACKAGE_CACHE") == "" {
		t.Skip("set MSM_UI_E2E_PACKAGE_CACHE to verified Redis archive")
	}
	for n, point := range []string{"Database initialization data published", "Database initialization committed"} {
		t.Run(point, func(t *testing.T) {
			h := newDevUIHarnessSize(t, 900, 520, n == 1)
			installUIInitializationRedis(t, h)
			m := h.a.dev.managerFactory()
			c := exec.Command(os.Args[0], "-test.run=^TestUIDatabaseInitializationCallerCrashHelper$")
			c.Env = append(os.Environ(), "MSM_UI_INITIALIZATION_ROOT="+h.root, "MSM_UI_INITIALIZATION_PORT="+fmt.Sprint(freeUIPort(t)), "MSM_UI_INITIALIZATION_POINT="+point)
			var exit *exec.ExitError
			if e := c.Run(); !errors.As(e, &exit) || exit.ExitCode() != 23 {
				t.Fatal(e)
			}
			var row devplatform.InstanceInitializationInfo
			deadline := time.Now().Add(5 * time.Second)
			for {
				rows, e := m.PendingInstanceInitializations(context.Background())
				if e != nil || len(rows) != 1 {
					t.Fatal(rows, e)
				}
				row = rows[0]
				if !row.Busy {
					break
				}
				if time.Now().After(deadline) {
					t.Fatal(row)
				}
				time.Sleep(20 * time.Millisecond)
			}
			password, e := os.ReadFile(filepath.Join(row.Path, "password"))
			if e != nil {
				t.Fatal(e)
			}
			h.click("Refresh")
			h.click("Databases")
			h.click("Unfinished initializations")
			if n == 0 {
				h.click("Stop and archive data " + row.ID[:8])
				archive := filepath.Join(h.root, "retained-initializations", row.ID)
				if got, e := os.ReadFile(filepath.Join(archive, "password")); e != nil || string(got) != string(password) {
					t.Fatal("archive changed credentials", e)
				}
				if _, e := os.Stat(filepath.Join(h.root, "instances", "crash-init")); !os.IsNotExist(e) {
					t.Fatal(e)
				}
			} else {
				h.click("Finish registration " + row.ID[:8])
				if got, e := os.ReadFile(filepath.Join(h.root, "instances", "crash-init", "password")); e != nil || string(got) != string(password) {
					t.Fatal("finish changed registered data", e)
				}
				_ = m.InstanceAction(context.Background(), "crash-init", "remove")
			}
			if rows, e := m.PendingInstanceInitializations(context.Background()); e != nil || len(rows) != 0 {
				t.Fatal(rows, e)
			}
		})
	}
}
func TestUIDatabaseInitializationCancellationKeepsAnotherPreparation(t *testing.T) {
	if os.Getenv("MSM_UI_E2E_PACKAGE_CACHE") == "" {
		t.Skip("set MSM_UI_E2E_PACKAGE_CACHE to verified Redis archive")
	}
	h := newDevUIHarnessSize(t, 900, 520, true)
	installUIInitializationRedis(t, h)
	first, _ := pauseUIInitialization(t, h, "cancel-first")
	second, release := pauseUIInitialization(t, h, "keep-second")
	p1, p2 := freeUIPort(t), freeUIPort(t)
	for p1 == p2 {
		p2 = freeUIPort(t)
	}
	configureUIInitialization(t, h, "cancel-first", p1)
	h.foregroundOnly = true
	h.click("Create instance")
	select {
	case <-first:
	case <-time.After(5 * time.Second):
		t.Fatal("first did not pause")
	}
	configureUIInitialization(t, h, "keep-second", p2)
	h.click("Create instance")
	select {
	case <-second:
	case <-time.After(5 * time.Second):
		t.Fatal("second did not pause")
	}
	h.click("Cancel initialization cancel-first")
	waitUIPackageCondition(t, h, func() bool {
		return !h.a.dev.databaseTasks["cancel-first"].Running && !h.a.dev.busy && !h.a.dev.databaseNeedsRefresh
	})
	if !h.a.dev.databaseTasks["keep-second"].Running {
		t.Fatal("independent initialization was cancelled")
	}
	release()
	h.foregroundOnly = false
	h.waitIdle()
	if len(h.a.dev.state.Instances) != 1 {
		t.Fatal(h.a.dev.state.Instances)
	}
	h.click("Unfinished initializations")
	records := h.a.dev.initializations
	if len(records) != 1 || records[0].InstanceID != "cancel-first" {
		t.Fatal(records)
	}
	h.click("Stop and archive data " + records[0].ID[:8])
	_ = h.a.dev.managerFactory().InstanceAction(context.Background(), "keep-second", "remove")
}

func TestUIDatabaseInitializationQuitWaitsAndCancellationKeepsPreparedData(t *testing.T) {
	if os.Getenv("MSM_UI_E2E_PACKAGE_CACHE") == "" {
		t.Skip("set MSM_UI_E2E_PACKAGE_CACHE to verified Redis archive")
	}
	h := newDevUIHarnessSize(t, 900, 520, true)
	installUIInitializationRedis(t, h)
	entered, _ := pauseUIInitialization(t, h, "quit-init")
	configureUIInitialization(t, h, "quit-init", freeUIPort(t))
	h.foregroundOnly = true
	h.click("Create instance")
	select {
	case <-entered:
	case <-time.After(5 * time.Second):
		t.Fatal("initialization did not pause")
	}
	calls := 0
	h.a.quitApp = func() { calls++ }
	h.a.requestQuit()
	h.ui.Frame()
	if !h.a.quitOperationBusy() {
		t.Fatal("quit ignored initialization")
	}
	if e := h.ui.Click("Keep services and quit"); e != nil {
		t.Fatal(e)
	}
	h.ui.Frame()
	if calls != 0 {
		t.Fatal("quit abandoned initialization")
	}
	h.click("Cancel background tasks")
	h.foregroundOnly = false
	h.waitIdle()
	if h.a.databaseTasksRunning() || len(h.a.dev.state.Instances) != 0 {
		t.Fatal("cancelled initialization was registered")
	}
	h.click("Cancel quit")
	h.click("Database tasks")
	h.click("Unfinished initializations")
	rows := h.a.dev.initializations
	if len(rows) != 1 || rows[0].InstanceID != "quit-init" {
		t.Fatal("cancel lost its recovery entry", rows)
	}
	if _, e := os.Stat(filepath.Join(rows[0].Path, "password")); e != nil {
		t.Fatal("cancel lost prepared data", e)
	}
	h.click("Stop and archive data " + rows[0].ID[:8])
}

func TestUIDatabaseInitializationArchiveCrashFinishesWhileReusedInstanceRuns(t *testing.T) {
	if os.Getenv("MSM_UI_E2E_PACKAGE_CACHE") == "" {
		t.Skip("set MSM_UI_E2E_PACKAGE_CACHE to verified Redis archive")
	}
	for n, point := range []string{"Database initialization data archived", "Database initialization archive committed"} {
		t.Run(point, func(t *testing.T) {
			h := newDevUIHarnessSize(t, 900, 520, n == 1)
			pkg := installUIInitializationRedis(t, h)
			m := h.a.dev.managerFactory()
			port := freeUIPort(t)
			runChild := func(point string, reconcile bool) {
				t.Helper()
				c := exec.Command(os.Args[0], "-test.run=^TestUIDatabaseInitializationCallerCrashHelper$")
				c.Env = append(os.Environ(), "MSM_UI_INITIALIZATION_ROOT="+h.root, "MSM_UI_INITIALIZATION_PORT="+fmt.Sprint(port), "MSM_UI_INITIALIZATION_POINT="+point)
				if reconcile {
					c.Env = append(c.Env, "MSM_UI_INITIALIZATION_RECONCILE=1")
				}
				var exit *exec.ExitError
				if e := c.Run(); !errors.As(e, &exit) || exit.ExitCode() != 23 {
					t.Fatal("child did not exit at the requested stage", e)
				}
			}
			runChild("Database initialization data published", false)
			runChild(point, true)
			rows, e := m.PendingInstanceInitializations(context.Background())
			if e != nil || len(rows) != 1 || rows[0].Status != "Archived" {
				t.Fatal(rows, e)
			}
			row := rows[0]
			oldPassword, e := os.ReadFile(filepath.Join(row.Path, "password"))
			if e != nil {
				t.Fatal(e)
			}
			h.click("Refresh")
			configureUIInitialization(t, h, "crash-init", port)
			h.click("Create instance")
			h.click("Start")
			password, e := os.ReadFile(filepath.Join(h.root, "instances", "crash-init", "password"))
			if e != nil || string(password) == string(oldPassword) {
				t.Fatal("new instance did not receive independent credentials", e)
			}
			h.click("Unfinished initializations")
			if !strings.Contains(strings.Join(h.ui.Texts(), "\n"), "Archived") {
				t.Fatal("archive completion is not visible", h.ui.Texts())
			}
			h.click("Finish archive " + row.ID[:8])
			if rows, e = m.PendingInstanceInitializations(context.Background()); e != nil || len(rows) != 0 {
				t.Fatal(rows, e)
			}
			got, e := os.ReadFile(filepath.Join(row.Path, "password"))
			if e != nil || string(got) != string(oldPassword) {
				t.Fatal("archive completion changed the old files", e)
			}
			c := exec.Command(filepath.Join(h.root, "packages", pkg.ID(), pkg.Binaries["redis-cli"]), "-p", fmt.Sprint(port), "PING")
			c.Env = append(os.Environ(), "REDISCLI_AUTH="+string(password))
			if b, e := c.CombinedOutput(); e != nil || strings.TrimSpace(string(b)) != "PONG" {
				t.Fatal("archive completion affected the new running instance", string(b), e)
			}
			h.click("Back to database settings")
			h.click("Instance crash-init")
			h.click("Stop")
		})
	}
}

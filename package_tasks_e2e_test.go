package main

import (
	"bufio"
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"macos-service-manager/internal/devplatform"
)

func pauseUIPackage(t *testing.T, h *devUIHarness, id string) (<-chan struct{}, func()) {
	t.Helper()
	entered, release := make(chan struct{}), make(chan struct{})
	var signal, unblock sync.Once
	stop := func() { unblock.Do(func() { close(release) }) }
	t.Cleanup(stop)
	base := h.a.dev.managerFactory
	h.a.dev.managerFactory = func() *devplatform.Manager {
		m := base()
		observer := m.Event
		m.Event = func(s string) {
			if observer != nil {
				observer(s)
			}
			if s == "Downloading "+id {
				signal.Do(func() { close(entered) })
				select {
				case <-release:
				case <-m.Context.Done():
				}
			}
		}
		return m
	}
	return entered, stop
}
func waitUIPackageCondition(t *testing.T, h *devUIHarness, condition func() bool) {
	t.Helper()
	deadline := time.NewTimer(15 * time.Second)
	defer deadline.Stop()
	for !condition() {
		select {
		case f := <-h.posted:
			f()
			h.ui.Frame()
		case <-deadline.C:
			t.Fatal("UI package condition did not finish", h.a.dev.output)
		}
	}
}
func seedUIPackage(t *testing.T, h *devUIHarness, name, version string) devplatform.Package {
	t.Helper()
	p, e := devplatform.Resolve(context.Background(), name, version)
	if e != nil {
		t.Fatal(e)
	}
	seedRuntimeCache(t, h.root, p)
	return p
}
func importUIPackageProject(t *testing.T, h *devUIHarness) string {
	t.Helper()
	dir := t.TempDir()
	for name, value := range map[string]string{".nvmrc": "20.19.0\n", "package.json": `{"scripts":{"dev":"node app.js"}}`} {
		if e := os.WriteFile(filepath.Join(dir, name), []byte(value), 0600); e != nil {
			t.Fatal(e)
		}
	}
	h.click("Projects")
	h.fill("Project folder", dir)
	h.click("Import project")
	return h.a.dev.id
}
func TestUIBackgroundPackagesAllowConfigurationAndKeepDrafts(t *testing.T) {
	if os.Getenv("MSM_UI_E2E_PACKAGE_CACHE") == "" {
		t.Skip("set MSM_UI_E2E_PACKAGE_CACHE to verified Node and Redis archives")
	}
	for _, dark := range []bool{false, true} {
		t.Run(fmt.Sprintf("minimum-dark-%t", dark), func(t *testing.T) {
			h := newDevUIHarnessSize(t, 900, 520, dark)
			node := seedUIPackage(t, h, "node", "20.19.0")
			redis := seedUIPackage(t, h, "redis", "7.4.7")
			id := importUIPackageProject(t, h)
			entered, release := pauseUIPackage(t, h, node.ID())
			h.click("Choose Node.js 20.19.0 from .nvmrc")
			h.click("Review package")
			h.foregroundOnly = true
			h.click("Download and install")
			select {
			case <-entered:
			case <-time.After(5 * time.Second):
				t.Fatal("background package task did not start")
			}
			if h.a.dev.busy || !h.a.packageTaskRunning(node.ID()) {
				t.Fatal("download still occupies the foreground")
			}
			h.click("Software redis")
			h.click("Review package")
			h.click("Download and install")
			if !h.a.packageTaskRunning(node.ID()) || !h.a.packageTaskRunning(redis.ID()) {
				t.Fatal("independent package tasks did not overlap")
			}
			h.click("Projects")
			h.click("Overview")
			h.fill("Project name", "Saved during downloads")
			// A short watchdog releases the simulated slow transfer if a save regresses
			// to holding the global lock; the test can then fail without hanging cleanup.
			unblockTimeout := time.AfterFunc(4*time.Second, release)
			h.click("Save project")
			unblockTimeout.Stop()
			state, e := h.a.dev.managerFactory().Read()
			if e != nil || state.Projects[id].Name != "Saved during downloads" || !h.a.packageTaskRunning(node.ID()) {
				t.Fatal("save waited for or lost the running download", state, e)
			}
			h.fill("Project name", "Unsaved draft survives")
			external := state.Projects[id].Clone()
			external.Name = "Saved in another caller"
			if e := h.a.dev.managerFactory().SaveProject(context.Background(), external); e != nil {
				t.Fatal(e)
			}
			release()
			h.foregroundOnly = false
			h.waitIdle()
			if h.a.dev.draft.Name != "Unsaved draft survives" || h.a.dev.state.Projects[id].Name != external.Name {
				t.Fatal("background refresh replaced the draft or lost external state", h.a.dev.draft.Name, h.a.dev.state.Projects[id].Name)
			}
			if len(h.a.dev.state.Packages) != 2 || h.a.dev.servicePackage != node.ID() {
				t.Fatal("completion did not reconcile both versions and the unchanged runtime choice", h.a.dev.servicePackage)
			}
			h.click("Save project")
			state, e = h.a.dev.managerFactory().Read()
			if e != nil || state.Projects[id].Name != "Unsaved draft survives" || state.Projects[id].Services[0].Package != node.ID() || len(state.Packages) != 2 {
				t.Fatal("final save lost concurrent changes", state, e)
			}
		})
	}
}
func TestUIBackgroundCompletionDoesNotOverrideLaterRuntimeChoice(t *testing.T) {
	if os.Getenv("MSM_UI_E2E_PACKAGE_CACHE") == "" {
		t.Skip("set MSM_UI_E2E_PACKAGE_CACHE to verified Node archives")
	}
	h := newDevUIHarnessSize(t, 900, 520, true)
	node20 := seedUIPackage(t, h, "node", "20.19.0")
	node22 := seedUIPackage(t, h, "node", "22.14.0")
	h.click("Software")
	h.fill("Exact version", "22.14.0")
	h.click("Review package")
	h.click("Download and install")
	id := importUIPackageProject(t, h)
	entered, release := pauseUIPackage(t, h, node20.ID())
	h.click("Choose Node.js 20.19.0 from .nvmrc")
	h.click("Review package")
	h.foregroundOnly = true
	h.click("Download and install")
	select {
	case <-entered:
	case <-time.After(5 * time.Second):
		t.Fatal("first runtime download did not start")
	}
	// Explicitly choose an already installed version after the first request.
	h.fill("Exact version", "22.14.0")
	h.click("Review package")
	h.click("Use installed version")
	waitUIPackageCondition(t, h, func() bool {
		return !h.a.packageTaskRunning(node22.ID()) && !h.a.dev.busy && !h.a.dev.packageNeedsRefresh
	})
	h.click("Projects")
	h.click("Save project")
	if h.a.dev.state.Projects[id].Services[0].Package != node22.ID() {
		t.Fatal("later selected version was not saved")
	}
	release()
	h.foregroundOnly = false
	h.waitIdle()
	if h.a.dev.servicePackage != node22.ID() || h.a.dev.draft.Services[0].Package != node22.ID() || h.a.dev.state.Projects[id].Services[0].Package != node22.ID() {
		t.Fatal("old background completion replaced a later runtime choice", h.a.dev.servicePackage)
	}
	out, e := h.a.dev.managerFactory().Exec(context.Background(), id, []string{"node", "--version"})
	if e != nil || out != "v22.14.0\n" {
		t.Fatal("saved project executed a different runtime", out, e)
	}
}
func TestUIQuitCanCancelBackgroundPackageBeforeLeaving(t *testing.T) {
	if os.Getenv("MSM_UI_E2E_PACKAGE_CACHE") == "" {
		t.Skip("set MSM_UI_E2E_PACKAGE_CACHE to verified Redis archive")
	}
	h := newDevUIHarnessSize(t, 900, 520, false)
	pkg := seedUIPackage(t, h, "redis", "7.4.7")
	entered, release := pauseUIPackage(t, h, pkg.ID())
	h.click("Software")
	h.click("Software redis")
	h.click("Review package")
	h.foregroundOnly = true
	h.click("Download and install")
	select {
	case <-entered:
	case <-time.After(5 * time.Second):
		t.Fatal("package did not pause")
	}
	calls := 0
	h.a.quitApp = func() { calls++ }
	h.a.requestQuit()
	h.ui.Frame()
	if !h.a.quitOperationBusy() {
		t.Fatal("quit ignored background work")
	}
	if e := h.ui.Click("Keep services and quit"); e != nil {
		t.Fatal(e)
	}
	h.ui.Frame()
	if calls != 0 {
		t.Fatal("quit abandoned background work")
	}
	h.click("Cancel background tasks")
	h.foregroundOnly = false
	h.waitIdle()
	if h.a.packageTasksRunning() {
		t.Fatal("cancel did not finish package work")
	}
	if _, ok := h.a.dev.state.Packages[pkg.ID()]; ok {
		t.Fatal("cancelled package was installed")
	}
	h.click("Cancel quit")
	if h.a.quit.open {
		t.Fatal("quit dialog did not close")
	}
	h.click("Package tasks")
	if !strings.Contains(strings.Join(h.ui.Texts(), "\n"), "Install did not complete") {
		t.Fatal("cancelled result was not visible", h.ui.Texts())
	}
	release()
	h.click("Review package")
	h.click("Download and install")
	if _, ok := h.a.dev.state.Packages[pkg.ID()]; !ok {
		t.Fatal("cancelled install could not be retried", h.a.dev.output)
	}
}

func TestUIPackageOtherCallerHelper(t *testing.T) {
	root := os.Getenv("MSM_UI_PACKAGE_CALLER_ROOT")
	if root == "" {
		return
	}
	m := devplatform.New(root)
	p, e := devplatform.Resolve(context.Background(), "redis", "7.4.7")
	if e != nil {
		os.Exit(25)
	}
	m.Event = func(s string) {
		if s == "Downloading "+p.ID() {
			fmt.Println("ready")
			_, _ = bufio.NewReader(os.Stdin).ReadString('\n')
		}
	}
	if e = m.Install(context.Background(), p); e != nil {
		os.Exit(26)
	}
	os.Exit(0)
}
func TestUIPackageOtherCallerBusyDetailsAndRetry(t *testing.T) {
	if os.Getenv("MSM_UI_E2E_PACKAGE_CACHE") == "" {
		t.Skip("set MSM_UI_E2E_PACKAGE_CACHE to verified Redis archive")
	}
	h := newDevUIHarnessSize(t, 900, 520, true)
	pkg := seedUIPackage(t, h, "redis", "7.4.7")
	c := exec.Command(os.Args[0], "-test.run=^TestUIPackageOtherCallerHelper$")
	c.Env = append(os.Environ(), "MSM_UI_PACKAGE_CALLER_ROOT="+h.root)
	in, e := c.StdinPipe()
	if e != nil {
		t.Fatal(e)
	}
	out, e := c.StdoutPipe()
	if e != nil {
		t.Fatal(e)
	}
	if e = c.Start(); e != nil {
		t.Fatal(e)
	}
	t.Cleanup(func() { in.Close(); _ = c.Process.Kill(); _ = c.Wait() })
	ready := make(chan bool, 1)
	go func() { scanner := bufio.NewScanner(out); ready <- scanner.Scan() && scanner.Text() == "ready" }()
	select {
	case ok := <-ready:
		if !ok {
			t.Fatal("caller did not acquire package")
		}
	case <-time.After(5 * time.Second):
		t.Fatal("caller did not reach preparation")
	}
	h.click("Software")
	h.click("Software redis")
	h.click("Review package")
	h.click("Download and install")
	if r := h.a.dev.packageContention; r == nil || r.PackageID != pkg.ID() {
		t.Fatal("UI did not identify the busy package", h.a.dev.output)
	}
	h.click("Review package tasks")
	var running devplatform.OperationRecord
	for _, r := range h.a.dev.operations {
		if r.Status == "Running" && r.Request.Resource == pkg.ID() {
			running = r
		}
	}
	if running.ID == "" || !running.Busy {
		t.Fatal("caller missing from shared task history", h.a.dev.operations)
	}
	h.click("Task " + running.ID)
	if !strings.Contains(strings.Join(h.ui.Texts(), "\n"), "Downloading") {
		t.Fatal("active preparation phase missing")
	}
	if _, e = in.Write([]byte("\n")); e != nil {
		t.Fatal(e)
	}
	in.Close()
	if e = c.Wait(); e != nil {
		t.Fatal(e)
	}
	h.click("Back to workspace")
	h.click("Software")
	h.click("Software redis")
	h.click("Review package")
	h.click("Use installed version")
	if _, ok := h.a.dev.state.Packages[pkg.ID()]; !ok || h.a.dev.packageContention != nil {
		t.Fatal("completed caller could not be retried", h.a.dev.output)
	}
}

func TestUIBackgroundCancelOnePackageKeepsOtherPackageRunning(t *testing.T) {
	if os.Getenv("MSM_UI_E2E_PACKAGE_CACHE") == "" {
		t.Skip("set MSM_UI_E2E_PACKAGE_CACHE to verified Node and Redis archives")
	}
	h := newDevUIHarnessSize(t, 900, 520, true)
	node := seedUIPackage(t, h, "node", "20.19.0")
	redis := seedUIPackage(t, h, "redis", "7.4.7")
	nodeEntered, _ := pauseUIPackage(t, h, node.ID())
	redisEntered, releaseRedis := pauseUIPackage(t, h, redis.ID())
	h.click("Software")
	h.fill("Exact version", "20.19.0")
	h.click("Review package")
	h.foregroundOnly = true
	h.click("Download and install")
	select {
	case <-nodeEntered:
	case <-time.After(5 * time.Second):
		t.Fatal("Node task did not pause")
	}
	h.click("Software redis")
	h.click("Review package")
	h.click("Download and install")
	select {
	case <-redisEntered:
	case <-time.After(5 * time.Second):
		t.Fatal("Redis task did not pause")
	}
	h.click("Software node")
	h.click("Cancel package " + node.ID())
	waitUIPackageCondition(t, h, func() bool {
		return !h.a.packageTaskRunning(node.ID()) && !h.a.dev.busy && !h.a.dev.packageNeedsRefresh
	})
	if !h.a.packageTaskRunning(redis.ID()) {
		t.Fatal("cancelling Node also cancelled Redis")
	}
	if _, ok := h.a.dev.state.Packages[node.ID()]; ok {
		t.Fatal("cancelled Node was registered")
	}
	releaseRedis()
	h.foregroundOnly = false
	h.waitIdle()
	if len(h.a.dev.state.Packages) != 1 {
		t.Fatal("independent completion lost its state", h.a.dev.state.Packages)
	}
	if _, ok := h.a.dev.state.Packages[redis.ID()]; !ok {
		t.Fatal("Redis did not finish after Node cancellation")
	}
	report, e := h.a.dev.managerFactory().VerifyPackage(context.Background(), redis.ID())
	if e != nil || report.Status != "Verified" {
		t.Fatal(report, e)
	}
}

package main

import (
	"bufio"
	"context"
	"fmt"
	"os"
	"os/exec"
	"strings"
	"sync"
	"testing"
	"time"

	"devbase/internal/devplatform"
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
func TestUIQuitCanCancelBackgroundPackageBeforeLeaving(t *testing.T) {
	if os.Getenv("MSM_UI_E2E_PACKAGE_CACHE") == "" {
		t.Skip("set MSM_UI_E2E_PACKAGE_CACHE to verified Redis archive")
	}
	h := newDevUIHarnessSize(t, 900, 520, false)
	pkg := seedUIPackage(t, h, "redis", "8.2")
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
	p, e := devplatform.Resolve(context.Background(), "redis", "8.2")
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
	pkg := seedUIPackage(t, h, "redis", "8.2")
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
	redis := seedUIPackage(t, h, "redis", "8.2")
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

package main

import (
	"context"
	"encoding/json"
	"fmt"
	"macos-service-manager/internal/devplatform"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"
	"unicode/utf8"
)

func TestUIProjectCommandCallerHelper(t *testing.T) {
	root := os.Getenv("MSM_UI_COMMAND_ROOT")
	if root == "" {
		return
	}
	_, _ = devplatform.New(root).Exec(context.Background(), "command-project", []string{"fixture", "private-runtime-value"})
	os.Exit(25)
}
func newUICommandFixture(t *testing.T, dark bool) (*devUIHarness, *devplatform.Manager, devplatform.Project) {
	t.Helper()
	h := newDevUIHarnessSize(t, 900, 520, dark)
	m := h.a.dev.managerFactory()
	s, e := m.Read()
	if e != nil {
		t.Fatal(e)
	}
	pkg := devplatform.Package{Name: "fixture", Version: "command", Arch: "arm64", Binaries: map[string]string{"fixture": "fixture", "npm": "fixture"}}
	dir := filepath.Join(h.root, "packages", pkg.ID())
	if e = os.MkdirAll(dir, 0700); e != nil {
		t.Fatal(e)
	}
	script := fmt.Sprintf("#!/usr/bin/python3\nimport os,pathlib,sys,time\nroot=pathlib.Path.cwd()\n(root/'entered').write_text('started')\n(root/'retained-source').write_text('42')\nprint('private command output',flush=True)\nwhile not (root/'release').exists():time.sleep(.02)\nprint('dependencies prepared',flush=True)\n")
	if e = os.WriteFile(filepath.Join(dir, "fixture"), []byte(script), 0700); e != nil {
		t.Fatal(e)
	}
	p := devplatform.Project{SchemaVersion: 1, ID: "command-project", Name: "Command project", Directory: t.TempDir(), Services: []devplatform.Service{{ID: "web", Package: pkg.ID(), Command: []string{"fixture"}}}}
	if e = os.WriteFile(filepath.Join(p.Directory, "package.json"), []byte(`{"name":"command-fixture","version":"1.0.0"}`), 0600); e != nil {
		t.Fatal(e)
	}
	s.Packages[pkg.ID()] = pkg
	b, _ := json.Marshal(s)
	if e = os.WriteFile(filepath.Join(h.root, "state.json"), b, 0600); e != nil {
		t.Fatal(e)
	}
	if e = m.SaveProject(context.Background(), p); e != nil {
		t.Fatal(e)
	}
	t.Cleanup(func() {
		h.a.cancelBackgroundTasks()
		h.foregroundOnly = false
		h.waitIdle()
		rows, _ := m.PendingProjectCommands(context.Background())
		for _, r := range rows {
			if r.CanResolve {
				_ = m.ResolveProjectCommand(context.Background(), r.ID, r.Revision)
			}
		}
	})
	h.click("Refresh")
	h.click("Projects")
	h.click("Project " + p.Name)
	return h, m, p
}
func waitUICommandTool(t *testing.T, p devplatform.Project) {
	t.Helper()
	deadline := time.Now().Add(6 * time.Second)
	for time.Now().Before(deadline) {
		if _, e := os.Stat(filepath.Join(p.Directory, "entered")); e == nil {
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatal("actual command did not start")
}
func TestUIBackgroundDependencyCommandKeepsIndependentConfigurationAndDraft(t *testing.T) {
	for _, dark := range []bool{false, true} {
		t.Run(fmt.Sprintf("minimum-dark-%t", dark), func(t *testing.T) {
			h, m, p := newUICommandFixture(t, dark)
			h.foregroundOnly = true
			h.click("Install project dependencies")
			waitUICommandTool(t, p)
			if h.a.dev.busy || !h.a.projectTaskRunning(p.ID) {
				t.Fatal("dependency command blocked foreground UI")
			}
			h.click("Import a project")
			h.fill("Project folder", t.TempDir())
			h.click("Import project")
			id := h.a.dev.id
			h.fill("Project name", "Saved while installing dependencies")
			h.click("Save project")
			s, e := m.Read()
			if e != nil || s.Projects[id].Name != "Saved while installing dependencies" {
				t.Fatal("command blocked independent save", e)
			}
			h.fill("Project name", "Unsaved dependency draft")
			h.click("Project tasks")
			h.click("Cancel project operation " + p.ID)
			waitUIPackageCondition(t, h, func() bool { return !h.a.projectTaskRunning(p.ID) && !h.a.dev.projectNeedsRefresh && !h.a.dev.busy })
			if h.a.dev.draft.Name != "Unsaved dependency draft" {
				t.Fatal("completion replaced newer draft")
			}
			if _, e = os.Stat(filepath.Join(p.Directory, "retained-source")); e != nil {
				t.Fatal("cancel removed dependency writes", e)
			}
			h.foregroundOnly = false
			h.click("Project " + p.Name)
			if !h.a.ask.open || h.a.ask.button != "Discard changes" || h.a.dev.draft.Name != "Unsaved dependency draft" {
				t.Fatal("dirty navigation did not preserve draft pending confirmation")
			}
			h.click("Discard changes")
			if e = os.WriteFile(filepath.Join(p.Directory, "release"), nil, 0600); e != nil {
				t.Fatal(e)
			}
			h.click("Install project dependencies")
			task := h.a.dev.projectTasks[p.ID]
			if task == nil || task.Running || !strings.Contains(task.Result, "dependencies prepared") {
				t.Fatal("dependency retry did not finish", h.a.dev.output, task)
			}
		})
	}
}
func TestUIProjectCommandCallerDeathCanStopOrFinishFromReview(t *testing.T) {
	for _, finish := range []bool{false, true} {
		t.Run(fmt.Sprintf("finish-%t", finish), func(t *testing.T) {
			h, m, p := newUICommandFixture(t, finish)
			c := exec.Command(os.Args[0], "-test.run=^TestUIProjectCommandCallerHelper$")
			c.Env = append(os.Environ(), "MSM_UI_COMMAND_ROOT="+h.root)
			if e := c.Start(); e != nil {
				t.Fatal(e)
			}
			t.Cleanup(func() { _ = c.Process.Kill(); _ = c.Wait() })
			waitUICommandTool(t, p)
			if e := c.Process.Kill(); e != nil {
				t.Fatal(e)
			}
			_ = c.Wait()
			h.click("Unfinished project commands")
			if len(h.a.dev.commandWork) != 1 {
				t.Fatal(h.a.dev.output, h.a.dev.commandWork)
			}
			row := h.a.dev.commandWork[0]
			if _, ok := h.ui.Find("Save project"); ok {
				t.Fatal("command review exposed an unrelated save action")
			}
			if row.CallerActive || !row.Running || !row.CanResolve {
				t.Fatal(row)
			}
			revealed := []string{}
			h.a.reveal = func(path string) { revealed = append(revealed, path) }
			h.click("Reveal private command files " + row.ID[:8])
			if len(revealed) == 0 || !strings.Contains(revealed[len(revealed)-1], "command-work") {
				t.Fatal("wrong private task folder", revealed)
			}
			if finish {
				if e := os.WriteFile(filepath.Join(p.Directory, "release"), nil, 0600); e != nil {
					t.Fatal(e)
				}
				deadline := time.Now().Add(8 * time.Second)
				for time.Now().Before(deadline) {
					h.click("Refresh project commands")
					row = h.a.dev.commandWork[0]
					if row.Status == "Succeeded" && !row.Running {
						break
					}
					time.Sleep(20 * time.Millisecond)
				}
				if row.Status != "Succeeded" {
					t.Fatal(row)
				}
			} else {
				paths, _ := filepath.Glob(filepath.Join(h.root, "jobs", "*command-"+row.ID+".plist"))
				if len(paths) != 1 {
					t.Fatal(paths)
				}
				original, e := os.ReadFile(paths[0])
				if e != nil {
					t.Fatal(e)
				}
				if e = os.WriteFile(paths[0], append(append([]byte{}, original...), []byte("\n<!-- changed -->")...), 0600); e != nil {
					t.Fatal(e)
				}
				h.click("Refresh project commands")
				if h.a.dev.commandWork[0].CanResolve {
					t.Fatal("UI offered stop for unknown ownership")
				}
				if e = os.WriteFile(paths[0], original, 0600); e != nil {
					t.Fatal(e)
				}
				h.click("Refresh project commands")
			}
			label := "Stop project command "
			if finish {
				label = "Finish project command "
			}
			h.click(label + row.ID[:8])
			if len(h.a.dev.commandWork) != 0 {
				t.Fatal("UI did not resolve command", h.a.dev.output)
			}
			if _, e := os.Stat(filepath.Join(p.Directory, "retained-source")); e != nil {
				t.Fatal("review removed source effects", e)
			}
			records, e := m.Operations(context.Background())
			if e != nil {
				t.Fatal(e)
			}
			found := false
			for _, r := range records {
				if r.Request.Kind == "exec" {
					found = true
					if r.Status != "Interrupted" {
						t.Fatal("review rewrote caller success", r)
					}
				}
			}
			if !found {
				t.Fatal("missing interrupted operation")
			}
			h.click("Project " + p.Name)
			if h.a.dev.commandWorkOpen {
				t.Fatal("instance navigation stayed in command review")
			}
		})
	}
}

func TestUIRealNPMDependencyCommandCancellationAndRetryUsesManagedNode(t *testing.T) {
	if os.Getenv("MSM_UI_E2E_PACKAGE_CACHE") == "" {
		t.Skip("set MSM_UI_E2E_PACKAGE_CACHE to verified Node archive")
	}
	h := newDevUIHarnessSize(t, 900, 520, true)
	pkg := seedUIPackage(t, h, "node", "22.14.0")
	h.click("Software")
	h.click("Software node")
	h.click("Review package")
	h.click("Download and install")
	m := h.a.dev.managerFactory()
	dir := t.TempDir()
	files := map[string]string{
		"package.json":      `{"name":"managed-npm-fixture","version":"1.0.0","scripts":{"postinstall":"node install-check.js"}}`,
		"package-lock.json": `{"name":"managed-npm-fixture","version":"1.0.0","lockfileVersion":3,"requires":true,"packages":{"":{"name":"managed-npm-fixture","version":"1.0.0","hasInstallScript":true}}}`,
		".npmrc":            "audit=false\nfund=false\nupdate-notifier=false\noffline=true\ncache=" + filepath.Join(dir, ".npm-cache") + "\n",
		"install-check.js":  `const fs=require('node:fs');fs.writeFileSync('postinstall-pid',String(process.pid));fs.writeFileSync('postinstall-effect','retained');console.log('postinstall '+process.version);const timer=setInterval(()=>{if(fs.existsSync('release')){clearInterval(timer);fs.writeFileSync('npm-result',process.version)}},20);`,
	}
	for name, data := range files {
		if e := os.WriteFile(filepath.Join(dir, name), []byte(data), 0600); e != nil {
			t.Fatal(e)
		}
	}
	p := devplatform.Project{SchemaVersion: 1, ID: "npm-project", Name: "Managed npm project", Directory: dir, Services: []devplatform.Service{{ID: "web", Package: pkg.ID(), Command: []string{"node", "--version"}}}}
	if e := m.SaveProject(context.Background(), p); e != nil {
		t.Fatal(e)
	}
	t.Cleanup(func() {
		h.a.cancelBackgroundTasks()
		h.foregroundOnly = false
		h.waitIdle()
		rows, _ := m.PendingProjectCommands(context.Background())
		for _, r := range rows {
			if r.CanResolve {
				_ = m.ResolveProjectCommand(context.Background(), r.ID, r.Revision)
			}
		}
	})
	h.click("Refresh")
	h.click("Projects")
	h.click("Project " + p.Name)
	h.foregroundOnly = true
	h.click("Install project dependencies")
	deadline := time.Now().Add(10 * time.Second)
	var childPID int
	for time.Now().Before(deadline) {
		b, e := os.ReadFile(filepath.Join(dir, "postinstall-pid"))
		if e == nil {
			childPID, _ = strconv.Atoi(string(b))
			if childPID > 0 {
				break
			}
		}
		time.Sleep(20 * time.Millisecond)
	}
	if childPID == 0 {
		t.Fatal("real npm lifecycle script did not start", h.a.dev.output)
	}
	if h.a.dev.busy {
		t.Fatal("real npm command occupied foreground UI")
	}
	h.click("Import a project")
	h.fill("Project folder", t.TempDir())
	h.click("Import project")
	h.fill("Project name", "Saved during real npm")
	h.click("Save project")
	h.fill("Project name", "Preserved npm draft")
	h.click("Project tasks")
	h.click("Cancel project operation " + p.ID)
	waitUIPackageCondition(t, h, func() bool { return !h.a.projectTaskRunning(p.ID) && !h.a.dev.projectNeedsRefresh && !h.a.dev.busy })
	deadline = time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) && syscall.Kill(childPID, 0) != syscall.ESRCH {
		time.Sleep(20 * time.Millisecond)
	}
	if syscall.Kill(childPID, 0) != syscall.ESRCH {
		t.Fatal("npm cancellation left its actual lifecycle child running", childPID)
	}
	if h.a.dev.draft.Name != "Preserved npm draft" {
		t.Fatal("npm completion replaced newer draft")
	}
	if _, e := os.Stat(filepath.Join(dir, "postinstall-effect")); e != nil {
		t.Fatal("cancellation erased package-script writes", e)
	}
	if e := os.WriteFile(filepath.Join(dir, "release"), nil, 0600); e != nil {
		t.Fatal(e)
	}
	h.foregroundOnly = false
	h.click("Project " + p.Name)
	h.click("Install project dependencies")
	result, e := os.ReadFile(filepath.Join(dir, "npm-result"))
	if e != nil || string(result) != "v22.14.0" {
		t.Fatal("npm retry did not use selected managed Node", string(result), e, h.a.dev.projectTasks[p.ID].Result)
	}
}

func TestUIProjectCommandResultBoundsLargeUnicodeOutput(t *testing.T) {
	h := newDevUIHarnessSize(t, 900, 520, true)
	p := devplatform.Project{SchemaVersion: 1, ID: "bounded-ui", Name: "Bounded command", Directory: t.TempDir()}
	raw := strings.Repeat("install-progress-😀\n", 4000) + "Final dependency status"
	h.a.startProjectOperation(p, "dependencies", func(*devplatform.Manager) (string, error) { return raw, nil })
	h.waitIdle()
	h.ui.Frame()
	task := h.a.dev.projectTasks[p.ID]
	if task == nil || task.Running || len(task.Result) > 17000 || !utf8.ValidString(task.Result) || !strings.HasSuffix(task.Result, "Final dependency status") || !strings.Contains(task.Result, "last 16 KiB") {
		t.Fatal("UI did not preserve a bounded readable tail", task)
	}
	if _, ok := h.ui.Find("Dismiss project result " + p.ID); !ok {
		t.Fatal("large command result hid its action")
	}
}

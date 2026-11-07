package devplatform

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"
)

func commandWorkFixture(t *testing.T) (*Manager, Package, Project, Project) {
	t.Helper()
	m := testManager(t)
	pkg := Package{Name: "fixture", Version: "command-test", Arch: "arm64", URL: "https://unused.invalid/command.zip", SHA256: strings.Repeat("a", 64), Format: "zip", Binaries: map[string]string{"fixture": "bin/fixture"}}
	script := `#!/usr/bin/python3
import os,pathlib,sys,time,subprocess
root=pathlib.Path.cwd()
child=subprocess.Popen(['/bin/sleep','30'])
(root/'source-effect').write_text('retained')
(root/'entered').write_text(str(os.getpid())+' '+str(child.pid))
print(sys.argv[1],flush=True)
while not (root/'release').exists():time.sleep(.02)
child.terminate();child.wait()
print('completed',flush=True)
`
	if e := os.MkdirAll(filepath.Join(m.packageDir(pkg), "bin"), 0700); e != nil {
		t.Fatal(e)
	}
	if e := os.WriteFile(filepath.Join(m.packageDir(pkg), "bin/fixture"), []byte(script), 0700); e != nil {
		t.Fatal(e)
	}
	a := Project{SchemaVersion: Schema, ID: "alpha", Name: "Alpha", Directory: t.TempDir(), Instances: []string{"linked"}, Services: []Service{{ID: "web", Package: pkg.ID(), Command: []string{"fixture"}}}}
	b := a.Clone()
	b.ID = "beta"
	b.Name = "Beta"
	b.Directory = t.TempDir()
	b.Instances = nil
	if e := m.withState(context.Background(), func(s *State) error {
		s.Packages[pkg.ID()] = pkg
		s.Instances["linked"] = Instance{ID: "linked", Kind: "redis", Package: pkg.ID(), Port: integrationPort(t)}
		return nil
	}); e != nil {
		t.Fatal(e)
	}
	for _, p := range []Project{a, b} {
		if e := m.SaveProject(context.Background(), p); e != nil {
			t.Fatal(e)
		}
	}
	t.Cleanup(func() {
		rows, _ := m.PendingProjectCommands(context.Background())
		for _, r := range rows {
			if r.CanResolve {
				if e := m.ResolveProjectCommand(context.Background(), r.ID, r.Revision); e != nil {
					t.Error("cleanup project command", e)
				}
			}
		}
	})
	return m, pkg, a, b
}
func waitCommandTool(t *testing.T, p Project) (int, int) {
	t.Helper()
	deadline := time.Now().Add(8 * time.Second)
	for time.Now().Before(deadline) {
		if b, e := os.ReadFile(filepath.Join(p.Directory, "entered")); e == nil {
			fields := strings.Fields(string(b))
			if len(fields) == 2 {
				a, _ := strconv.Atoi(fields[0])
				b, _ := strconv.Atoi(fields[1])
				if a > 0 && b > 0 {
					return a, b
				}
			}
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatal("command tool did not execute", p.ID)
	return 0, 0
}
func TestProjectCommandCallerHelper(t *testing.T) {
	root := os.Getenv("MSM_COMMAND_CRASH_ROOT")
	if root == "" {
		return
	}
	_, _ = New(root).Exec(context.Background(), "alpha", []string{"fixture", os.Getenv("MSM_COMMAND_SECRET")})
	os.Exit(27)
}
func crashCommandCaller(t *testing.T, m *Manager, p Project, secret string) (ProjectCommandInfo, int, int) {
	t.Helper()
	c := exec.Command(os.Args[0], "-test.run=^TestProjectCommandCallerHelper$")
	c.Env = append(os.Environ(), "MSM_COMMAND_CRASH_ROOT="+m.Root, "MSM_COMMAND_SECRET="+secret)
	if e := c.Start(); e != nil {
		t.Fatal(e)
	}
	t.Cleanup(func() { _ = c.Process.Kill(); _ = c.Wait() })
	tool, child := waitCommandTool(t, p)
	if e := c.Process.Kill(); e != nil {
		t.Fatal(e)
	}
	_ = c.Wait()
	rows, e := m.PendingProjectCommands(context.Background())
	deadline := time.Now().Add(3 * time.Second)
	for e == nil && len(rows) == 1 && rows[0].CallerActive && time.Now().Before(deadline) {
		time.Sleep(20 * time.Millisecond)
		rows, e = m.PendingProjectCommands(context.Background())
	}
	if e != nil || len(rows) != 1 || rows[0].CallerActive || !rows[0].Running || !rows[0].CanResolve {
		t.Fatal(rows, e)
	}
	return rows[0], tool, child
}
func TestProjectCommandCallerDeathRetainsResourcesAndPrivateOutputUntilReview(t *testing.T) {
	m, pkg, p, _ := commandWorkFixture(t)
	secret := "command-private-密钥😀 \"quoted\" " + randomID()
	r, _, _ := crashCommandCaller(t, m, p, secret)
	ctx := context.Background()
	dir, e := m.ProjectCommandFiles(r.ID)
	if e != nil {
		t.Fatal(e)
	}
	for _, name := range []string{"input.json", "output.log"} {
		info, e := os.Stat(filepath.Join(dir, name))
		if e != nil || info.Mode().Perm() != 0600 {
			t.Fatal("private task permissions", name, info, e)
		}
	}
	for _, path := range []string{filepath.Join(dir, "record.json"), filepath.Join(m.Root, "jobs", m.label("command-"+r.ID)+".plist")} {
		b, e := os.ReadFile(path)
		if e != nil || strings.Contains(string(b), secret) {
			t.Fatal("command input in metadata/job arguments", path, e)
		}
	}
	assertBusy := func(e error) {
		t.Helper()
		var busy *ResourceBusyError
		if !errors.As(e, &busy) {
			t.Fatal("active command did not reserve its resources", e)
		}
	}
	changed := p.Clone()
	changed.Name = "Not allowed"
	assertBusy(m.SaveProject(ctx, changed))
	assertBusy(m.Down(ctx, p.ID))
	assertBusy(m.RemoveProject(ctx, p.ID))
	assertBusy(m.InstanceAction(ctx, "linked", "remove"))
	if e = m.SaveProject(ctx, Project{SchemaVersion: Schema, ID: "unrelated", Name: "Independent saved", Directory: t.TempDir()}); e != nil {
		t.Fatal("command blocked independent settings", e)
	}
	if e = os.WriteFile(filepath.Join(p.Directory, "release"), nil, 0600); e != nil {
		t.Fatal(e)
	}
	deadline := time.Now().Add(8 * time.Second)
	for time.Now().Before(deadline) {
		rows, e := m.PendingProjectCommands(ctx)
		if e != nil || len(rows) != 1 {
			t.Fatal(rows, e)
		}
		r = rows[0]
		if r.Status == "Succeeded" && !r.Running {
			break
		}
		time.Sleep(20 * time.Millisecond)
	}
	if r.Status != "Succeeded" || !r.HasOutput {
		t.Fatal(r)
	}
	assertBusy(m.SaveProject(ctx, changed))
	if e = m.RepairPackage(ctx, pkg.ID()); e == nil || !strings.Contains(e.Error(), "in use") {
		t.Fatal("unreviewed command released its runtime", e)
	}
	if e = m.RunProjectCommandWorker(ctx, r.ID); e == nil {
		t.Fatal("replayed completed command")
	}
	if e = m.ResolveProjectCommand(ctx, r.ID, "stale"); e == nil {
		t.Fatal("accepted stale command review")
	}
	records, e := m.Operations(ctx)
	if e != nil {
		t.Fatal(e)
	}
	interrupted := false
	for _, record := range records {
		if record.Request.Kind == "exec" {
			if record.Status != "Interrupted" {
				t.Fatal("invented caller success", record)
			}
			interrupted = true
		}
		path, _, _ := m.operationFiles(record.ID)
		data, _ := os.ReadFile(path)
		if strings.Contains(string(data), secret) {
			t.Fatal("private command in history")
		}
	}
	if !interrupted {
		t.Fatal("caller crash not recorded")
	}
	if e = m.ResolveProjectCommand(ctx, r.ID, r.Revision); e != nil {
		t.Fatal(e)
	}
	if _, e = os.Stat(dir); !os.IsNotExist(e) {
		t.Fatal("review retained private inputs/output", e)
	}
	if b, e := os.ReadFile(filepath.Join(p.Directory, "source-effect")); e != nil || string(b) != "retained" {
		t.Fatal("review rolled back source files", e)
	}
	if e = m.SaveProject(ctx, changed); e != nil {
		t.Fatal("review retained project reservation", e)
	}
}
func TestProjectCommandOrphanStopChecksOwnershipAndEndsToolChildren(t *testing.T) {
	m, _, p, _ := commandWorkFixture(t)
	r, tool, child := crashCommandCaller(t, m, p, "private-"+randomID())
	ctx := context.Background()
	path := filepath.Join(m.Root, "jobs", m.label("command-"+r.ID)+".plist")
	original, e := os.ReadFile(path)
	if e != nil {
		t.Fatal(e)
	}
	t.Cleanup(func() { _ = os.WriteFile(path, original, 0600) })
	if e = os.WriteFile(path, append(append([]byte{}, original...), []byte("\n<!-- changed -->")...), 0600); e != nil {
		t.Fatal(e)
	}
	rows, e := m.PendingProjectCommands(ctx)
	if e != nil || len(rows) != 1 || rows[0].CanResolve {
		t.Fatal(rows, e)
	}
	if e = m.ResolveProjectCommand(ctx, r.ID, rows[0].Revision); e == nil {
		t.Fatal("stopped modified command job")
	}
	if syscall.Kill(tool, 0) != nil {
		t.Fatal("ownership refusal stopped the command")
	}
	if e = os.WriteFile(path, original, 0600); e != nil {
		t.Fatal(e)
	}
	rows, e = m.PendingProjectCommands(ctx)
	if e != nil {
		t.Fatal(e)
	}
	if e = m.ResolveProjectCommand(ctx, r.ID, rows[0].Revision); e != nil {
		t.Fatal(e)
	}
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) && (syscall.Kill(tool, 0) != syscall.ESRCH || syscall.Kill(child, 0) != syscall.ESRCH) {
		time.Sleep(20 * time.Millisecond)
	}
	if syscall.Kill(tool, 0) != syscall.ESRCH || syscall.Kill(child, 0) != syscall.ESRCH {
		t.Fatal("owned command left tool/child running", tool, child)
	}
	if _, e = os.Stat(filepath.Join(p.Directory, "source-effect")); e != nil {
		t.Fatal("stop erased source writes", e)
	}
}
func TestProjectCommandsOverlapAndCancelOnlyTheSelectedProject(t *testing.T) {
	m, _, a, b := commandWorkFixture(t)
	ctxA, cancelA := context.WithCancel(context.Background())
	defer cancelA()
	ctxB, cancelB := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancelB()
	type result struct {
		out string
		err error
	}
	doneA, doneB := make(chan result, 1), make(chan result, 1)
	go func() { out, e := New(m.Root).Exec(ctxA, a.ID, []string{"fixture", "first"}); doneA <- result{out, e} }()
	go func() { out, e := New(m.Root).Exec(ctxB, b.ID, []string{"fixture", "second"}); doneB <- result{out, e} }()
	waitCommandTool(t, a)
	toolB, _ := waitCommandTool(t, b)
	cancelA()
	res := <-doneA
	if !errors.Is(res.err, context.Canceled) {
		t.Fatal(res)
	}
	if syscall.Kill(toolB, 0) != nil {
		t.Fatal("cancelling alpha stopped beta")
	}
	if e := os.WriteFile(filepath.Join(b.Directory, "release"), nil, 0600); e != nil {
		t.Fatal(e)
	}
	res = <-doneB
	if res.err != nil || res.out != "second\ncompleted\n" {
		t.Fatal(res)
	}
	rows, e := m.PendingProjectCommands(context.Background())
	if e != nil || len(rows) != 0 {
		t.Fatal(rows, e)
	}
}
func TestProjectCommandFailureReturnsOutputButDoesNotStoreItInHistory(t *testing.T) {
	m := testManager(t)
	p := Project{SchemaVersion: Schema, ID: "failure", Name: "Failure", Directory: t.TempDir(), Services: []Service{{ID: "one", Command: []string{"/bin/sh"}}}}
	if e := m.SaveProject(context.Background(), p); e != nil {
		t.Fatal(e)
	}
	secret := fmt.Sprint("private-", randomID())
	out, e := m.Exec(context.Background(), p.ID, []string{"/bin/sh", "-c", "printf '%s' \"$1\"; exit 3", "--", secret})
	if e == nil || !strings.Contains(out, secret) {
		t.Fatal("failed command lost its output", out, e)
	}
	rows, e := m.Operations(context.Background())
	if e != nil {
		t.Fatal(e)
	}
	for _, r := range rows {
		path, _, _ := m.operationFiles(r.ID)
		b, _ := os.ReadFile(path)
		if strings.Contains(string(b), secret) {
			t.Fatal("private output in history")
		}
	}
}

func TestProjectCommandOutputLimitStopsBeforeFurtherSourceChanges(t *testing.T) {
	m := testManager(t)
	p := Project{SchemaVersion: Schema, ID: "bounded", Name: "Bounded output", Directory: t.TempDir(), Services: []Service{{ID: "one", Command: []string{"/usr/bin/python3"}}}}
	if e := m.SaveProject(context.Background(), p); e != nil {
		t.Fatal(e)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	script := "import os,pathlib; [os.write(1,b'x'*(1024*1024)) for _ in range(70)]; pathlib.Path('unexpected').write_text('finished')"
	out, e := m.Exec(ctx, p.ID, []string{"/usr/bin/python3", "-c", script})
	if e == nil || !strings.Contains(out, "output exceeds 64 MiB") || len(out) > int(maxCommandOutput)+16384 {
		t.Fatal("command output limit was not enforced", len(out), e)
	}
	if _, e = os.Stat(filepath.Join(p.Directory, "unexpected")); !os.IsNotExist(e) {
		t.Fatal("oversized command continued after output rejection", e)
	}
	rows, e := m.PendingProjectCommands(context.Background())
	if e != nil || len(rows) != 0 {
		t.Fatal("bounded command retained resources", rows, e)
	}
}

func TestProjectCommandPreservesRawArgumentBytes(t *testing.T) {
	m := testManager(t)
	p := Project{SchemaVersion: Schema, ID: "raw-arguments", Name: "Raw arguments", Directory: t.TempDir(), Services: []Service{{ID: "one", Command: []string{"/bin/true"}}}}
	if e := m.SaveProject(context.Background(), p); e != nil {
		t.Fatal(e)
	}
	raw := []byte{'a', ' ', 0xff, '"', 0xfe, 'z'}
	out, e := m.Exec(context.Background(), p.ID, []string{"/usr/bin/printf", "%s", string(raw)})
	if e != nil || !bytes.Equal([]byte(out), raw) {
		t.Fatal("command argument bytes changed in private handoff", []byte(out), e)
	}
}
func TestProjectCommandRejectsChangedPrivateInputBeforeExecution(t *testing.T) {
	m, _, p, _ := commandWorkFixture(t)
	realRun := m.Run
	injected := false
	m.Run = func(ctx context.Context, name string, args ...string) (string, error) {
		if !injected && name == "launchctl" && len(args) > 0 && args[0] == "bootstrap" {
			rows, e := m.commandWorkRecords()
			if e != nil || len(rows) != 1 {
				t.Fatal(rows, e)
			}
			dir, e := m.commandWorkDir(rows[0].ID)
			if e != nil {
				t.Fatal(e)
			}
			if e = os.WriteFile(filepath.Join(dir, "input.json"), []byte(`{"Encoding":"bytes-v1","Args":["L2Jpbi90cnVl"]}`), 0600); e != nil {
				t.Fatal(e)
			}
			injected = true
		}
		return realRun(ctx, name, args...)
	}
	if _, e := m.Exec(context.Background(), p.ID, []string{"fixture", "original"}); e == nil || !injected {
		t.Fatal("changed input was executed", e)
	}
	if _, e := os.Stat(filepath.Join(p.Directory, "entered")); !os.IsNotExist(e) {
		t.Fatal("unverified command started", e)
	}
	rows, e := m.PendingProjectCommands(context.Background())
	if e != nil || len(rows) != 1 || !rows[0].CanResolve {
		t.Fatal("failed input did not retain a reviewable reservation", rows, e)
	}
	if e = m.ResolveProjectCommand(context.Background(), rows[0].ID, rows[0].Revision); e != nil {
		t.Fatal(e)
	}
}

func TestProjectCommandKilledWorkerReturnsForReviewWithoutWaitingForCallerCancellation(t *testing.T) {
	for _, executionReason := range []bool{false, true} {
		t.Run(fmt.Sprintf("execution-reason-%t", executionReason), func(t *testing.T) {
			m, _, p, _ := commandWorkFixture(t)
			if executionReason {
				realRun := m.Run
				m.Run = func(ctx context.Context, name string, args ...string) (string, error) {
					out, e := realRun(ctx, name, args...)
					if name == "launchctl" && len(args) > 1 && args[0] == "print" && strings.Contains(args[1], ".command-") {
						job := parseJobState(out)
						if !job.Running && job.PID == 0 && job.Runs > 0 {
							// AMFI rejection reports a reason without a numeric exit code.
							out = "state = not running\nruns = 1\nlast exit reason = OS_REASON_EXEC\n"
						}
					}
					return out, e
				}
			}
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			done := make(chan error, 1)
			go func() { _, e := m.Exec(ctx, p.ID, []string{"fixture", "worker killed"}); done <- e }()
			waitCommandTool(t, p)
			records, e := m.commandWorkRecords()
			if e != nil || len(records) != 1 {
				t.Fatal(records, e)
			}
			job, e := m.readJobState(context.Background(), "command-"+records[0].ID)
			if e != nil || job.PID <= 0 {
				t.Fatal(job, e)
			}
			if e = syscall.Kill(job.PID, syscall.SIGKILL); e != nil {
				t.Fatal(e)
			}
			select {
			case e = <-done:
				if e == nil || !strings.Contains(e.Error(), "without a confirmed result") {
					t.Fatal("unconfirmed worker exit was accepted", e)
				}
			case <-time.After(5 * time.Second):
				t.Fatal("caller kept waiting after terminal worker exit")
			}
			rows, e := m.PendingProjectCommands(context.Background())
			if e != nil || len(rows) != 1 || !rows[0].CanResolve {
				t.Fatal("killed worker lost its reviewable reservation", rows, e)
			}
			if e = m.ResolveProjectCommand(context.Background(), rows[0].ID, rows[0].Revision); e != nil {
				t.Fatal(e)
			}
		})
	}
}

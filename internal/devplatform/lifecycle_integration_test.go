package devplatform

import (
	"context"
	"errors"
	"fmt"
	"macos-service-manager/internal/system"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"

	"howett.net/plist"
)

const lifecycleHTTPServer = `import http.server,sys,time,pathlib
with open('launches','a') as f: f.write(str(time.monotonic())+'\n')
class Handler(http.server.BaseHTTPRequestHandler):
 def do_GET(self):
  self.send_response(200 if pathlib.Path('ready').exists() else 503); self.end_headers()
http.server.HTTPServer(('127.0.0.1',int(sys.argv[1])),Handler).serve_forever()
`

func TestServiceLifecycleReadinessRollbackAndCrash(t *testing.T) {
	python, err := exec.LookPath("python3")
	if err != nil {
		t.Skip("python3 is required for real launchd lifecycle verification")
	}
	oldAdmin := system.RunAdmin
	system.RunAdmin = func(string, ...string) (string, error) {
		return "", errors.New("administrator operations are disabled in lifecycle tests")
	}
	t.Cleanup(func() { system.RunAdmin = oldAdmin })
	m := testManager(t)
	ctx := context.Background()
	project := func(id string, ready bool) Project {
		t.Helper()
		dir := t.TempDir()
		port := integrationPort(t)
		if err := os.WriteFile(filepath.Join(dir, "server.py"), []byte(lifecycleHTTPServer), 0600); err != nil {
			t.Fatal(err)
		}
		if ready {
			if err := os.WriteFile(filepath.Join(dir, "ready"), nil, 0600); err != nil {
				t.Fatal(err)
			}
		}
		p := Project{SchemaVersion: Schema, ID: id, Name: id, Directory: dir, Services: []Service{{ID: "web", Command: []string{python, "server.py", strconv.Itoa(port)}, Health: fmt.Sprintf("http://127.0.0.1:%d/", port), ReadinessTimeoutSeconds: 2}}}
		if err := m.SaveProject(ctx, p); err != nil {
			t.Fatal(err)
		}
		t.Cleanup(func() {
			if err := m.Down(context.Background(), id); err != nil {
				t.Error(err)
			}
		})
		return p
	}
	keeper := project("keeper", true)
	if err = m.Up(ctx, keeper.ID); err != nil {
		t.Fatal(err)
	}
	assertKeeper := func() {
		t.Helper()
		s, _ := m.Read()
		if got := m.Snapshot(ctx, s).States["project:keeper"]; got != "Healthy" {
			t.Fatal("rollback interrupted an already-running project", got)
		}
	}
	assertKeeper()
	failed := project("failed-chain", true)
	// An unrelated listener answering the health URL must not hide this exit.
	failed.Services = append(failed.Services, Service{ID: "bad", Command: []string{"/usr/bin/false"}, Health: keeper.Services[0].Health, ReadinessTimeoutSeconds: 2, DependsOn: []string{"web"}})
	if err = m.SaveProject(ctx, failed); err != nil {
		t.Fatal(err)
	}
	if err = m.Up(ctx, failed.ID); err == nil || !strings.Contains(err.Error(), "exited with code 1") {
		t.Fatal("fast exit was reported ready", err)
	}
	for _, id := range []string{"web", "bad"} {
		if m.loaded(ctx, "p-"+failed.ID+"-"+id) {
			t.Fatal("failure left a newly started job registered", id)
		}
	}
	assertKeeper()
	slow := project("unready", false)
	slow.Services[0].ReadinessTimeoutSeconds = 1
	if err = m.SaveProject(ctx, slow); err != nil {
		t.Fatal(err)
	}
	start := time.Now()
	if err = m.Up(ctx, slow.ID); !errors.Is(err, context.DeadlineExceeded) {
		t.Fatal("unready service ignored timeout", err)
	}
	if elapsed := time.Since(start); elapsed < time.Second || elapsed > 4*time.Second {
		t.Fatal("configured readiness timeout was not respected", elapsed)
	}
	if m.loaded(ctx, "p-"+slow.ID+"-web") {
		t.Fatal("readiness failure did not roll back its process")
	}
	assertKeeper()
	state, _ := m.Read()
	first := m.Snapshot(ctx, state).Services[ServiceStatusKey(keeper.ID, "web")]
	if first.PID <= 0 {
		t.Fatal("missing managed process PID")
	}
	if err = syscall.Kill(first.PID, syscall.SIGKILL); err != nil {
		t.Fatal(err)
	}
	wait := func(predicate func() bool) {
		t.Helper()
		deadline := time.Now().Add(8 * time.Second)
		for time.Now().Before(deadline) {
			if predicate() {
				return
			}
			time.Sleep(100 * time.Millisecond)
		}
		t.Fatal("launchd state did not settle")
	}
	wait(func() bool { return m.Snapshot(ctx, state).States["project:keeper"] == "Failed" })
	time.Sleep(1200 * time.Millisecond)
	if job, _ := m.readJobState(ctx, "p-keeper-web"); job.Running {
		t.Fatal("default policy restarted a crashed service")
	}
	if err = m.Down(ctx, keeper.ID); err != nil {
		t.Fatal(err)
	}
	keeper.Services[0].Restart = true
	keeper.Services[0].RestartThrottleSeconds = 3
	keeper.Services[0].ReadinessTimeoutSeconds = 5
	if err = m.SaveProject(ctx, keeper); err != nil {
		t.Fatal(err)
	}
	if err = os.Remove(filepath.Join(keeper.Directory, "launches")); err != nil {
		t.Fatal(err)
	}
	if err = m.Up(ctx, keeper.ID); err != nil {
		t.Fatal(err)
	}
	jobPath := filepath.Join(m.Root, "jobs", m.label("p-keeper-web")+".plist")
	b, err := os.ReadFile(jobPath)
	if err != nil {
		t.Fatal(err)
	}
	var job map[string]any
	if _, err = plist.Unmarshal(b, &job); err != nil {
		t.Fatal(err)
	}
	if fmt.Sprint(job["ThrottleInterval"]) != "3" || job["KeepAlive"] != true {
		t.Fatal("restart policy was not written", job["ThrottleInterval"], job["KeepAlive"])
	}
	before, err := m.readJobState(ctx, "p-keeper-web")
	if err != nil || before.PID <= 0 {
		t.Fatal(err, before)
	}
	if err = syscall.Kill(before.PID, syscall.SIGKILL); err != nil {
		t.Fatal(err)
	}
	wait(func() bool {
		after, err := m.readJobState(ctx, "p-keeper-web")
		return err == nil && after.Running && after.PID > 0 && after.PID != before.PID
	})
	wait(func() bool {
		b, _ := os.ReadFile(filepath.Join(keeper.Directory, "launches"))
		return len(strings.Fields(string(b))) >= 2
	})
	launches, _ := os.ReadFile(filepath.Join(keeper.Directory, "launches"))
	times := strings.Fields(string(launches))
	a, _ := strconv.ParseFloat(times[0], 64)
	btime, _ := strconv.ParseFloat(times[1], 64)
	if btime-a < 2.5 {
		t.Fatal("launchd ignored the configured restart throttle", btime-a)
	}
	if err = m.Down(ctx, keeper.ID); err != nil {
		t.Fatal(err)
	}
	// KeepAlive must not resurrect the process after the user stops its job.
	time.Sleep(3500 * time.Millisecond)
	if m.loaded(ctx, "p-keeper-web") {
		t.Fatal("stopped service was registered again")
	}
	address := strings.TrimPrefix(keeper.Services[0].Health, "http://")
	address = strings.TrimSuffix(address, "/")
	if conn, err := net.DialTimeout("tcp", address, time.Second); err == nil {
		conn.Close()
		t.Fatal("stopped restart-enabled service is still listening")
	}
}

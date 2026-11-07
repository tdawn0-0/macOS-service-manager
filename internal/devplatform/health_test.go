package devplatform

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"strings"
	"sync/atomic"
	"testing"
	"time"
)

func TestServiceHealthValidation(t *testing.T) {
	for _, address := range []string{"", "process", "tcp://127.0.0.1:3000", "tcp://[::1]:3000", "http://localhost:8080/ready", "https://localhost/ready?mode=dev"} {
		if err := (Service{ID: "web", Command: []string{"web"}, Health: address}).Validate(); err != nil {
			t.Fatal(address, err)
		}
	}
	for _, address := range []string{"shell:echo ok", "tcp://localhost", "tcp://localhost:0", "tcp://localhost:65536", "tcp://localhost:3000/path", "http://user:password@localhost/", "http://localhost:65536/", "http://localhost/#fragment"} {
		if err := (Service{ID: "web", Command: []string{"web"}, Health: address}).Validate(); err == nil {
			t.Fatal("accepted invalid readiness", address)
		}
	}
	for _, service := range []Service{{ID: "web", Command: []string{"web"}, ReadinessTimeoutSeconds: -1}, {ID: "web", Command: []string{"web"}, ReadinessTimeoutSeconds: 3601}, {ID: "web", Command: []string{"web"}, RestartThrottleSeconds: -1}, {ID: "web", Command: []string{"web"}, RestartThrottleSeconds: 3601}} {
		if err := service.Validate(); err == nil {
			t.Fatal("accepted invalid service policy", service)
		}
	}
}

func TestLaunchdProcessStateIgnoresNestedCoalitions(t *testing.T) {
	state := parseJobState("gui/501/example = {\n\tstate = running\n\truns = 2\n\tpid = 123\n\tlast exit code = 7\n\tlast terminating signal = Killed: 9\n\tresource coalition = {\n\t\tstate = active\n\t\tpid = 999\n\t}\n}")
	if !state.Running || state.PID != 123 || state.Runs != 2 || state.LastExitCode == nil || *state.LastExitCode != 7 || state.LastSignal != "9" {
		t.Fatal(state)
	}
}

func TestStopWaitUsesOwnedProcessRatherThanNestedPID(t *testing.T) {
	m := testManager(t)
	prints := 0
	m.Run = func(ctx context.Context, name string, args ...string) (string, error) {
		if args[0] == "print" {
			prints++
			if prints == 1 {
				return fmt.Sprintf("\tstate = running\n\tpid = 99999999\n\tresource coalition = {\n\t\tpid = %d\n\t}", os.Getpid()), nil
			}
			return "", errors.New("job is gone")
		}
		return "", nil
	}
	ctx, cancel := context.WithTimeout(context.Background(), 200*time.Millisecond)
	defer cancel()
	if err := m.stopJob(ctx, "fixture"); err != nil {
		t.Fatal("waited on a PID from a nested object", err)
	}
}

func TestRuntimeSnapshotSeparatesProcessReadinessAndCrash(t *testing.T) {
	m := testManager(t)
	var ready atomic.Bool
	ready.Store(true)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !ready.Load() {
			w.WriteHeader(503)
		}
	}))
	defer server.Close()
	jobOutput := "state = running\npid = 123\nruns = 1"
	queryErr := error(nil)
	m.Run = func(ctx context.Context, name string, args ...string) (string, error) {
		if strings.HasSuffix(args[len(args)-1], m.label("p-app-web")) {
			return jobOutput, queryErr
		}
		return "", errors.New("not found")
	}
	s := emptyState()
	p := Project{ID: "app", Services: []Service{{ID: "web", Command: []string{"web"}, Health: server.URL}}}
	s.Projects[p.ID] = p
	check := func(expected string) {
		t.Helper()
		r := m.Snapshot(context.Background(), s)
		if r.States["project:app"] != expected || r.Services[ServiceStatusKey("app", "web")].Status != expected {
			t.Fatal("incorrect observed state", expected, r)
		}
	}
	check("Healthy")
	ready.Store(false)
	check("Not ready")
	p.Services[0].Health = ""
	s.Projects[p.ID] = p
	check("Running")
	p.Services[0].Health = "process"
	s.Projects[p.ID] = p
	check("Healthy")
	jobOutput = "state = not running\nruns = 4\nlast exit code = 23"
	check("Failed")
	p.Services[0].Restart = true
	p.Services[0].RestartThrottleSeconds = 7
	s.Projects[p.ID] = p
	check("Restarting")
	observed := m.Snapshot(context.Background(), s).Services[ServiceStatusKey("app", "web")]
	if observed.LastExitCode == nil || *observed.LastExitCode != 23 || observed.Launches != 4 || !strings.Contains(observed.Message, "7 seconds") || !strings.Contains(observed.Message, "Repeated launches") {
		t.Fatal(observed)
	}
	queryErr = context.DeadlineExceeded
	check("Unknown")
	queryErr = errors.New("not found")
	check("Stopped")
}

func TestRuntimeSnapshotDoesNotAssignStandaloneDatabaseToInactiveProject(t *testing.T) {
	m := testManager(t)
	m.Run = func(ctx context.Context, name string, args ...string) (string, error) {
		if strings.HasSuffix(args[len(args)-1], m.label("db-shared")) {
			return "state = running\npid = 123", nil
		}
		return "", errors.New("not found")
	}
	s := emptyState()
	s.Instances["shared"] = Instance{ID: "shared"}
	s.Projects["inactive"] = Project{ID: "inactive", Instances: []string{"shared"}, Services: []Service{{ID: "api", Command: []string{"api"}}}}
	r := m.Snapshot(context.Background(), s)
	if r.States["project:inactive"] != "Stopped" || r.States["instance:shared"] != "Running" {
		t.Fatal(r)
	}
}

func TestReadinessProbeDoesNotLeakHTTPQueryInErrors(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(503) }))
	server.Close()
	err := probeHealth(context.Background(), server.URL+"/?token=do-not-log-this")
	if err == nil || strings.Contains(fmt.Sprint(err), "do-not-log-this") {
		t.Fatal(err)
	}
}

func TestObservedHealthUsesTheStartupProbeDeadline(t *testing.T) {
	m := testManager(t)
	m.Run = func(context.Context, string, ...string) (string, error) { return "state = running\npid = 123", nil }
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { time.Sleep(600 * time.Millisecond); w.WriteHeader(204) }))
	defer server.Close()
	result := m.observeService(context.Background(), "slow", Service{Health: server.URL})
	if result.Status != "Healthy" {
		t.Fatal("an acceptable startup probe became unready only because status polling used a shorter deadline", result)
	}
}

func TestRuntimeSnapshotFairBudget(t *testing.T) {
	m := testManager(t)
	s := emptyState()
	slow := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { <-r.Context().Done() }))
	defer slow.Close()
	fast := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(204) }))
	defer fast.Close()
	for n := 0; n < 6; n++ {
		id := fmt.Sprintf("slow-%d", n)
		s.Projects[id] = Project{ID: id, Directory: t.TempDir(), Services: []Service{{ID: "web", Command: []string{"fixture"}, Health: slow.URL}}}
	}
	s.Projects["z-healthy"] = Project{ID: "z-healthy", Directory: t.TempDir(), Services: []Service{{ID: "web", Command: []string{"fixture"}, Health: fast.URL}}}
	m.Run = func(context.Context, string, ...string) (string, error) { return "state = running\npid = 123", nil }
	r := m.Snapshot(context.Background(), s)
	if r.States["project:z-healthy"] != "Healthy" || r.Services[ServiceStatusKey("z-healthy", "web")].Status != "Healthy" {
		t.Fatal("later healthy service was starved", r.States)
	}
	for n := 0; n < 6; n++ {
		status := r.States["project:"+fmt.Sprintf("slow-%d", n)]
		if status != "Not ready" && status != "Unknown" {
			t.Fatal("slow service not classified", status)
		}
	}
}

func TestRuntimeSnapshotBoundedCancellation(t *testing.T) {
	m := testManager(t)
	s := emptyState()
	for n := 0; n < 12; n++ {
		id := fmt.Sprintf("db-%02d", n)
		s.Instances[id] = Instance{ID: id}
	}
	var active, peak atomic.Int32
	entered := make(chan struct{}, 4)
	m.Run = func(ctx context.Context, _ string, _ ...string) (string, error) {
		n := active.Add(1)
		defer active.Add(-1)
		for old := peak.Load(); n > old; old = peak.Load() {
			if peak.CompareAndSwap(old, n) {
				break
			}
		}
		select {
		case entered <- struct{}{}:
		default:
		}
		<-ctx.Done()
		return "", ctx.Err()
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	done := make(chan RuntimeSnapshot, 1)
	go func() { done <- m.Snapshot(ctx, s) }()
	for n := 0; n < 4; n++ {
		select {
		case <-entered:
		case <-time.After(3 * time.Second):
			t.Fatal("worker pool did not start")
		}
	}
	cancel()
	select {
	case r := <-done:
		for _, status := range r.States {
			if status != "Unknown" {
				t.Fatal("cancelled observation inferred state", status)
			}
		}
	case <-time.After(3 * time.Second):
		t.Fatal("cancelled snapshot did not return")
	}
	if active.Load() != 0 || peak.Load() > 4 {
		t.Fatal("observation concurrency leaked/exceeded", active.Load(), peak.Load())
	}
}
func TestRuntimeSnapshotKeepsCompletedResults(t *testing.T) {
	m := testManager(t)
	s := emptyState()
	s.Instances["slow"] = Instance{ID: "slow"}
	fast := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(204) }))
	defer fast.Close()
	s.Projects["fast"] = Project{ID: "fast", Directory: t.TempDir(), Services: []Service{{ID: "web", Command: []string{"fixture"}, Health: fast.URL}}}
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	m.Run = func(check context.Context, _ string, args ...string) (string, error) {
		if strings.HasSuffix(args[len(args)-1], m.label("db-slow")) {
			<-check.Done()
			cancel()
			return "", check.Err()
		}
		return "state = running\npid = 123", nil
	}
	r := m.Snapshot(ctx, s)
	if ctx.Err() == nil {
		t.Fatal("fixture failed to expire shared context")
	}
	if r.States["instance:slow"] != "Unknown" || r.States["project:fast"] != "Healthy" || r.Services[ServiceStatusKey("fast", "web")].Status != "Healthy" {
		t.Fatal("completed health erased by other timeout", r.States)
	}
}
func TestRuntimeSnapshotProxyAndPID(t *testing.T) {
	t.Run("empty", func(t *testing.T) {
		m := testManager(t)
		m.Run = func(context.Context, string, ...string) (string, error) {
			t.Fatal("empty snapshot queried process")
			return "", nil
		}
		r := m.Snapshot(context.Background(), emptyState())
		if len(r.States) != 0 {
			t.Fatal(r.States)
		}
	})
	t.Run("proxy-only", func(t *testing.T) {
		m := testManager(t)
		s := emptyState()
		s.Projects["proxy-only"] = Project{ID: "proxy-only", Directory: t.TempDir()}
		m.Run = func(_ context.Context, name string, args ...string) (string, error) {
			if strings.HasSuffix(args[len(args)-1], m.label("project-proxy-only")) {
				return "state = not running", nil
			}
			if strings.HasSuffix(args[len(args)-1], m.label("proxy")) {
				return "state = running\npid = 123", nil
			}
			return "", errors.New("not found")
		}
		r := m.Snapshot(context.Background(), s)
		if r.States["project:proxy-only"] != "Running" {
			t.Fatal(r.States)
		}
	})
	t.Run("changed-pid", func(t *testing.T) {
		m := testManager(t)
		s := emptyState()
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(204) }))
		defer server.Close()
		s.Projects["app"] = Project{ID: "app", Directory: t.TempDir(), Services: []Service{{ID: "web", Command: []string{"fixture"}, Health: server.URL}}}
		var calls atomic.Int32
		m.Run = func(_ context.Context, _ string, args ...string) (string, error) {
			if strings.HasSuffix(args[len(args)-1], m.label("p-app-web")) {
				return fmt.Sprintf("state = running\npid = %d", calls.Add(1)), nil
			}
			return "", errors.New("not found")
		}
		r := m.Snapshot(context.Background(), s)
		if r.States["project:app"] != "Not ready" || calls.Load() != 2 {
			t.Fatal("PID ownership check lost", r.States)
		}
	})
}

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

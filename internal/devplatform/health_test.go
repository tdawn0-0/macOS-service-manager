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

func TestProbeHealthValidation(t *testing.T) {
	for _, address := range []string{"", "process", "tcp://127.0.0.1:3000", "http://localhost:8080/ready"} {
		if err := validateHealth(address); err != nil {
			t.Fatal(address, err)
		}
	}
	for _, address := range []string{"shell:echo ok", "tcp://localhost:0", "http://user:password@localhost/"} {
		if err := validateHealth(address); err == nil {
			t.Fatal("accepted invalid readiness", address)
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

func TestReadinessProbeDoesNotLeakHTTPQueryInErrors(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(503) }))
	server.Close()
	err := probeHealth(context.Background(), server.URL+"/?token=do-not-log-this")
	if err == nil || strings.Contains(fmt.Sprint(err), "do-not-log-this") {
		t.Fatal(err)
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
func TestRuntimeSnapshotProxyAndPID(t *testing.T) {
	m := testManager(t)
	m.Run = func(context.Context, string, ...string) (string, error) {
		t.Fatal("empty snapshot queried process")
		return "", nil
	}
	if r := m.Snapshot(context.Background(), emptyState()); len(r.States) != 0 {
		t.Fatal(r.States)
	}
	s := emptyState()
	s.ProxyRules = []ProxyRule{{Domain: "proxy.test", LocalPort: 3000}}
	m.Run = func(_ context.Context, _ string, args ...string) (string, error) {
		if strings.HasSuffix(args[len(args)-1], m.label("proxy")) {
			return "state = running\npid = 123", nil
		}
		return "", errors.New("not found")
	}
	if r := m.Snapshot(context.Background(), s); r.States["proxy"] != "Running" {
		t.Fatal(r.States)
	}
}

package main

import (
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/egoist/mygo/ui"

	"devbase/internal/devplatform"
	"devbase/internal/system"
)

func TestDevLogsDrawerLifecycleTargetStreamFilterAndLivePoll(t *testing.T) {
	oldRun := system.Run
	defer func() { system.Run = oldRun }()
	var consoleCalls []string
	system.Run = func(name string, args ...string) (string, error) {
		consoleCalls = append(consoleCalls, strings.Join(append([]string{name}, args...), " "))
		return "", nil
	}

	root := t.TempDir()
	logsDir := filepath.Join(root, "logs")
	if err := os.MkdirAll(logsDir, 0o700); err != nil {
		t.Fatal(err)
	}
	proxyErr := filepath.Join(logsDir, "proxy.err.log")
	proxyOut := filepath.Join(logsDir, "proxy.log")
	tunnelErr := filepath.Join(logsDir, "tunnel.err.log")
	dbErr := filepath.Join(logsDir, "db-atlas-postgres.err.log")
	if err := os.WriteFile(proxyErr, []byte("proxy-err-1\nproxy-err-2\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(proxyOut, []byte("proxy-out-1\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(tunnelErr, []byte("INFO connected\nwarn: slow response\nWARN: retry stream\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(dbErr, []byte("postgres ready\n"), 0o600); err != nil {
		t.Fatal(err)
	}

	// pollDevLog must be a safe no-op when a.post == nil (pure layout fixtures).
	fx := designFixture(t)
	fx.devLog.open = true
	fx.pollDevLog()
	if fx.devLog.reading || fx.devLog.lines != nil {
		t.Fatalf("expected pollDevLog to do nothing with nil post, got %+v", fx.devLog)
	}

	for _, size := range [][2]int{{900, 520}, {1120, 720}} {
		for _, dark := range []bool{false, true} {
			a := designFixture(t)
			a.spawn = func(f func()) { f() }
			a.post = func(f func()) { f() }
			a.dev.managerFactory = func() *devplatform.Manager { return devplatform.New(root) }

			tt := ui.NewTester(a.view, size[0], size[1])
			tt.SetDark(dark)
			tt.Frame()

			// Verify top bar actions at this size before and after opening logs.
			for _, name := range []string{"Refresh", "Tasks", "Logs", "Diagnostics", "Live logs"} {
				r, ok := tt.Find(name)
				if !ok || r.W <= 0 || r.H <= 0 {
					t.Fatalf("%dx%d dark=%t: missing %q in %v", size[0], size[1], dark, name, tt.Texts())
				}
			}

			if err := tt.Click("Logs"); err != nil {
				t.Fatalf("%dx%d dark=%t: click Logs: %v", size[0], size[1], dark, err)
			}
			tt.Frame()
			if !a.devLog.open || a.devLog.jobID != "proxy" {
				t.Fatalf("expected proxy logs open, got open=%v job=%q", a.devLog.open, a.devLog.jobID)
			}
			if !tt.HasText("proxy-err-1") || !tt.HasText("proxy-err-2") {
				t.Fatalf("missing proxy error lines: %v", tt.Texts())
			}

			// All drawer controls must remain reachable at 900x520 and 1120x720.
			for _, ctrl := range []string{"Log target", "Error", "Output", "Filter logs", "Open in Console", "Close logs"} {
				r, ok := tt.Find(ctrl)
				if !ok || r.W <= 0 || r.H <= 0 {
					t.Fatalf("%dx%d dark=%t: missing drawer control %q in %v", size[0], size[1], dark, ctrl, tt.Texts())
				}
			}

			if err := tt.Click("Close logs"); err != nil {
				t.Fatal(err)
			}
			tt.Frame()
			if a.devLog.open {
				t.Fatal("Close logs did not close drawer")
			}

			// Reopen via status bar "Live logs" button.
			if err := tt.Click("Live logs"); err != nil {
				t.Fatal(err)
			}
			tt.Frame()
			if !a.devLog.open {
				t.Fatal("Live logs did not open drawer")
			}
		}
	}

	// Full interactive checks: live tailing, Error/Output switch, target selector, filter, Open in Console.
	a := designFixture(t)
	a.spawn = func(f func()) { f() }
	a.post = func(f func()) { f() }
	a.dev.managerFactory = func() *devplatform.Manager { return devplatform.New(root) }
	tt := ui.NewTester(a.view, 1120, 720)
	tt.Frame()
	if err := tt.Click("Logs"); err != nil {
		t.Fatal(err)
	}
	tt.Frame()

	// Append line and expire poll interval to verify live polling.
	f, err := os.OpenFile(proxyErr, os.O_APPEND|os.O_WRONLY, 0)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := f.WriteString("proxy-err-3\n"); err != nil {
		t.Fatal(err)
	}
	_ = f.Close()
	a.devLog.last = a.devLog.last.Add(-2 * time.Second)
	tt.Frame()
	tt.Frame()
	if !tt.HasText("proxy-err-3") {
		t.Fatalf("live poll did not pick up appended line: %v", tt.Texts())
	}

	// Switch to Output stream (.log).
	if err := tt.Click("Output"); err != nil {
		t.Fatal(err)
	}
	tt.Advance(300 * time.Millisecond)
	tt.Frame()
	if a.devLog.stream != 1 || !tt.HasText("proxy-out-1") {
		t.Fatalf("expected proxy.log output, stream=%d texts=%v", a.devLog.stream, tt.Texts())
	}

	// Switch target to tunnel and back to Error stream.
	if err := tt.Click("Error"); err != nil {
		t.Fatal(err)
	}
	tt.Advance(300 * time.Millisecond)
	tt.Frame()
	if err := tt.Click("Log target"); err != nil {
		t.Fatal(err)
	}
	tt.Frame()
	if err := tt.Click("tunnel"); err != nil {
		t.Fatal(err)
	}
	tt.Frame()
	if a.devLog.jobID != "tunnel" || !tt.HasText("INFO connected") || !tt.HasText("warn: slow response") {
		t.Fatalf("expected tunnel error logs, job=%q texts=%v", a.devLog.jobID, tt.Texts())
	}

	// Case-insensitive query filtering.
	if err := tt.Click("Filter logs"); err != nil {
		t.Fatal(err)
	}
	tt.Type("WARN")
	tt.Frame()
	if !tt.HasText("warn: slow response") || !tt.HasText("WARN: retry stream") || tt.HasText("INFO connected") {
		t.Fatalf("case-insensitive filter failed: %v", tt.Texts())
	}

	// Clear filter and switch to db-atlas-postgres.
	a.devLog.query = ""
	tt.Frame()
	if err := tt.Click("Log target"); err != nil {
		t.Fatal(err)
	}
	tt.Frame()
	if err := tt.Click("db-atlas-postgres"); err != nil {
		t.Fatal(err)
	}
	tt.Frame()
	if a.devLog.jobID != "db-atlas-postgres" || !tt.HasText("postgres ready") {
		t.Fatalf("expected db-atlas-postgres logs, job=%q texts=%v", a.devLog.jobID, tt.Texts())
	}

	// Open in Console calls system.OpenInConsole with the active log path.
	consoleCalls = nil
	if err := tt.Click("Open in Console"); err != nil {
		t.Fatal(err)
	}
	tt.Frame()
	if !slices.Equal(consoleCalls, []string{"open -a Console " + dbErr}) {
		t.Fatalf("unexpected Console calls: %v", consoleCalls)
	}
}

func TestDevLogsEmptyAndMissingStates(t *testing.T) {
	root := t.TempDir()
	logsDir := filepath.Join(root, "logs")
	if err := os.MkdirAll(logsDir, 0o700); err != nil {
		t.Fatal(err)
	}
	emptyOut := filepath.Join(logsDir, "proxy.log")
	if err := os.WriteFile(emptyOut, nil, 0o600); err != nil {
		t.Fatal(err)
	}

	a := designFixture(t)
	a.spawn = func(f func()) { f() }
	a.post = func(f func()) { f() }
	a.dev.managerFactory = func() *devplatform.Manager { return devplatform.New(root) }

	tt := ui.NewTester(a.view, 1120, 720)
	tt.Frame()
	if err := tt.Click("Logs"); err != nil {
		t.Fatal(err)
	}
	tt.Frame()
	if !tt.HasText("No logs yet.") {
		t.Fatalf("expected No logs yet for missing proxy.err.log, got %v", tt.Texts())
	}

	// Creating the missing file makes the next poll display its contents.
	if err := os.WriteFile(filepath.Join(logsDir, "proxy.err.log"), []byte("booted\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	a.devLog.last = a.devLog.last.Add(-2 * time.Second)
	tt.Frame()
	tt.Frame()
	if !tt.HasText("booted") {
		t.Fatalf("expected newly created log line, got %v", tt.Texts())
	}

	// Switching to an empty 0-byte file displays "The log is empty.".
	if err := tt.Click("Output"); err != nil {
		t.Fatal(err)
	}
	tt.Frame()
	if !tt.HasText("The log is empty.") {
		t.Fatalf("expected empty log state, got %v", tt.Texts())
	}
}

func TestBackgroundLaunchFlag(t *testing.T) {
	if !startHiddenInBackground([]string{"--background"}) {
		t.Fatal("expected --background flag to start hidden")
	}
	if startHiddenInBackground([]string{"--dev", "status"}) {
		t.Fatal("did not expect --dev status to start hidden")
	}

	a := newAppWith(nil, nil, t.TempDir())
	var dockCalls []bool
	a.dockOn = func(on bool) { dockCalls = append(dockCalls, on) }

	configureBackgroundLaunch(a, false)
	if !a.dockShown || len(dockCalls) != 0 {
		t.Fatalf("expected normal launch to keep dock shown, got shown=%v calls=%v", a.dockShown, dockCalls)
	}

	configureBackgroundLaunch(a, true)
	if a.dockShown || !slices.Equal(dockCalls, []bool{false}) {
		t.Fatalf("expected background launch to hide dock, got shown=%v calls=%v", a.dockShown, dockCalls)
	}
}

package devplatform

import (
	"context"
	"devbase/internal/launchd"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestLogRotationBoundsArchivesAndSkipsLinks(t *testing.T) {
	m := testManager(t)
	path := filepath.Join(m.Root, "logs", "worker.log")
	outside := filepath.Join(t.TempDir(), "outside.log")
	if e := os.WriteFile(outside, []byte("untouched"), 0600); e != nil {
		t.Fatal(e)
	}
	if e := os.Symlink(outside, filepath.Join(m.Root, "logs", "link.log")); e != nil {
		t.Fatal(e)
	}
	writer, e := os.OpenFile(path, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0600)
	if e != nil {
		t.Fatal(e)
	}
	defer writer.Close()
	for n := 0; n < 3; n++ {
		if _, e = writer.WriteString(strings.Repeat(string(rune('a'+n)), int(logLimit)+100)); e != nil {
			t.Fatal(e)
		}
		if e = m.RotateLogs(context.Background()); e != nil {
			t.Fatal(e)
		}
	}
	if _, e = writer.WriteString("still-writing"); e != nil {
		t.Fatal(e)
	}
	b, e := os.ReadFile(path)
	if e != nil || string(b) != "still-writing" {
		t.Fatal("active descriptor stopped writing", e)
	}
	for _, suffix := range []string{".1", ".2"} {
		info, e := os.Stat(path + suffix)
		if e != nil || info.Size() != logLimit || info.Mode().Perm() != 0600 {
			t.Fatal("archive is not bounded and private", e)
		}
	}
	if _, e = os.Stat(path + ".3"); !os.IsNotExist(e) {
		t.Fatal("kept excess archives")
	}
	b, e = os.ReadFile(outside)
	if e != nil || string(b) != "untouched" {
		t.Fatal("followed a log symlink", e)
	}
}

func TestLogRotationLaunchdLifecycle(t *testing.T) {
	if os.Getenv("MSM_LOG_INTEGRATION") != "1" {
		t.Skip("set MSM_LOG_INTEGRATION=1 on a logged-in Mac")
	}
	m := testManager(t)
	m.LoginDir = t.TempDir()
	m.Program = filepath.Join(t.TempDir(), "devmgr")
	if out, e := exec.Command("go", "build", "-o", m.Program, "../../cmd/devmgr").CombinedOutput(); e != nil {
		t.Fatalf("build: %v %s", e, out)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	defer m.StopLogMaintenance(context.Background())
	pathScheduled := filepath.Join(m.Root, "logs", "scheduled.log")
	if e := os.WriteFile(pathScheduled, make([]byte, logLimit+1), 0600); e != nil {
		t.Fatal(e)
	}
	if e := m.EnsureLogMaintenance(ctx); e != nil {
		t.Fatal(e)
	}
	spec, e := launchd.LoadSpec(m.maintenancePath())
	if e != nil || spec.StartInterval != 60 || spec.KeepAlive || !spec.RunAtLoad || !m.loaded(ctx, "maintenance") {
		t.Fatal("maintenance registration failed", e)
	}
	for {
		info, e := os.Stat(pathScheduled)
		if e == nil && info.Size() == 0 {
			break
		}
		select {
		case <-ctx.Done():
			t.Fatal("scheduled helper did not rotate logs independently")
		case <-time.After(20 * time.Millisecond):
		}
	}
	defer m.stopJob(context.Background(), "rotation-writer")
	if _, e = m.startJob(ctx, "rotation-writer", []string{"/bin/sh", "-c", "/bin/dd if=/dev/zero bs=1048576 count=6 2>/dev/null; while :; do printf 'heartbeat\\n'; /bin/sleep .1; done"}, m.Root, nil, false); e != nil {
		t.Fatal(e)
	}
	path := filepath.Join(m.Root, "logs", "rotation-writer.log")
	wait := func(check func() bool) {
		t.Helper()
		for !check() {
			select {
			case <-ctx.Done():
				t.Fatal("rotation integration timed out")
			case <-time.After(20 * time.Millisecond):
			}
		}
	}
	wait(func() bool { info, e := os.Stat(path); return e == nil && info.Size() > logLimit })
	if e = m.RotateLogs(ctx); e != nil {
		t.Fatal(e)
	}
	wait(func() bool { b, e := os.ReadFile(path); return e == nil && strings.Contains(string(b), "heartbeat") })
	info, e := os.Stat(path)
	if e != nil || info.Size() > 1024 {
		t.Fatal("launchd output descriptor did not append after truncation", e)
	}
	if e = m.SetLogRotation(ctx, false); e != nil {
		t.Fatal(e)
	}
	state, _ := m.Read()
	if !state.LogRotationDisabled || m.loaded(ctx, "maintenance") {
		t.Fatal("maintenance did not stop")
	}
}

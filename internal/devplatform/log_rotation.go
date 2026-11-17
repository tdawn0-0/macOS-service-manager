package devplatform

import (
	"context"
	"errors"
	"fmt"
	"io"
	"devbase/internal/launchd"
	"os"
	"path/filepath"
	"strings"
	"syscall"
)

const logLimit int64 = 5 << 20

func (m *Manager) maintenancePath() string {
	dir := m.LoginDir
	if dir == "" {
		if m.usesUserStorage() {
			dir = launchd.UserAgents().Dir
		} else {
			dir = filepath.Join(m.Root, "jobs")
		}
	}
	return filepath.Join(dir, m.label("maintenance")+".plist")
}

// EnsureLogMaintenance uses a short, scheduled user task, not a supervisor.
// A stable helper allows it to keep working when the app is closed or moved.
func (m *Manager) EnsureLogMaintenance(ctx context.Context) error {
	return m.withState(ctx, func(s *State) error { return m.ensureLogMaintenance(ctx, s) })
}
func (m *Manager) ensureLogMaintenance(ctx context.Context, s *State) error {
	if s.LogRotationDisabled {
		return nil
	}
	exe, e := m.helperExecutable("maintenance")
	if e != nil {
		return e
	}
	path := m.maintenancePath()
	existing := ""
	if _, e = os.Stat(path); e == nil {
		existing = path
	}
	spec := launchd.Spec{Label: m.label("maintenance"), Args: []string{exe, "--dev", "maintain", m.Root}, WorkingDir: m.Root, StartInterval: 60, RunAtLoad: true, StdOut: "/dev/null", StdErr: "/dev/null"}
	if _, e = launchd.Save(filepath.Dir(path), spec, existing); e != nil {
		return e
	}
	if e = os.Chmod(path, 0600); e != nil {
		return e
	}
	if m.loaded(ctx, "maintenance") {
		return nil
	}
	_, e = m.Run(ctx, "launchctl", "bootstrap", domain(), path)
	return e
}

func (m *Manager) stopLogMaintenance(ctx context.Context) error {
	if e := m.stopJob(ctx, "maintenance"); e != nil {
		return e
	}
	e := os.Remove(m.maintenancePath())
	if errors.Is(e, os.ErrNotExist) {
		return nil
	}
	return e
}

func (m *Manager) SetLogRotation(ctx context.Context, enabled bool) error {
	return m.withState(ctx, func(s *State) error {
		s.LogRotationDisabled = !enabled
		if enabled {
			return m.ensureLogMaintenance(ctx, s)
		}
		return m.stopLogMaintenance(ctx)
	})
}

// RotateLogs preserves launchd's open descriptors using copy/truncate. Each
// archive keeps at most the last 5 MiB. Writes during copy/truncate may be lost;
// these are development diagnostics, not an audit or database recovery log.
func (m *Manager) RotateLogs(ctx context.Context) error {
	if e := m.init(); e != nil {
		return e
	}
	lock, e := os.OpenFile(filepath.Join(m.Root, "logs.rotate.lock"), os.O_CREATE|os.O_RDWR, 0600)
	if e != nil {
		return e
	}
	defer lock.Close()
	if e = syscall.Flock(int(lock.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); e != nil {
		if e == syscall.EWOULDBLOCK {
			return nil
		}
		return e
	}
	defer syscall.Flock(int(lock.Fd()), syscall.LOCK_UN)
	entries, e := os.ReadDir(filepath.Join(m.Root, "logs"))
	if e != nil {
		return e
	}
	for _, entry := range entries {
		if e = ctx.Err(); e != nil {
			return e
		}
		if entry.Type()&os.ModeSymlink != 0 || entry.IsDir() || !strings.HasSuffix(entry.Name(), ".log") {
			continue
		}
		path := filepath.Join(m.Root, "logs", entry.Name())
		if e = rotateLog(path); e != nil {
			return fmt.Errorf("rotate %s: %w", entry.Name(), e)
		}
	}
	return nil
}

func rotateLog(path string) error {
	f, e := os.OpenFile(path, os.O_RDWR|syscall.O_NOFOLLOW, 0600)
	if e != nil {
		return e
	}
	defer f.Close()
	info, e := f.Stat()
	if e != nil {
		return e
	}
	if !info.Mode().IsRegular() || info.Size() <= logLimit {
		return nil
	}
	b := make([]byte, logLimit)
	n, e := f.ReadAt(b, info.Size()-logLimit)
	if e != nil && e != io.EOF {
		return e
	}
	if e = os.Rename(path+".1", path+".2"); e != nil && !errors.Is(e, os.ErrNotExist) {
		return e
	}
	if e = atomicWrite(path+".1", b[:n], 0600); e != nil {
		return e
	}
	return f.Truncate(0)
}

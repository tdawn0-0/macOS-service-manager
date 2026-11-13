package devplatform

import (
	"context"
	"crypto/sha256"
	"errors"
	"fmt"
	"devbase/internal/launchd"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"syscall"
	"time"
)

func runCommand(ctx context.Context, name string, args ...string) (string, error) {
	ctx, cancel := context.WithTimeout(ctx, 5*time.Minute)
	defer cancel()
	c := exec.CommandContext(ctx, name, args...)
	b, e := c.CombinedOutput()
	if e != nil {
		return "", fmt.Errorf("%s: %w: %s", filepath.Base(name), e, strings.TrimSpace(string(b)))
	}
	return string(b), nil
}
func (m *Manager) label(id string) string {
	sum := sha256.Sum256([]byte(filepath.Clean(m.Root)))
	return fmt.Sprintf("xyz.jyeu.devbase.dev.%x.%s", sum[:4], id)
}
func domain() string { return fmt.Sprintf("gui/%d", os.Getuid()) }
func (m *Manager) loaded(ctx context.Context, id string) bool {
	_, e := m.Run(ctx, "launchctl", "print", domain()+"/"+m.label(id))
	return e == nil
}

func (m *Manager) startJob(ctx context.Context, id string, args []string, dir string, env map[string]string, restart bool) (bool, error) {
	return m.startJobPolicy(ctx, id, args, dir, env, restart, 0)
}
func (m *Manager) startJobPolicy(ctx context.Context, id string, args []string, dir string, env map[string]string, restart bool, throttle int) (bool, error) {
	if !regexp.MustCompile(`^[a-zA-Z0-9][a-zA-Z0-9._-]{0,199}$`).MatchString(id) {
		return false, errors.New("invalid job identifier")
	}
	if out, e := m.Run(ctx, "launchctl", "print", domain()+"/"+m.label(id)); e == nil {
		if strings.Contains(out, "state = running") {
			return false, nil
		}
		_, e = m.Run(ctx, "launchctl", "kickstart", domain()+"/"+m.label(id))
		return e == nil, e
	}
	path := filepath.Join(m.Root, "jobs", m.label(id)+".plist")
	existing := ""
	if _, e := os.Stat(path); e == nil {
		existing = path
	}
	// Reuse the existing plist validation and atomic job writer.
	spec := launchd.Spec{Label: m.label(id), Args: args, WorkingDir: dir, Env: env, StdOut: filepath.Join(m.Root, "logs", id+".log"), StdErr: filepath.Join(m.Root, "logs", id+".err.log"), RunAtLoad: true, KeepAlive: restart}
	if throttle > 0 {
		spec.ThrottleInterval = &throttle
	}
	path, e := launchd.Save(filepath.Join(m.Root, "jobs"), spec, existing)
	if e != nil {
		return false, e
	}
	if e = os.Chmod(path, 0600); e != nil {
		return false, e
	}
	if m.initialization != nil {
		if e = m.recordInitializationJob(id, path); e != nil {
			return false, e
		}
	}
	if e = m.recordDatabaseWorkJob(id, path); e != nil {
		return false, e
	}
	_, e = m.Run(ctx, "launchctl", "bootstrap", domain(), path)
	return e == nil, e
}
func (m *Manager) stopJob(ctx context.Context, id string) error {
	ctx, cancel := context.WithTimeout(ctx, 30*time.Second)
	defer cancel()
	if e := ctx.Err(); e != nil {
		return e
	}
	target := domain() + "/" + m.label(id)
	out, e := m.Run(ctx, "launchctl", "print", target)
	if e != nil {
		if ctx.Err() != nil {
			return ctx.Err()
		}
		return nil
	}
	pid := parseJobState(out).PID
	if _, e = m.Run(ctx, "launchctl", "bootout", target); e != nil {
		return e
	}
	for {
		_, printErr := m.Run(ctx, "launchctl", "print", target)
		processGone := pid == 0 || syscall.Kill(pid, 0) == syscall.ESRCH
		if printErr != nil && processGone {
			return nil
		}
		select {
		case <-ctx.Done():
			return fmt.Errorf("waiting for %s to stop: %w", id, ctx.Err())
		case <-time.After(100 * time.Millisecond):
		}
	}
}

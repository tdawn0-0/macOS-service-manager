package devplatform

import (
	"context"
	"errors"
	"fmt"
	"io"
	"macos-service-manager/internal/launchd"
	"os"
	"path/filepath"
	"strings"
)

func (m *Manager) loginPath(id string) string {
	dir := m.LoginDir
	if dir == "" {
		dir = launchd.UserAgents().Dir
	}
	return filepath.Join(dir, m.label("login-"+id)+".plist")
}

// A rejected project save must not leave a login preference that was never
// committed. This snapshot is taken under the project lease and state lock.
func (m *Manager) snapshotLoginConfiguration(id string) (func() error, error) {
	path := m.loginPath(id)
	previous, e := readExportFile(filepath.Dir(path), filepath.Base(path), 8<<20)
	existed := e == nil
	if e != nil && !errors.Is(e, os.ErrNotExist) {
		return nil, e
	}
	return func() error {
		if existed {
			if e := atomicWrite(path, previous, 0600); e != nil {
				return e
			}
			return syncExportDirectory(filepath.Dir(path))
		}
		if e := os.Remove(path); e != nil {
			if errors.Is(e, os.ErrNotExist) {
				return nil
			}
			return e
		}
		return syncExportDirectory(filepath.Dir(path))
	}, nil
}
func (m *Manager) configureLogin(ctx context.Context, p Project) error {
	path := m.loginPath(p.ID)
	if !p.Autostart {
		if _, e := os.Stat(path); errors.Is(e, os.ErrNotExist) {
			return nil
		}
		if e := m.stopJob(ctx, "login-"+p.ID); e != nil {
			return e
		}
		if e := os.Remove(path); e != nil && !errors.Is(e, os.ErrNotExist) {
			return e
		}
		return nil
	}
	exe, e := m.helperExecutable("project-launcher")
	if e != nil {
		return e
	}
	existing := ""
	if _, e = os.Stat(path); e == nil {
		existing = path
	}
	spec := launchd.Spec{Label: m.label("login-" + p.ID), Args: []string{exe, "--dev", "up", p.ID}, WorkingDir: p.Directory, Env: map[string]string{"MSM_DEV_HOME": m.Root}, RunAtLoad: true, StdOut: filepath.Join(m.Root, "logs", "login-"+p.ID+".log"), StdErr: filepath.Join(m.Root, "logs", "login-"+p.ID+".err.log")}
	path, e = launchd.Save(filepath.Dir(path), spec, existing)
	if e != nil {
		return e
	}
	return os.Chmod(path, 0600)
}

// Environment files are data: substitutions and shell commands are never run.
func readEnvironment(path string) (map[string]string, error) {
	f, e := os.Open(path)
	if e != nil {
		return nil, e
	}
	defer f.Close()
	info, e := f.Stat()
	if e != nil {
		return nil, e
	}
	if info.Size() > 1<<20 {
		return nil, errors.New("environment file exceeds 1 MiB")
	}
	b, e := io.ReadAll(io.LimitReader(f, (1<<20)+1))
	if e != nil {
		return nil, e
	}
	if len(b) > 1<<20 {
		return nil, errors.New("environment file exceeds 1 MiB")
	}
	env := map[string]string{}
	for n, line := range strings.Split(string(b), "\n") {
		line = strings.TrimSpace(line)
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		line = strings.TrimPrefix(line, "export ")
		key, value, ok := strings.Cut(line, "=")
		key = strings.TrimSpace(key)
		value = strings.TrimSpace(value)
		if !ok || key == "" || strings.ContainsAny(key, " =\x00") {
			return nil, fmt.Errorf("invalid environment assignment on line %d", n+1)
		}
		if len(value) >= 2 && ((value[0] == '\'' && value[len(value)-1] == '\'') || (value[0] == '"' && value[len(value)-1] == '"')) {
			value = value[1 : len(value)-1]
		}
		env[key] = value
	}
	return env, nil
}

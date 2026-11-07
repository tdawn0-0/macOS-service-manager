package devplatform

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
)

type UVPython struct{ Key, Version, Path, Implementation, Arch, OS string }

var uvPythonKey = regexp.MustCompile(`^(cpython|pypy|graalpy)-[0-9][A-Za-z0-9.+_-]*$`)

func (m *Manager) UVPath() (string, error) {
	s, err := m.Read()
	if err != nil {
		return "", err
	}
	if s.UVExecutable != "" {
		f, e := os.Stat(s.UVExecutable)
		if e != nil || !filepath.IsAbs(s.UVExecutable) || !f.Mode().IsRegular() || f.Mode()&0111 == 0 {
			return "", errors.New("selected uv executable is unavailable; select an installed uv executable")
		}
		return s.UVExecutable, nil
	}
	candidates := []string{}
	if p, e := exec.LookPath("uv"); e == nil {
		candidates = append(candidates, p)
	}
	home := m.Home
	if home == "" {
		home, _ = os.UserHomeDir()
	}
	candidates = append(candidates, filepath.Join(home, ".local", "bin", "uv"), "/opt/homebrew/bin/uv")
	for _, p := range candidates {
		if !filepath.IsAbs(p) {
			continue
		}
		if f, e := os.Stat(p); e == nil && f.Mode().IsRegular() && f.Mode()&0111 != 0 {
			return p, nil
		}
	}
	return "", errors.New("uv was not found; select an installed uv executable")
}
func (m *Manager) SetUVPath(ctx context.Context, path string) error {
	if !filepath.IsAbs(path) {
		return errors.New("choose an absolute uv executable path")
	}
	f, e := os.Stat(path)
	if e != nil {
		return e
	}
	if !f.Mode().IsRegular() || f.Mode()&0111 == 0 {
		return errors.New("uv path is not executable")
	}
	out, e := m.Run(ctx, path, "--version")
	if e != nil {
		return e
	}
	if !strings.HasPrefix(out, "uv ") {
		return errors.New("executable is not uv")
	}
	return m.withState(ctx, func(s *State) error { s.UVExecutable = path; return nil })
}
func (m *Manager) uvRun(ctx context.Context, args ...string) (string, error) {
	if e := ctx.Err(); e != nil {
		return "", e
	}
	path, e := m.UVPath()
	if e != nil {
		return "", e
	}
	args = append([]string{"--no-progress", "--directory", os.TempDir()}, args...)
	if m.Home != "" {
		return m.Run(ctx, "/usr/bin/env", append([]string{"HOME=" + m.Home, path}, args...)...)
	}
	return m.Run(ctx, path, args...)
}
func (m *Manager) UVPythons(ctx context.Context, installed bool) ([]UVPython, error) {
	args := []string{"python", "list", "--managed-python", "--output-format", "json"}
	if installed {
		args = append(args, "--only-installed")
	} else {
		args = append(args, "--only-downloads", "--all-versions")
	}
	out, e := m.uvRun(ctx, args...)
	if e != nil {
		return nil, e
	}
	var rows []UVPython
	if e = json.Unmarshal([]byte(out), &rows); e != nil {
		return nil, errors.New("uv must support python list --output-format json; update uv")
	}
	if rows == nil {
		return nil, errors.New("unexpected uv version list")
	}
	for _, r := range rows {
		if !uvPythonKey.MatchString(r.Key) || r.Version == "" || installed && !filepath.IsAbs(r.Path) {
			return nil, errors.New("invalid uv version entry")
		}
	}
	return rows, nil
}
func (m *Manager) InstallUVPython(ctx context.Context, key string) error {
	rows, e := m.UVPythons(ctx, false)
	if e != nil {
		return e
	}
	found := false
	for _, r := range rows {
		if r.Key == key && r.OS == "macos" && r.Arch == "aarch64" {
			found = true
		}
	}
	if !found {
		return errors.New("select an available macOS arm64 Python version")
	}
	_, e = m.uvRun(ctx, "python", "install", "--no-bin", key)
	return e
}
func (m *Manager) UVPythonAction(ctx context.Context, key, action string) error {
	rows, e := m.UVPythons(ctx, true)
	if e != nil {
		return e
	}
	found := false
	for _, r := range rows {
		if r.Key == key && r.Path != "" {
			found = true
		}
	}
	if !found {
		return errors.New("selected uv-managed Python is no longer installed")
	}
	switch action {
	case "uninstall":
		_, e = m.uvRun(ctx, "python", "uninstall", key)
	case "default":
		_, e = m.uvRun(ctx, "python", "pin", "--global", "--no-project", key)
	default:
		return errors.New("unsupported Python action")
	}
	return e
}

func (m *Manager) UVDescription(ctx context.Context) (string, error) {
	p, e := m.UVPath()
	if e != nil {
		return "", e
	}
	version, e := m.uvRun(ctx, "--version")
	if e != nil {
		return "", e
	}
	if !strings.HasPrefix(version, "uv ") {
		return "", errors.New("selected executable is not uv")
	}
	return strings.TrimSpace(version) + " · " + p, nil
}

// UVGlobalPin reads uv's global selection without resolving or installing an
// interpreter. A directory or environment can still override this selection.
func (m *Manager) UVGlobalPin(ctx context.Context) (string, error) {
	out, e := m.uvRun(ctx, "python", "pin", "--global", "--no-project")
	if e != nil {
		if strings.Contains(out, "No Python version file found") || strings.Contains(e.Error(), "No Python version file found") {
			return "", nil
		}
		return "", e
	}
	return strings.TrimSpace(out), nil
}

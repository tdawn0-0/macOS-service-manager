package devplatform

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strings"
)

func validAdditionalRuntime(name string) bool { return name == "java" || name == "go" }
func (m *Manager) runtimeScript(name string) string {
	return filepath.Join(m.Root, "runtimes", name, "init.sh")
}
func shellLiteral(s string) string { return "'" + strings.ReplaceAll(s, "'", "'\\''") + "'" }
func (m *Manager) SetRuntimeDefault(ctx context.Context, name, id string) error {
	if !validAdditionalRuntime(name) {
		return errors.New("unsupported runtime")
	}
	return m.withStateTransaction(ctx, func(s *State) (func() error, func(), error) {
		p, ok := s.Packages[id]
		if !ok || p.Name != name {
			return nil, nil, errors.New("choose an installed runtime version")
		}
		binary, e := m.Binary(s, id, name)
		if e != nil {
			return nil, nil, e
		}
		return m.saveRuntimeDefault(s, name, id, binary)
	})
}
func (m *Manager) saveRuntimeDefault(s *State, name, id, binary string) (func() error, func(), error) {
	path := m.runtimeScript(name)
	old, readErr := os.ReadFile(path)
	if readErr != nil && !errors.Is(readErr, os.ErrNotExist) {
		return nil, nil, readErr
	}
	script := runtimeInitScript(name, binary)
	if e := atomicWrite(path, []byte(script), 0600); e != nil {
		return nil, nil, e
	}
	if s.RuntimeDefaults == nil {
		s.RuntimeDefaults = map[string]string{}
	}
	s.RuntimeDefaults[name] = id
	undo := func() error {
		if errors.Is(readErr, os.ErrNotExist) {
			return os.Remove(path)
		}
		return atomicWrite(path, old, 0600)
	}
	return undo, nil, nil
}

func (m *Manager) ConfigureRuntimeShell(ctx context.Context, name string, enable bool) error {
	if !validAdditionalRuntime(name) {
		return errors.New("unsupported runtime")
	}
	if e := ctx.Err(); e != nil {
		return e
	}
	if enable {
		// Refresh existing init scripts too, under the same state transaction
		// as default selection so an older selection cannot overwrite a new one.
		e := m.withStateTransaction(ctx, func(s *State) (func() error, func(), error) {
			id := s.RuntimeDefaults[name]
			if id == "" {
				return nil, nil, errors.New("select a default version first")
			}
			binary, e := m.runtimeBinary(s, id, name)
			if e != nil {
				return nil, nil, e
			}
			return m.saveRuntimeDefault(s, name, id, binary)
		})
		if e != nil {
			return e
		}
	}
	begin := "# BEGIN Service Manager " + name
	end := "# END Service Manager " + name
	block := begin + "\n[ ! -r " + shellLiteral(m.runtimeScript(name)) + " ] || . " + shellLiteral(m.runtimeScript(name)) + "\n" + end + "\n"
	_, e := m.configureZshShell(ctx, begin, end, block, enable)
	return e
}

func runtimeInitScript(name, binary string) string {
	script := runtimePathScript(filepath.Dir(binary))
	if name == "java" {
		script += "export JAVA_HOME=" + shellLiteral(filepath.Dir(filepath.Dir(binary))) + "\n"
	}
	return script
}

// Move the selected bin to the front even after macOS path_helper runs.
// Preserve empty PATH entries and avoid duplicates in nested shells.
func runtimePathScript(bin string) string {
	return `__msm_runtime_path() {
  local bin rest item result
  bin=` + shellLiteral(bin) + `
  rest="$PATH"
  result="$bin"
  while :; do
    item="${rest%%:*}"
    [ "$item" = "$bin" ] || result="$result:$item"
    case "$rest" in *:*) rest="${rest#*:}" ;; *) break ;; esac
  done
  export PATH="$result"
}
__msm_runtime_path
unset -f __msm_runtime_path
`
}

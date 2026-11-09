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
	undo, e := m.updateRuntimeScripts(name, []string{runtimeInitScript(name, binary), runtimeFishScript(name, binary)})
	if e != nil {
		return nil, nil, e
	}
	if s.RuntimeDefaults == nil {
		s.RuntimeDefaults = map[string]string{}
	}
	s.RuntimeDefaults[name] = id
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
	begin := "# BEGIN Devbase " + name
	end := "# END Devbase " + name
	_, e := m.configureRuntimeShellFiles(ctx, name, begin, end, enable)
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

// Both shell formats track the same default and roll back together on failure.
// A nil scripts slice removes the default's setup files.
func (m *Manager) updateRuntimeScripts(name string, scripts []string) (func() error, error) {
	type snapshot struct {
		path   string
		data   []byte
		exists bool
	}
	var snapshots []snapshot
	for _, suffix := range []string{".sh", ".fish"} {
		path := strings.TrimSuffix(m.runtimeScript(name), ".sh") + suffix
		data, err := os.ReadFile(path)
		if err != nil && !errors.Is(err, os.ErrNotExist) {
			return nil, err
		}
		snapshots = append(snapshots, snapshot{path, data, err == nil})
	}
	written := 0
	undo := func() error {
		var result error
		for i := written - 1; i >= 0; i-- {
			old := snapshots[i]
			var err error
			if old.exists {
				err = atomicWrite(old.path, old.data, 0600)
			} else {
				err = os.Remove(old.path)
				if errors.Is(err, os.ErrNotExist) {
					err = nil
				}
			}
			result = errors.Join(result, err)
		}
		return result
	}
	for i, old := range snapshots {
		var err error
		if scripts == nil {
			err = os.Remove(old.path)
			if errors.Is(err, os.ErrNotExist) {
				err = nil
			}
		} else {
			err = atomicWrite(old.path, []byte(scripts[i]), 0600)
		}
		if err != nil {
			return nil, errors.Join(err, undo())
		}
		written++
	}
	return undo, nil
}

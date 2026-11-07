package devplatform

import (
	"bytes"
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
		path := m.runtimeScript(name)
		old, readErr := os.ReadFile(path)
		if readErr != nil && !errors.Is(readErr, os.ErrNotExist) {
			return nil, nil, readErr
		}
		script := "export PATH=" + shellLiteral(filepath.Dir(binary)) + ":\"$PATH\"\n"
		if name == "java" {
			script += "export JAVA_HOME=" + shellLiteral(filepath.Dir(filepath.Dir(binary))) + "\n"
		}
		if e = atomicWrite(path, []byte(script), 0600); e != nil {
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
	})
}
func (m *Manager) ConfigureRuntimeShell(ctx context.Context, name string, enable bool) error {
	if !validAdditionalRuntime(name) {
		return errors.New("unsupported runtime")
	}
	if e := ctx.Err(); e != nil {
		return e
	}
	rc, e := m.zshrc()
	if e != nil {
		return e
	}
	config, e := readNodeShellConfiguration(rc)
	if e != nil {
		return e
	}
	begin := "# BEGIN Service Manager " + name + "\n"
	end := "# END Service Manager " + name + "\n"
	b := bytes.Clone(config.data)
	start := bytes.Index(b, []byte(begin))
	stop := bytes.Index(b, []byte(end))
	if bytes.Count(b, []byte(begin)) > 1 || bytes.Count(b, []byte(end)) > 1 {
		return errors.New("duplicate runtime shell blocks; review .zshrc")
	}
	if (start < 0) != (stop < 0) || stop >= 0 && stop < start {
		return errors.New("damaged runtime shell block; review .zshrc")
	}
	if start >= 0 {
		b = append(b[:start:start], b[stop+len(end):]...)
	}
	if enable {
		s, e := m.Read()
		if e != nil {
			return e
		}
		id := s.RuntimeDefaults[name]
		if id == "" {
			return errors.New("select a default version first")
		}
		if _, e = m.Binary(&s, id, name); e != nil {
			return e
		}
		if len(b) > 0 && b[len(b)-1] != '\n' {
			b = append(b, '\n')
		}
		b = append(b, []byte(begin+"[ ! -f "+shellLiteral(m.runtimeScript(name))+" ] || . "+shellLiteral(m.runtimeScript(name))+"\n"+end)...)
	}
	return config.commit(b)
}

package devplatform

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

func (m *Manager) zshStartupFiles() ([]string, error) {
	home, err := m.home()
	if err != nil {
		return nil, err
	}
	// An explicit fixture home must never escape to the real user's ZDOTDIR.
	if dir := os.Getenv("ZDOTDIR"); m.Home == "" && dir != "" {
		if !filepath.IsAbs(dir) {
			return nil, errors.New("ZDOTDIR must be an absolute path")
		}
		home = dir
	}
	// zshenv covers -c; zprofile restores PATH after macOS path_helper for
	// -lc; zshrc keeps interactive startup and existing integrations working.
	return []string{filepath.Join(home, ".zshenv"), filepath.Join(home, ".zprofile"), filepath.Join(home, ".zshrc")}, nil
}

func updateShellBlock(data []byte, begin, end, block string, enable bool) ([]byte, error) {
	b := bytes.Clone(data)
	legacyBegin := strings.Replace(begin, "Devbase", "Service Manager", 1)
	legacyEnd := strings.Replace(end, "Devbase", "Service Manager", 1)
	if begin == nodeBlockBegin {
		legacyBegin = "# BEGIN Service Manager node"
		legacyEnd = "# END Service Manager node"
	}
	if legacyBegin != begin && bytes.Contains(b, []byte(legacyBegin)) {
		if cleaned, err := updateShellBlock(b, legacyBegin, legacyEnd, "", false); err == nil {
			b = cleaned
		}
	}
	if begin == nodeBlockBegin {
		legacyNodeBegin := "# >>> Service Manager Node.js >>>"
		legacyNodeEnd := "# <<< Service Manager Node.js <<<"
		if bytes.Contains(b, []byte(legacyNodeBegin)) {
			if cleaned, err := updateShellBlock(b, legacyNodeBegin, legacyNodeEnd, "", false); err == nil {
				b = cleaned
			}
		}
	}
	start, stop := bytes.Index(b, []byte(begin)), bytes.Index(b, []byte(end))
	if bytes.Count(b, []byte(begin)) > 1 || bytes.Count(b, []byte(end)) > 1 {
		return nil, errors.New("duplicate runtime shell blocks")
	}
	if (start < 0) != (stop < 0) || stop >= 0 && stop < start {
		return nil, errors.New("damaged runtime shell block")
	}
	if start >= 0 {
		finish := stop + len(end)
		if start > 0 && b[start-1] != '\n' || finish < len(b) && b[finish] != '\n' {
			return nil, errors.New("runtime shell markers must be on separate lines")
		}
		if finish < len(b) {
			finish++
		}
		if enable {
			// Upgrade in place, preserving the user's ordering and surrounding text.
			return append(append(b[:start:start], block...), b[finish:]...), nil
		}
		if begin == nodeBlockBegin && start > 0 && b[start-1] == '\n' && (start == 1 || b[start-2] == '\n') {
			start--
		}
		return append(b[:start:start], b[finish:]...), nil
	}
	if !enable {
		return b, nil
	}
	if len(b) > 0 && b[len(b)-1] != '\n' {
		b = append(b, '\n')
	}
	if begin == nodeBlockBegin && len(b) > 0 {
		b = append(b, '\n')
	}
	return append(b, block...), nil
}

type shellStartupFile struct {
	path string
	fish bool
}

func (m *Manager) hasShell(name string) bool {
	if m.shellAvailable != nil {
		return m.shellAvailable(name)
	}
	var candidates []string
	if path, err := exec.LookPath(name); err == nil {
		candidates = append(candidates, path)
	}
	if path := os.Getenv("SHELL"); filepath.IsAbs(path) && filepath.Base(path) == name {
		candidates = append(candidates, path)
	}
	for _, dir := range []string{"/bin", "/usr/bin", "/opt/homebrew/bin", "/usr/local/bin"} {
		candidates = append(candidates, filepath.Join(dir, name))
	}
	for _, path := range candidates {
		if f, err := os.Stat(path); err == nil && f.Mode().IsRegular() && f.Mode().Perm()&0111 != 0 {
			return true
		}
	}
	return false
}

func (m *Manager) shellStartupFiles(enable bool) ([]shellStartupFile, error) {
	home, err := m.home()
	if err != nil {
		return nil, err
	}
	var files []shellStartupFile
	if !enable || m.hasShell("zsh") {
		paths, err := m.zshStartupFiles()
		if err != nil {
			return nil, err
		}
		for _, path := range paths {
			files = append(files, shellStartupFile{path: path})
		}
		if !enable { // Clean legacy home locations even when ZDOTDIR changed.
			for _, name := range []string{".zshenv", ".zprofile", ".zshrc"} {
				files = append(files, shellStartupFile{path: filepath.Join(home, name)})
			}
		}
	}
	if !enable || m.hasShell("bash") {
		files = append(files, shellStartupFile{path: filepath.Join(home, ".bashrc")})
		found := false
		for _, name := range []string{".bash_profile", ".bash_login", ".profile"} {
			path := filepath.Join(home, name)
			if _, err := os.Lstat(path); err == nil {
				files = append(files, shellStartupFile{path: path})
				found = true
				if enable {
					break
				}
			} else if !errors.Is(err, os.ErrNotExist) {
				return nil, err
			}
		}
		if enable && !found {
			files = append(files, shellStartupFile{path: filepath.Join(home, ".bash_profile")})
		}
	}
	if !enable || m.hasShell("fish") {
		configHome := filepath.Join(home, ".config")
		if dir := os.Getenv("XDG_CONFIG_HOME"); m.Home == "" && dir != "" {
			if !filepath.IsAbs(dir) {
				return nil, errors.New("XDG_CONFIG_HOME must be an absolute path")
			}
			configHome = dir
		}
		files = append(files, shellStartupFile{path: filepath.Join(configHome, "fish", "conf.d", "devbase.fish"), fish: true})
		if !enable {
			files = append(files, shellStartupFile{path: filepath.Join(configHome, "fish", "conf.d", "service-manager.fish"), fish: true})
			files = append(files, shellStartupFile{path: filepath.Join(home, ".config", "fish", "conf.d", "devbase.fish"), fish: true})
			files = append(files, shellStartupFile{path: filepath.Join(home, ".config", "fish", "conf.d", "service-manager.fish"), fish: true})
		}
	}
	return files, nil
}

func (m *Manager) bashEnvPath() string   { return filepath.Join(m.Root, "shell", "bash-env.sh") }
func (m *Manager) bashSetupPath() string { return filepath.Join(m.Root, "shell", "bash-setup.sh") }
func (m *Manager) fishSetupPath() string { return filepath.Join(m.Root, "shell", "bash-setup.fish") }

// BashCommand also works for GUI/AI processes that never inherited a configured
// shell environment. The caller replaces the placeholder with its command.
func (m *Manager) BashCommand() string {
	return "BASH_ENV=" + shellQuote(m.bashEnvPath()) + " /bin/bash -c 'YOUR_COMMAND'"
}

func (m *Manager) writeBashSetup(fishEnabled bool) error {
	path := shellQuote(m.bashEnvPath())
	setup := `# Written by Devbase.
# Keep dynamic user BASH_ENV expressions intact; never evaluate them here.
case "${BASH_ENV-}" in
  *` + shellQuote("$") + `*|*` + shellQuote("`") + `*) ;;
  *)
    if [ "${BASH_ENV-}" != ` + path + ` ]; then
      export MSM_PREVIOUS_BASH_ENV="${BASH_ENV-}"
    fi
    export BASH_ENV=` + path + `
    ;;
esac
`
	if err := atomicWrite(m.bashSetupPath(), []byte(setup), 0600); err != nil {
		return err
	}
	if !fishEnabled {
		return nil
	}
	fish := `# Written by Devbase.
if not string match -rq ` + fishQuote("[$`]") + ` -- "$BASH_ENV"
  if not set -q BASH_ENV; or test "$BASH_ENV" != ` + fishQuote(m.bashEnvPath()) + `
    set -gx MSM_PREVIOUS_BASH_ENV "$BASH_ENV"
  end
  set -gx BASH_ENV ` + fishQuote(m.bashEnvPath()) + `
end
`
	return atomicWrite(m.fishSetupPath(), []byte(fish), 0600)
}

func (m *Manager) bashEnvHeader() string {
	return `# Written by Devbase. BASH_ENV is read by non-interactive Bash.
# Preserve the caller's existing startup file; do not evaluate it as shell code.
case $- in
  *i*) ;;
  *)
    if [ "${__MSM_BASH_ENV_PID-}" != "$$" ]; then
      __MSM_BASH_ENV_PID=$$
      if [ -n "${MSM_PREVIOUS_BASH_ENV-}" ] && [ "$MSM_PREVIOUS_BASH_ENV" != ` + shellQuote(m.bashEnvPath()) + ` ] && [ -r "$MSM_PREVIOUS_BASH_ENV" ]; then
        . "$MSM_PREVIOUS_BASH_ENV"
      fi
    fi
    ;;
esac
`
}

func (m *Manager) configureRuntimeShellFiles(ctx context.Context, name, begin, end string, enable bool) (string, error) {
	files, err := m.shellStartupFiles(enable)
	if err != nil {
		return "", err
	}
	if enable && len(files) == 0 {
		return "", errors.New("no supported shell found")
	}
	// Remember configured locations so Disable can clean shells that have since
	// been uninstalled, or whose ZDOTDIR/XDG_CONFIG_HOME has changed.
	locationsPath := filepath.Join(m.Root, "shell", name+"-locations.json")
	locations, err := readNodeShellConfiguration(locationsPath)
	if err != nil {
		return "", err
	}
	var remembered []string
	if len(locations.data) > 0 {
		if err := json.Unmarshal(locations.data, &remembered); err != nil {
			return "", err
		}
	}
	if !enable {
		for _, path := range remembered {
			if !filepath.IsAbs(path) {
				return "", errors.New("invalid recorded shell configuration path")
			}
			files = append(files, shellStartupFile{path: path, fish: strings.HasSuffix(path, ".fish")})
		}
	}
	files = append(files, shellStartupFile{path: m.bashEnvPath()})
	var paths []string
	for _, file := range files {
		paths = append(paths, file.path)
	}
	label := strings.Join(paths, ", ")
	script := m.runtimeScript(name)
	if name == "node" {
		script = m.nodeInitPath()
	}
	body := "[ ! -r " + shellQuote(script) + " ] || . " + shellQuote(script) + "\n"
	bashBlock := begin + "\n" + body + end + "\n"
	block := begin + "\nif [ -n \"${BASH_VERSION-}${ZSH_VERSION-}\" ]; then\n" + body +
		"[ ! -r " + shellQuote(m.bashSetupPath()) + " ] || . " + shellQuote(m.bashSetupPath()) + "\nfi\n" + end + "\n"
	fishScript := strings.TrimSuffix(script, ".sh") + ".fish"
	fishBlock := begin + "\nif test -r " + fishQuote(script) + "; and test -r " + fishQuote(fishScript) + "\n  source " + fishQuote(fishScript) + "\nend\n" +
		"if test -r " + fishQuote(m.fishSetupPath()) + "\n  source " + fishQuote(m.fishSetupPath()) + "\nend\n" + end + "\n"
	type change struct {
		config nodeShellConfiguration
		data   []byte
	}
	var changes []change
	seen := map[string]bool{}
	// Validate every file before changing any; preserve symlinks and permissions.
	for _, file := range files {
		path := file.path
		if err := ctx.Err(); err != nil {
			return label, err
		}
		config, err := readNodeShellConfiguration(path)
		if err != nil {
			return label, fmt.Errorf("%s: %w", path, err)
		}
		if seen[config.target] {
			continue
		}
		seen[config.target] = true
		content, newBlock := config.data, block
		if file.fish {
			newBlock = fishBlock
		}
		if path == m.bashEnvPath() {
			newBlock = bashBlock
			if len(content) == 0 && enable {
				content = []byte(m.bashEnvHeader())
			}
		}
		data, err := updateShellBlock(content, begin, end, newBlock, enable)
		if err != nil {
			return label, fmt.Errorf("%s: %w; review this file", path, err)
		}
		if !bytes.Equal(config.data, data) {
			changes = append(changes, change{config, data})
		}
	}
	if enable && m.hasShell("fish") {
		home, err := m.home()
		if err != nil {
			return label, err
		}
		configHome := filepath.Join(home, ".config")
		if dir := os.Getenv("XDG_CONFIG_HOME"); m.Home == "" && dir != "" && filepath.IsAbs(dir) {
			configHome = dir
		}
		legacyPaths := []string{
			filepath.Join(configHome, "fish", "conf.d", "service-manager.fish"),
			filepath.Join(home, ".config", "fish", "conf.d", "service-manager.fish"),
		}
		for _, legacyPath := range legacyPaths {
			if _, err := os.Lstat(legacyPath); err == nil {
				config, err := readNodeShellConfiguration(legacyPath)
				if err != nil {
					return label, fmt.Errorf("%s: %w", legacyPath, err)
				}
				if seen[config.target] {
					continue
				}
				seen[config.target] = true
				data, err := updateShellBlock(config.data, begin, end, "", false)
				if err != nil {
					return label, fmt.Errorf("%s: %w; review this file", legacyPath, err)
				}
				if !bytes.Equal(config.data, data) {
					changes = append(changes, change{config, data})
				}
			}
		}
	}
	if enable {
		unique := map[string]bool{}
		var saved []string
		for _, path := range append(remembered, paths...) {
			if !unique[path] {
				unique[path] = true
				saved = append(saved, path)
			}
		}
		data, err := json.Marshal(saved)
		if err != nil {
			return label, err
		}
		if !bytes.Equal(locations.data, data) {
			changes = append(changes, change{locations, data})
		}
		if err := m.writeBashSetup(m.hasShell("fish")); err != nil {
			return label, err
		}
	}
	for i, change := range changes {
		err := ctx.Err()
		if err == nil {
			err = change.config.commit(change.data)
		}
		if err == nil {
			continue
		}
		// Roll back earlier writes only while their content and targets still match.
		for j := i - 1; j >= 0; j-- {
			old := changes[j]
			current, rollbackErr := readNodeShellConfiguration(old.config.rc)
			if rollbackErr == nil {
				if current.target != old.config.target || !bytes.Equal(current.data, old.data) {
					rollbackErr = errors.New("configuration changed during rollback")
				} else {
					rollbackErr = current.commit(old.config.data)
				}
			}
			if rollbackErr != nil {
				err = errors.Join(err, fmt.Errorf("restore %s: %w", old.config.rc, rollbackErr))
			}
		}
		return label, err
	}
	return label, nil
}

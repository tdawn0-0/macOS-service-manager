package devplatform

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"syscall"
	"time"
)

// What Node.js version management leaves where: the default as a link, the
// global npm packages of each version apart from the verified installation,
// and the shell setup.
func (m *Manager) nodeHome() string        { return filepath.Join(m.Root, "node") }
func (m *Manager) nodeDefaultLink() string { return filepath.Join(m.nodeHome(), "default") }
func (m *Manager) nodeGlobalPrefix(version string) string {
	return filepath.Join(m.nodeHome(), "global", version)
}
func (m *Manager) nodeInitPath() string    { return filepath.Join(m.nodeHome(), "init.sh") }
func (m *Manager) nodeWrapperPath() string { return filepath.Join(m.nodeHome(), "resolve") }

// InstalledNode is a Node.js version in the workspace.
type InstalledNode struct {
	ID      string
	Version string
	LTS     string
	Default bool
	Package Package
}

func (m *Manager) cachedNodeReleases() []NodeRelease {
	var cached nodeIndexCache
	if b, e := os.ReadFile(m.nodeIndexPath()); e == nil && json.Unmarshal(b, &cached) == nil {
		return cached.Releases
	}
	return nil
}

func installedNodes(s State, index []NodeRelease) []InstalledNode {
	lts := map[string]string{}
	for _, r := range index {
		lts[r.Version] = r.LTS
	}
	var out []InstalledNode
	for id, p := range s.Packages {
		if p.Name == "node" && p.Formula == "" && (p.Arch == "arm64" || p.Arch == "universal") {
			out = append(out, InstalledNode{ID: id, Version: p.Version, LTS: lts[p.Version], Default: id == s.NodeDefault, Package: p})
		}
	}
	sort.Slice(out, func(i, j int) bool { return versionGreater(out[i].Version, out[j].Version) })
	return out
}

// InstalledNodes lists the installed Node.js versions, newest first.
func (m *Manager) InstalledNodes() ([]InstalledNode, error) {
	s, e := m.Read()
	if e != nil {
		return nil, e
	}
	return installedNodes(s, m.cachedNodeReleases()), nil
}

// defaultFirstNode makes the first Node.js version installed the default, as
// fnm does; later installs leave the default alone.
func (m *Manager) defaultFirstNode(ctx context.Context, p Package) error {
	if p.Name != "node" || p.Formula != "" {
		return nil
	}
	return m.withState(ctx, func(s *State) error {
		if _, ok := s.Packages[p.ID()]; !ok || s.NodeDefault != "" {
			return nil
		}
		s.NodeDefault = p.ID()
		return m.linkNodeDefault(*s)
	})
}

// SetNodeDefault makes an installed version the one a shell uses outside a
// directory that asks for another.
func (m *Manager) SetNodeDefault(ctx context.Context, id string) error {
	return m.withState(ctx, func(s *State) error {
		p, ok := s.Packages[id]
		if !ok || p.Name != "node" {
			return errors.New("that Node.js version is not installed")
		}
		s.NodeDefault = id
		return m.linkNodeDefault(*s)
	})
}

// linkNodeDefault points the stable path of the default at its installation.
func (m *Manager) linkNodeDefault(s State) error {
	link := m.nodeDefaultLink()
	if e := os.MkdirAll(m.nodeHome(), 0700); e != nil {
		return e
	}
	if e := os.Remove(link); e != nil && !errors.Is(e, os.ErrNotExist) {
		return e
	}
	p, ok := s.Packages[s.NodeDefault]
	if !ok {
		return nil
	}
	return os.Symlink(m.packageDir(p), link)
}

// A NodeSelection is the version a directory runs.
type NodeSelection struct {
	ID      string
	Version string
	Dir     string // the installation
	Prefix  string // where npm install -g puts packages for this version
	Spec    string // what was asked, empty for the default
	Source  string // the file, or "default"
	System  bool   // .nvmrc says "system": no managed version applies
}

// Bin is the installation's bin directory.
func (n NodeSelection) Bin() string { return filepath.Join(n.Dir, "bin") }

// Path is what goes ahead of PATH: the global packages, then Node.js.
func (n NodeSelection) Path() string {
	if n.System {
		return ""
	}
	return filepath.Join(n.Prefix, "bin") + string(os.PathListSeparator) + n.Bin()
}

// NodeMissingError says that what a directory asks for is not installed.
type NodeMissingError struct{ Spec, Source string }

func (e *NodeMissingError) Error() string {
	return fmt.Sprintf("Node.js %s, asked for by %s, is not installed. Install it in Devbase: Software, Node.js", e.Spec, e.Source)
}

// NoNodeDefaultError says that nothing is installed or chosen.
var ErrNoNodeDefault = errors.New("no default Node.js version. Install one in Devbase: Software, Node.js")

// NodeVersionFile finds what a directory, or the nearest one above it, asks
// for: .node-version, .nvmrc, then the engines.node of package.json.
func NodeVersionFile(dir string) (spec, source string) {
	dir, e := filepath.Abs(dir)
	if e != nil {
		return "", ""
	}
	for {
		for _, name := range []string{".node-version", ".nvmrc"} {
			if b, e := os.ReadFile(filepath.Join(dir, name)); e == nil {
				for _, line := range strings.Split(string(b), "\n") {
					if line = strings.TrimSpace(strings.SplitN(line, "#", 2)[0]); line != "" {
						return line, filepath.Join(dir, name)
					}
				}
			}
		}
		if b, e := os.ReadFile(filepath.Join(dir, "package.json")); e == nil {
			var pkg struct {
				Engines struct {
					Node string `json:"node"`
				} `json:"engines"`
			}
			if json.Unmarshal(b, &pkg) == nil && strings.TrimSpace(pkg.Engines.Node) != "" {
				return strings.TrimSpace(pkg.Engines.Node), filepath.Join(dir, "package.json")
			}
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			return "", ""
		}
		dir = parent
	}
}

func (m *Manager) selectionOf(n InstalledNode, spec, source string) NodeSelection {
	return NodeSelection{ID: n.ID, Version: n.Version, Dir: m.packageDir(n.Package), Prefix: m.nodeGlobalPrefix(n.Version), Spec: spec, Source: source}
}

// SelectNode chooses the version for a directory. override, when set, beats
// the directory's files, which beat the default.
func (m *Manager) SelectNode(dir, override string) (NodeSelection, error) {
	s, e := m.Read()
	if e != nil {
		return NodeSelection{}, e
	}
	installed := installedNodes(s, m.cachedNodeReleases())
	spec, source := strings.TrimSpace(override), "the requested version"
	if spec == "" {
		spec, source = NodeVersionFile(dir)
	}
	if spec == "" {
		for _, n := range installed {
			if n.ID == s.NodeDefault {
				return m.selectionOf(n, "", "default"), nil
			}
		}
		return NodeSelection{}, ErrNoNodeDefault
	}
	if strings.EqualFold(spec, "system") {
		return NodeSelection{Spec: spec, Source: source, System: true}, nil
	}
	candidates := make([]NodeRelease, len(installed))
	for i, n := range installed {
		candidates[i] = NodeRelease{Version: n.Version, LTS: n.LTS}
	}
	best, ok, e := PickNode(spec, candidates)
	if e != nil {
		return NodeSelection{}, fmt.Errorf("%s: %w", source, e)
	}
	if !ok {
		return NodeSelection{}, &NodeMissingError{Spec: spec, Source: source}
	}
	for _, n := range installed {
		if n.Version == best.Version {
			return m.selectionOf(n, spec, source), nil
		}
	}
	return NodeSelection{}, &NodeMissingError{Spec: spec, Source: source}
}

// The shell setup. The rc file only sources init.sh, which this app rewrites,
// so a change here reaches every shell without editing a dotfile again.
const (
	nodeBlockBegin = "# >>> Devbase Node.js >>>"
	nodeBlockEnd   = "# <<< Devbase Node.js <<<"
)

func shellQuote(s string) string { return "'" + strings.ReplaceAll(s, "'", `'\''`) + "'" }

func (m *Manager) nodeInitScript() string {
	return `# Written by Devbase. Changes are overwritten.
MSM_NODE_RESOLVE=` + shellQuote(m.nodeWrapperPath()) + `
export MSM_NODE_HOME=` + shellQuote(m.nodeHome()) + `

# Puts the Node.js version for the current directory first in PATH: the one
# .node-version, .nvmrc or engines.node of package.json names, else the default.
__msm_node_apply() {
  local out frag prefix rest item clean have old_global old_bin new_global new_bin
  out="$("$MSM_NODE_RESOLVE" node path "$PWD")" || out=""
  frag="${out%%$'\n'*}"
  prefix=""
  case "$out" in *$'\n'*) prefix="${out#*$'\n'}" ;; esac
  old_global="${__MSM_NODE_PATH%%:*}"
  old_bin="${__MSM_NODE_PATH#*:}"
  new_global="${frag%%:*}"
  new_bin="${frag#*:}"
  rest="$PATH"
  clean=""
  have=""
  while :; do
    item="${rest%%:*}"
    if { [ -n "$old_global" ] && [ "$item" = "$old_global" ]; } ||
       { [ -n "$old_bin" ] && [ "$item" = "$old_bin" ]; } ||
       { [ -n "$new_global" ] && [ "$item" = "$new_global" ]; } ||
       { [ -n "$new_bin" ] && [ "$item" = "$new_bin" ]; }; then
      :
    else
      clean="$clean${have:+:}$item"
      have=1
    fi
    case "$rest" in *:*) rest="${rest#*:}" ;; *) break ;; esac
  done
  export PATH="$clean"
  if [ -n "$frag" ]; then
    export PATH="$frag${have:+:$clean}"
    export npm_config_prefix="$prefix"
  else
    unset npm_config_prefix
  fi
  export __MSM_NODE_PATH="$frag"
}

if [ -n "$ZSH_VERSION" ]; then
  autoload -Uz add-zsh-hook 2>/dev/null && add-zsh-hook chpwd __msm_node_apply
elif [ -n "$BASH_VERSION" ]; then
  # Non-interactive AI commands often cd before invoking Node. Refresh at each
  # tool invocation too, without replacing the user's cd, traps or functions.
  declare -F node >/dev/null || node() { __msm_node_apply; command node "$@"; }
  declare -F npm >/dev/null || npm() { __msm_node_apply; command npm "$@"; }
  declare -F npx >/dev/null || npx() { __msm_node_apply; command npx "$@"; }
  declare -F corepack >/dev/null || corepack() { __msm_node_apply; command corepack "$@"; }
  if [[ $- == *i* ]]; then
  __msm_node_prompt() {
    [ "$PWD" = "$__MSM_NODE_PWD" ] && return 0
    __MSM_NODE_PWD="$PWD"
    __msm_node_apply
  }
  case ";$PROMPT_COMMAND;" in *";__msm_node_prompt;"*) ;; *) PROMPT_COMMAND="__msm_node_prompt${PROMPT_COMMAND:+;$PROMPT_COMMAND}" ;; esac
  fi
fi
__msm_node_apply
`
}

func (m *Manager) home() (string, error) {
	if m.Home != "" {
		return m.Home, nil
	}
	return os.UserHomeDir()
}

func (m *Manager) nodeBlock() string {
	return nodeBlockBegin + "\n[ ! -r " + shellQuote(m.nodeInitPath()) + " ] || . " + shellQuote(m.nodeInitPath()) + "\n" + nodeBlockEnd + "\n"
}

// NodeShellEnabled requires all supported shells and startup modes, so older rc-only setups offer
// Enable again to upgrade without silently editing more user files.
func (m *Manager) NodeShellEnabled() bool {
	if !m.RuntimeShellEnabled("node") {
		return false
	}
	data, err := os.ReadFile(m.nodeInitPath())
	if err != nil || string(data) != m.nodeInitScript() {
		return false
	}
	if m.hasShell("fish") {
		data, err = os.ReadFile(filepath.Join(m.nodeHome(), "init.fish"))
		if err != nil || string(data) != m.nodeFishScript() {
			return false
		}
	}
	return true
}

func (m *Manager) RuntimeShellEnabled(name string) bool {
	begin, end := "# BEGIN Devbase "+name, "# END Devbase "+name
	if name == "node" {
		begin, end = nodeBlockBegin, nodeBlockEnd
	}

	files, e := m.shellStartupFiles(true)
	if e != nil || len(files) == 0 {
		return false
	}
	files = append(files, shellStartupFile{path: m.bashEnvPath()})
	for _, file := range files {
		b, e := os.ReadFile(file.path)
		if e != nil {
			return false
		}
		hasNew := bytes.Contains(b, []byte(begin)) && bytes.Contains(b, []byte(end))
		legacyBegin := strings.Replace(begin, "Devbase", "Service Manager", 1)
		legacyEnd := strings.Replace(end, "Devbase", "Service Manager", 1)
		hasLegacy := bytes.Contains(b, []byte(legacyBegin)) && bytes.Contains(b, []byte(legacyEnd))
		if !hasNew && !hasLegacy {
			return false
		}
	}
	return true
}

// WriteNodeShell writes the command wrapper and the setup script, without
// touching any dotfile.
func (m *Manager) WriteNodeShell() error {
	exe, e := m.helperExecutable("cli")
	if e != nil {
		return e
	}
	if e = os.MkdirAll(m.nodeHome(), 0700); e != nil {
		return e
	}
	wrapper := "#!/bin/sh\nif [ -x " + shellQuote(exe) + " ]; then exec " + shellQuote(exe) + " --dev \"$@\"; fi\n"
	if m.usesUserStorage() {
		fallback := "/Applications/Devbase.app/Contents/MacOS/Devbase"
		wrapper += "if [ -x " + shellQuote(fallback) + " ]; then exec " + shellQuote(fallback) + " --dev \"$@\"; fi\n"
	}
	wrapper += "exit 1\n"
	if e = atomicWrite(m.nodeWrapperPath(), []byte(wrapper), 0700); e != nil {
		return e
	}
	if e = atomicWrite(m.nodeInitPath(), []byte(m.nodeInitScript()), 0600); e != nil {
		return e
	}
	return atomicWrite(filepath.Join(m.nodeHome(), "init.fish"), []byte(m.nodeFishScript()), 0600)
}

// RefreshNodeShellIfPresent updates an existing Node.js command wrapper and
// setup scripts to point at the current helper executable without creating
// them when Node shell integration has never been written or touching dotfiles.
func (m *Manager) RefreshNodeShellIfPresent() error {
	if _, err := os.Stat(m.nodeWrapperPath()); err != nil {
		if errors.Is(err, os.ErrNotExist) {
			return nil
		}
		return err
	}
	return m.WriteNodeShell()
}

// nodeShellConfiguration keeps the observed identity separate from the path
// replaced by atomicWrite, so a dotfile symlink remains intact.
type nodeShellConfiguration struct {
	rc, target  string
	entry, file os.FileInfo
	data        []byte
	mode        os.FileMode
}

func readNodeShellConfiguration(rc string) (nodeShellConfiguration, error) {
	c := nodeShellConfiguration{rc: rc, target: rc, mode: 0644}
	entry, e := os.Lstat(rc)
	if errors.Is(e, os.ErrNotExist) {
		return c, nil
	}
	if e != nil {
		return c, e
	}
	c.entry = entry
	target, e := filepath.EvalSymlinks(rc)
	if e != nil {
		return c, fmt.Errorf("cannot resolve shell configuration; repair the link and retry: %w", e)
	}
	c.target = target
	file, e := os.Lstat(target)
	if e != nil {
		return c, e
	}
	if !file.Mode().IsRegular() {
		return c, errors.New("shell configuration target must be a regular file")
	}
	if st, ok := file.Sys().(*syscall.Stat_t); ok && st.Nlink > 1 {
		return c, errors.New("hard-linked shell configuration is not supported; use a single file or symbolic link")
	}
	c.file = file
	c.mode = file.Mode().Perm()
	c.data, e = os.ReadFile(target)
	return c, e
}
func (c nodeShellConfiguration) commit(data []byte) error {
	current, e := readNodeShellConfiguration(c.rc)
	if e != nil {
		return fmt.Errorf("shell configuration changed; review it and retry: %w", e)
	}
	same := c.target == current.target && c.mode == current.mode && bytes.Equal(c.data, current.data)
	if c.entry == nil {
		same = same && current.entry == nil
	} else {
		same = same && current.entry != nil && os.SameFile(c.entry, current.entry)
	}
	if c.file == nil {
		same = same && current.file == nil
	} else {
		same = same && current.file != nil && os.SameFile(c.file, current.file)
	}
	if !same {
		return errors.New("shell configuration changed; review it and retry")
	}
	return atomicWrite(c.target, data, c.mode)
}

// EnableNodeShell covers zsh, Bash and fish, including non-interactive sessions.
func (m *Manager) EnableNodeShell(ctx context.Context) (string, error) {
	if e := ctx.Err(); e != nil {
		return "", e
	}
	if e := m.WriteNodeShell(); e != nil {
		return "", e
	}
	return m.configureRuntimeShellFiles(ctx, "node", nodeBlockBegin, nodeBlockEnd, true)
}

// DisableNodeShell removes only this runtime's marked blocks in all modes.
func (m *Manager) DisableNodeShell(ctx context.Context) (string, error) {
	return m.configureRuntimeShellFiles(ctx, "node", nodeBlockBegin, nodeBlockEnd, false)
}

// NodeIndexAge says how old the cached release list is, zero when there is none.
func (m *Manager) NodeIndexAge() time.Duration {
	var cached nodeIndexCache
	if b, e := os.ReadFile(m.nodeIndexPath()); e == nil && json.Unmarshal(b, &cached) == nil {
		return time.Since(cached.FetchedAt)
	}
	return 0
}

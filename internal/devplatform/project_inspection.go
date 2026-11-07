package devplatform

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"regexp"
	"strings"
)

var exactRuntimeVersion = regexp.MustCompile(`^[0-9]+\.[0-9]+\.[0-9]+$`)

type RuntimeHint struct {
	Name    string `json:"name"`
	Version string `json:"version"`
	Source  string `json:"source"`
}

func (h RuntimeHint) ExactVersion() string {
	version := strings.TrimPrefix(strings.TrimSpace(h.Version), "v")
	if exactRuntimeVersion.MatchString(version) {
		return version
	}
	return ""
}
func RuntimeConflicts(hints []RuntimeHint) []string {
	versions := map[string]string{}
	conflicts := map[string]bool{}
	for _, hint := range hints {
		version := hint.ExactVersion()
		if version == "" {
			continue
		}
		if previous, ok := versions[hint.Name]; ok && previous != version {
			conflicts[hint.Name] = true
		}
		versions[hint.Name] = version
	}
	var out []string
	for _, name := range []string{"node", "php", "go"} {
		if conflicts[name] {
			out = append(out, name)
		}
	}
	return out
}

type ProjectInspection struct {
	RuntimeHints []RuntimeHint
	Services     []Service
}

// These files are declarations only. No project program or shell is evaluated.
func InspectProject(dir string) (ProjectInspection, error) {
	result := ProjectInspection{Services: []Service{}}
	stat, e := os.Stat(dir)
	if !filepath.IsAbs(dir) || e != nil || !stat.IsDir() {
		return result, errors.New("select an existing absolute project directory")
	}
	for _, file := range []struct{ name, runtime string }{{".nvmrc", "node"}, {".node-version", "node"}, {".php-version", "php"}} {
		b, e := readProjectFile(dir, file.name, 1024)
		if errors.Is(e, os.ErrNotExist) {
			continue
		}
		if e != nil {
			return result, e
		}
		var declaration string
		for _, line := range strings.Split(string(b), "\n") {
			line = strings.TrimSpace(strings.SplitN(line, "#", 2)[0])
			if line == "" {
				continue
			}
			if declaration != "" {
				return result, fmt.Errorf("%s must contain one version declaration", file.name)
			}
			declaration = line
		}
		if declaration != "" {
			if e = addRuntimeHint(&result, file.runtime, declaration, file.name); e != nil {
				return result, e
			}
		}
	}
	if b, e := readProjectFile(dir, "package.json", 1<<20); e == nil {
		var node struct {
			Scripts map[string]string `json:"scripts"`
			Engines map[string]string `json:"engines"`
			Volta   struct {
				Node string `json:"node"`
			} `json:"volta"`
		}
		if e = json.Unmarshal(b, &node); e != nil {
			return result, fmt.Errorf("read package.json: %w", e)
		}
		for _, declaration := range []struct{ value, source string }{{node.Volta.Node, "package.json · volta.node"}, {node.Engines["node"], "package.json · engines.node"}} {
			if declaration.value != "" {
				if e = addRuntimeHint(&result, "node", declaration.value, declaration.source); e != nil {
					return result, e
				}
			}
		}
		script := ""
		if node.Scripts["dev"] != "" {
			script = "dev"
		} else if node.Scripts["start"] != "" {
			script = "start"
		}
		if script != "" {
			result.Services = []Service{{ID: "web", Command: []string{"npm", "run", script}, Health: "tcp://127.0.0.1:3000"}}
		}
	} else if !errors.Is(e, os.ErrNotExist) {
		return result, e
	}
	php := false
	if b, e := readProjectFile(dir, "composer.json", 1<<20); e == nil {
		php = true
		var composer struct {
			Require map[string]json.RawMessage `json:"require"`
		}
		if e = json.Unmarshal(b, &composer); e != nil {
			return result, fmt.Errorf("read composer.json: %w", e)
		}
		if value := composer.Require["php"]; len(value) > 0 {
			var version string
			if e = json.Unmarshal(value, &version); e != nil {
				return result, errors.New("composer.json PHP requirement must be text")
			}
			if e = addRuntimeHint(&result, "php", version, "composer.json · require.php"); e != nil {
				return result, e
			}
		}
	} else if !errors.Is(e, os.ErrNotExist) {
		return result, e
	}
	public := projectFileExists(dir, "public/index.php")
	php = php || public || projectFileExists(dir, "index.php")
	if php && len(result.Services) == 0 {
		args := []string{"php", "-S", "127.0.0.1:8000"}
		if public {
			args = append(args, "-t", "public")
		}
		result.Services = []Service{{ID: "web", Command: args, Health: "http://127.0.0.1:8000"}}
	}
	if len(result.Services) == 0 {
		if b, e := readProjectFile(dir, "go.mod", 128<<10); e == nil {
			result.Services = []Service{{ID: "web", Command: []string{"go", "run", "."}, Health: "tcp://127.0.0.1:8080"}}
			for _, line := range strings.Split(string(b), "\n") {
				fields := strings.Fields(line)
				if len(fields) == 2 && fields[0] == "go" {
					if e = addRuntimeHint(&result, "go", fields[1], "go.mod · go"); e != nil {
						return result, e
					}
					break
				}
			}
		} else if !errors.Is(e, os.ErrNotExist) {
			return result, e
		}
	}
	return result, nil
}

func addRuntimeHint(result *ProjectInspection, name, version, source string) error {
	version = strings.TrimSpace(version)
	if len(version) > 128 || strings.ContainsAny(version, "\x00\r\n") {
		return fmt.Errorf("invalid runtime declaration in %s", source)
	}
	if version != "" {
		result.RuntimeHints = append(result.RuntimeHints, RuntimeHint{Name: name, Version: version, Source: source})
	}
	return nil
}

func readProjectFile(dir, name string, limit int64) ([]byte, error) {
	if !safeRelative(name) {
		return nil, errors.New("project declaration path must be relative")
	}
	path, e := filepath.EvalSymlinks(filepath.Join(dir, name))
	if e != nil {
		return nil, e
	}
	if !inside(dir, path) {
		return nil, fmt.Errorf("%s points outside the project", name)
	}
	f, e := os.Open(path)
	if e != nil {
		return nil, e
	}
	defer f.Close()
	info, e := f.Stat()
	if e != nil {
		return nil, e
	}
	if !info.Mode().IsRegular() {
		return nil, fmt.Errorf("%s is not a regular file", name)
	}
	if info.Size() > limit {
		return nil, fmt.Errorf("%s exceeds its size limit", name)
	}
	b, e := io.ReadAll(io.LimitReader(f, limit+1))
	if e != nil {
		return nil, e
	}
	if int64(len(b)) > limit {
		return nil, fmt.Errorf("%s exceeds its size limit", name)
	}
	return b, nil
}
func projectFileExists(dir, name string) bool {
	path, e := filepath.EvalSymlinks(filepath.Join(dir, name))
	if e != nil || !inside(dir, path) {
		return false
	}
	info, e := os.Stat(path)
	return e == nil && info.Mode().IsRegular()
}

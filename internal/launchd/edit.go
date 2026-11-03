package launchd

import (
	"bytes"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"

	"howett.net/plist"

	"macos-service-manager/internal/system"
)

// Spec is the part of a launch agent's property list that the editor
// changes. The keys it does not know stay as they were.
type Spec struct {
	Label string
	// Args is ProgramArguments: the program, then its arguments.
	Args       []string
	WorkingDir string
	RunAtLoad  bool
	KeepAlive  bool
	// KeepAliveCustom says the file has KeepAlive conditions, which the
	// editor leaves alone; KeepAlive is true for them.
	KeepAliveCustom bool
	// StartInterval runs the job every this many seconds; 0 is never.
	StartInterval int
	Env           map[string]string
	StdOut        string
	StdErr        string
}

var labelRE = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9._-]*$`)

// Validate checks that the spec makes a launch agent that launchd accepts.
func (s Spec) Validate() error {
	if !labelRE.MatchString(s.Label) {
		return fmt.Errorf("the label may hold letters, digits, dots, dashes and underscores, as com.example.job")
	}
	if len(s.Args) == 0 {
		return fmt.Errorf("give the program to run")
	}
	if !filepath.IsAbs(s.Args[0]) {
		return fmt.Errorf("the program %q must be an absolute path", s.Args[0])
	}
	for name, path := range map[string]string{"working directory": s.WorkingDir, "standard output": s.StdOut, "standard error": s.StdErr} {
		if path != "" && !filepath.IsAbs(path) {
			return fmt.Errorf("the %s %q must be an absolute path", name, path)
		}
	}
	if s.StartInterval < 0 {
		return fmt.Errorf("the interval cannot be negative")
	}
	for k := range s.Env {
		if k == "" || strings.Contains(k, "=") {
			return fmt.Errorf("the environment variable name %q is not valid", k)
		}
	}
	return nil
}

// LoadSpec reads the editable part of the property list at path.
func LoadSpec(path string) (Spec, error) {
	f, err := readFile(path)
	if err != nil {
		return Spec{}, err
	}
	s := Spec{Label: f.Label, Args: f.ProgramArguments, Env: f.Environment}
	if s.Args == nil && f.Program != nil {
		s.Args = []string{*f.Program}
	}
	if f.WorkingDirectory != nil {
		s.WorkingDir = *f.WorkingDirectory
	}
	if f.RunAtLoad != nil {
		s.RunAtLoad = *f.RunAtLoad
	}
	switch k := f.KeepAlive.(type) {
	case bool:
		s.KeepAlive = k
	case nil:
	default:
		s.KeepAlive, s.KeepAliveCustom = true, true
	}
	if f.StartInterval != nil {
		s.StartInterval = *f.StartInterval
	}
	if f.StandardOutPath != nil {
		s.StdOut = *f.StandardOutPath
	}
	if f.StandardErrorPath != nil {
		s.StdErr = *f.StandardErrorPath
	}
	return s, nil
}

// Marshal writes the spec into a property list. Given the current file, it
// keeps the keys the spec does not hold.
func (s Spec) Marshal(current []byte) ([]byte, error) {
	dict := map[string]any{}
	if current != nil {
		if _, err := plist.Unmarshal(current, &dict); err != nil {
			return nil, fmt.Errorf("the current file is not a property list: %w", err)
		}
	}
	set := func(key string, ok bool, v any) {
		if ok {
			dict[key] = v
		} else {
			delete(dict, key)
		}
	}
	dict["Label"] = s.Label
	dict["ProgramArguments"] = s.Args
	dict["RunAtLoad"] = s.RunAtLoad
	if !s.KeepAliveCustom {
		dict["KeepAlive"] = s.KeepAlive
	}
	set("WorkingDirectory", s.WorkingDir != "", s.WorkingDir)
	set("StartInterval", s.StartInterval > 0, s.StartInterval)
	set("EnvironmentVariables", len(s.Env) > 0, s.Env)
	set("StandardOutPath", s.StdOut != "", s.StdOut)
	set("StandardErrorPath", s.StdErr != "", s.StdErr)
	// ProgramArguments holds the program: a Program key would only repeat it.
	if p, ok := dict["Program"].(string); ok && len(s.Args) > 0 && p == s.Args[0] {
		delete(dict, "Program")
	}
	var buf bytes.Buffer
	enc := plist.NewEncoderForFormat(&buf, plist.XMLFormat)
	enc.Indent("\t")
	if err := enc.Encode(dict); err != nil {
		return nil, err
	}
	return buf.Bytes(), nil
}

// Save writes the spec as a launch agent in dir and returns its path. To
// edit the file at existing, pass its path; to make a new one, "", which
// fails if the label is taken.
func Save(dir string, s Spec, existing string) (string, error) {
	if err := s.Validate(); err != nil {
		return "", err
	}
	path := existing
	var current []byte
	if existing == "" {
		path = filepath.Join(dir, s.Label+".plist")
		if _, err := os.Stat(path); err == nil {
			return "", fmt.Errorf("%s already exists", filepath.Base(path))
		}
	} else {
		var err error
		if current, err = os.ReadFile(existing); err != nil {
			return "", err
		}
	}
	data, err := s.Marshal(current)
	if err != nil {
		return "", err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return "", err
	}
	tmp, err := os.CreateTemp(filepath.Dir(path), ".msm-*.plist")
	if err != nil {
		return "", err
	}
	defer os.Remove(tmp.Name())
	if _, err := tmp.Write(data); err != nil {
		tmp.Close()
		return "", err
	}
	if err := tmp.Close(); err != nil {
		return "", err
	}
	if _, err := system.Run("plutil", "-lint", tmp.Name()); err != nil {
		return "", fmt.Errorf("the property list is not valid: %w", err)
	}
	if err := os.Chmod(tmp.Name(), 0o644); err != nil {
		return "", err
	}
	return path, os.Rename(tmp.Name(), path)
}

// ParseLines splits text into its trimmed, non-empty lines.
func ParseLines(text string) []string {
	var lines []string
	for _, l := range strings.Split(text, "\n") {
		if l = strings.TrimSpace(l); l != "" {
			lines = append(lines, l)
		}
	}
	return lines
}

// ParseEnv reads lines of KEY=value.
func ParseEnv(text string) (map[string]string, error) {
	var env map[string]string
	for _, l := range ParseLines(text) {
		k, v, ok := strings.Cut(l, "=")
		if !ok || strings.TrimSpace(k) == "" {
			return nil, fmt.Errorf("%q is not KEY=value", l)
		}
		if env == nil {
			env = map[string]string{}
		}
		env[strings.TrimSpace(k)] = strings.TrimSpace(v)
	}
	return env, nil
}

// FormatEnv writes env as lines of KEY=value, sorted.
func FormatEnv(env map[string]string) string {
	keys := make([]string, 0, len(env))
	for k := range env {
		keys = append(keys, k)
	}
	slices.Sort(keys)
	lines := make([]string, len(keys))
	for i, k := range keys {
		lines[i] = k + "=" + env[k]
	}
	return strings.Join(lines, "\n")
}

package launchd

import (
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"howett.net/plist"
)

func TestValidate(t *testing.T) {
	ok := Spec{Label: "com.example.job", Args: []string{"/bin/echo", "hi"}}
	if err := ok.Validate(); err != nil {
		t.Fatalf("valid spec: %v", err)
	}
	for name, mutate := range map[string]func(*Spec){
		"empty label":       func(s *Spec) { s.Label = "" },
		"label with slash":  func(s *Spec) { s.Label = "a/b" },
		"label with space":  func(s *Spec) { s.Label = "a b" },
		"no program":        func(s *Spec) { s.Args = nil },
		"relative program":  func(s *Spec) { s.Args = []string{"echo"} },
		"relative workdir":  func(s *Spec) { s.WorkingDir = "tmp" },
		"relative stdout":   func(s *Spec) { s.StdOut = "out.log" },
		"negative interval": func(s *Spec) { s.StartInterval = -1 },
		"bad env name":      func(s *Spec) { s.Env = map[string]string{"A=B": "c"} },
	} {
		s := ok
		mutate(&s)
		if err := s.Validate(); err == nil {
			t.Errorf("%s should be invalid", name)
		}
	}
}

func decode(t *testing.T, data []byte) map[string]any {
	t.Helper()
	m := map[string]any{}
	if _, err := plist.Unmarshal(data, &m); err != nil {
		t.Fatalf("not a plist: %v\n%s", err, data)
	}
	return m
}

func TestMarshalKeepsUnknownKeysAndConditions(t *testing.T) {
	current := []byte(xmlPlist)
	spec, err := specFromBytes(t, current)
	if err != nil {
		t.Fatal(err)
	}
	if !spec.KeepAliveCustom || !spec.KeepAlive {
		t.Fatalf("KeepAlive conditions should be kept as they are: %+v", spec)
	}
	spec.StartInterval = 0
	spec.StdErr = ""
	spec.Args = []string{"/bin/echo", "changed"}
	out, err := spec.Marshal(append(current[:len(current)-len("</dict></plist>")], []byte("<key>Nice</key><integer>5</integer></dict></plist>")...))
	if err != nil {
		t.Fatal(err)
	}
	m := decode(t, out)
	if _, ok := m["KeepAlive"].(map[string]any); !ok {
		t.Errorf("KeepAlive conditions lost: %v", m["KeepAlive"])
	}
	if n, _ := m["Nice"].(uint64); n != 5 {
		t.Errorf("an unknown key was lost: %v", m["Nice"])
	}
	if _, ok := m["StartInterval"]; ok {
		t.Error("an interval of 0 removes the key")
	}
	if _, ok := m["StandardErrorPath"]; ok {
		t.Error("an empty path removes the key")
	}
	if args, _ := m["ProgramArguments"].([]any); len(args) != 2 || args[1] != "changed" {
		t.Errorf("args = %v", m["ProgramArguments"])
	}
}

func specFromBytes(t *testing.T, data []byte) (Spec, error) {
	t.Helper()
	path := filepath.Join(t.TempDir(), "x.plist")
	if err := os.WriteFile(path, data, 0o644); err != nil {
		t.Fatal(err)
	}
	return LoadSpec(path)
}

func TestSaveNewAndEdit(t *testing.T) {
	stubRun(t, "")
	dir := t.TempDir()
	spec := Spec{Label: "com.example.new", Args: []string{"/bin/sleep", "60"}, RunAtLoad: true, KeepAlive: true,
		StartInterval: 30, Env: map[string]string{"A": "1"}, StdOut: "/tmp/new.log"}
	path, err := Save(dir, spec, "")
	if err != nil {
		t.Fatal(err)
	}
	if path != filepath.Join(dir, "com.example.new.plist") {
		t.Errorf("path = %s", path)
	}
	loaded, err := LoadSpec(path)
	if err != nil {
		t.Fatal(err)
	}
	if loaded.Label != spec.Label || !slices.Equal(loaded.Args, spec.Args) || !loaded.RunAtLoad || !loaded.KeepAlive ||
		loaded.StartInterval != 30 || loaded.Env["A"] != "1" || loaded.StdOut != "/tmp/new.log" {
		t.Errorf("round trip: %+v", loaded)
	}
	if _, err := Save(dir, spec, ""); err == nil || !strings.Contains(err.Error(), "already exists") {
		t.Errorf("a taken label should fail: %v", err)
	}
	// Edit the same file.
	loaded.Args = []string{"/bin/sleep", "120"}
	if _, err := Save(dir, loaded, path); err != nil {
		t.Fatal(err)
	}
	again, _ := LoadSpec(path)
	if again.Args[1] != "120" {
		t.Errorf("edit not saved: %+v", again)
	}
	if left, _ := filepath.Glob(filepath.Join(dir, ".msm-*")); len(left) != 0 {
		t.Errorf("temporary files left: %v", left)
	}
	if _, err := Save(dir, Spec{Label: "bad label"}, ""); err == nil {
		t.Error("an invalid spec should not be saved")
	}
}

func TestEnvText(t *testing.T) {
	env, err := ParseEnv("B=2\n\n A = x y \nC=")
	if err != nil {
		t.Fatal(err)
	}
	if env["B"] != "2" || env["A"] != "x y" || env["C"] != "" || len(env) != 3 {
		t.Errorf("env = %q", env)
	}
	if FormatEnv(map[string]string{"B": "2", "A": "1"}) != "A=1\nB=2" {
		t.Error("FormatEnv sorts")
	}
	if _, err := ParseEnv("no equals sign"); err == nil {
		t.Error("a line without = should fail")
	}
	if got := ParseLines("  /bin/echo \n\nhello world\n"); !slices.Equal(got, []string{"/bin/echo", "hello world"}) {
		t.Errorf("ParseLines = %q", got)
	}
}

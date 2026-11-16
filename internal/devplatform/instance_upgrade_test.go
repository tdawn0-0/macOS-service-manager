package devplatform

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"strings"
	"testing"

	"howett.net/plist"
)

const (
	upgradeOldMySQL = "mysql@8.4.4-arm64"
	upgradeNewMySQL = "mysql@8.4.13-arm64"
)

func upgradeAddPackage(t *testing.T, m *Manager, s *State, name, version, bin string, withBinary bool) string {
	t.Helper()
	p := Package{Name: name, Version: version, Arch: "arm64", Binaries: map[string]string{bin: "bin/" + bin}}
	if withBinary {
		dir := filepath.Join(m.packageDir(p), "bin")
		if err := os.MkdirAll(dir, 0755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(dir, bin), []byte("#!/bin/sh\nexit 0\n"), 0755); err != nil {
			t.Fatal(err)
		}
	}
	s.Packages[p.ID()] = p
	return p.ID()
}

// upgradeFixture builds a stopped instance of kind on the older patch release,
// with a newer patch installed and (for MySQL) a clean shutdown in its log.
func upgradeFixture(t *testing.T, kind string) (*Manager, Instance, string) {
	t.Helper()
	m := testManager(t)
	server := "mysqld"
	if kind == "mongodb" {
		server = "mongod"
	}
	i := Instance{ID: "database", Kind: kind, Port: 54321, Persistent: true}
	err := m.withState(context.Background(), func(s *State) error {
		i.Package = upgradeAddPackage(t, m, s, kind, "8.4.4", server, true)
		upgradeAddPackage(t, m, s, kind, "8.4.13", server, true)
		s.Instances[i.ID] = i
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	dir := filepath.Join(m.Root, "instances", i.ID)
	if err := os.MkdirAll(dir, 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "data-sentinel"), []byte("42"), 0600); err != nil {
		t.Fatal(err)
	}
	if kind == "mysql" {
		upgradeWriteLog(t, m, i.ID, "2026-01-01 [System] ready for connections\n2026-01-01 [System] Shutdown complete\n")
	}
	m.Run = func(context.Context, string, ...string) (string, error) {
		return "", errors.New("Could not find service")
	}
	return m, i, dir
}

func upgradeWriteLog(t *testing.T, m *Manager, id, text string) {
	t.Helper()
	dir := filepath.Join(m.Root, "logs")
	if err := os.MkdirAll(dir, 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "db-"+id+".err.log"), []byte(text), 0600); err != nil {
		t.Fatal(err)
	}
}

// upgradeWriteJob writes a generated job file for the instance that names the
// folder of the given package, and returns its path.
func upgradeWriteJob(t *testing.T, m *Manager, id, pkg string) string {
	t.Helper()
	dir := filepath.Join(m.Root, "jobs")
	if err := os.MkdirAll(dir, 0700); err != nil {
		t.Fatal(err)
	}
	var p Package
	s, err := m.Read()
	if err != nil {
		t.Fatal(err)
	}
	p = s.Packages[pkg]
	job := map[string]any{"Label": m.label("db-" + id), "ProgramArguments": []string{filepath.Join(m.packageDir(p), "bin", "mysqld")}}
	b, err := plist.Marshal(job, plist.XMLFormat)
	if err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(dir, m.label("db-"+id)+".plist")
	if err := os.WriteFile(path, b, 0600); err != nil {
		t.Fatal(err)
	}
	return path
}

func upgradeSaved(t *testing.T, m *Manager, id string) Instance {
	t.Helper()
	s, err := m.Read()
	if err != nil {
		t.Fatal(err)
	}
	return s.Instances[id]
}

func TestPatchSeries(t *testing.T) {
	for _, c := range []struct {
		in     string
		series string
		patch  int
	}{{"8.4.13", "8.4", 13}, {"8.0.5", "8.0", 5}} {
		series, patch, ok := PatchSeries(c.in)
		if !ok || series != c.series || patch != c.patch {
			t.Errorf("PatchSeries(%q) = %q, %d, %v", c.in, series, patch, ok)
		}
	}
	for _, in := range []string{"8.4", "17.11_1", "v8.4.4", "8.4.x", ""} {
		if _, _, ok := PatchSeries(in); ok {
			t.Errorf("PatchSeries(%q) accepted", in)
		}
	}
}

func TestNewerListedPatch(t *testing.T) {
	if got := NewerListedPatch("8.4.4", []string{"8.4.4", "8.4.9", "8.4.13", "8.4.2", "8.0.99", "9.1.50"}); got != "8.4.13" {
		t.Errorf("got %q", got)
	}
	for name, c := range map[string]struct {
		installed string
		listed    []string
	}{
		"equal":     {"8.4.4", []string{"8.4.4"}},
		"lower":     {"8.4.4", []string{"8.4.3", "8.4.1"}},
		"series":    {"8.4.4", []string{"8.0.38"}},
		"malformed": {"8.4.4", []string{"8.4", "v8.4.9", "8.4.x", ""}},
		"installed": {"8.4", []string{"8.4.9"}},
		"none":      {"8.4.4", nil},
	} {
		if got := NewerListedPatch(c.installed, c.listed); got != "" {
			t.Errorf("%s: got %q", name, got)
		}
	}
}

func TestUpgradeTargets(t *testing.T) {
	pkg := func(name, version, arch string) Package {
		return Package{Name: name, Version: version, Arch: arch}
	}
	packages := map[string]Package{}
	for _, p := range []Package{
		pkg("mysql", "8.4.4", "arm64"), pkg("mysql", "8.4.9", "arm64"), pkg("mysql", "8.4.13", "arm64"),
		pkg("mysql", "8.4.2", "arm64"), pkg("mysql", "8.0.40", "arm64"), pkg("mysql", "8.4.20", "universal"),
		pkg("mongodb", "8.4.30", "arm64"), pkg("redis", "7.2.1", "arm64"), pkg("redis", "7.2.9", "arm64"),
		pkg("postgresql", "17.1", "arm64"),
	} {
		packages[p.ID()] = p
	}
	s := State{Packages: packages}
	got := UpgradeTargets(s, Instance{Kind: "mysql", Package: "mysql@8.4.4-arm64"})
	if want := []string{"mysql@8.4.13-arm64", "mysql@8.4.9-arm64"}; !reflect.DeepEqual(got, want) {
		t.Errorf("got %v want %v", got, want)
	}
	if got := UpgradeTargets(s, Instance{Kind: "mysql", Package: "mysql@8.4.13-arm64"}); len(got) != 0 {
		t.Errorf("newest has targets: %v", got)
	}
	for _, i := range []Instance{
		{Kind: "redis", Package: "redis@8.2.11-arm64"},
		{Kind: "postgresql", Package: "postgresql@17.1-arm64"},
		{Kind: "mysql", Package: "mysql@8.4.1-arm64"},
	} {
		if got := UpgradeTargets(s, i); len(got) != 0 {
			t.Errorf("%s: got %v", i.Kind, got)
		}
	}
}

func TestUpgradeInstancePackageMovesOnlyThePackage(t *testing.T) {
	for _, kind := range []string{"mysql", "mongodb"} {
		t.Run(kind, func(t *testing.T) {
			m, i, dir := upgradeFixture(t, kind)
			external, err := filepath.EvalSymlinks(t.TempDir())
			if err != nil {
				t.Fatal(err)
			}
			ext := filepath.Join(external, "data")
			if err := os.Mkdir(ext, 0700); err != nil {
				t.Fatal(err)
			}
			info, err := os.Lstat(ext)
			if err != nil {
				t.Fatal(err)
			}
			identity, err := deletionIdentity(info)
			if err != nil {
				t.Fatal(err)
			}
			i.DataDirectory, i.DataOwner, i.DataIdentity = ext, "owner-token", identity
			if err := os.WriteFile(filepath.Join(ext, dataOwnerFile), []byte(m.Root+"\n"+i.ID+"\nowner-token\n"), 0600); err != nil {
				t.Fatal(err)
			}
			if err := os.WriteFile(filepath.Join(ext, "sentinel"), []byte("external"), 0600); err != nil {
				t.Fatal(err)
			}
			if err := m.withState(context.Background(), func(s *State) error { s.Instances[i.ID] = i; return nil }); err != nil {
				t.Fatal(err)
			}
			target := kind + "@8.4.13-arm64"
			if err := m.UpgradeInstancePackage(context.Background(), i, target); err != nil {
				t.Fatal(err)
			}
			s, err := m.Read()
			if err != nil {
				t.Fatal(err)
			}
			want := i
			want.Package = target
			if s.Instances[i.ID] != want {
				t.Errorf("instance = %+v, want %+v", s.Instances[i.ID], want)
			}
			for _, id := range []string{kind + "@8.4.4-arm64", target} {
				if _, ok := s.Packages[id]; !ok {
					t.Errorf("package %s missing", id)
				}
			}
			for path, content := range map[string]string{filepath.Join(dir, "data-sentinel"): "42", filepath.Join(ext, "sentinel"): "external"} {
				if b, err := os.ReadFile(path); err != nil || string(b) != content {
					t.Errorf("%s changed: %q %v", path, b, err)
				}
			}
		})
	}
}

func TestUpgradeInstancePackageRefuses(t *testing.T) {
	const target = upgradeNewMySQL
	cases := []struct {
		name   string
		kind   string
		target string
		setup  func(t *testing.T, m *Manager, i *Instance)
	}{
		{name: "job registered", setup: func(t *testing.T, m *Manager, i *Instance) {
			m.Run = func(context.Context, string, ...string) (string, error) { return "state = running", nil }
		}},
		{name: "observation fails", setup: func(t *testing.T, m *Manager, i *Instance) {
			m.Run = func(context.Context, string, ...string) (string, error) { return "", errors.New("launchctl exploded") }
		}},
		{name: "expected differs", setup: func(t *testing.T, m *Manager, i *Instance) { i.Port++ }},
		{name: "lower patch", target: "mysql@8.4.2-arm64", setup: func(t *testing.T, m *Manager, i *Instance) {
			if err := m.withState(context.Background(), func(s *State) error {
				upgradeAddPackage(t, m, s, "mysql", "8.4.2", "mysqld", true)
				return nil
			}); err != nil {
				t.Fatal(err)
			}
		}},
		{name: "other series", target: "mysql@8.0.40-arm64", setup: func(t *testing.T, m *Manager, i *Instance) {
			if err := m.withState(context.Background(), func(s *State) error {
				upgradeAddPackage(t, m, s, "mysql", "8.0.40", "mysqld", true)
				return nil
			}); err != nil {
				t.Fatal(err)
			}
		}},
		{name: "not installed", target: "mysql@8.4.99-arm64"},
		{name: "binary missing", setup: func(t *testing.T, m *Manager, i *Instance) {
			if err := os.Remove(filepath.Join(m.packageDir(Package{Name: "mysql", Version: "8.4.13", Arch: "arm64"}), "bin", "mysqld")); err != nil {
				t.Fatal(err)
			}
		}},
		{name: "redis", kind: "redis"},
		{name: "log not clean", setup: func(t *testing.T, m *Manager, i *Instance) {
			upgradeWriteLog(t, m, i.ID, "[System] Shutdown complete\n[System] ready for connections\n")
		}},
		{name: "log missing", setup: func(t *testing.T, m *Manager, i *Instance) {
			if err := os.Remove(filepath.Join(m.Root, "logs", "db-"+i.ID+".err.log")); err != nil {
				t.Fatal(err)
			}
		}},
		{name: "unfinished task", setup: func(t *testing.T, m *Manager, i *Instance) {
			dir := filepath.Join(m.Root, "database-work", "task")
			if err := os.MkdirAll(dir, 0700); err != nil {
				t.Fatal(err)
			}
			identity, err := initializationIdentity(dir)
			if err != nil {
				t.Fatal(err)
			}
			b, err := json.Marshal(databaseWorkRecord{Schema: 1, ID: "task", Identity: identity, Instance: *i, Request: databaseWorkRequest{Kind: "backup", InstanceID: i.ID}})
			if err != nil {
				t.Fatal(err)
			}
			if err := os.WriteFile(filepath.Join(dir, "record.json"), b, 0600); err != nil {
				t.Fatal(err)
			}
		}},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			m, i, _ := upgradeFixture(t, "mysql")
			if c.kind == "redis" {
				i.Kind = "redis"
			}
			saved := upgradeSaved(t, m, i.ID)
			job := upgradeWriteJob(t, m, i.ID, "mysql@8.4.4-arm64")
			expected := i
			if c.setup != nil {
				c.setup(t, m, &expected)
			}
			to := c.target
			if to == "" {
				to = target
			}
			if err := m.UpgradeInstancePackage(context.Background(), expected, to); err == nil {
				t.Fatal("expected refusal")
			}
			if c.name == "job registered" || c.name == "lower patch" {
				if _, err := os.Stat(job); err != nil {
					t.Errorf("job file removed by a refused upgrade: %v", err)
				}
			}
			if got := upgradeSaved(t, m, i.ID); got != saved {
				t.Errorf("saved instance changed: %+v -> %+v", saved, got)
			}
		})
	}
}

func TestUpgradeInstancePackageStateStaysDecodable(t *testing.T) {
	m, i, _ := upgradeFixture(t, "mysql")
	keys := func() []string {
		b, err := os.ReadFile(filepath.Join(m.Root, "state.json"))
		if err != nil {
			t.Fatal(err)
		}
		var raw struct {
			Instances map[string]map[string]json.RawMessage `json:"instances"`
		}
		if err := json.NewDecoder(bytes.NewReader(b)).Decode(&raw); err != nil {
			t.Fatal(err)
		}
		var out []string
		for k := range raw.Instances[i.ID] {
			out = append(out, k)
		}
		sort.Strings(out)
		return out
	}
	before := keys()
	if err := m.UpgradeInstancePackage(context.Background(), i, upgradeNewMySQL); err != nil {
		t.Fatal(err)
	}
	if _, err := m.Read(); err != nil {
		t.Fatal(err)
	}
	if after := keys(); !reflect.DeepEqual(before, after) {
		t.Errorf("instance keys changed: %v -> %v", before, after)
	}
}

func TestUpgradeInstancePackageDropsStaleJobFile(t *testing.T) {
	m, i, _ := upgradeFixture(t, "mysql")
	job := upgradeWriteJob(t, m, i.ID, "mysql@8.4.4-arm64")
	other := filepath.Join(m.Root, "jobs", m.label("db-other")+".plist")
	if err := os.WriteFile(other, []byte("unrelated"), 0600); err != nil {
		t.Fatal(err)
	}
	if err := m.UpgradeInstancePackage(context.Background(), i, upgradeNewMySQL); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(job); !errors.Is(err, os.ErrNotExist) {
		t.Errorf("instance job file still present: %v", err)
	}
	if b, err := os.ReadFile(other); err != nil || string(b) != "unrelated" {
		t.Errorf("unrelated job file changed: %q %v", b, err)
	}
}

func TestUpgradeThenUninstallOldPackage(t *testing.T) {
	m, i, _ := upgradeFixture(t, "mysql")
	upgradeWriteJob(t, m, i.ID, "mysql@8.4.4-arm64")
	if err := m.UpgradeInstancePackage(context.Background(), i, upgradeNewMySQL); err != nil {
		t.Fatal(err)
	}
	// Uninstall may still fail for a reason this fixture cannot satisfy; only
	// the launchd job reference must be gone.
	if err := m.Uninstall(context.Background(), "mysql@8.4.4-arm64"); err != nil && strings.Contains(err.Error(), "referenced by a launchd job") {
		t.Errorf("old package still blocked by a job file: %v", err)
	}
}

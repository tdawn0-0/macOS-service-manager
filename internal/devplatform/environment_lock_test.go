package devplatform

import (
	"context"
	"encoding/json"
	"net"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func exportedEnvironmentFixture(t *testing.T) (*Manager, Project, Package) {
	t.Helper()
	m := testManager(t)
	pkg, e := Resolve(context.Background(), "redis", "7.4.7")
	if e != nil {
		t.Fatal(e)
	}
	l, e := net.Listen("tcp4", "127.0.0.1:0")
	if e != nil {
		t.Fatal(e)
	}
	port := l.Addr().(*net.TCPAddr).Port
	l.Close()
	p := Project{SchemaVersion: Schema, ID: "original", Name: "Environment", Directory: t.TempDir(), Instances: []string{"cache"}, Services: []Service{{ID: "worker", Package: pkg.ID(), Command: []string{"redis-cli", "--version"}, Environment: map[string]string{"SECRET": "do-not-export"}, EnvFile: "/private/secret.env"}}}
	e = m.withState(context.Background(), func(s *State) error {
		s.Packages[pkg.ID()] = pkg
		s.Instances["cache"] = Instance{ID: "cache", Kind: "redis", Package: pkg.ID(), Port: port, Persistent: true}
		s.Projects[p.ID] = p
		return nil
	})
	if e != nil {
		t.Fatal(e)
	}
	if e = m.ExportProject(context.Background(), p.ID); e != nil {
		t.Fatal(e)
	}
	return m, p, pkg
}

func TestEnvironmentImportIsIsolatedAndDoesNotExecute(t *testing.T) {
	m, p, _ := exportedEnvironmentFixture(t)
	for _, name := range []string{"devmanager.json", "devmanager.lock.json"} {
		b, e := os.ReadFile(filepath.Join(p.Directory, name))
		if e != nil {
			t.Fatal(e)
		}
		if strings.Contains(string(b), "do-not-export") || strings.Contains(string(b), "/private/secret.env") {
			t.Fatal("secret exported")
		}
	}
	// A repository cannot turn an import into a login job.
	b, _ := os.ReadFile(filepath.Join(p.Directory, "devmanager.json"))
	var shared Project
	if e := json.Unmarshal(b, &shared); e != nil {
		t.Fatal(e)
	}
	shared.Autostart = true
	if e := writeJSON(filepath.Join(p.Directory, "devmanager.json"), shared); e != nil {
		t.Fatal(e)
	}
	m.Run = func(context.Context, string, ...string) (string, error) {
		t.Fatal("import executed a command")
		return "", nil
	}
	imported, e := m.Import(context.Background(), p.Directory)
	if e != nil {
		t.Fatal(e)
	}
	if imported.Autostart || imported.ID == p.ID || imported.Instances[0] == p.Instances[0] {
		t.Fatal("working copy was not isolated")
	}
	lock, e := readEnvironmentLock(m.projectLockPath(imported.ID))
	if e != nil {
		t.Fatal(e)
	}
	s, _ := m.Read()
	if len(s.Instances) != 1 || lock.Instances[imported.Instances[0]].Port == s.Instances["cache"].Port {
		t.Fatal("import reused local database or port")
	}
}

func TestEnvironmentPrepareRejectsChangedChecksumBeforeInstalling(t *testing.T) {
	m, p, pkg := exportedEnvironmentFixture(t)
	lock, e := readEnvironmentLock(filepath.Join(p.Directory, "devmanager.lock.json"))
	if e != nil {
		t.Fatal(e)
	}
	pkg.SHA256 = strings.Repeat("0", 64)
	pkg.URL = "https://untrusted.invalid/payload"
	lock.Packages[pkg.ID()] = pkg
	if e = writeJSON(filepath.Join(p.Directory, "devmanager.lock.json"), lock); e != nil {
		t.Fatal(e)
	}
	// Keep the pair internally consistent. Preparation must independently reject
	// a repository's altered catalog digest even when its own files agree.
	declaration, _ := os.ReadFile(filepath.Join(p.Directory, "devmanager.json"))
	var shared sharedProject
	if e = json.Unmarshal(declaration, &shared); e != nil {
		t.Fatal(e)
	}
	lockBytes, _ := os.ReadFile(filepath.Join(p.Directory, "devmanager.lock.json"))
	shared.LockSHA256 = digestBytes(lockBytes)
	if e = writeJSON(filepath.Join(p.Directory, "devmanager.json"), shared); e != nil {
		t.Fatal(e)
	}
	imported, e := m.Import(context.Background(), p.Directory)
	if e != nil {
		t.Fatal(e)
	}
	e = m.PrepareEnvironment(context.Background(), imported.ID)
	if e == nil || !strings.Contains(e.Error(), "checksum") {
		t.Fatal("untrusted digest was accepted", e)
	}
	entries, _ := os.ReadDir(filepath.Join(m.Root, "downloads"))
	if len(entries) > 0 {
		t.Fatal("download began before lock validation")
	}
}

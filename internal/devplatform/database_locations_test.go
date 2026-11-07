package devplatform

import (
	"bytes"
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestDatabaseCustomDataDirectory(t *testing.T) {
	t.Run("lifecycle", customDataLifecycle)
	t.Run("failure-recovery", customDataFailureRecovery)
	t.Run("engine-arguments", customDataArguments)
	t.Run("backup-restore", customDataBackupRestore)
	m := testManager(t)
	m.Home = t.TempDir()
	base := realTempDir(t)
	i := Instance{ID: "one", Kind: "postgresql", DataDirectory: filepath.Join(base, "custom data"), DataOwner: "owner"}
	s := emptyState()
	for _, bad := range []string{"relative", "/", base, filepath.Join(m.Root, "instances"), base + "/../escape"} {
		candidate := i
		candidate.DataDirectory = bad
		candidate.DataOwner = ""
		if e := m.validateDataLocation(s, candidate); e == nil {
			t.Fatal("accepted unsafe location", bad)
		}
	}
	if e := m.validateDataLocation(s, i); e != nil {
		t.Fatal(e)
	}
	if e := m.prepareDataLocation(&i); e != nil {
		t.Fatal(e)
	}
	if e := m.verifyDataLocation(i); e != nil {
		t.Fatal(e)
	}
	if m.DatabaseDataDirectory(i) != filepath.Join(i.DataDirectory, "data") {
		t.Fatal("wrong data path")
	}
	nested := i
	nested.ID = "two"
	nested.DataDirectory = filepath.Join(i.DataDirectory, "nested")
	if e := m.validateDataLocation(s, nested); e == nil {
		t.Fatal("accepted nested directory")
	}
	foreign := i
	foreign.DataOwner = "different"
	if e := m.verifyDataLocation(foreign); e == nil {
		t.Fatal("accepted wrong owner")
	}
	os.Symlink(t.TempDir(), filepath.Join(i.DataDirectory, "data"))
	if e := m.verifyDataLocation(i); e == nil {
		t.Fatal("followed data symlink")
	}
	os.Remove(filepath.Join(i.DataDirectory, "data"))
	s.Instances[i.ID] = i
	p, e := m.instanceDeletionPreview(context.Background(), s, i.ID)
	if e != nil || p.CanDelete || p.Path != m.DatabaseDataDirectory(i) {
		t.Fatal(p, e)
	}
	for _, kind := range []string{"mysql", "postgresql", "redis", "mongodb"} {
		i.Kind = kind
		if m.DatabaseDataDirectory(i) != filepath.Join(i.DataDirectory, "data") {
			t.Fatal(kind)
		}
	}
}
func TestDatabaseVersionIsolation(t *testing.T) {
	t.Run("two-exact-kegs", customPostgresVersionInstances)
	m := testManager(t)
	a := Instance{ID: "pg17", Kind: "postgresql", Package: "postgresql@17.7-arm64", Port: 55432, DataDirectory: filepath.Join(realTempDir(t), "data")}
	b := a
	b.ID = "pg18"
	b.Package = "postgresql@18.1-arm64"
	b.Port = 55433
	b.DataDirectory = filepath.Join(realTempDir(t), "data")
	s := emptyState()
	s.Instances[a.ID] = a
	if e := m.validateDataLocation(s, b); e != nil {
		t.Fatal(e)
	}
	if m.DatabaseDataDirectory(a) == m.DatabaseDataDirectory(b) || a.Package == b.Package || a.Port == b.Port {
		t.Fatal("instances share identity")
	}
	b.DataDirectory = a.DataDirectory
	if e := m.validateDataLocation(s, b); e == nil {
		t.Fatal("accepted shared location")
	}
}

func realTempDir(t *testing.T) string {
	t.Helper()
	p, e := filepath.EvalSymlinks(t.TempDir())
	if e != nil {
		t.Fatal(e)
	}
	return p
}

func customDataLifecycle(t *testing.T) {
	script := `#!/usr/bin/python3
import pathlib,shlex,socket,sys
config=pathlib.Path(sys.argv[1])
lines=[shlex.split(x) for x in config.read_text().splitlines()]
port=int(next(x[1] for x in lines if x[0]=='port'))
data=pathlib.Path(next(x[1] for x in lines if x[0]=='dir'))
(data/'started').write_text('kept')
s=socket.socket(); s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1); s.bind(('127.0.0.1',port)); s.listen()
while True:
 c,_=s.accept(); c.close()
`
	m, p := initializationFixture(t, script)
	i := initializationInstance(t, p, "custom")
	i.DataDirectory = filepath.Join(realTempDir(t), "space in location")
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	if e := m.CreateInstance(ctx, i); e != nil {
		t.Fatal(e)
	}
	t.Cleanup(func() { _ = m.InstanceAction(context.Background(), i.ID, "stop") })
	s, e := m.Read()
	if e != nil {
		t.Fatal(e)
	}
	i = s.Instances[i.ID]
	// A fresh manager must retain ownership and location across app restart.
	m = New(m.Root)
	for _, action := range []string{"start", "restart", "stop"} {
		if e = m.InstanceAction(ctx, i.ID, action); e != nil {
			t.Fatal(action, e)
		}
	}
	marker := filepath.Join(m.DatabaseDataDirectory(i), "started")
	if b, e := os.ReadFile(marker); e != nil || string(b) != "kept" {
		t.Fatal(string(b), e)
	}
	port := integrationPort(t)
	if e = m.ChangeInstancePort(ctx, i, port); e != nil {
		t.Fatal(e)
	}
	s, _ = m.Read()
	i = s.Instances[i.ID]
	config, _ := os.ReadFile(filepath.Join(m.Root, "instances", i.ID, "valkey.conf"))
	if !strings.Contains(string(config), fmt.Sprintf("dir %q", m.DatabaseDataDirectory(i))) {
		t.Fatal("port edit changed data path")
	}
	preview, e := m.PreviewInstanceDeletion(ctx, i.ID)
	if e != nil || preview.CanDelete || !strings.Contains(preview.Detail, "retained") {
		t.Fatal(preview, e)
	}
	if e = m.DeleteInstanceData(ctx, i.ID, preview.Fingerprint, i.ID); e == nil {
		t.Fatal("deleted external data")
	}
	if e = m.InstanceAction(ctx, i.ID, "remove"); e != nil {
		t.Fatal(e)
	}
	if b, e := os.ReadFile(marker); e != nil || string(b) != "kept" {
		t.Fatal("remove changed data", e)
	}
	if e = m.CreateInstance(ctx, i); e == nil {
		t.Fatal("adopted retained data")
	}
}

func customDataFailureRecovery(t *testing.T) {
	for _, failure := range []string{"cancel", "state-write"} {
		t.Run(failure, func(t *testing.T) {
			m, p := initializationFixture(t, "#!/bin/sh\nexit 0\n")
			i := initializationInstance(t, p, "failed")
			i.DataDirectory = filepath.Join(realTempDir(t), "retained external")
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			statePath := filepath.Join(m.Root, "state.json")
			m.Event = func(event string) {
				if event == "Database initialization data published" {
					if failure == "cancel" {
						cancel()
					} else {
						os.Rename(statePath, statePath+".saved")
						os.Mkdir(statePath, 0700)
					}
				}
			}
			if e := m.CreateInstance(ctx, i); e == nil {
				t.Fatal("expected initialization failure")
			}
			m.Event = nil
			if failure == "state-write" {
				os.Remove(statePath)
				os.Rename(statePath+".saved", statePath)
			}
			before, e := os.ReadFile(filepath.Join(i.DataDirectory, dataOwnerFile))
			if e != nil {
				t.Fatal(e)
			}
			r := initializationInfoOne(t, New(m.Root))
			if !strings.Contains(r.Detail, i.DataDirectory) {
				t.Fatal("recovery hides external location", r)
			}
			if _, e := New(m.Root).ResolveInstanceInitialization(context.Background(), r.ID, r.Revision); e != nil {
				t.Fatal(e)
			}
			after, e := os.ReadFile(filepath.Join(i.DataDirectory, dataOwnerFile))
			if e != nil || !bytes.Equal(before, after) {
				t.Fatal("recovery touched external directory", e)
			}
			if e = m.CreateInstance(context.Background(), i); e == nil {
				t.Fatal("reused failed external location")
			}
		})
	}
}

func customDataArguments(t *testing.T) {
	m, p := initializationFixture(t, "#!/bin/sh\nexit 0\n")
	for _, name := range []string{"postgres", "mysqld", "mongod"} {
		p.Binaries[name] = "tool"
	}
	if e := m.withState(context.Background(), func(s *State) error { s.Packages[p.ID()] = p; return nil }); e != nil {
		t.Fatal(e)
	}
	s, _ := m.Read()
	for _, kind := range []string{"postgresql", "mysql", "mongodb", "redis"} {
		i := Instance{ID: kind, Kind: kind, Package: p.ID(), Port: integrationPort(t), DataDirectory: filepath.Join(realTempDir(t), "data location"), DataOwner: "test-owner"}
		if e := m.prepareDataLocation(&i); e != nil {
			t.Fatal(e)
		}
		dir := filepath.Join(m.Root, "instances", i.ID)
		os.MkdirAll(dir, 0700)
		args, e := m.instanceStartArguments(&s, i)
		if e != nil {
			t.Fatal(e)
		}
		if kind == "redis" {
			if e = m.initializeInstanceData(context.Background(), &s, i, dir); e != nil {
				t.Fatal(e)
			}
			b, _ := os.ReadFile(filepath.Join(dir, "valkey.conf"))
			if !strings.Contains(string(b), fmt.Sprintf("dir %q", m.DatabaseDataDirectory(i))) {
				t.Fatal(string(b))
			}
		} else if !strings.Contains(strings.Join(args, "\n"), m.DatabaseDataDirectory(i)) {
			t.Fatal(args)
		}
	}
}

func customPostgresVersionInstances(t *testing.T) {
	// Exact-keg fixtures exercise the supported Homebrew catalog contract. These
	// are command fixtures, not native PostgreSQL server acceptance.
	m, p, _ := brewFixture(t, "postgresql")
	m.Run = runCommand
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	var instances []Instance
	for n, version := range []string{"17.11_1", "18.6_1"} {
		p.Version = version
		p.Formula = "postgresql@" + strings.Split(version, ".")[0]
		writeBrewKeg(t, m, p, "homebrew/core")
		script := "#!/bin/sh\n[ \"$1\" = -D ] || exit 8\n/bin/mkdir \"$2\"\n"
		os.WriteFile(filepath.Join(m.packageDir(p), "bin", "initdb"), []byte(script), 0700)
		if e := m.withState(ctx, func(s *State) error { s.Packages[p.ID()] = p; return nil }); e != nil {
			t.Fatal(e)
		}
		i := Instance{ID: fmt.Sprintf("pg%d", n), Kind: "postgresql", Package: p.ID(), Port: integrationPort(t), DataDirectory: filepath.Join(realTempDir(t), "postgres data")}
		if len(instances) > 0 {
			for i.Port == instances[0].Port {
				i.Port = integrationPort(t)
			}
		}
		if e := m.CreateInstance(ctx, i); e != nil {
			t.Fatal(e)
		}
		instances = append(instances, i)
	}
	s, e := m.Read()
	if e != nil || len(s.Instances) != 2 {
		t.Fatal(e)
	}
	for _, i := range instances {
		saved := s.Instances[i.ID]
		if saved.Package != i.Package || saved.DataDirectory != i.DataDirectory || saved.Port != i.Port {
			t.Fatal(saved)
		}
		if e = m.verifyDataLocation(saved); e != nil {
			t.Fatal(e)
		}
		if e = m.Uninstall(ctx, saved.Package); e == nil {
			t.Fatal("removed in-use keg")
		}
	}
}

func customDataBackupRestore(t *testing.T) {
	m, p := databaseToolFixture(t)
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	s, e := m.Read()
	if e != nil {
		t.Fatal(e)
	}
	i := s.Instances["first"]
	i.DataDirectory = filepath.Join(realTempDir(t), "external backup source")
	i.DataOwner = "backup-owner"
	if e = m.prepareDataLocation(&i); e != nil {
		t.Fatal(e)
	}
	if e = os.Mkdir(m.DatabaseDataDirectory(i), 0700); e != nil {
		t.Fatal(e)
	}
	sentinel := filepath.Join(m.DatabaseDataDirectory(i), "retained-content")
	os.WriteFile(sentinel, []byte("keep"), 0600)
	if e = m.withState(ctx, func(s *State) error { s.Instances[i.ID] = i; return nil }); e != nil {
		t.Fatal(e)
	}
	os.WriteFile(filepath.Join(m.Root, "release"), nil, 0600)
	os.WriteFile(filepath.Join(m.packageDir(p), "pg_restore"), []byte("#!/bin/sh\nexit 0\n"), 0700)
	backup, e := m.Backup(ctx, i.ID, "source")
	if e != nil {
		t.Fatal(e)
	}
	if b, e := os.ReadFile(backup); e != nil || string(b) != "completed dump" {
		t.Fatal("backup did not publish", e)
	}
	if e = m.Restore(ctx, i.ID, "restored", backup); e != nil {
		t.Fatal(e)
	}
	if _, e = os.Stat(filepath.Join(m.Root, "entered-restored")); e != nil {
		t.Fatal("restore did not create new target", e)
	}
	if b, e := os.ReadFile(sentinel); e != nil || string(b) != "keep" {
		t.Fatal("logical operation changed external data path", e)
	}
	os.WriteFile(filepath.Join(i.DataDirectory, dataOwnerFile), []byte("foreign-owner"), 0600)
	if e = m.Restore(ctx, i.ID, "forbidden", backup); e == nil {
		t.Fatal("restore ignored ownership change")
	}
	if _, e = m.Backup(ctx, i.ID, "forbidden"); e == nil {
		t.Fatal("backup ignored ownership change")
	}
	if _, e = os.Stat(filepath.Join(m.Root, "entered-forbidden")); !os.IsNotExist(e) {
		t.Fatal("foreign operation reached engine", e)
	}
}

package devplatform

import (
	"archive/zip"
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"testing"
	"time"
)

func initializationFixture(t *testing.T, script string) (*Manager, Package) {
	t.Helper()
	m := testManager(t)
	var b bytes.Buffer
	z := zip.NewWriter(&b)
	h := &zip.FileHeader{Name: "tool", Method: zip.Deflate}
	h.SetMode(0755)
	w, e := z.CreateHeader(h)
	if e != nil {
		t.Fatal(e)
	}
	w.Write([]byte(script))
	z.Close()
	sum := sha256.Sum256(b.Bytes())
	p := Package{Name: "initfixture", Version: "1", Arch: runtime.GOARCH, URL: "https://unused.invalid/package", SHA256: hex.EncodeToString(sum[:]), Format: "zip", Binaries: map[string]string{"redis-server": "tool", "initdb": "tool"}}
	os.WriteFile(filepath.Join(m.Root, "downloads", p.SHA256), b.Bytes(), 0600)
	if e = m.Install(context.Background(), p); e != nil {
		t.Fatal(e)
	}
	return m, p
}
func initializationInstance(t *testing.T, p Package, id string) Instance {
	return Instance{ID: id, Kind: "redis", Package: p.ID(), Port: integrationPort(t), Persistent: true}
}
func initializationInfoOne(t *testing.T, m *Manager) InstanceInitializationInfo {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for {
		list, e := m.PendingInstanceInitializations(context.Background())
		if e != nil || len(list) != 1 {
			t.Fatal(list, e)
		}
		if !list[0].Busy {
			return list[0]
		}
		if time.Now().After(deadline) {
			t.Fatal("caller lease remained busy", list)
		}
		time.Sleep(20 * time.Millisecond)
	}
}
func TestInstanceInitializationParallelReservationsAndUnrelatedConfiguration(t *testing.T) {
	m, p := initializationFixture(t, "#!/bin/sh\nexit 0\n")
	a, b := initializationInstance(t, p, "first"), initializationInstance(t, p, "second")
	for b.Port == a.Port {
		b.Port = integrationPort(t)
	}
	entered := make(chan string, 2)
	release := make(chan struct{})
	var once sync.Once
	unblock := func() { once.Do(func() { close(release) }) }
	defer unblock()
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	m.Event = func(s string) {
		if strings.HasPrefix(s, "Initializing database ") {
			entered <- s
			select {
			case <-release:
			case <-ctx.Done():
			}
		}
	}
	other := New(m.Root)
	other.Event = m.Event
	done := make(chan error, 2)
	remaining := 2
	defer func() {
		unblock()
		cancel()
		for remaining > 0 {
			select {
			case <-done:
				remaining--
			case <-time.After(5 * time.Second):
				return
			}
		}
	}()
	go func() { done <- m.CreateInstance(ctx, a) }()
	go func() { done <- other.CreateInstance(ctx, b) }()
	for n := 0; n < 2; n++ {
		select {
		case <-entered:
		case <-time.After(3 * time.Second):
			unblock()
			t.Fatal("independent initialization held the state lock")
		}
	}
	short, stop := context.WithTimeout(ctx, 2*time.Second)
	defer stop()
	project := Project{SchemaVersion: 1, ID: "during-init", Name: "Saved during initialization", Directory: t.TempDir()}
	if e := New(m.Root).SaveProject(short, project); e != nil {
		unblock()
		t.Fatal(e)
	}
	var busy *ResourceBusyError
	if e := New(m.Root).CreateInstance(short, a); !errors.As(e, &busy) || busy.Kind != "database instance" {
		unblock()
		t.Fatal("same instance was not reserved", e)
	}
	conflicting := a
	conflicting.ID = "other-id"
	if e := New(m.Root).CreateInstance(short, conflicting); !errors.As(e, &busy) || busy.Kind != "TCP port" {
		unblock()
		t.Fatal("port was not reserved", e)
	}
	if e := New(m.Root).Uninstall(short, p.ID()); e == nil {
		unblock()
		t.Fatal("initializing package was uninstalled")
	}
	rows, e := m.PendingInstanceInitializations(short)
	if e != nil || len(rows) != 2 || !rows[0].Busy || rows[0].CanResolve {
		t.Fatal(rows, e)
	}
	unblock()
	for n := 0; n < 2; n++ {
		remaining--
		if e = <-done; e != nil {
			t.Fatal(e)
		}
	}
	state, e := m.Read()
	if e != nil || len(state.Instances) != 2 || state.Projects[project.ID].Name != project.Name {
		t.Fatal(state, e)
	}
	for _, i := range []Instance{a, b} {
		config, e := os.ReadFile(filepath.Join(m.Root, "instances", i.ID, "valkey.conf"))
		if e != nil || !strings.Contains(string(config), fmt.Sprintf("dir %q", filepath.Join(m.Root, "instances", i.ID))) {
			t.Fatal(string(config), e)
		}
	}
}
func TestInstanceInitializationCommitFailureKeepsPreparedDataAndReconciles(t *testing.T) {
	for _, failure := range []string{"cancel", "state-write"} {
		t.Run(failure, func(t *testing.T) {
			m, p := initializationFixture(t, "#!/bin/sh\nexit 0\n")
			i := initializationInstance(t, p, "preserved")
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			state := filepath.Join(m.Root, "state.json")
			m.Event = func(s string) {
				if s == "Database initialization data published" {
					if failure == "cancel" {
						cancel()
					} else {
						os.Rename(state, state+".saved")
						os.Mkdir(state, 0700)
					}
				}
			}
			if e := m.CreateInstance(ctx, i); e == nil {
				t.Fatal("expected publication failure")
			}
			m.Event = nil
			if failure == "state-write" {
				os.Remove(state)
				os.Rename(state+".saved", state)
			}
			if _, e := os.Stat(filepath.Join(m.Root, "instances", i.ID)); !errors.Is(e, os.ErrNotExist) {
				t.Fatal("failed registration left canonical data", e)
			}
			r := initializationInfoOne(t, m)
			before, e := os.ReadFile(filepath.Join(r.Path, "password"))
			if e != nil || len(before) == 0 {
				t.Fatal(e)
			}
			if _, e = m.ResolveInstanceInitialization(context.Background(), r.ID, "stale"); e == nil {
				t.Fatal("accepted old preview")
			}
			archive, e := m.ResolveInstanceInitialization(context.Background(), r.ID, r.Revision)
			if e != nil {
				t.Fatal(e)
			}
			after, e := os.ReadFile(filepath.Join(archive, "password"))
			if e != nil || !bytes.Equal(before, after) {
				t.Fatal("reconciliation changed credentials", e)
			}
			if e = m.CreateInstance(context.Background(), i); e != nil {
				t.Fatal("original ID was not reusable", e)
			}
		})
	}
}
func TestInstanceInitializationCrashHelper(t *testing.T) {
	root := os.Getenv("MSM_INITIALIZATION_CRASH_ROOT")
	if root == "" {
		return
	}
	m := New(root)
	s, _ := m.Read()
	var pkg string
	for key := range s.Packages {
		pkg = key
		break
	}
	port := 0
	fmt.Sscanf(os.Getenv("MSM_INITIALIZATION_CRASH_PORT"), "%d", &port)
	i := Instance{ID: "interrupted", Kind: "redis", Package: pkg, Port: port, Persistent: true}
	if os.Getenv("MSM_INITIALIZATION_FINITE") == "1" {
		i.Kind = "postgresql"
	}
	m.Event = func(s string) {
		if s == os.Getenv("MSM_INITIALIZATION_CRASH_POINT") {
			os.Exit(23)
		}
	}
	if os.Getenv("MSM_INITIALIZATION_RECONCILE") == "1" {
		rows, e := m.PendingInstanceInitializations(context.Background())
		if e == nil && len(rows) == 1 {
			_, _ = m.ResolveInstanceInitialization(context.Background(), rows[0].ID, rows[0].Revision)
		}
		os.Exit(26)
	}
	_ = m.CreateInstance(context.Background(), i)
	os.Exit(26)
}
func TestInstanceInitializationActualCrashBeforeAndAfterRegistration(t *testing.T) {
	for _, point := range []string{"Database initialization data published", "Database initialization committed"} {
		t.Run(point, func(t *testing.T) {
			m, p := initializationFixture(t, "#!/bin/sh\nexit 0\n")
			port := integrationPort(t)
			c := exec.Command(os.Args[0], "-test.run=^TestInstanceInitializationCrashHelper$")
			c.Env = append(os.Environ(), "MSM_INITIALIZATION_CRASH_ROOT="+m.Root, "MSM_INITIALIZATION_CRASH_PORT="+fmt.Sprint(port), "MSM_INITIALIZATION_CRASH_POINT="+point)
			var exit *exec.ExitError
			if e := c.Run(); !errors.As(e, &exit) || exit.ExitCode() != 23 {
				t.Fatal(e)
			}
			r := initializationInfoOne(t, m)
			if point == "Database initialization committed" {
				if r.Status != "Published" {
					t.Fatal(r)
				}
			} else if r.Status != "Uncommitted" {
				t.Fatal(r)
			}
			archive, e := m.ResolveInstanceInitialization(context.Background(), r.ID, r.Revision)
			if e != nil {
				t.Fatal(e)
			}
			s, e := m.Read()
			if e != nil {
				t.Fatal(e)
			}
			if point == "Database initialization committed" {
				if archive != "" || s.Instances["interrupted"].Package != p.ID() {
					t.Fatal("published data was archived", archive, s)
				}
			} else {
				if archive == "" {
					t.Fatal("unregistered data was not retained")
				}
				if _, ok := s.Instances["interrupted"]; ok {
					t.Fatal("failed initialization registered")
				}
			}
		})
	}
}
func TestInstanceInitializationFiniteLaunchdJobSurvivesCallerAndIsSafelyArchived(t *testing.T) {
	m, p := initializationFixture(t, "#!/bin/sh\n/bin/mkdir -p \"$2\"\n/usr/bin/touch \"$2/ready\"\nexec /bin/sleep 30\n")
	port := integrationPort(t)
	c := exec.Command(os.Args[0], "-test.run=^TestInstanceInitializationCrashHelper$")
	c.Env = append(os.Environ(), "MSM_INITIALIZATION_CRASH_ROOT="+m.Root, "MSM_INITIALIZATION_CRASH_PORT="+fmt.Sprint(port), "MSM_INITIALIZATION_FINITE=1")
	if e := c.Start(); e != nil {
		t.Fatal(e)
	}
	defer func() { _ = c.Process.Kill(); _ = c.Wait() }()
	var row InstanceInitializationInfo
	deadline := time.Now().Add(10 * time.Second)
	for time.Now().Before(deadline) {
		rows, e := m.PendingInstanceInitializations(context.Background())
		if e != nil {
			t.Fatal(e)
		}
		if len(rows) == 1 {
			row = rows[0]
			if _, e = os.Stat(filepath.Join(row.Path, "data/ready")); e == nil {
				break
			}
		}
		time.Sleep(20 * time.Millisecond)
	}
	if row.ID == "" {
		t.Fatal("finite initializer did not register")
	}
	if _, e := os.Stat(filepath.Join(row.Path, "data/ready")); e != nil {
		t.Fatal(e)
	}
	_ = c.Process.Kill()
	_ = c.Wait()
	row = initializationInfoOne(t, m)
	if e := m.RepairPackage(context.Background(), p.ID()); e == nil {
		t.Fatal("orphaned init job did not protect its binary")
	}
	job := "init-" + row.ID + "-tool"
	plist := filepath.Join(m.Root, "jobs", m.label(job)+".plist")
	original, e := os.ReadFile(plist)
	if e != nil {
		t.Fatal(e)
	}
	if e = os.WriteFile(plist, append(append([]byte{}, original...), []byte("\n<!-- changed ownership evidence -->\n")...), 0600); e != nil {
		t.Fatal(e)
	}
	if _, e = m.ResolveInstanceInitialization(context.Background(), row.ID, row.Revision); e == nil {
		t.Fatal("stopped an initializer with changed ownership evidence")
	}
	out, e := m.Run(context.Background(), "launchctl", "print", domain()+"/"+m.label(job))
	if e != nil || !parseJobState(out).Running {
		t.Fatal("ownership rejection stopped the initializer", e)
	}
	if e = os.WriteFile(plist, original, 0600); e != nil {
		t.Fatal(e)
	}
	archive, e := m.ResolveInstanceInitialization(context.Background(), row.ID, row.Revision)
	if e != nil {
		t.Fatal(e)
	}
	if _, e = os.Stat(filepath.Join(archive, "data/ready")); e != nil {
		t.Fatal("stopping initializer lost prepared data", e)
	}
	if e = m.Uninstall(context.Background(), p.ID()); e != nil {
		t.Fatal("owned init job configuration leaked", e)
	}
}

func TestInstanceInitializationToolFailureRetainsDataAndDoesNotRegister(t *testing.T) {
	m, p := initializationFixture(t, "#!/bin/sh\nexit 9\n")
	i := initializationInstance(t, p, "tool-failed")
	i.Kind = "postgresql"
	if e := m.CreateInstance(context.Background(), i); e == nil || !strings.Contains(e.Error(), "exit code 9") {
		t.Fatal("failed initializer was reported as successful", e)
	}
	s, e := m.Read()
	if e != nil || len(s.Instances) != 0 {
		t.Fatal("failed initializer registered an instance", e)
	}
	r := initializationInfoOne(t, m)
	if _, e = os.Stat(filepath.Join(r.Path, "password")); e != nil {
		t.Fatal("failed initialization lost prepared data", e)
	}
	if _, e = m.ResolveInstanceInitialization(context.Background(), r.ID, r.Revision); e != nil {
		t.Fatal(e)
	}
	if e = m.Uninstall(context.Background(), p.ID()); e != nil {
		t.Fatal("failed initialization retained a job reference", e)
	}
}

func TestInstanceInitializationAlreadyArchivedCanFinishWithoutTouchingReusedID(t *testing.T) {
	m, p := initializationFixture(t, "#!/bin/sh\nexit 0\n")
	i := initializationInstance(t, p, "reuse-after-archive")
	ctx, cancel := context.WithCancel(context.Background())
	m.Event = func(s string) {
		if s == "Database initialization prepared" {
			cancel()
		}
	}
	if e := m.CreateInstance(ctx, i); e == nil {
		t.Fatal("expected cancelled registration")
	}
	m.Event = nil
	r := initializationInfoOne(t, m)
	password, e := os.ReadFile(filepath.Join(r.Path, "password"))
	if e != nil {
		t.Fatal(e)
	}
	archive := filepath.Join(m.Root, "retained-initializations", r.ID)
	if e = os.MkdirAll(filepath.Dir(archive), 0700); e != nil {
		t.Fatal(e)
	}
	if e = os.Rename(r.Path, archive); e != nil {
		t.Fatal(e)
	}
	archived := initializationInfoOne(t, m)
	if archived.Status != "Archived" || !archived.CanResolve || archived.Path != archive {
		t.Fatal("archive move cannot be finished from its retained identity", archived)
	}
	if _, e = m.ResolveInstanceInitialization(context.Background(), r.ID, r.Revision); e == nil {
		t.Fatal("accepted the preview from before the archive move")
	}
	if e = m.CreateInstance(context.Background(), i); e != nil {
		t.Fatal("archived data blocked reuse of the instance ID", e)
	}
	currentPassword, e := os.ReadFile(filepath.Join(m.Root, "instances", i.ID, "password"))
	if e != nil {
		t.Fatal(e)
	}
	archived = initializationInfoOne(t, m)
	path, e := m.ResolveInstanceInitialization(context.Background(), archived.ID, archived.Revision)
	if e != nil || path != archive {
		t.Fatal(path, e)
	}
	after, e := os.ReadFile(filepath.Join(archive, "password"))
	if e != nil || !bytes.Equal(after, password) {
		t.Fatal("finish changed the archive", e)
	}
	after, e = os.ReadFile(filepath.Join(m.Root, "instances", i.ID, "password"))
	if e != nil || !bytes.Equal(after, currentPassword) {
		t.Fatal("finish touched a new instance with the old ID", e)
	}
	rows, e := m.PendingInstanceInitializations(context.Background())
	if e != nil || len(rows) != 0 {
		t.Fatal(rows, e)
	}
}

func TestInstanceInitializationArchiveActualCrashCanFinish(t *testing.T) {
	for _, source := range []string{"staged", "uncommitted"} {
		for _, point := range []string{"Database initialization data archived", "Database initialization archive committed"} {
			t.Run(source+"/"+point, func(t *testing.T) {
				m, p := initializationFixture(t, "#!/bin/sh\nexit 0\n")
				port := integrationPort(t)
				runChild := func(point string, reconcile bool) {
					t.Helper()
					c := exec.Command(os.Args[0], "-test.run=^TestInstanceInitializationCrashHelper$")
					c.Env = append(os.Environ(), "MSM_INITIALIZATION_CRASH_ROOT="+m.Root, "MSM_INITIALIZATION_CRASH_PORT="+fmt.Sprint(port), "MSM_INITIALIZATION_CRASH_POINT="+point)
					if reconcile {
						c.Env = append(c.Env, "MSM_INITIALIZATION_RECONCILE=1")
					}
					var exit *exec.ExitError
					if e := c.Run(); !errors.As(e, &exit) || exit.ExitCode() != 23 {
						t.Fatal("child did not exit at the requested stage", e)
					}
				}
				if source == "uncommitted" {
					runChild("Database initialization data published", false)
				} else {
					ctx, cancel := context.WithCancel(context.Background())
					defer cancel()
					m.Event = func(s string) {
						if s == "Database initialization prepared" {
							cancel()
						}
					}
					if e := m.CreateInstance(ctx, Instance{ID: "interrupted", Kind: "redis", Package: p.ID(), Port: port}); e == nil {
						t.Fatal("expected cancelled registration")
					}
					m.Event = nil
				}
				before := initializationInfoOne(t, m)
				password, e := os.ReadFile(filepath.Join(before.Path, "password"))
				if e != nil {
					t.Fatal(e)
				}
				runChild(point, true)
				after := initializationInfoOne(t, m)
				if after.Status != "Archived" || !after.CanResolve {
					t.Fatal("archive crash requires manual file changes", after)
				}
				if _, e = m.ResolveInstanceInitialization(context.Background(), before.ID, before.Revision); e == nil {
					t.Fatal("accepted a preview from before the move")
				}
				archive, e := m.ResolveInstanceInitialization(context.Background(), after.ID, after.Revision)
				if e != nil || archive != after.Path {
					t.Fatal(archive, e)
				}
				got, e := os.ReadFile(filepath.Join(archive, "password"))
				if e != nil || !bytes.Equal(got, password) {
					t.Fatal("finishing the archive changed prepared data", e)
				}
				if rows, e := m.PendingInstanceInitializations(context.Background()); e != nil || len(rows) != 0 {
					t.Fatal(rows, e)
				}
			})
		}
	}
}

func TestInstanceInitializationArchiveRecordRemovalFailureIsReportedAndRetryable(t *testing.T) {
	m, p := initializationFixture(t, "#!/bin/sh\nexit 0\n")
	i := initializationInstance(t, p, "record-removal")
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	m.Event = func(s string) {
		if s == "Database initialization prepared" {
			cancel()
		}
	}
	if e := m.CreateInstance(ctx, i); e == nil {
		t.Fatal("expected cancelled registration")
	}
	r := initializationInfoOne(t, m)
	dir := filepath.Join(m.Root, "initializations", r.ID)
	t.Cleanup(func() { _ = os.Chmod(dir, 0700) })
	m.Event = func(s string) {
		if s == "Database initialization archive committed" {
			if e := os.Chmod(dir, 0500); e != nil {
				t.Fatal(e)
			}
		}
	}
	_, e := m.ResolveInstanceInitialization(context.Background(), r.ID, r.Revision)
	if e == nil {
		t.Fatal("record cleanup failure was reported as successful")
	}
	if e = os.Chmod(dir, 0700); e != nil {
		t.Fatal(e)
	}
	m.Event = nil
	r = initializationInfoOne(t, m)
	if r.Status != "Archived" || !r.CanResolve {
		t.Fatal("cleanup failure cannot be retried", r)
	}
	if _, e = os.Stat(filepath.Join(r.Path, "password")); e != nil {
		t.Fatal("cleanup failure lost the archive", e)
	}
	if _, e = m.ResolveInstanceInitialization(context.Background(), r.ID, r.Revision); e != nil {
		t.Fatal(e)
	}
}

func TestInstanceInitializationForeignArchiveIsNotAccepted(t *testing.T) {
	m, p := initializationFixture(t, "#!/bin/sh\nexit 0\n")
	i := initializationInstance(t, p, "foreign-archive")
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	m.Event = func(s string) {
		if s == "Database initialization prepared" {
			cancel()
		}
	}
	if e := m.CreateInstance(ctx, i); e == nil {
		t.Fatal("expected cancelled registration")
	}
	m.Event = nil
	r := initializationInfoOne(t, m)
	// Keep the original directory alive so an inode cannot be recycled for the substitute.
	if e := os.Rename(r.Path, r.Path+"-preserved"); e != nil {
		t.Fatal(e)
	}
	archive := filepath.Join(m.Root, "retained-initializations", r.ID)
	if e := os.MkdirAll(archive, 0700); e != nil {
		t.Fatal(e)
	}
	value := filepath.Join(archive, "foreign-value")
	if e := os.WriteFile(value, []byte("42"), 0600); e != nil {
		t.Fatal(e)
	}
	r = initializationInfoOne(t, m)
	if r.Status != "Needs inspection" || r.CanResolve {
		t.Fatal("accepted a directory with a different identity", r)
	}
	if _, e := m.ResolveInstanceInitialization(context.Background(), r.ID, r.Revision); e == nil {
		t.Fatal("cleared the record for an unowned archive")
	}
	if b, e := os.ReadFile(value); e != nil || string(b) != "42" {
		t.Fatal("changed the unowned archive", e)
	}
}

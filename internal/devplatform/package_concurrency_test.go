package devplatform

import (
	"archive/zip"
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"syscall"
	"testing"
	"time"
)

func packageArchive(t *testing.T, version string) (Package, []byte) {
	t.Helper()
	var b bytes.Buffer
	z := zip.NewWriter(&b)
	h := &zip.FileHeader{Name: "bin/fixture", Method: zip.Deflate}
	h.SetMode(0755)
	w, e := z.CreateHeader(h)
	if e != nil {
		t.Fatal(e)
	}
	fmt.Fprintf(w, "#!/bin/sh\nprintf '%s'\n", version)
	if e = z.Close(); e != nil {
		t.Fatal(e)
	}
	sum := sha256.Sum256(b.Bytes())
	return Package{Name: "fixture", Version: version, Arch: runtime.GOARCH, URL: "https://unused.invalid/archive.zip", SHA256: hex.EncodeToString(sum[:]), Format: "zip", Binaries: map[string]string{"fixture": "bin/fixture"}}, b.Bytes()
}
func TestPackageDownloadsRunConcurrentlyAndMergeCurrentState(t *testing.T) {
	m := testManager(t)
	p1, b1 := packageArchive(t, "1.0.0")
	p2, b2 := packageArchive(t, "2.0.0")
	entered := make(chan string, 2)
	release := make(chan struct{})
	var once sync.Once
	unblock := func() { once.Do(func() { close(release) }) }
	defer unblock()
	server := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		entered <- r.URL.Path
		select {
		case <-release:
		case <-r.Context().Done():
			return
		}
		if r.URL.Path == "/one.zip" {
			w.Write(b1)
		} else {
			w.Write(b2)
		}
	}))
	defer server.Close()
	old := http.DefaultTransport
	http.DefaultTransport = server.Client().Transport
	defer func() { http.DefaultTransport = old }()
	p1.URL = server.URL + "/one.zip"
	p2.URL = server.URL + "/two.zip"
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	done := make(chan error, 2)
	go func() { done <- m.Install(ctx, p1) }()
	go func() { done <- New(m.Root).Install(ctx, p2) }()
	for n := 0; n < 2; n++ {
		select {
		case <-entered:
		case <-time.After(3 * time.Second):
			unblock()
			t.Fatal("independent package download was serialized behind the workspace lock")
		}
	}
	short, stop := context.WithTimeout(ctx, 2*time.Second)
	defer stop()
	project := Project{SchemaVersion: 1, ID: "concurrent-config", Name: "Saved during download", Directory: t.TempDir()}
	if e := New(m.Root).SaveProject(short, project); e != nil {
		unblock()
		t.Fatal("download blocked unrelated project save", e)
	}
	var busy *PackageBusyError
	if e := New(m.Root).Install(short, p1); !errors.As(e, &busy) || busy.PackageID != p1.ID() {
		unblock()
		t.Fatal("same package did not report its busy resource", e)
	}
	records, e := m.Operations(ctx)
	if e != nil {
		t.Fatal(e)
	}
	running := 0
	for _, r := range records {
		if r.Status == "Running" && r.Phase == "Downloading" && r.Busy {
			running++
		}
	}
	if running != 2 {
		t.Fatal("download phases did not match active callers", records)
	}
	unblock()
	for n := 0; n < 2; n++ {
		if e = <-done; e != nil {
			t.Fatal(e)
		}
	}
	state, e := m.Read()
	if e != nil || len(state.Packages) != 2 || state.Projects[project.ID].Name != project.Name {
		t.Fatal(state, e)
	}
	assertIntegrity(t, m, p1, "Verified")
	assertIntegrity(t, m, p2, "Verified")
}
func TestPackageCancellingOneDownloadKeepsOtherDownloadAndConfiguration(t *testing.T) {
	m := testManager(t)
	p1, b1 := packageArchive(t, "1.0.0")
	p2, b2 := packageArchive(t, "2.0.0")
	entered := make(chan string, 2)
	release := make(chan struct{})
	var once sync.Once
	unblock := func() { once.Do(func() { close(release) }) }
	defer unblock()
	server := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		entered <- r.URL.Path
		select {
		case <-release:
		case <-r.Context().Done():
			return
		}
		if r.URL.Path == "/one" {
			w.Write(b1)
		} else {
			w.Write(b2)
		}
	}))
	defer server.Close()
	old := http.DefaultTransport
	http.DefaultTransport = server.Client().Transport
	defer func() { http.DefaultTransport = old }()
	p1.URL = server.URL + "/one"
	p2.URL = server.URL + "/two"
	first, cancelFirst := context.WithCancel(context.Background())
	defer cancelFirst()
	second, cancelSecond := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancelSecond()
	done1, done2 := make(chan error, 1), make(chan error, 1)
	go func() { done1 <- m.Install(first, p1) }()
	go func() { done2 <- New(m.Root).Install(second, p2) }()
	for n := 0; n < 2; n++ {
		select {
		case <-entered:
		case <-time.After(3 * time.Second):
			unblock()
			t.Fatal("second download did not reach network")
		}
	}
	cancelFirst()
	if e := <-done1; !errors.Is(e, context.Canceled) {
		t.Fatal(e)
	}
	project := Project{SchemaVersion: 1, ID: "keep", Name: "Keep config", Directory: t.TempDir()}
	if e := m.SaveProject(second, project); e != nil {
		t.Fatal(e)
	}
	unblock()
	if e := <-done2; e != nil {
		t.Fatal(e)
	}
	state, e := m.Read()
	if e != nil || len(state.Packages) != 1 || state.Projects[project.ID].Name != project.Name {
		t.Fatal(state, e)
	}
	if _, ok := state.Packages[p1.ID()]; ok {
		t.Fatal("cancelled download was registered")
	}
	assertIntegrity(t, m, p2, "Verified")
	entries, _ := filepath.Glob(filepath.Join(m.Root, "packages", ".install-*"))
	if len(entries) > 0 {
		t.Fatal("cancelled staging files remain", entries)
	}
}
func TestPackageRepairRechecksNewlyStartedProjectBeforePublication(t *testing.T) {
	m, p := integrityFixture(t)
	ctx := context.Background()
	project := Project{SchemaVersion: 1, ID: "late-user", Name: "Started during repair", Directory: t.TempDir(), Services: []Service{{ID: "worker", Package: p.ID(), Command: []string{"fixture", "hold", filepath.Join(t.TempDir(), "ready")}}}}
	if e := m.SaveProject(ctx, project); e != nil {
		t.Fatal(e)
	}
	defer m.Down(context.Background(), project.ID)
	damaged := filepath.Join(m.packageDir(p), "lib/data")
	os.WriteFile(damaged, []byte("old damaged bytes"), 0755)
	var started bool
	var pid int
	m.Event = func(s string) {
		if s == "Downloading "+p.ID() && !started {
			started = true
			// Simulate a legacy caller that starts a registered job without the
			// package lease. Current Up waits for repair's exclusive lease.
			state, e := m.Read()
			if e != nil {
				t.Fatal(e)
			}
			args, env, e := m.serviceArgs(&state, project, project.Services[0])
			if e != nil {
				t.Fatal(e)
			}
			job := "p-" + project.ID + "-worker"
			if _, e = m.startJob(ctx, job, args, project.Directory, env, false); e != nil {
				t.Fatal(e)
			}
			if e = m.waitService(ctx, job, project.Services[0]); e != nil {
				t.Fatal(e)
			}
			pid = m.Snapshot(ctx, state).Services[ServiceStatusKey(project.ID, "worker")].PID
		}
	}
	if e := m.RepairPackage(ctx, p.ID()); e == nil || !strings.Contains(e.Error(), "stop project") {
		t.Fatal("repair replaced a newly running user", e)
	}
	m.Event = nil
	if !started || pid == 0 {
		t.Fatal("fixture did not start during preparation")
	}
	state, _ := m.Read()
	if current := m.Snapshot(ctx, state).Services[ServiceStatusKey(project.ID, "worker")].PID; current != pid {
		t.Fatal("repair stopped another operation's job", pid, current)
	}
	if b, e := os.ReadFile(damaged); e != nil || string(b) != "old damaged bytes" {
		t.Fatal("repair overwrote the active version", string(b), e)
	}
	stages, _ := filepath.Glob(filepath.Join(m.Root, "packages", ".install-*"))
	if len(stages) > 0 {
		t.Fatal("refused repair leaked prepared tree", stages)
	}
}
func TestPackageRepairRollbackFailureRetainsOriginalTree(t *testing.T) {
	m, p := integrityFixture(t)
	file := filepath.Join(m.packageDir(p), "lib/data")
	os.WriteFile(file, []byte("retain damaged original"), 0755)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	m.Event = func(s string) {
		if strings.HasPrefix(s, "Committing package files") {
			if e := os.Rename(m.packageDir(p), m.packageDir(p)+"-published"); e != nil {
				t.Fatal(e)
			}
			cancel()
		}
	}
	if e := m.RepairPackage(ctx, p.ID()); e == nil || !strings.Contains(e.Error(), "retained") {
		t.Fatal("expected retained rollback failure", e)
	}
	stages, _ := filepath.Glob(filepath.Join(m.Root, "packages", ".install-*"))
	if len(stages) != 1 {
		t.Fatal("cleanup erased or lost original staged tree", stages)
	}
	if b, e := os.ReadFile(filepath.Join(stages[0], "lib/data")); e != nil || string(b) != "retain damaged original" {
		t.Fatal(string(b), e)
	}
}
func TestPackageLeaseProcessHelper(t *testing.T) {
	root, id := os.Getenv("MSM_PACKAGE_LEASE_ROOT"), os.Getenv("MSM_PACKAGE_LEASE_ID")
	if root == "" {
		return
	}
	m := New(root)
	unlock, e := m.packageLease(context.Background(), id, true)
	if e != nil {
		os.Exit(25)
	}
	defer unlock()
	if e = os.WriteFile(filepath.Join(root, "lease-ready"), []byte("ready"), 0600); e != nil {
		os.Exit(26)
	}
	var b [1]byte
	_, _ = os.Stdin.Read(b[:])
	os.Exit(0)
}
func TestPackageSharedLeaseWaitReportsResourceAndCanCancelWithoutStateLock(t *testing.T) {
	m, p := integrityFixture(t)
	c := exec.Command(os.Args[0], "-test.run=^TestPackageLeaseProcessHelper$")
	c.Env = append(os.Environ(), "MSM_PACKAGE_LEASE_ROOT="+m.Root, "MSM_PACKAGE_LEASE_ID="+p.ID())
	in, e := c.StdinPipe()
	if e != nil {
		t.Fatal(e)
	}
	if e = c.Start(); e != nil {
		t.Fatal(e)
	}
	defer func() { in.Close(); _ = c.Process.Kill(); _ = c.Wait() }()
	deadline := time.Now().Add(5 * time.Second)
	for {
		if _, e = os.Stat(filepath.Join(m.Root, "lease-ready")); e == nil {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("child lease did not start")
		}
		time.Sleep(10 * time.Millisecond)
	}
	waiting := make(chan struct{})
	var once sync.Once
	observer := New(m.Root)
	observer.Event = func(s string) {
		if s == "Waiting for package "+p.ID() {
			once.Do(func() { close(waiting) })
		}
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	done := make(chan error, 1)
	go func() { _, e := observer.VerifyPackage(ctx, p.ID()); done <- e }()
	select {
	case <-waiting:
	case <-time.After(3 * time.Second):
		t.Fatal("no package waiting reason")
	}
	short, stop := context.WithTimeout(context.Background(), 2*time.Second)
	defer stop()
	if e = m.SaveProject(short, Project{SchemaVersion: 1, ID: "while-leased", Name: "Unrelated change", Directory: t.TempDir()}); e != nil {
		t.Fatal("shared wait held global state", e)
	}
	cancel()
	if e = <-done; !errors.Is(e, context.Canceled) {
		t.Fatal(e)
	}
	in.Close()
	if e = c.Wait(); e != nil {
		t.Fatal(e)
	}
}

func TestPackageLeaseRejectsLinkedStoreAndSpecialLockWithoutTouchingOutside(t *testing.T) {
	m := testManager(t)
	outside := t.TempDir()
	locks := filepath.Join(m.Root, "package-locks")
	if e := os.Remove(locks); e != nil {
		t.Fatal(e)
	}
	if e := os.Symlink(outside, locks); e != nil {
		t.Fatal(e)
	}
	if unlock, e := m.packageLease(context.Background(), "fixture@1-arm64", true); e == nil {
		unlock()
		t.Fatal("followed linked package lease store")
	}
	entries, _ := os.ReadDir(outside)
	if len(entries) != 0 {
		t.Fatal(entries)
	}
	os.Remove(locks)
	os.Mkdir(locks, 0700)
	target := filepath.Join(outside, "keep")
	os.WriteFile(target, []byte("outside data"), 0600)
	key := filepath.Join(locks, "fixture@1-arm64")
	os.Symlink(target, key)
	if unlock, e := m.packageLease(context.Background(), "fixture@1-arm64", true); e == nil {
		unlock()
		t.Fatal("followed linked lease file")
	}
	if b, e := os.ReadFile(target); e != nil || string(b) != "outside data" {
		t.Fatal(string(b), e)
	}
	os.Remove(key)
	if e := syscall.Mkfifo(key, 0600); e != nil {
		t.Fatal(e)
	}
	done := make(chan error, 1)
	go func() {
		unlock, e := m.packageLease(context.Background(), "fixture@1-arm64", true)
		if unlock != nil {
			unlock()
		}
		done <- e
	}()
	select {
	case e := <-done:
		if e == nil {
			t.Fatal("accepted FIFO lease")
		}
	case <-time.After(time.Second):
		t.Fatal("blocked opening special lease")
	}
}

func TestProjectStartupWaitingForPackageRepairCanCancelAndKeepsConfigurationAvailable(t *testing.T) {
	m, p := integrityFixture(t)
	ctx := context.Background()
	project := Project{SchemaVersion: Schema, ID: "waiting-start", Name: "Waiting start", Directory: t.TempDir(), Services: []Service{{ID: "worker", Package: p.ID(), Command: []string{"fixture", "hold", filepath.Join(t.TempDir(), "ready")}}}}
	if e := m.SaveProject(ctx, project); e != nil {
		t.Fatal(e)
	}
	invoked := false
	m.Event = func(message string) {
		if message != "Downloading "+p.ID() || invoked {
			return
		}
		invoked = true
		childCtx, cancel := context.WithCancel(ctx)
		defer cancel()
		worker := New(m.Root)
		waited := make(chan struct{})
		var signal sync.Once
		worker.Event = func(s string) {
			if s == "Waiting for package "+p.ID() {
				signal.Do(func() { close(waited) })
			}
		}
		done := make(chan error, 1)
		go func() { done <- worker.Up(childCtx, project.ID) }()
		defer func() {
			cancel()
			select {
			case <-done:
			case <-time.After(5 * time.Second):
			}
		}()
		select {
		case <-waited:
		case <-time.After(3 * time.Second):
			t.Fatal("startup did not report the exclusive repair lease")
		}
		short, stop := context.WithTimeout(ctx, 2*time.Second)
		defer stop()
		if e := m.SaveProject(short, Project{SchemaVersion: Schema, ID: "during-package-wait", Name: "Saved", Directory: t.TempDir()}); e != nil {
			t.Fatal("package wait blocked independent configuration", e)
		}
		cancel()
		e := <-done
		done <- e
		if !errors.Is(e, context.Canceled) {
			t.Fatal("startup package wait could not cancel", e)
		}
		if m.loaded(ctx, "p-"+project.ID+"-worker") {
			t.Fatal("cancelled package wait started a process")
		}
	}
	if e := m.RepairPackage(ctx, p.ID()); e != nil {
		t.Fatal(e)
	}
	if !invoked {
		t.Fatal("repair did not exercise the lease wait")
	}
	s, e := m.Read()
	if e != nil || s.Projects["during-package-wait"].Name != "Saved" {
		t.Fatal("repair replaced newer configuration", e)
	}
}

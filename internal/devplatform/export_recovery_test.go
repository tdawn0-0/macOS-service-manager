package devplatform

import (
	"bytes"
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"
)

func exportRecoveryFixture(t *testing.T) (*Manager, Project) {
	t.Helper()
	m := testManager(t)
	p := Project{SchemaVersion: Schema, ID: "export-source", Name: "Previous environment", Directory: t.TempDir(), Services: []Service{{ID: "web", Command: []string{"/usr/bin/true"}}}}
	if err := m.SaveProject(context.Background(), p); err != nil {
		t.Fatal(err)
	}
	if err := m.ExportProject(context.Background(), p.ID); err != nil {
		t.Fatal(err)
	}
	// Change both the declaration and lock, so interruption really leaves a mix.
	pkg, err := Resolve(context.Background(), "redis", "7.4.7")
	if err != nil {
		t.Fatal(err)
	}
	p.Name = "Prepared environment"
	p.Services[0] = Service{ID: "web", Package: pkg.ID(), Command: []string{"redis-cli", "--version"}}
	if err = m.withState(context.Background(), func(s *State) error { s.Projects[p.ID] = p; s.Packages[pkg.ID()] = pkg; return nil }); err != nil {
		t.Fatal(err)
	}
	return m, p
}
func cancelExportAfter(t *testing.T, m *Manager, p Project, stage string) {
	t.Helper()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	m.Event = func(s string) {
		if strings.HasSuffix(s, stage) {
			cancel()
		}
	}
	err := m.ExportProject(ctx, p.ID)
	m.Event = func(string) {}
	if !errors.Is(err, context.Canceled) {
		t.Fatal("expected interrupted export", err)
	}
}
func TestEnvironmentExportRecoveryCancellationAtEveryStage(t *testing.T) {
	for _, stage := range []string{"Prepared", "Saved devmanager.lock.json", "Saved devmanager.json"} {
		t.Run(stage, func(t *testing.T) {
			m, p := exportRecoveryFixture(t)
			cancelExportAfter(t, m, p, stage)
			preview, err := New(m.Root).InspectExportRecovery(context.Background(), p.Directory)
			if err != nil || preview == nil || !preview.CanResume || len(preview.Files) != 2 || preview.ProjectName != p.Name || len(preview.PackageIDs) != 1 {
				t.Fatal("recovery unavailable", preview, err)
			}
			info, err := os.Stat(filepath.Join(p.Directory, exportRecordName))
			if err != nil || info.Mode().Perm() != 0600 {
				t.Fatal("recovery record permissions", err)
			}
			if _, err = m.Import(context.Background(), p.Directory); err == nil || !strings.Contains(err.Error(), "unfinished") {
				t.Fatal("import accepted unfinished operation", err)
			}
			if err = m.RecoverExport(context.Background(), p.Directory, preview.Fingerprint); err != nil {
				t.Fatal(err)
			}
			if pending, err := m.InspectExportRecovery(context.Background(), p.Directory); err != nil || pending != nil {
				t.Fatal("commit left pending record", pending, err)
			}
			imported, err := m.Import(context.Background(), p.Directory)
			if err != nil || imported.Name != p.Name {
				t.Fatal("completed pair could not import", imported, err)
			}
		})
	}
}

func TestEnvironmentExportCrashChild(t *testing.T) {
	root := os.Getenv("MSM_EXPORT_CRASH_TEST_ROOT")
	if root == "" {
		return
	}
	m := New(root)
	m.Event = func(s string) {
		if strings.HasSuffix(s, "Saved devmanager.lock.json") {
			os.Exit(23)
		}
	}
	if err := m.ExportProject(context.Background(), "export-source"); err != nil {
		t.Fatal(err)
	}
	t.Fatal("child did not exit during export")
}
func TestEnvironmentExportRecoveryAfterProcessExit(t *testing.T) {
	m, p := exportRecoveryFixture(t)
	c := exec.Command(os.Args[0], "-test.run=^TestEnvironmentExportCrashChild$")
	c.Env = append(os.Environ(), "MSM_EXPORT_CRASH_TEST_ROOT="+m.Root)
	out, err := c.CombinedOutput()
	var exit *exec.ExitError
	if !errors.As(err, &exit) || exit.ExitCode() != 23 {
		t.Fatalf("crash helper: %v %s", err, out)
	}
	// A fresh manager must see authoritative on-disk state, not an in-memory task.
	m = New(m.Root)
	preview, err := m.InspectExportRecovery(context.Background(), p.Directory)
	if err != nil || preview == nil || preview.Phase != "Partially saved" {
		t.Fatal(preview, err)
	}
	if err = m.RecoverExport(context.Background(), p.Directory, preview.Fingerprint); err != nil {
		t.Fatal(err)
	}
	if _, err = m.Import(context.Background(), p.Directory); err != nil {
		t.Fatal(err)
	}
}
func TestEnvironmentExportRecoveryKeepsChangedFilesAndRequiresFreshPreview(t *testing.T) {
	m, p := exportRecoveryFixture(t)
	cancelExportAfter(t, m, p, "Saved devmanager.lock.json")
	preview, _ := m.InspectExportRecovery(context.Background(), p.Directory)
	path := filepath.Join(p.Directory, "devmanager.json")
	changed := []byte("user changed this after the export began\n")
	if err := os.WriteFile(path, changed, 0600); err != nil {
		t.Fatal(err)
	}
	if err := m.RecoverExport(context.Background(), p.Directory, preview.Fingerprint); err == nil {
		t.Fatal("stale preview accepted")
	}
	preview, err := m.InspectExportRecovery(context.Background(), p.Directory)
	if err != nil || preview.CanResume {
		t.Fatal("external edit did not block overwrite", preview, err)
	}
	if err = m.RecoverExport(context.Background(), p.Directory, preview.Fingerprint); err == nil {
		t.Fatal("modified file overwritten")
	}
	beforeLock, _ := os.ReadFile(filepath.Join(p.Directory, "devmanager.lock.json"))
	if err = m.DiscardExport(context.Background(), p.Directory, preview.Fingerprint); err != nil {
		t.Fatal(err)
	}
	after, _ := os.ReadFile(path)
	afterLock, _ := os.ReadFile(filepath.Join(p.Directory, "devmanager.lock.json"))
	if !bytes.Equal(changed, after) || !bytes.Equal(beforeLock, afterLock) {
		t.Fatal("discard changed user files")
	}
	if err = m.ExportProject(context.Background(), p.ID); err != nil {
		t.Fatal("cannot explicitly export again after discard", err)
	}
	if _, err = m.Import(context.Background(), p.Directory); err != nil {
		t.Fatal(err)
	}
}
func TestEnvironmentExportDiscardDoesNotMakeMixedPairImportable(t *testing.T) {
	m, p := exportRecoveryFixture(t)
	cancelExportAfter(t, m, p, "Saved devmanager.lock.json")
	preview, _ := m.InspectExportRecovery(context.Background(), p.Directory)
	if err := m.DiscardExport(context.Background(), p.Directory, preview.Fingerprint); err != nil {
		t.Fatal(err)
	}
	if _, err := m.Import(context.Background(), p.Directory); err == nil || !strings.Contains(err.Error(), "do not match") {
		t.Fatal("mixed pair accepted", err)
	}
}
func TestEnvironmentExportReaderWaitsForWriterAcrossRoots(t *testing.T) {
	m, p := exportRecoveryFixture(t)
	entered, release := make(chan struct{}), make(chan struct{})
	m.Event = func(s string) {
		if strings.HasSuffix(s, "Saved devmanager.lock.json") {
			close(entered)
			<-release
		}
	}
	done := make(chan error, 1)
	go func() { done <- m.ExportProject(context.Background(), p.ID) }()
	select {
	case <-entered:
	case <-time.After(3 * time.Second):
		t.Fatal("writer did not reach commit")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 150*time.Millisecond)
	_, err := New(t.TempDir()).Import(ctx, p.Directory)
	cancel()
	close(release)
	if writerErr := <-done; writerErr != nil {
		t.Fatal(writerErr)
	}
	if !errors.Is(err, context.DeadlineExceeded) {
		t.Fatal("reader observed half-written pair", err)
	}
	if _, err = New(t.TempDir()).Import(context.Background(), p.Directory); err != nil {
		t.Fatal(err)
	}
}
func TestEnvironmentExportRecordDoesNotStorePreviousSecrets(t *testing.T) {
	m, p := exportRecoveryFixture(t)
	if err := os.WriteFile(filepath.Join(p.Directory, "devmanager.json"), []byte(`{"legacySecret":"do-not-copy-previous-secret"}`), 0600); err != nil {
		t.Fatal(err)
	}
	cancelExportAfter(t, m, p, "Prepared")
	r, _, err := readExportRecord(p.Directory)
	if err != nil {
		t.Fatal(err)
	}
	for _, file := range r.Files {
		if bytes.Contains(file.Next, []byte("do-not-copy-previous-secret")) {
			t.Fatal("recovery stores prior secrets")
		}
	}
}
func TestEnvironmentExportRefusesSpecialTargetsWithoutBlocking(t *testing.T) {
	for _, kind := range []string{"fifo", "symlink"} {
		t.Run(kind, func(t *testing.T) {
			m, p := exportRecoveryFixture(t)
			path := filepath.Join(p.Directory, "devmanager.json")
			if err := os.Remove(path); err != nil {
				t.Fatal(err)
			}
			var err error
			if kind == "fifo" {
				err = syscall.Mkfifo(path, 0600)
			} else {
				err = os.Symlink(filepath.Join(t.TempDir(), "outside"), path)
			}
			if err != nil {
				t.Fatal(err)
			}
			ctx, cancel := context.WithTimeout(context.Background(), time.Second)
			defer cancel()
			if err = m.ExportProject(ctx, p.ID); err == nil {
				t.Fatal("special target replaced")
			}
			if _, err = os.Lstat(filepath.Join(p.Directory, exportRecordName)); !errors.Is(err, os.ErrNotExist) {
				t.Fatal("record created before targets were validated", err)
			}
		})
	}
}

func TestEnvironmentExportLockUsesDirectoryIdentityForCaseAliases(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "case-project")
	if err := os.Mkdir(dir, 0700); err != nil {
		t.Fatal(err)
	}
	alias := filepath.Join(filepath.Dir(dir), "CASE-PROJECT")
	a, err := os.Stat(dir)
	if err != nil {
		t.Fatal(err)
	}
	b, err := os.Stat(alias)
	if errors.Is(err, os.ErrNotExist) {
		t.Skip("case-sensitive filesystem has no case alias")
	}
	if err != nil || !os.SameFile(a, b) {
		t.Fatal("unexpected directory alias", err)
	}
	unlock, err := lockEnvironmentFiles(context.Background(), dir)
	if err != nil {
		t.Fatal(err)
	}
	defer unlock()
	ctx, cancel := context.WithTimeout(context.Background(), 100*time.Millisecond)
	defer cancel()
	if other, err := lockEnvironmentFiles(ctx, alias); err == nil {
		other()
		t.Fatal("case alias bypassed resource lock")
	} else if !errors.Is(err, context.DeadlineExceeded) {
		t.Fatal(err)
	}
}

func TestEnvironmentExportWriteFailureCanResume(t *testing.T) {
	if os.Geteuid() == 0 {
		t.Skip("root bypasses directory write permissions")
	}
	m, p := exportRecoveryFixture(t)
	t.Cleanup(func() { _ = os.Chmod(p.Directory, 0700) })
	m.Event = func(message string) {
		if strings.HasSuffix(message, "Saved devmanager.lock.json") {
			if err := os.Chmod(p.Directory, 0500); err != nil {
				t.Fatal(err)
			}
		}
	}
	err := m.ExportProject(context.Background(), p.ID)
	m.Event = func(string) {}
	if resetErr := os.Chmod(p.Directory, 0700); resetErr != nil {
		t.Fatal(resetErr)
	}
	if !errors.Is(err, os.ErrPermission) {
		t.Fatal("expected a real file publication failure", err)
	}
	preview, err := m.InspectExportRecovery(context.Background(), p.Directory)
	if err != nil || preview == nil || !preview.CanResume {
		t.Fatal("write failure lost recovery", preview, err)
	}
	if err = m.RecoverExport(context.Background(), p.Directory, preview.Fingerprint); err != nil {
		t.Fatal(err)
	}
	if _, err = m.Import(context.Background(), p.Directory); err != nil {
		t.Fatal(err)
	}
}

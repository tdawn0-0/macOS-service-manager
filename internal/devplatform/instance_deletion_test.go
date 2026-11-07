package devplatform

import (
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

func deletionFixture(t *testing.T) (*Manager, Instance, string) {
	t.Helper()
	m := testManager(t)
	m.Run = func(ctx context.Context, name string, args ...string) (string, error) {
		if name == "launchctl" && len(args) > 0 && args[0] == "print" {
			return "", errors.New("Could not find service")
		}
		return "", errors.New("unexpected command")
	}
	i := Instance{ID: "alpha", Kind: "redis", Package: "redis@test-universal", Port: 56379, Persistent: true}
	dir := filepath.Join(m.Root, "instances", i.ID)
	if e := os.MkdirAll(filepath.Join(dir, "data"), 0700); e != nil {
		t.Fatal(e)
	}
	for name, value := range map[string]string{"password": "secret-password-must-not-escape", "data/value": "42", "valkey.conf": "bind 127.0.0.1\n"} {
		if e := os.WriteFile(filepath.Join(dir, name), []byte(value), 0600); e != nil {
			t.Fatal(e)
		}
	}
	if e := m.withState(context.Background(), func(s *State) error {
		s.Instances[i.ID] = i
		s.Packages[i.Package] = Package{Name: "redis", Version: "test", Arch: "universal"}
		return nil
	}); e != nil {
		t.Fatal(e)
	}
	return m, i, dir
}
func deletionPreview(t *testing.T, m *Manager, id string) InstanceDeletionPreview {
	t.Helper()
	p, e := m.PreviewInstanceDeletion(context.Background(), id)
	if e != nil {
		t.Fatal(e)
	}
	return p
}
func pendingDeletion(t *testing.T, m *Manager) InstanceDeletionInfo {
	t.Helper()
	var list []InstanceDeletionInfo
	var e error
	deadline := time.Now().Add(5 * time.Second)
	for {
		list, e = m.PendingInstanceDeletions(context.Background())
		if e != nil {
			t.Fatal(e)
		}
		if len(list) != 1 {
			t.Fatal(list)
		}
		if !list[0].Busy || time.Now().After(deadline) {
			break
		}
		time.Sleep(20 * time.Millisecond)
	}
	return list[0]
}
func TestInstanceDeletionPreviewAndPermanentRemovalKeepsOtherResources(t *testing.T) {
	m, i, dir := deletionFixture(t)
	ctx := context.Background()
	outside := filepath.Join(t.TempDir(), "keep")
	os.WriteFile(outside, []byte("outside"), 0600)
	os.Symlink(filepath.Dir(outside), filepath.Join(dir, "outside-link"))
	backup := filepath.Join(m.Root, "backups", "keep.dump")
	os.WriteFile(backup, []byte("backup"), 0600)
	pkg := filepath.Join(m.Root, "packages", "keep")
	os.WriteFile(pkg, []byte("runtime"), 0600)
	p := deletionPreview(t, m, i.ID)
	if !p.CanDelete || p.Path != dir || p.Bytes < 2 || p.Entries < 5 {
		t.Fatal(p)
	}
	before, _ := os.ReadFile(filepath.Join(dir, "password"))
	if e := m.DeleteInstanceData(ctx, i.ID, p.Fingerprint, "wrong"); e == nil {
		t.Fatal("accepted wrong confirmation")
	}
	after, _ := os.ReadFile(filepath.Join(dir, "password"))
	if string(before) != string(after) {
		t.Fatal("confirmation changed data")
	}
	if e := m.DeleteInstanceData(ctx, i.ID, p.Fingerprint, i.ID); e != nil {
		t.Fatal(e)
	}
	if _, e := os.Stat(dir); !errors.Is(e, os.ErrNotExist) {
		t.Fatal("data remains", e)
	}
	s, e := m.Read()
	if e != nil {
		t.Fatal(e)
	}
	if _, ok := s.Instances[i.ID]; ok {
		t.Fatal("instance record remains")
	}
	if len(s.Packages) != 1 {
		t.Fatal("removed runtime")
	}
	for path, want := range map[string]string{outside: "outside", backup: "backup", pkg: "runtime"} {
		b, e := os.ReadFile(path)
		if e != nil || string(b) != want {
			t.Fatal(path, e)
		}
	}
	list, e := m.PendingInstanceDeletions(ctx)
	if e != nil || len(list) != 0 {
		t.Fatal(list, e)
	}
	records, e := m.Operations(ctx)
	if e != nil {
		t.Fatal(e)
	}
	for _, r := range records {
		b, _ := os.ReadFile(filepath.Join(m.operationRoot(), r.ID, "record.json"))
		if strings.Contains(string(b), "secret-password-must-not-escape") {
			t.Fatal("credential in journal")
		}
	}
}
func TestInstanceDeletionRejectsReferencesRunningUnknownAndChangedData(t *testing.T) {
	m, i, dir := deletionFixture(t)
	ctx := context.Background()
	p := deletionPreview(t, m, i.ID)
	os.WriteFile(filepath.Join(dir, "data", "new"), []byte("new"), 0600)
	if e := m.DeleteInstanceData(ctx, i.ID, p.Fingerprint, i.ID); e == nil {
		t.Fatal("accepted changed tree")
	}
	m.Run = func(context.Context, string, ...string) (string, error) { return "state = running", nil }
	p = deletionPreview(t, m, i.ID)
	if p.CanDelete {
		t.Fatal("allowed loaded job")
	}
	m.Run = func(context.Context, string, ...string) (string, error) {
		return "", errors.New("observation unavailable")
	}
	p = deletionPreview(t, m, i.ID)
	if p.CanDelete || !strings.Contains(p.Detail, "Cannot confirm") {
		t.Fatal(p)
	}
	if _, e := os.Stat(filepath.Join(dir, "data", "value")); e != nil {
		t.Fatal(e)
	}
}
func TestInstanceDeletionCancellationBeforeAndAfterCommit(t *testing.T) {
	for _, point := range []string{"Database data staged for deletion", "Database deletion committed", "Deleting staged database data"} {
		t.Run(point, func(t *testing.T) {
			m, i, dir := deletionFixture(t)
			p := deletionPreview(t, m, i.ID)
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			m.Event = func(s string) {
				if s == point {
					cancel()
				}
			}
			if e := m.DeleteInstanceData(ctx, i.ID, p.Fingerprint, i.ID); !errors.Is(e, context.Canceled) {
				t.Fatal(e)
			}
			info := pendingDeletion(t, m)
			m.Event = func(string) {}
			if point == "Database data staged for deletion" {
				if info.Status != "Not moved" || !info.CanRestore {
					t.Fatal(info)
				}
				if b, e := os.ReadFile(filepath.Join(dir, "data/value")); e != nil || string(b) != "42" {
					t.Fatal(e)
				}
				if e := m.RestoreInstanceDeletion(context.Background(), info.ID, info.Revision); e != nil {
					t.Fatal(e)
				}
			} else {
				if !info.CanErase || info.CanRestore {
					t.Fatal(info)
				}
				if e := m.FinishInstanceDeletion(context.Background(), info.ID, info.Revision, "wrong"); e == nil {
					t.Fatal("accepted wrong confirmation")
				}
				if e := m.FinishInstanceDeletion(context.Background(), info.ID, info.Revision, i.ID); e != nil {
					t.Fatal(e)
				}
				if _, e := os.Stat(dir); !errors.Is(e, os.ErrNotExist) {
					t.Fatal(e)
				}
			}
			list, e := m.PendingInstanceDeletions(context.Background())
			if e != nil || len(list) != 0 {
				t.Fatal(list, e)
			}
		})
	}
}
func TestInstanceDeletionStateSaveFailureRollsBackData(t *testing.T) {
	m, i, dir := deletionFixture(t)
	p := deletionPreview(t, m, i.ID)
	state := filepath.Join(m.Root, "state.json")
	saved := state + ".saved"
	m.Event = func(s string) {
		if s == "Database data staged for deletion" {
			if e := os.Rename(state, saved); e != nil {
				t.Fatal(e)
			}
			if e := os.Mkdir(state, 0700); e != nil {
				t.Fatal(e)
			}
		}
	}
	if e := m.DeleteInstanceData(context.Background(), i.ID, p.Fingerprint, i.ID); e == nil {
		t.Fatal("expected actual state write failure")
	}
	os.Remove(state)
	os.Rename(saved, state)
	if b, e := os.ReadFile(filepath.Join(dir, "data/value")); e != nil || string(b) != "42" {
		t.Fatal(e)
	}
	info := pendingDeletion(t, m)
	if !info.CanRestore {
		t.Fatal(info)
	}
	if e := m.RestoreInstanceDeletion(context.Background(), info.ID, info.Revision); e != nil {
		t.Fatal(e)
	}
}
func TestInstanceDeletionCrashHelper(t *testing.T) {
	root := os.Getenv("MSM_DELETION_CRASH_ROOT")
	if root == "" {
		return
	}
	m := New(root)
	p, e := m.PreviewInstanceDeletion(context.Background(), "alpha")
	if e != nil {
		os.Exit(25)
	}
	m.Event = func(s string) {
		if s == os.Getenv("MSM_DELETION_CRASH_POINT") {
			os.Exit(23)
		}
	}
	_ = m.DeleteInstanceData(context.Background(), "alpha", p.Fingerprint, "alpha")
	os.Exit(26)
}
func TestInstanceDeletionActualProcessCrashRestoreOrFinish(t *testing.T) {
	for _, point := range []string{"Database data staged for deletion", "Database deletion committed"} {
		t.Run(point, func(t *testing.T) {
			m, i, dir := deletionFixture(t)
			c := exec.Command(os.Args[0], "-test.run=^TestInstanceDeletionCrashHelper$")
			c.Env = append(os.Environ(), "MSM_DELETION_CRASH_ROOT="+m.Root, "MSM_DELETION_CRASH_POINT="+point)
			var exit *exec.ExitError
			if e := c.Run(); !errors.As(e, &exit) || exit.ExitCode() != 23 {
				t.Fatal(e)
			}
			info := pendingDeletion(t, m)
			if point == "Database data staged for deletion" {
				if !info.CanRestore || info.CanErase {
					t.Fatal(info)
				}
				if e := m.FinishInstanceDeletion(context.Background(), info.ID, info.Revision, i.ID); e == nil {
					t.Fatal("erased uncommitted data")
				}
				if e := m.RestoreInstanceDeletion(context.Background(), info.ID, info.Revision); e != nil {
					t.Fatal(e)
				}
				if b, e := os.ReadFile(filepath.Join(dir, "data/value")); e != nil || string(b) != "42" {
					t.Fatal(e)
				}
			} else {
				if !info.CanErase {
					t.Fatal(info)
				}
				if e := m.FinishInstanceDeletion(context.Background(), info.ID, info.Revision, i.ID); e != nil {
					t.Fatal(e)
				}
			}
		})
	}
}
func TestInstanceDeletionResumeRejectsChangedSnapshotAndKeepsReusedID(t *testing.T) {
	m, i, dir := deletionFixture(t)
	p := deletionPreview(t, m, i.ID)
	ctx, cancel := context.WithCancel(context.Background())
	m.Event = func(s string) {
		if s == "Database deletion committed" {
			cancel()
		}
	}
	_ = m.DeleteInstanceData(ctx, i.ID, p.Fingerprint, i.ID)
	m.Event = func(string) {}
	info := pendingDeletion(t, m)
	os.WriteFile(filepath.Join(info.StagedPath, "extra"), []byte("late"), 0600)
	if e := m.FinishInstanceDeletion(context.Background(), info.ID, info.Revision, i.ID); e == nil {
		t.Fatal("accepted changed deletion preview")
	}
	if e := os.MkdirAll(dir, 0700); e != nil {
		t.Fatal(e)
	}
	os.WriteFile(filepath.Join(dir, "new-instance"), []byte("new data"), 0600)
	newInstance := i
	newInstance.Port++
	m.withState(context.Background(), func(s *State) error { s.Instances[i.ID] = newInstance; return nil })
	job := filepath.Join(m.Root, "jobs", m.label("db-"+i.ID)+".plist")
	os.WriteFile(job, []byte("new instance job"), 0600)
	info = pendingDeletion(t, m)
	if !info.CanErase || info.CanRestore {
		t.Fatal(info)
	}
	if e := m.FinishInstanceDeletion(context.Background(), info.ID, info.Revision, i.ID); e != nil {
		t.Fatal(e)
	}
	for path, want := range map[string]string{filepath.Join(dir, "new-instance"): "new data", job: "new instance job"} {
		b, e := os.ReadFile(path)
		if e != nil || string(b) != want {
			t.Fatal(path, e)
		}
	}
}
func TestInstanceDeletionRejectsUnsafeStorageWithoutFollowingLinks(t *testing.T) {
	m, i, dir := deletionFixture(t)
	syscall.Mkfifo(filepath.Join(dir, "fifo"), 0600)
	if _, e := m.PreviewInstanceDeletion(context.Background(), i.ID); e == nil {
		t.Fatal("accepted FIFO")
	}
	os.Remove(filepath.Join(dir, "fifo"))
	outside := t.TempDir()
	os.Rename(dir, dir+"-saved")
	os.Symlink(outside, dir)
	if _, e := m.PreviewInstanceDeletion(context.Background(), i.ID); e == nil {
		t.Fatal("accepted linked data root")
	}
	os.Remove(dir)
	os.Rename(dir+"-saved", dir)
	os.Symlink(outside, filepath.Join(m.Root, "deletions"))
	if _, e := m.PreviewInstanceDeletion(context.Background(), i.ID); e == nil {
		t.Fatal("accepted linked deletion store")
	}
	entries, _ := os.ReadDir(outside)
	if len(entries) != 0 {
		t.Fatal("wrote outside managed storage")
	}
}

func TestInstanceDeletionRollbackConflictNeverPermitsUncommittedErasure(t *testing.T) {
	m, i, dir := deletionFixture(t)
	p := deletionPreview(t, m, i.ID)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	m.Event = func(s string) {
		if s == "Database data staged for deletion" {
			if e := os.Mkdir(dir, 0700); e != nil {
				t.Fatal(e)
			}
			os.WriteFile(filepath.Join(dir, "foreign"), []byte("keep foreign directory"), 0600)
			cancel()
		}
	}
	if e := m.DeleteInstanceData(ctx, i.ID, p.Fingerprint, i.ID); e == nil {
		t.Fatal("expected cancellation and occupied rollback target")
	}
	m.Event = func(string) {}
	info := pendingDeletion(t, m)
	if info.CanErase || info.CanRestore || info.Status != "Restore blocked" {
		t.Fatal(info)
	}
	if e := m.FinishInstanceDeletion(context.Background(), info.ID, info.Revision, i.ID); e == nil {
		t.Fatal("erased registered instance data after failed rollback")
	}
	if b, e := os.ReadFile(filepath.Join(info.StagedPath, "data/value")); e != nil || string(b) != "42" {
		t.Fatal(e)
	}
	if e := os.Rename(dir, dir+"-foreign"); e != nil {
		t.Fatal(e)
	}
	info = pendingDeletion(t, m)
	if !info.CanRestore {
		t.Fatal(info)
	}
	if e := m.RestoreInstanceDeletion(context.Background(), info.ID, info.Revision); e != nil {
		t.Fatal(e)
	}
	for path, want := range map[string]string{filepath.Join(dir, "data/value"): "42", filepath.Join(dir+"-foreign", "foreign"): "keep foreign directory"} {
		b, e := os.ReadFile(path)
		if e != nil || string(b) != want {
			t.Fatal(path, e)
		}
	}
}
func TestInstanceDeletionLiveLeaseRejectsConcurrentRecovery(t *testing.T) {
	m, i, _ := deletionFixture(t)
	p := deletionPreview(t, m, i.ID)
	ctx, cancel := context.WithCancel(context.Background())
	m.Event = func(s string) {
		if s == "Database deletion committed" {
			cancel()
		}
	}
	_ = m.DeleteInstanceData(ctx, i.ID, p.Fingerprint, i.ID)
	m.Event = func(string) {}
	info := pendingDeletion(t, m)
	entered, release := make(chan struct{}), make(chan struct{})
	done := make(chan error, 1)
	go func() {
		done <- m.withDeletionLease(context.Background(), info.ID, false, func(string, string) error { close(entered); <-release; return nil })
	}()
	<-entered
	list, e := m.PendingInstanceDeletions(context.Background())
	if e != nil || len(list) != 1 || !list[0].Busy || list[0].CanErase {
		t.Fatal(list, e)
	}
	if e = m.FinishInstanceDeletion(context.Background(), info.ID, info.Revision, i.ID); e == nil {
		t.Fatal("concurrent erasure was allowed")
	}
	close(release)
	if e = <-done; e != nil {
		t.Fatal(e)
	}
	info = pendingDeletion(t, m)
	if e = m.FinishInstanceDeletion(context.Background(), info.ID, info.Revision, i.ID); e != nil {
		t.Fatal(e)
	}
}

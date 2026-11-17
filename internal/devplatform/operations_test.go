package devplatform

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"syscall"
	"testing"
)

func TestOperationLiveLeaseResultsAndRedaction(t *testing.T) {
	m := testManager(t)
	ctx := context.Background()
	entered, release := make(chan struct{}), make(chan struct{})
	done := make(chan error, 1)
	go func() {
		done <- m.runOperation(ctx, OperationRequest{Kind: "up", Resource: "project"}, "", func(w *Manager) error {
			w.emit("Starting api password=must-not-persist")
			close(entered)
			<-release
			return errors.New("secret-password must-not-persist")
		})
	}()
	<-entered
	records, e := New(m.Root).Operations(ctx)
	if e != nil || len(records) != 1 || !records[0].Busy || records[0].Status != "Running" || records[0].Phase != "Starting" {
		t.Fatal(records, e)
	}
	r := records[0]
	if e = m.DismissOperation(ctx, r.ID, r.Revision); e == nil {
		t.Fatal("dismissed live operation")
	}
	if e = m.RetryOperation(ctx, r.ID, r.Revision); e == nil {
		t.Fatal("retried live operation")
	}
	close(release)
	if e = <-done; e == nil {
		t.Fatal("lost action error")
	}
	records, e = m.Operations(ctx)
	if e != nil || records[0].Status != "Failed" || records[0].Busy {
		t.Fatal(records, e)
	}
	path, _, _ := m.operationFiles(r.ID)
	b, e := os.ReadFile(path)
	if e != nil || strings.Contains(string(b), "must-not-persist") || strings.Contains(string(b), "password") {
		t.Fatal(string(b), e)
	}
	info, _ := os.Stat(path)
	if info.Mode().Perm() != 0600 {
		t.Fatal(info.Mode())
	}
	dto, _ := json.Marshal(records[0])
	if !strings.Contains(string(dto), "Revision") {
		t.Fatal("CLI cannot act on public task snapshot")
	}
	if e = m.DismissOperation(ctx, r.ID, r.Revision); e == nil {
		t.Fatal("accepted stale preview")
	}
	if e = m.DismissOperation(ctx, r.ID, records[0].Revision); e != nil {
		t.Fatal(e)
	}
}
func TestOperationCancellationAndConcurrentCallers(t *testing.T) {
	m := testManager(t)
	var wg sync.WaitGroup
	for n := 0; n < 12; n++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			e := m.runOperation(context.Background(), OperationRequest{Kind: "up", Resource: "project"}, "", func(w *Manager) error { return context.Canceled })
			if !errors.Is(e, context.Canceled) {
				t.Error(e)
			}
		}()
	}
	wg.Wait()
	records, e := m.Operations(context.Background())
	if e != nil || len(records) != 12 {
		t.Fatal(len(records), e)
	}
	ids := map[string]bool{}
	for _, r := range records {
		if r.Status != "Cancelled" || r.Busy || ids[r.ID] {
			t.Fatal(r)
		}
		ids[r.ID] = true
	}
}
func TestOperationCrashHelper(t *testing.T) {
	root := os.Getenv("MSM_OPERATION_CRASH_ROOT")
	if root == "" {
		return
	}
	m := New(root)
	if os.Getenv("MSM_OPERATION_BACKUP_CRASH") == "1" {
		id := os.Getenv("MSM_OPERATION_BACKUP_ID")
		if id == "" {
			id = "pg"
		}
		_, _ = m.Backup(context.Background(), id, "source")
		os.Exit(24)
	}
	_ = m.runOperation(context.Background(), OperationRequest{Kind: "db-start", Resource: "missing"}, "", func(w *Manager) error { w.emit("Starting api"); os.Exit(23); return nil })
	os.Exit(25)
}
func TestOperationActualProcessCrashAndRetryLineage(t *testing.T) {
	m := testManager(t)
	ctx := context.Background()
	c := exec.Command(os.Args[0], "-test.run=^TestOperationCrashHelper$")
	c.Env = append(os.Environ(), "MSM_OPERATION_CRASH_ROOT="+m.Root)
	err := c.Run()
	var exit *exec.ExitError
	if !errors.As(err, &exit) || exit.ExitCode() != 23 {
		t.Fatal(err)
	}
	records, e := m.Operations(ctx)
	if e != nil || len(records) != 1 || records[0].Status != "Interrupted" {
		t.Fatal(records, e)
	}
	r := records[0]
	if e = m.RetryOperation(ctx, r.ID, r.Revision); e == nil {
		t.Fatal("missing instance retry must fail")
	}
	records, e = m.Operations(ctx)
	if e != nil || len(records) != 2 {
		t.Fatal(records, e)
	}
	var child, parent OperationRecord
	for _, item := range records {
		if item.ID == r.ID {
			parent = item
		} else {
			child = item
		}
	}
	if child.Status != "Failed" || child.RetryOf != r.ID || parent.Followup != child.ID || OperationCanRetry(parent) {
		t.Fatal(parent, child)
	}
	if e = m.RetryOperation(ctx, r.ID, r.Revision); e == nil {
		t.Fatal("accepted outdated parent record")
	}
}
func TestOperationSpecialRecordsNeverBlockOrFollowLinks(t *testing.T) {
	m := testManager(t)
	outside := t.TempDir()
	if e := os.Symlink(outside, m.operationRoot()); e != nil {
		t.Fatal(e)
	}
	if e := m.InstanceAction(context.Background(), "missing", "start"); e == nil {
		t.Fatal("followed linked operation store")
	}
	entries, _ := os.ReadDir(outside)
	if len(entries) != 0 {
		t.Fatal(entries)
	}
	os.Remove(m.operationRoot())
	os.Mkdir(m.operationRoot(), 0700)
	dir := filepath.Join(m.operationRoot(), "fake")
	os.Mkdir(dir, 0700)
	syscall.Mkfifo(filepath.Join(dir, "record.json"), 0600)
	if _, e := m.Operations(context.Background()); e == nil {
		t.Fatal("accepted FIFO")
	}
}

// Listing tasks on a new root only reads; dismissing a task removes its
// record and nothing it worked on.
func TestOperationsReadOnlyListingAndDismissKeepsResources(t *testing.T) {
	root := filepath.Join(t.TempDir(), "new-root")
	m := New(root)
	ctx := context.Background()
	if records, e := m.Operations(ctx); e != nil || len(records) != 0 {
		t.Fatal(records, e)
	}
	if _, e := os.Stat(root); !os.IsNotExist(e) {
		t.Fatal("listing tasks created storage", e)
	}
	if e := m.InstanceAction(ctx, "missing", "start"); e == nil {
		t.Fatal("started a missing instance")
	}
	records, e := m.Operations(ctx)
	if e != nil || len(records) != 1 || records[0].Revision == "" {
		t.Fatal(records, e)
	}
	kept := filepath.Join(root, "instances", "keep")
	if e = os.MkdirAll(kept, 0700); e != nil {
		t.Fatal(e)
	}
	if e = m.DismissOperation(ctx, records[0].ID, records[0].Revision); e != nil {
		t.Fatal(e)
	}
	if _, e = os.Stat(kept); e != nil {
		t.Fatal("dismiss removed a resource", e)
	}
	if records, e = m.Operations(ctx); e != nil || len(records) != 0 {
		t.Fatal("dismissed task is still listed", records, e)
	}
}

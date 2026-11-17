package devplatform

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"syscall"
	"time"
)

// Requests deliberately exclude command arguments, environment, passwords and URLs.
// These are private local records, never part of a shared environment or report.
type OperationRequest struct {
	Port                                                              int `json:",omitempty"`
	Kind, Resource, Name, Version, Arch, SHA256, Database, SourcePath string
}
type OperationRecord struct {
	Schema                                             int
	ID                                                 string
	Request                                            OperationRequest
	Phase, Status, Result, Artifact, RetryOf, Followup string
	StartedAt, UpdatedAt                               time.Time
	Revision                                           string `json:",omitempty"`
	Busy                                               bool   `json:",omitempty"`
}
type operationWriter struct {
	mu     sync.Mutex
	record OperationRecord
	path   string
	err    error
}

func writeOperationRecord(path string, r OperationRecord) error {
	r.Revision = ""
	r.Busy = false
	if e := writeJSON(path, r); e != nil {
		return e
	}
	return syncDirectory(filepath.Dir(path))
}
func (w *operationWriter) save() {
	if w.err == nil {
		w.record.UpdatedAt = time.Now()
		w.err = writeOperationRecord(w.path, w.record)
	}
}
func (w *operationWriter) phase(phase string) {
	w.mu.Lock()
	defer w.mu.Unlock()
	if w.record.Phase != phase {
		w.record.Phase = phase
		w.save()
	}
}
func (m *Manager) operationArtifact(path string) {
	if m.databaseWork != nil {
		if e := writeJSON(filepath.Join(m.databaseWork.dir, "artifact.json"), struct{ Path string }{path}); e != nil {
			m.emit("Could not record database output")
		}
		_ = syncDirectory(m.databaseWork.dir)
	}
	if m.operation == nil {
		return
	}
	w := m.operation
	w.mu.Lock()
	defer w.mu.Unlock()
	w.record.Artifact = path
	w.save()
}
func operationWorkPhase(kind string) string {
	switch kind {
	case "install", "repair":
		return "Preparing"
	case "initialize":
		return "Initializing"
	case "backup":
		return "Backing up"
	case "restore":
		return "Restoring"
	case "db-start":
		return "Starting"
	case "db-stop":
		return "Stopping"
	case "db-restart":
		return "Restarting"
	}
	return "Working"
}
func operationPhase(message string) string {
	for _, p := range [][2]string{{"Checking installed package ", "Checking files"}, {"Downloading ", "Downloading"}, {"Verifying and extracting ", "Extracting"}, {"Committing package ", "Publishing"}, {"Starting ", "Starting"}, {"Stopping ", "Stopping"}, {"Waiting ", "Waiting"}, {"Installed ", "Committed"}, {"Repaired ", "Committed"}} {
		if strings.HasPrefix(message, p[0]) {
			return p[1]
		}
	}
	return ""
}
func (m *Manager) operationRoot() string { return filepath.Join(m.Root, "operations") }
func (m *Manager) validateOperationRoot() error {
	if !filepath.IsAbs(m.Root) {
		return errors.New("storage root must be absolute")
	}
	info, e := os.Lstat(m.operationRoot())
	if errors.Is(e, os.ErrNotExist) {
		return nil
	}
	if e != nil {
		return e
	}
	if !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
		return errors.New("operation store must be a plain directory")
	}
	return nil
}
func (m *Manager) runOperation(ctx context.Context, req OperationRequest, retryOf string, fn func(*Manager) error) error {
	if m.operation != nil {
		return fn(m)
	}
	for _, value := range []string{req.Resource, req.Name, req.Version, req.Arch, req.SHA256, req.Database} {
		if len(value) > 256 || strings.ContainsAny(value, "\x00\n\r") {
			return errors.New("operation metadata is invalid")
		}
	}
	if len(req.SourcePath) > 4096 || strings.ContainsRune(req.SourcePath, 0) {
		return errors.New("operation source path is invalid")
	}
	if e := ctx.Err(); e != nil {
		return e
	}
	if e := m.validateOperationRoot(); e != nil {
		return e
	}
	if e := m.init(); e != nil {
		return e
	}
	if e := os.MkdirAll(m.operationRoot(), 0700); e != nil {
		return e
	}
	id := randomID()
	dir := filepath.Join(m.operationRoot(), id)
	if e := os.Mkdir(dir, 0700); e != nil {
		return e
	}
	f, e := os.OpenFile(filepath.Join(dir, "owner.lock"), os.O_CREATE|os.O_EXCL|os.O_RDWR, 0600)
	if e != nil {
		return e
	}
	defer f.Close()
	if e = syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); e != nil {
		return e
	}
	defer syscall.Flock(int(f.Fd()), syscall.LOCK_UN)
	w := &operationWriter{path: filepath.Join(dir, "record.json"), record: OperationRecord{Schema: 1, ID: id, Request: req, Status: "Running", Phase: "Queued", RetryOf: retryOf, StartedAt: time.Now()}}
	w.save()
	if w.err != nil {
		return w.err
	}
	if e = syncDirectory(m.operationRoot()); e != nil {
		return e
	}
	worker := *m
	worker.operation = w
	worker.Event = func(message string) {
		if phase := operationPhase(message); phase != "" {
			w.phase(phase)
		}
		if m.Event != nil {
			m.Event(message)
		}
	}
	if m.Event != nil {
		m.Event("Operation " + id + " · " + req.Kind)
	}
	var err error
	if retryOf != "" {
		parent, _, e := m.operationFiles(retryOf)
		if e == nil {
			var prior OperationRecord
			prior, e = readOperation(parent, retryOf)
			if e == nil {
				prior.Followup = id
				prior.UpdatedAt = time.Now()
				e = writeOperationRecord(parent, prior)
			}
		}
		err = e
	}
	if err == nil {
		err = fn(&worker)
	}
	w.mu.Lock()
	defer w.mu.Unlock()
	w.record.Phase = "Finished"
	w.record.Status = "Succeeded"
	w.record.Result = "Completed."
	if err != nil {
		w.record.Status = "Failed"
		w.record.Result = "Stopped with an error. Inspect the current resource before retrying."
		if ctx.Err() != nil || errors.Is(err, context.Canceled) || errors.Is(err, context.DeadlineExceeded) {
			w.record.Status = "Cancelled"
			w.record.Result = "Cancelled or timed out. Partial work may remain; inspect the current resource."
		}
	}
	// Never persist command output or raw errors: either may contain credentials.
	w.save()
	return errors.Join(err, w.err)
}
func (m *Manager) operationFiles(id string) (string, string, error) {
	if e := validID(id); e != nil {
		return "", "", e
	}
	if e := m.validateOperationRoot(); e != nil {
		return "", "", e
	}
	dir := filepath.Join(m.operationRoot(), id)
	info, e := os.Lstat(dir)
	if e != nil {
		return "", "", e
	}
	if !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
		return "", "", errors.New("operation directory is not a plain directory")
	}
	return filepath.Join(dir, "record.json"), filepath.Join(dir, "owner.lock"), nil
}
func readOperation(path, id string) (OperationRecord, error) {
	var r OperationRecord
	f, e := os.OpenFile(path, os.O_RDONLY|syscall.O_NOFOLLOW|syscall.O_NONBLOCK, 0)
	if e != nil {
		return r, e
	}
	defer f.Close()
	info, e := f.Stat()
	if e != nil {
		return r, e
	}
	if !info.Mode().IsRegular() {
		return r, errors.New("operation record is not a regular file")
	}
	b, e := io.ReadAll(io.LimitReader(f, 65537))
	if e != nil {
		return r, e
	}
	if len(b) > 65536 {
		return r, errors.New("operation record is too large")
	}
	if e = json.Unmarshal(b, &r); e != nil {
		return r, e
	}
	if r.Schema != 1 || r.ID != id {
		return r, errors.New("operation record identity mismatch")
	}
	r.Revision = digestBytes(b)
	return r, nil
}
func openOperationLease(path string) (*os.File, error) {
	f, e := os.OpenFile(path, os.O_RDONLY|syscall.O_NOFOLLOW|syscall.O_NONBLOCK, 0)
	if e != nil {
		return nil, e
	}
	info, e := f.Stat()
	if e != nil || !info.Mode().IsRegular() {
		f.Close()
		return nil, errors.New("operation lease is not a regular file")
	}
	e = syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB)
	if e != nil {
		f.Close()
		return nil, e
	}
	return f, nil
}
func closeOperationLease(f *os.File) { syscall.Flock(int(f.Fd()), syscall.LOCK_UN); f.Close() }
func (m *Manager) Operations(ctx context.Context) ([]OperationRecord, error) {
	if e := m.validateOperationRoot(); e != nil {
		return nil, e
	}
	dirs, e := os.ReadDir(m.operationRoot())
	if errors.Is(e, os.ErrNotExist) {
		return nil, nil
	}
	if e != nil {
		return nil, e
	}
	var records []OperationRecord
	for _, d := range dirs {
		if e = ctx.Err(); e != nil {
			return nil, e
		}
		if !d.IsDir() {
			continue
		}
		record, lease, e := m.operationFiles(d.Name())
		if e != nil {
			return nil, e
		}
		r, e := readOperation(record, d.Name())
		if errors.Is(e, os.ErrNotExist) {
			continue
		}
		if e != nil {
			return nil, e
		}
		f, e := openOperationLease(lease)
		if e == nil {
			latest, readErr := readOperation(record, d.Name())
			closeOperationLease(f)
			if errors.Is(readErr, os.ErrNotExist) {
				continue
			}
			if readErr != nil {
				return nil, readErr
			}
			r = latest
			if r.Status == "Running" {
				r.Status = "Interrupted"
				r.Result = "Owner released its operation lease without a recorded result. Recheck live jobs and resource state; no success is assumed."
			}
		} else if errors.Is(e, syscall.EWOULDBLOCK) {
			r.Busy = true
		} else {
			return nil, e
		}
		records = append(records, r)
	}
	sort.Slice(records, func(i, j int) bool { return records[i].StartedAt.After(records[j].StartedAt) })
	return records, nil
}
func (m *Manager) withReviewedOperation(ctx context.Context, id, revision string, fn func(string, OperationRecord) error) error {
	if e := ctx.Err(); e != nil {
		return e
	}
	record, lease, e := m.operationFiles(id)
	if e != nil {
		return e
	}
	f, e := openOperationLease(lease)
	if errors.Is(e, syscall.EWOULDBLOCK) {
		return errors.New("operation is still owned by a live caller; wait or cancel it in that caller")
	}
	if e != nil {
		return e
	}
	defer closeOperationLease(f)
	r, e := readOperation(record, id)
	if e != nil {
		return e
	}
	if r.Revision != revision {
		return errors.New("operation changed; refresh before acting")
	}
	return fn(record, r)
}
func (m *Manager) DismissOperation(ctx context.Context, id, revision string) error {
	return m.withReviewedOperation(ctx, id, revision, func(path string, r OperationRecord) error {
		if e := os.Remove(path); e != nil {
			return e
		}
		return syncDirectory(filepath.Dir(path))
	})
}
func OperationCanRetry(r OperationRecord) bool {
	if r.Busy || r.Followup != "" || r.Status == "Succeeded" || r.Status == "Running" {
		return false
	}
	switch r.Request.Kind {
	case "install", "repair", "db-start", "db-stop", "db-restart", "backup":
		return true
	}
	return false
}
func (m *Manager) RetryOperation(ctx context.Context, id, revision string) error {
	return m.withReviewedOperation(ctx, id, revision, func(path string, r OperationRecord) error {
		if r.Status == "Running" {
			r.Status = "Interrupted"
		}
		if !OperationCanRetry(r) {
			return errors.New("review this operation in its resource settings; automatic retry is unavailable")
		}
		req := r.Request
		// Link to the new durable operation before executing any follow-up work.
		return m.runOperation(ctx, req, r.ID, func(w *Manager) error {
			switch req.Kind {
			case "install":
				p, e := w.ResolvePackage(ctx, req.Name, req.Version)
				if e != nil {
					return e
				}
				if p.ID() != req.Resource || p.Arch != req.Arch || p.SHA256 != req.SHA256 {
					return errors.New("package source changed; review a fresh package preview")
				}
				return w.install(ctx, p)
			case "repair":
				return w.repairPackage(ctx, req.Resource)
			case "db-start", "db-stop", "db-restart":
				return w.instanceAction(ctx, req.Resource, strings.TrimPrefix(req.Kind, "db-"))
			case "backup":
				_, e := w.backup(ctx, req.Resource, req.Database)
				return e
			}
			return fmt.Errorf("unsupported operation %s", req.Kind)
		})
	})
}

func (m *Manager) Install(ctx context.Context, p Package) error {
	return m.runOperation(ctx, OperationRequest{Kind: "install", Resource: p.ID(), Name: p.Name, Version: p.Version, Arch: p.Arch, SHA256: p.SHA256}, "", func(w *Manager) error {
		if e := w.install(ctx, p); e != nil {
			return e
		}
		return w.defaultFirstNode(ctx, p)
	})
}
func (m *Manager) RepairPackage(ctx context.Context, id string) error {
	return m.runOperation(ctx, OperationRequest{Kind: "repair", Resource: id}, "", func(w *Manager) error { return w.repairPackage(ctx, id) })
}
func (m *Manager) CreateInstance(ctx context.Context, i Instance) error {
	return m.runOperation(ctx, OperationRequest{Kind: "initialize", Resource: i.ID, Name: i.Kind, Version: i.Package, Port: i.Port}, "", func(w *Manager) error { w.operation.phase("Initializing"); return w.createInstance(ctx, i) })
}
func (m *Manager) Backup(ctx context.Context, id, name string) (string, error) {
	var path string
	e := m.runOperation(ctx, OperationRequest{Kind: "backup", Resource: id, Database: name}, "", func(w *Manager) error {
		var e error
		w.operation.phase("Backing up")
		path, e = w.runDatabaseWork(ctx, databaseWorkRequest{Kind: "backup", InstanceID: id, Database: name}, databaseWorkInput{})
		return e
	})
	return path, e
}
func (m *Manager) Restore(ctx context.Context, id, name, path string) error {
	if strings.HasSuffix(path, ".partial") {
		return errors.New("incomplete backup output cannot be restored")
	}
	return m.runOperation(ctx, OperationRequest{Kind: "restore", Resource: id, Database: name, SourcePath: path}, "", func(w *Manager) error {
		w.operation.phase("Restoring")
		_, e := w.runDatabaseWork(ctx, databaseWorkRequest{Kind: "restore", InstanceID: id, Database: name, SourcePath: path}, databaseWorkInput{})
		return e
	})
}
func (m *Manager) InstanceAction(ctx context.Context, id, action string) error {
	if action != "start" && action != "stop" && action != "restart" {
		return m.instanceAction(ctx, id, action)
	}
	return m.runOperation(ctx, OperationRequest{Kind: "db-" + action, Resource: id}, "", func(w *Manager) error { return w.instanceAction(ctx, id, action) })
}

func (m *Manager) CreateDatabase(ctx context.Context, id, name string) error {
	return m.runOperation(ctx, OperationRequest{Kind: "create-database", Resource: id, Database: name}, "", func(w *Manager) error {
		_, e := w.runDatabaseWork(ctx, databaseWorkRequest{Kind: "create-database", InstanceID: id, Database: name}, databaseWorkInput{})
		return e
	})
}
func (m *Manager) CreateUser(ctx context.Context, id, name, password, database string) error {
	return m.runOperation(ctx, OperationRequest{Kind: "create-user", Resource: id, Database: database}, "", func(w *Manager) error {
		_, e := w.runDatabaseWork(ctx, databaseWorkRequest{Kind: "create-user", InstanceID: id, Database: database}, databaseWorkInput{User: name, Password: password})
		return e
	})
}

package devplatform

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"time"
)

type databaseWorkRequest struct{ Kind, InstanceID, Database, SourcePath string }
type databaseWorkInput struct{ User, Password string }
type databaseWorkRecord struct {
	Schema                                   int
	ID, Identity, JobDigest, ParentOperation string
	Instance                                 Instance
	Request                                  databaseWorkRequest
	CreatedAt                                time.Time
}
type databaseWorkResult struct {
	Status, Output string
	FinishedAt     time.Time
}
type databaseWorkContext struct {
	record databaseWorkRecord
	dir    string
}
type DatabaseWorkInfo struct {
	ID, InstanceID, Kind, Database, Status, Detail, Output, Revision string
	CallerActive, Running, CanResolve                                bool
}
type DatabaseWorkBusyError struct{ InstanceID, WorkID string }

func (e *DatabaseWorkBusyError) Error() string {
	return fmt.Sprintf("database instance %s has unfinished task %s; review Unfinished database tasks before changing it", e.InstanceID, e.WorkID)
}
func (e *DatabaseWorkBusyError) Unwrap() error {
	return &ResourceBusyError{Kind: "database instance", ID: e.InstanceID}
}

func (m *Manager) databaseWorkPaths(id string) (string, string, error) {
	if !filepath.IsAbs(m.Root) {
		return "", "", errors.New("storage root must be absolute")
	}
	if e := validID(id); e != nil {
		return "", "", e
	}
	root := filepath.Join(m.Root, "database-work")
	if e := plainDeletionDirectory(root); e != nil && !errors.Is(e, os.ErrNotExist) {
		return "", "", e
	}
	dir := filepath.Join(root, id)
	return dir, filepath.Join(dir, "record.json"), nil
}
func readDatabaseWork(dir, id string) (databaseWorkRecord, []byte, error) {
	var r databaseWorkRecord
	if e := plainDeletionDirectory(dir); e != nil {
		return r, nil, e
	}
	b, e := readExportFile(dir, "record.json", 65536)
	if e != nil {
		return r, nil, e
	}
	if e = json.Unmarshal(b, &r); e != nil {
		return r, nil, e
	}
	if r.Schema != 1 || r.ID != id || r.Request.InstanceID != r.Instance.ID || validID(r.Instance.ID) != nil || r.Identity == "" {
		return r, nil, errors.New("database task identity mismatch")
	}
	if r.JobDigest != "" && !validDigest(r.JobDigest) {
		return r, nil, errors.New("invalid database task ownership digest")
	}
	switch r.Request.Kind {
	case "backup", "restore", "create-database", "create-user":
	default:
		return r, nil, errors.New("unknown database task kind")
	}
	identity, e := initializationIdentity(dir)
	if e != nil || identity != r.Identity {
		return r, nil, errors.New("database task directory changed")
	}
	return r, b, nil
}
func (m *Manager) databaseWorkRecords() ([]databaseWorkRecord, error) {
	root := filepath.Join(m.Root, "database-work")
	if e := plainDeletionDirectory(root); errors.Is(e, os.ErrNotExist) {
		return nil, nil
	} else if e != nil {
		return nil, e
	}
	dirs, e := os.ReadDir(root)
	if e != nil {
		return nil, e
	}
	var records []databaseWorkRecord
	for _, d := range dirs {
		if !d.IsDir() {
			continue
		}
		dir, _, e := m.databaseWorkPaths(d.Name())
		if e != nil {
			return nil, e
		}
		r, _, e := readDatabaseWork(dir, d.Name())
		if errors.Is(e, os.ErrNotExist) {
			continue
		}
		if e != nil {
			return nil, e
		}
		records = append(records, r)
	}
	return records, nil
}
func (m *Manager) requireDatabaseWorkIdle(id string) error {
	records, e := m.databaseWorkRecords()
	if e != nil {
		return e
	}
	for _, r := range records {
		if r.Instance.ID == id && (m.databaseWork == nil || m.databaseWork.record.ID != r.ID) {
			return &DatabaseWorkBusyError{InstanceID: id, WorkID: r.ID}
		}
	}
	return nil
}
func (m *Manager) requirePackageDatabaseWorkIdle(id string) error {
	records, e := m.databaseWorkRecords()
	if e != nil {
		return e
	}
	for _, r := range records {
		if r.Instance.Package == id {
			return fmt.Errorf("package %s is reserved by unfinished database task %s", id, r.ID)
		}
	}
	return nil
}
func (m *Manager) recordDatabaseWorkJob(job, path string) error {
	c := m.databaseWork
	if c == nil {
		return nil
	}
	if job != "dbwork-"+c.record.ID {
		return errors.New("unexpected database task job")
	}
	b, e := readExportFile(filepath.Dir(path), filepath.Base(path), 1<<20)
	if e != nil {
		return e
	}
	c.record.JobDigest = digestBytes(b)
	if e = writeJSON(filepath.Join(c.dir, "record.json"), c.record); e != nil {
		return e
	}
	return syncExportDirectory(c.dir)
}
func (m *Manager) stopDatabaseWorkJob(ctx context.Context, r databaseWorkRecord) error {
	job := "dbwork-" + r.ID
	path := filepath.Join(m.Root, "jobs", m.label(job)+".plist")
	b, e := readExportFile(filepath.Dir(path), filepath.Base(path), 1<<20)
	if e == nil {
		if r.JobDigest == "" || digestBytes(b) != r.JobDigest {
			return errors.New("database task job ownership changed; keep its files for inspection")
		}
		if e = m.stopJob(ctx, job); e != nil {
			return e
		}
	} else if !errors.Is(e, os.ErrNotExist) {
		return e
	}
	if e = m.verifyShutdownTarget(ctx, domain()+"/"+m.label(job)); e != nil {
		return e
	}
	if len(b) > 0 {
		if e = os.Remove(path); e != nil && !errors.Is(e, os.ErrNotExist) {
			return e
		}
	}
	return nil
}
func readDatabaseWorkResult(dir string) (databaseWorkResult, []byte, error) {
	var r databaseWorkResult
	b, e := readExportFile(dir, "result.json", 65536)
	if e != nil {
		return r, nil, e
	}
	if e = json.Unmarshal(b, &r); e != nil {
		return r, nil, e
	}
	if r.Status != "Succeeded" && r.Status != "Failed" && r.Status != "Cancelled" {
		return r, nil, errors.New("invalid database task result")
	}
	return r, b, nil
}
func (m *Manager) workArtifact(dir string, r databaseWorkRecord) string {
	b, e := readExportFile(dir, "artifact.json", 8192)
	if e != nil {
		return ""
	}
	var artifact struct{ Path string }
	if json.Unmarshal(b, &artifact) != nil {
		return ""
	}
	path := filepath.Clean(artifact.Path)
	if r.Request.Kind != "backup" || filepath.Dir(path) != filepath.Join(m.Root, "backups") || !strings.HasPrefix(filepath.Base(path), r.Instance.ID+"-") {
		return ""
	}
	return path
}
func (m *Manager) clearDatabaseWork(dir string) error {
	execution, e := openOperationLease(filepath.Join(dir, "execution.lock"))
	if e != nil {
		return errors.New("database task process still owns its execution lease; wait and refresh")
	}
	defer closeOperationLease(execution)
	if e = os.RemoveAll(dir); e != nil {
		return e
	}
	return syncExportDirectory(filepath.Dir(dir))
}
func (m *Manager) runDatabaseWork(ctx context.Context, req databaseWorkRequest, input databaseWorkInput) (string, error) {
	if e := validID(req.InstanceID); e != nil {
		return "", e
	}
	if e := ctx.Err(); e != nil {
		return "", e
	}
	if e := m.init(); e != nil {
		return "", e
	}
	release, e := m.resourceLeases(ctx, instanceResourceKeys([]string{req.InstanceID})...)
	if e != nil {
		return "", e
	}
	defer func() {
		if release != nil {
			release()
		}
	}()
	state, e := m.Read()
	if e != nil {
		return "", e
	}
	instance, ok := state.Instances[req.InstanceID]
	if !ok {
		return "", errors.New("database instance not found")
	}
	if e := m.verifyDataLocation(instance); e != nil {
		return "", e
	}
	pkgRelease, e := m.packageLease(ctx, instance.Package, false)
	if e != nil {
		return "", e
	}
	defer pkgRelease()
	id := randomID()
	dir, path, e := m.databaseWorkPaths(id)
	if e != nil {
		return "", e
	}
	if e = os.MkdirAll(filepath.Dir(dir), 0700); e != nil {
		return "", e
	}
	if e = os.Mkdir(dir, 0700); e != nil {
		return "", e
	}
	for _, name := range []string{"control.lock", "execution.lock"} {
		if e = atomicWrite(filepath.Join(dir, name), nil, 0600); e != nil {
			return "", e
		}
	}
	control, e := openOperationLease(filepath.Join(dir, "control.lock"))
	if e != nil {
		return "", e
	}
	defer closeOperationLease(control)
	handoff, e := openOperationLease(filepath.Join(dir, "execution.lock"))
	if e != nil {
		return "", e
	}
	defer func() {
		if handoff != nil {
			closeOperationLease(handoff)
		}
	}()
	identity, e := initializationIdentity(dir)
	if e != nil {
		return "", e
	}
	record := databaseWorkRecord{Schema: 1, ID: id, Identity: identity, Instance: instance, Request: req, CreatedAt: time.Now()}
	if m.operation != nil {
		record.ParentOperation = m.operation.record.ID
	}
	if e = writeJSON(filepath.Join(dir, "input.json"), input); e != nil {
		return "", e
	}
	if e = writeJSON(path, record); e != nil {
		return "", e
	}
	if e = syncExportDirectory(dir); e != nil {
		return "", e
	}
	if e = syncExportDirectory(filepath.Dir(dir)); e != nil {
		return "", e
	}
	m.operationArtifact(path)
	m.emit("Using database " + req.InstanceID)
	worker := *m
	worker.databaseWork = &databaseWorkContext{record: record, dir: dir}
	program, e := m.helperExecutable("database-worker")
	if ctx.Err() != nil {
		e = ctx.Err()
	}
	if e == nil {
		_, e = worker.startJob(ctx, "dbwork-"+id, []string{program, "--dev", "database-worker", m.Root, id}, m.Root, nil, false)
	}
	record = worker.databaseWork.record
	release()
	release = nil
	closeOperationLease(handoff)
	handoff = nil
	if e != nil {
		cleanup, cancel := context.WithTimeout(context.Background(), 35*time.Second)
		defer cancel()
		if stopErr := m.stopDatabaseWorkJob(cleanup, record); stopErr != nil {
			return "", errors.Join(e, stopErr)
		}
		return "", errors.Join(e, m.clearDatabaseWork(dir))
	}
	m.emit("Running database task " + id)
	var result databaseWorkResult
	for {
		observed, resultBytes, resultErr := readDatabaseWorkResult(dir)
		out, observeErr := m.Run(ctx, "launchctl", "print", domain()+"/"+m.label("dbwork-"+id))
		if ctx.Err() != nil {
			cleanup, cancel := context.WithTimeout(context.Background(), 35*time.Second)
			defer cancel()
			stopErr := m.stopDatabaseWorkJob(cleanup, record)
			if stopErr == nil {
				if artifact := m.workArtifact(dir, record); artifact != "" {
					m.operationArtifact(artifact)
				}
				stopErr = m.clearDatabaseWork(dir)
			}
			return "", errors.Join(ctx.Err(), stopErr)
		}
		if observeErr != nil {
			return "", fmt.Errorf("could not observe database task %s; review Unfinished database tasks: %w", id, observeErr)
		}
		job := parseJobState(out)
		if job.finished() {
			observed, resultBytes, resultErr = readDatabaseWorkResult(dir)
			if resultErr != nil || len(resultBytes) == 0 {
				return "", fmt.Errorf("database task %s exited without a confirmed result; review Unfinished database tasks", id)
			}
			if observed.Status == "Succeeded" && !job.successfulExit() {
				return "", errors.New("database task result and job exit disagree; review its files")
			}
			result = observed
			break
		}
		select {
		case <-ctx.Done():
		case <-time.After(100 * time.Millisecond):
		}
	}
	artifact := m.workArtifact(dir, record)
	if artifact != "" {
		m.operationArtifact(artifact)
	}
	var operationErr error
	if result.Status != "Succeeded" {
		message := "Database operation did not complete."
		if b, e := readExportFile(dir, "error.log", 16384); e == nil && len(b) > 0 {
			message = string(b)
		}
		operationErr = errors.New(message)
	}
	cleanup, cancel := context.WithTimeout(context.Background(), 35*time.Second)
	defer cancel()
	if e = m.stopDatabaseWorkJob(cleanup, record); e != nil {
		return "", errors.Join(operationErr, e)
	}
	if e = m.clearDatabaseWork(dir); e != nil {
		return "", errors.Join(operationErr, e)
	}
	if operationErr != nil {
		return "", operationErr
	}
	return result.Output, nil
}

// RunDatabaseWorker is an internal launchd entry point. It never replays an
// existing result and runs only while the recorded job identifies this process.
func (m *Manager) RunDatabaseWorker(ctx context.Context, id string) error {
	ctx, cancel := context.WithTimeout(ctx, 30*time.Minute)
	defer cancel()
	dir, _, e := m.databaseWorkPaths(id)
	if e != nil {
		return e
	}
	record, _, e := readDatabaseWork(dir, id)
	if e != nil {
		return e
	}
	if _, _, e = readDatabaseWorkResult(dir); e == nil {
		return errors.New("database task already has a result")
	} else if !errors.Is(e, os.ErrNotExist) {
		return e
	}
	var execution *os.File
	for {
		execution, e = openOperationLease(filepath.Join(dir, "execution.lock"))
		if e == nil {
			break
		}
		if !errors.Is(e, syscall.EWOULDBLOCK) {
			return e
		}
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(20 * time.Millisecond):
		}
	}
	defer closeOperationLease(execution)
	path := filepath.Join(m.Root, "jobs", m.label("dbwork-"+id)+".plist")
	b, e := readExportFile(filepath.Dir(path), filepath.Base(path), 1<<20)
	if e != nil || record.JobDigest == "" || digestBytes(b) != record.JobDigest {
		return errors.New("database task job ownership is not verified")
	}
	job, e := m.readJobState(ctx, "dbwork-"+id)
	if e != nil || job.PID != os.Getpid() {
		return errors.New("database task must run as its recorded launchd job")
	}
	var input databaseWorkInput
	b, e = readExportFile(dir, "input.json", 1<<20)
	if e != nil {
		return e
	}
	if e = json.Unmarshal(b, &input); e != nil {
		return e
	}
	defer os.Remove(filepath.Join(dir, "input.json"))
	m.databaseWork = &databaseWorkContext{record: record, dir: dir}
	if e := m.verifyDataLocation(record.Instance); e != nil {
		return e
	}
	m.emit("Executing database task " + id)
	var output string
	switch record.Request.Kind {
	case "backup":
		output, e = m.backup(ctx, record.Request.InstanceID, record.Request.Database)
	case "restore":
		e = m.restore(ctx, record.Request.InstanceID, record.Request.Database, record.Request.SourcePath)
	case "create-database":
		e = m.createDatabase(ctx, record.Request.InstanceID, record.Request.Database)
	case "create-user":
		e = m.createUser(ctx, record.Request.InstanceID, input.User, input.Password, record.Request.Database)
	}
	result := databaseWorkResult{Status: "Succeeded", Output: output, FinishedAt: time.Now()}
	if e != nil {
		result.Status = "Failed"
		if ctx.Err() != nil {
			result.Status = "Cancelled"
		}
		message := e.Error()
		secrets := []string{input.Password}
		if password, err := os.ReadFile(filepath.Join(m.Root, "instances", record.Instance.ID, "password")); err == nil {
			secrets = append(secrets, string(password))
		}
		for _, secret := range secrets {
			if secret != "" {
				message = strings.ReplaceAll(message, secret, "[redacted]")
				quoted := strconv.Quote(secret)
				if len(quoted) > 2 {
					message = strings.ReplaceAll(message, quoted[1:len(quoted)-1], "[redacted]")
				}
			}
		}
		if len(message) > 16000 {
			message = message[:16000]
		}
		if err := atomicWrite(filepath.Join(dir, "error.log"), []byte(message), 0600); err != nil {
			return err
		}
	}
	if err := writeJSON(filepath.Join(dir, "result.json"), result); err != nil {
		return err
	}
	if err := syncExportDirectory(dir); err != nil {
		return err
	}
	if e != nil {
		return fmt.Errorf("database task %s did not complete", id)
	}
	return nil
}

func (m *Manager) databaseWorkInfo(ctx context.Context, id string) (DatabaseWorkInfo, error) {
	dir, _, e := m.databaseWorkPaths(id)
	if e != nil {
		return DatabaseWorkInfo{}, e
	}
	r, b, e := readDatabaseWork(dir, id)
	if e != nil {
		return DatabaseWorkInfo{}, e
	}
	info := DatabaseWorkInfo{ID: id, InstanceID: r.Instance.ID, Kind: r.Request.Kind, Database: r.Request.Database, Status: "Interrupted", Detail: "Review this task before changing the instance. Database and output files are retained."}
	control, e := openOperationLease(filepath.Join(dir, "control.lock"))
	if e == nil {
		closeOperationLease(control)
	} else if errors.Is(e, syscall.EWOULDBLOCK) {
		info.CallerActive = true
	} else {
		return info, e
	}
	result, resultBytes, resultErr := readDatabaseWorkResult(dir)
	if resultErr != nil && !errors.Is(resultErr, os.ErrNotExist) {
		return info, resultErr
	}
	path := filepath.Join(m.Root, "jobs", m.label("dbwork-"+id)+".plist")
	jobBytes, pathErr := readExportFile(filepath.Dir(path), filepath.Base(path), 1<<20)
	owned := pathErr == nil && r.JobDigest != "" && digestBytes(jobBytes) == r.JobDigest
	absent := false
	job, observeErr := m.readJobState(ctx, "dbwork-"+id)
	if observeErr == nil {
		info.Running = job.Running || job.PID > 0
	} else {
		absent = m.verifyShutdownTarget(ctx, domain()+"/"+m.label("dbwork-"+id)) == nil
	}
	info.CanResolve = !info.CallerActive && (owned || (errors.Is(pathErr, os.ErrNotExist) && absent)) && (observeErr == nil || absent)
	if info.CallerActive {
		info.Status = "Running"
		info.Detail = "The caller still owns this task. Cancel it there or wait for completion."
	} else if info.Running {
		info.Status = "Running without caller"
		info.Detail = "The caller exited, but the recorded worker is still running. Refresh to wait, or explicitly stop it. Existing database changes are not undone."
	} else if resultErr == nil && observeErr == nil && job.finished() {
		info.Status = result.Status
		info.Detail = "The worker finished. Finish review to release this instance. Database and output files are retained."
		if result.Status == "Succeeded" && !job.successfulExit() {
			info.Status = "Needs inspection"
			info.Detail = "The saved result disagrees with the worker exit. Files are retained; no write will be replayed."
		}
	}
	if !owned && !absent {
		info.Status = "Needs inspection"
		info.Detail = "Worker ownership or its live state could not be verified. No process will be stopped."
	}
	info.Output = m.workArtifact(dir, r)
	info.Revision = digestBytes(append(append(append([]byte{}, b...), resultBytes...), jobBytes...))
	return info, nil
}
func (m *Manager) PendingDatabaseWork(ctx context.Context) ([]DatabaseWorkInfo, error) {
	records, e := m.databaseWorkRecords()
	if e != nil {
		return nil, e
	}
	var result []DatabaseWorkInfo
	for _, r := range records {
		if e := ctx.Err(); e != nil {
			return nil, e
		}
		info, e := m.databaseWorkInfo(ctx, r.ID)
		if errors.Is(e, os.ErrNotExist) {
			continue
		}
		if e != nil {
			return nil, e
		}
		result = append(result, info)
	}
	return result, nil
}

// ResolveDatabaseWork never retries writes or rolls back an already changed target.
// It stops only the digest-verified finite job and releases its reservation.
func (m *Manager) ResolveDatabaseWork(ctx context.Context, id, revision string) error {
	return m.runOperation(ctx, OperationRequest{Kind: "reconcile-database-work", Resource: id}, "", func(w *Manager) error {
		dir, _, e := w.databaseWorkPaths(id)
		if e != nil {
			return e
		}
		r, _, e := readDatabaseWork(dir, id)
		if e != nil {
			return e
		}
		control, e := openOperationLease(filepath.Join(dir, "control.lock"))
		if e != nil {
			return errors.New("database task still has a caller; cancel it there or wait")
		}
		defer closeOperationLease(control)
		info, e := w.databaseWorkInfo(ctx, id)
		if e != nil {
			return e
		}
		// This caller now owns control.lock; the revision still validates the exact files reviewed.
		if info.Revision != revision {
			return errors.New("database task changed; refresh and review its current result")
		}
		// Probe ownership independently of the caller flag, which is ours now.
		// An explicit orphan review keeps diagnostic output. Normal cancellation
		// still removes unfinished output in the original caller.
		if e = atomicWrite(filepath.Join(dir, "retain-output"), []byte(r.ID), 0600); e != nil {
			return e
		}
		if e = syncExportDirectory(dir); e != nil {
			return e
		}
		if e = w.stopDatabaseWorkJob(ctx, r); e != nil {
			return e
		}
		copy := *w
		copy.databaseWork = &databaseWorkContext{record: r, dir: dir}
		release, e := copy.resourceLeases(ctx, instanceResourceKeys([]string{r.Instance.ID})...)
		if e != nil {
			return e
		}
		defer release()
		if output := w.workArtifact(dir, r); output != "" {
			w.operationArtifact(output)
		}
		return w.clearDatabaseWork(dir)
	})
}

func (m *Manager) retainDatabaseWorkOutput() bool {
	c := m.databaseWork
	if c == nil {
		return false
	}
	b, e := readExportFile(c.dir, "retain-output", 1024)
	return e == nil && string(b) == c.record.ID
}

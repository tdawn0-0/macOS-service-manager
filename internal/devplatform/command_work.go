package devplatform

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"syscall"
	"time"
)

const maxCommandOutput int64 = 64 << 20

type commandWorkRecord struct {
	Schema                                                                        int
	ID, Identity, ProjectID, StateDigest, InputDigest, JobDigest, ParentOperation string
	Instances, Packages                                                           []string
	CreatedAt                                                                     time.Time
}
type commandWorkInput struct {
	Encoding string
	Args     [][]byte
}

type commandWorkResult struct {
	Status     string
	FinishedAt time.Time
}
type commandWorkContext struct {
	record commandWorkRecord
	dir    string
}
type ProjectCommandInfo struct {
	ID, ProjectID, Status, Detail, Revision      string
	CallerActive, Running, CanResolve, HasOutput bool
}
type ProjectCommandBusyError struct{ ProjectID, WorkID string }

func (e *ProjectCommandBusyError) Error() string {
	return fmt.Sprintf("project %s has unfinished command %s; review Unfinished project commands before changing it", e.ProjectID, e.WorkID)
}
func (e *ProjectCommandBusyError) Unwrap() error {
	return &ResourceBusyError{Kind: "project", ID: e.ProjectID}
}
func (m *Manager) commandWorkDir(id string) (string, error) {
	if !filepath.IsAbs(m.Root) {
		return "", errors.New("storage root must be absolute")
	}
	if e := validID(id); e != nil {
		return "", e
	}
	parent := filepath.Join(m.Root, "command-work")
	if e := plainDeletionDirectory(parent); e != nil && !errors.Is(e, os.ErrNotExist) {
		return "", e
	}
	return filepath.Join(parent, id), nil
}
func readCommandWork(dir, id string) (commandWorkRecord, []byte, error) {
	var r commandWorkRecord
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
	if r.Schema != 1 || r.ID != id || validID(r.ProjectID) != nil || !validDigest(r.StateDigest) || !validDigest(r.InputDigest) || (r.JobDigest != "" && !validDigest(r.JobDigest)) {
		return r, nil, errors.New("project command identity mismatch")
	}
	identity, e := initializationIdentity(dir)
	if e != nil || identity != r.Identity {
		return r, nil, errors.New("project command directory changed")
	}
	for _, id := range r.Instances {
		if e := validID(id); e != nil {
			return r, nil, e
		}
	}
	return r, b, nil
}
func (m *Manager) commandWorkRecords() ([]commandWorkRecord, error) {
	parent := filepath.Join(m.Root, "command-work")
	if e := plainDeletionDirectory(parent); errors.Is(e, os.ErrNotExist) {
		return nil, nil
	} else if e != nil {
		return nil, e
	}
	dirs, e := os.ReadDir(parent)
	if e != nil {
		return nil, e
	}
	var records []commandWorkRecord
	for _, d := range dirs {
		if !d.IsDir() {
			continue
		}
		dir, e := m.commandWorkDir(d.Name())
		if e != nil {
			return nil, e
		}
		r, _, e := readCommandWork(dir, d.Name())
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
func (m *Manager) requireCommandResourceIdle(key resourceKey) error {
	records, e := m.commandWorkRecords()
	if e != nil {
		return e
	}
	for _, r := range records {
		if m.commandWork != nil && m.commandWork.record.ID == r.ID {
			continue
		}
		conflict := key.Kind == "project" && key.ID == r.ProjectID
		if key.Kind == "database instance" {
			for _, id := range r.Instances {
				conflict = conflict || id == key.ID
			}
		}
		if conflict {
			return &ProjectCommandBusyError{ProjectID: r.ProjectID, WorkID: r.ID}
		}
	}
	return nil
}
func (m *Manager) requirePackageCommandIdle(id string) error {
	records, e := m.commandWorkRecords()
	if e != nil {
		return e
	}
	for _, r := range records {
		for _, p := range r.Packages {
			if p == id {
				return fmt.Errorf("package %s is in use by unfinished project command %s", id, r.ID)
			}
		}
	}
	return nil
}
func commandStateDigest(s *State, p Project) string {
	instances := map[string]Instance{}
	for _, id := range p.Instances {
		instances[id] = s.Instances[id]
	}
	b, _ := json.Marshal(struct {
		Project   Project
		Instances map[string]Instance
	}{p, instances})
	return digestBytes(b)
}
func (m *Manager) recordCommandWorkJob(id, path string) error {
	c := m.commandWork
	if c == nil {
		return nil
	}
	if id != "command-"+c.record.ID {
		return errors.New("unexpected project command job")
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
func (m *Manager) stopCommandWork(ctx context.Context, r commandWorkRecord) error {
	id := "command-" + r.ID
	path := filepath.Join(m.Root, "jobs", m.label(id)+".plist")
	b, e := readExportFile(filepath.Dir(path), filepath.Base(path), 1<<20)
	if e == nil {
		if r.JobDigest == "" || digestBytes(b) != r.JobDigest {
			return errors.New("project command job ownership changed; keep its files for inspection")
		}
		if e = m.stopJob(ctx, id); e != nil {
			return e
		}
	} else if !errors.Is(e, os.ErrNotExist) {
		return e
	}
	if e = m.verifyShutdownTarget(ctx, domain()+"/"+m.label(id)); e != nil {
		return e
	}
	if len(b) > 0 {
		if e = os.Remove(path); e != nil && !errors.Is(e, os.ErrNotExist) {
			return e
		}
	}
	return nil
}
func clearCommandWork(dir string) error {
	execution, e := openOperationLease(filepath.Join(dir, "execution.lock"))
	if e != nil {
		return errors.New("project command still owns its execution lease; wait and refresh")
	}
	defer closeOperationLease(execution)
	if e = os.RemoveAll(dir); e != nil {
		return e
	}
	return syncExportDirectory(filepath.Dir(dir))
}
func readCommandResult(dir string) (commandWorkResult, []byte, error) {
	var r commandWorkResult
	b, e := readExportFile(dir, "result.json", 65536)
	if e != nil {
		return r, nil, e
	}
	if e = json.Unmarshal(b, &r); e != nil {
		return r, nil, e
	}
	if r.Status != "Succeeded" && r.Status != "Failed" && r.Status != "Cancelled" {
		return r, nil, errors.New("invalid project command result")
	}
	return r, b, nil
}
func commandOutput(dir string) (string, error) {
	b, e := readExportFile(dir, "output.log", maxCommandOutput)
	if errors.Is(e, os.ErrNotExist) {
		return "", nil
	}
	return string(b), e
}
func (m *Manager) runCommandWork(ctx context.Context, id string, args []string) (string, error) {
	if len(args) == 0 {
		return "", errors.New("command required")
	}
	if e := validID(id); e != nil {
		return "", e
	}
	rawArgs := make([][]byte, len(args))
	for i, arg := range args {
		rawArgs[i] = []byte(arg)
	}
	input, e := json.Marshal(commandWorkInput{Encoding: "bytes-v1", Args: rawArgs})
	if e != nil {
		return "", e
	}
	if len(input) > 2<<20 {
		return "", errors.New("project command arguments exceed size limit")
	}
	quiet := *m
	quiet.Event = nil
	var c *commandWorkContext
	var control, handoff *os.File
	defer func() {
		if control != nil {
			closeOperationLease(control)
		}
		if handoff != nil {
			closeOperationLease(handoff)
		}
	}()
	launched := false
	e = quiet.withProjectLifecycle(ctx, id, true, func(s *State) error {
		p := s.Projects[id]
		if len(p.Services) == 0 {
			return errors.New("project has no service environment")
		}
		if e := p.requireLocalSettings(ctx); e != nil {
			return e
		}
		workID := randomID()
		dir, e := m.commandWorkDir(workID)
		if e != nil {
			return e
		}
		if e = os.MkdirAll(filepath.Dir(dir), 0700); e != nil {
			return e
		}
		if e = os.Mkdir(dir, 0700); e != nil {
			return e
		}
		// No process can exist before bootstrap; remove a preparation that fails.
		prepared := false
		defer func() {
			if !prepared {
				_ = os.RemoveAll(dir)
			}
		}()
		for _, name := range []string{"control.lock", "execution.lock"} {
			if e = atomicWrite(filepath.Join(dir, name), nil, 0600); e != nil {
				return e
			}
		}
		control, e = openOperationLease(filepath.Join(dir, "control.lock"))
		if e != nil {
			return e
		}
		handoff, e = openOperationLease(filepath.Join(dir, "execution.lock"))
		if e != nil {
			return e
		}
		identity, e := initializationIdentity(dir)
		if e != nil {
			return e
		}
		r := commandWorkRecord{Schema: 1, ID: workID, Identity: identity, ProjectID: id, StateDigest: commandStateDigest(s, p), InputDigest: digestBytes(input), Instances: append([]string{}, p.Instances...), CreatedAt: time.Now()}
		seen := map[string]bool{}
		for _, svc := range p.Services {
			if svc.Package != "" {
				seen[svc.Package] = true
			}
		}
		for _, iid := range p.Instances {
			if i, ok := s.Instances[iid]; ok {
				seen[i.Package] = true
			}
		}
		r.Packages = diagnosticKeys(seen)
		if m.operation != nil {
			r.ParentOperation = m.operation.record.ID
		}
		if e = atomicWrite(filepath.Join(dir, "input.json"), input, 0600); e != nil {
			return e
		}
		if e = writeJSON(filepath.Join(dir, "record.json"), r); e != nil {
			return e
		}
		if e = syncExportDirectory(dir); e != nil {
			return e
		}
		if e = syncExportDirectory(filepath.Dir(dir)); e != nil {
			return e
		}
		c = &commandWorkContext{record: r, dir: dir}
		prepared = true
		program, e := m.helperExecutable("command-worker")
		if e != nil {
			return e
		}
		worker := quiet
		worker.commandWork = c
		// A durable record exists before the process can be submitted to launchd.
		launched = true
		_, e = worker.startJob(ctx, "command-"+workID, []string{program, "--dev", "command-worker", m.Root, workID}, m.Root, nil, false)
		return e
	})
	// Resource leases are now released; the worker can safely take them over.
	if handoff != nil {
		closeOperationLease(handoff)
		handoff = nil
	}
	cleanup := func() (string, error) {
		if c == nil {
			return "", nil
		}
		stop, cancel := context.WithTimeout(context.Background(), 35*time.Second)
		defer cancel()
		if launched {
			if e := m.stopCommandWork(stop, c.record); e != nil {
				return "", e
			}
		}
		output, e := commandOutput(c.dir)
		if detail, readErr := readExportFile(c.dir, "error.log", 16384); readErr == nil && len(detail) > 0 {
			if output != "" {
				output += "\n"
			}
			output += string(detail)
		}
		return output, errors.Join(e, clearCommandWork(c.dir))
	}
	if e != nil {
		output, stopErr := cleanup()
		return output, errors.Join(e, stopErr)
	}
	for {
		job, observeErr := m.readJobState(ctx, "command-"+c.record.ID)
		if ctx.Err() != nil {
			output, stopErr := cleanup()
			return output, errors.Join(ctx.Err(), stopErr)
		}
		if observeErr != nil {
			return "", fmt.Errorf("could not observe project command %s; review Unfinished project commands", c.record.ID)
		}
		if job.finished() {
			result, _, resultErr := readCommandResult(c.dir)
			if resultErr != nil {
				return "", errors.New("project command exited without a confirmed result; review Unfinished project commands")
			}
			if result.Status == "Succeeded" && !job.successfulExit() {
				return "", errors.New("project command result and exit disagree; review its files")
			}
			output, stopErr := cleanup()
			if result.Status != "Succeeded" {
				return output, errors.Join(errors.New("project command did not complete; review its output and target before running another command"), stopErr)
			}
			return output, stopErr
		}
		select {
		case <-ctx.Done():
		case <-time.After(100 * time.Millisecond):
		}
	}
}

// RunProjectCommandWorker is an internal finite launchd entry point. Inputs and
// output are private and never copied into operation history or launch arguments.
func (m *Manager) RunProjectCommandWorker(ctx context.Context, id string) error {
	ctx, cancel := context.WithTimeout(ctx, 30*time.Minute)
	defer cancel()
	dir, e := m.commandWorkDir(id)
	if e != nil {
		return e
	}
	r, _, e := readCommandWork(dir, id)
	if e != nil {
		return e
	}
	if _, _, e = readCommandResult(dir); e == nil {
		return errors.New("project command already has a result")
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
	path := filepath.Join(m.Root, "jobs", m.label("command-"+id)+".plist")
	b, e := readExportFile(filepath.Dir(path), filepath.Base(path), 1<<20)
	if e != nil || r.JobDigest == "" || digestBytes(b) != r.JobDigest {
		return errors.New("project command ownership is not verified")
	}
	job, e := m.readJobState(ctx, "command-"+id)
	if e != nil || job.PID != os.Getpid() {
		return errors.New("project command must run as its recorded launchd job")
	}
	b, e = readExportFile(dir, "input.json", 2<<20)
	if e != nil || digestBytes(b) != r.InputDigest {
		return errors.New("project command input changed")
	}
	var input commandWorkInput
	if e = json.Unmarshal(b, &input); e != nil {
		return e
	}
	if input.Encoding != "bytes-v1" {
		return errors.New("unsupported private command input encoding")
	}
	args := make([]string, len(input.Args))
	for i, arg := range input.Args {
		args[i] = string(arg)
	}
	defer os.Remove(filepath.Join(dir, "input.json"))
	m.commandWork = &commandWorkContext{record: r, dir: dir}
	e = m.withProjectLifecycle(ctx, r.ProjectID, true, func(s *State) error {
		if commandStateDigest(s, s.Projects[r.ProjectID]) != r.StateDigest {
			return errors.New("project command environment changed")
		}
		_, e := m.execProject(ctx, r.ProjectID, args)
		return e
	})
	if e != nil {
		message := e.Error()
		if len(message) > 16000 {
			message = message[:16000]
		}
		if err := atomicWrite(filepath.Join(dir, "error.log"), []byte(message), 0600); err != nil {
			return err
		}
	}
	result := commandWorkResult{Status: "Succeeded", FinishedAt: time.Now()}
	if e != nil {
		result.Status = "Failed"
		if ctx.Err() != nil {
			result.Status = "Cancelled"
		}
	}
	if err := writeJSON(filepath.Join(dir, "result.json"), result); err != nil {
		return err
	}
	if err := syncExportDirectory(dir); err != nil {
		return err
	}
	if e != nil {
		return errors.New("project command failed")
	}
	return nil
}

type commandOutputWriter struct {
	file      *os.File
	remaining int64
	cancel    context.CancelFunc
	exceeded  bool
}

func (w *commandOutputWriter) Write(p []byte) (int, error) {
	if int64(len(p)) > w.remaining {
		w.exceeded = true
		w.cancel()
		return 0, errors.New("project command output exceeds 64 MiB")
	}
	n, e := w.file.Write(p)
	w.remaining -= int64(n)
	return n, e
}
func (m *Manager) commandWorkInfo(ctx context.Context, id string) (ProjectCommandInfo, error) {
	dir, e := m.commandWorkDir(id)
	if e != nil {
		return ProjectCommandInfo{}, e
	}
	r, b, e := readCommandWork(dir, id)
	if e != nil {
		return ProjectCommandInfo{}, e
	}
	info := ProjectCommandInfo{ID: id, ProjectID: r.ProjectID, Status: "Interrupted", Detail: "No confirmed result. Review this command before changing the project. Commands are never replayed automatically."}
	control, e := openOperationLease(filepath.Join(dir, "control.lock"))
	if e == nil {
		closeOperationLease(control)
	} else if errors.Is(e, syscall.EWOULDBLOCK) {
		info.CallerActive = true
	} else {
		return info, e
	}
	result, resultBytes, resultErr := readCommandResult(dir)
	if resultErr != nil && !errors.Is(resultErr, os.ErrNotExist) {
		return info, resultErr
	}
	path := filepath.Join(m.Root, "jobs", m.label("command-"+id)+".plist")
	jobBytes, pathErr := readExportFile(filepath.Dir(path), filepath.Base(path), 1<<20)
	owned := pathErr == nil && r.JobDigest != "" && digestBytes(jobBytes) == r.JobDigest
	job, observeErr := m.readJobState(ctx, "command-"+id)
	absent := false
	if observeErr == nil {
		info.Running = job.Running || job.PID > 0
	} else {
		absent = m.verifyShutdownTarget(ctx, domain()+"/"+m.label("command-"+id)) == nil
	}
	info.CanResolve = !info.CallerActive && (owned || (errors.Is(pathErr, os.ErrNotExist) && absent)) && (observeErr == nil || absent)
	if info.CallerActive {
		info.Status = "Running"
		info.Detail = "The caller still owns this command. Cancel it there or wait."
	} else if info.Running {
		info.Status = "Running without caller"
		info.Detail = "The caller exited but the recorded command is still running. Wait and refresh, or explicitly stop it. Changes already written are retained."
	} else if resultErr == nil && observeErr == nil && job.finished() {
		info.Status = result.Status
		info.Detail = "The worker finished. Finish review to release its resources. The original caller's interrupted history is kept."
		if result.Status == "Succeeded" && !job.successfulExit() {
			info.Status = "Needs inspection"
			info.Detail = "The result disagrees with the worker exit. Review private files before clearing this task."
		}
	}
	if !owned && !absent {
		info.Status = "Needs inspection"
		info.Detail = "Command ownership or its live state could not be verified. No process will be stopped."
	}
	f, e := os.Lstat(filepath.Join(dir, "output.log"))
	info.HasOutput = e == nil && f.Mode().IsRegular() && f.Size() > 0
	info.Revision = digestBytes(append(append(append([]byte{}, b...), resultBytes...), jobBytes...))
	return info, nil
}
func (m *Manager) PendingProjectCommands(ctx context.Context) ([]ProjectCommandInfo, error) {
	records, e := m.commandWorkRecords()
	if e != nil {
		return nil, e
	}
	var infos []ProjectCommandInfo
	for _, r := range records {
		if e = ctx.Err(); e != nil {
			return nil, e
		}
		info, e := m.commandWorkInfo(ctx, r.ID)
		if errors.Is(e, os.ErrNotExist) {
			continue
		}
		if e != nil {
			return nil, e
		}
		infos = append(infos, info)
	}
	return infos, nil
}

// ResolveProjectCommand stops only the recorded owned job. Files written by the
// command outside its private task directory are never removed or rolled back.
func (m *Manager) ResolveProjectCommand(ctx context.Context, id, revision string) error {
	return m.runOperation(ctx, OperationRequest{Kind: "reconcile-project-command", Resource: id}, "", func(w *Manager) error {
		dir, e := w.commandWorkDir(id)
		if e != nil {
			return e
		}
		r, _, e := readCommandWork(dir, id)
		if e != nil {
			return e
		}
		control, e := openOperationLease(filepath.Join(dir, "control.lock"))
		if e != nil {
			return errors.New("project command still has a caller; cancel it there or wait")
		}
		defer closeOperationLease(control)
		info, e := w.commandWorkInfo(ctx, id)
		if e != nil {
			return e
		}
		if info.Revision != revision {
			return errors.New("project command changed; refresh and review it again")
		}
		if e = w.stopCommandWork(ctx, r); e != nil {
			return e
		}
		own := *w
		own.commandWork = &commandWorkContext{record: r, dir: dir}
		keys := append(projectResourceKeys(r.ProjectID), instanceResourceKeys(r.Instances)...)
		release, e := own.resourceLeases(ctx, keys...)
		if e != nil {
			return e
		}
		defer release()
		return clearCommandWork(dir)
	})
}

// ProjectCommandFiles returns only the identity-checked private task folder.
func (m *Manager) ProjectCommandFiles(id string) (string, error) {
	dir, e := m.commandWorkDir(id)
	if e != nil {
		return "", e
	}
	_, _, e = readCommandWork(dir, id)
	return dir, e
}

var _ io.Writer = (*commandOutputWriter)(nil)

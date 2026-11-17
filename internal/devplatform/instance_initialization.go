package devplatform

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"strconv"
	"syscall"
	"time"
)

type initializationRecord struct {
	Schema    int
	ID        string
	Instance  Instance
	Identity  string
	Jobs      map[string]string
	Ready     bool
	CreatedAt time.Time
}
type initializationContext struct {
	record    initializationRecord
	path, dir string
}
type InstanceInitializationInfo struct {
	ID, InstanceID, Kind, Package, Path, Status, Detail, Revision string
	Port                                                          int
	Busy, CanResolve                                              bool
}

func (m *Manager) instanceDirectory(id string) string {
	if c := m.initialization; c != nil && c.record.Instance.ID == id {
		return c.dir
	}
	return filepath.Join(m.Root, "instances", id)
}
func (m *Manager) databaseJobID(id string) string {
	if c := m.initialization; c != nil && c.record.Instance.ID == id {
		return "init-" + c.record.ID + "-server"
	}
	return "db-" + id
}
func (m *Manager) initializationPaths(id string) (string, string, string, error) {
	if e := validID(id); e != nil {
		return "", "", "", e
	}
	if !filepath.IsAbs(m.Root) {
		return "", "", "", errors.New("storage root must be absolute")
	}
	for _, name := range []string{"instances", "jobs", "initializations", "retained-initializations"} {
		e := plainDeletionDirectory(filepath.Join(m.Root, name))
		if e != nil && !errors.Is(e, os.ErrNotExist) {
			return "", "", "", e
		}
	}
	dir := filepath.Join(m.Root, "initializations", id)
	return dir, filepath.Join(dir, "record.json"), filepath.Join(dir, "data"), nil
}
func readInitialization(path, id string) (initializationRecord, []byte, error) {
	var r initializationRecord
	b, e := readRegularFile(filepath.Dir(path), filepath.Base(path), 65536)
	if e != nil {
		return r, nil, e
	}
	if e = json.Unmarshal(b, &r); e != nil {
		return r, nil, e
	}
	if r.Schema != 1 || r.ID != id || validID(r.Instance.ID) != nil || r.Identity == "" {
		return r, nil, errors.New("initialization record identity mismatch")
	}
	for job, digest := range r.Jobs {
		if (job != "init-"+id+"-server" && job != "init-"+id+"-tool") || !validDigest(digest) {
			return r, nil, errors.New("initialization job ownership is invalid")
		}
	}
	return r, b, nil
}
func (m *Manager) recordInitializationJob(job, path string) error {
	c := m.initialization
	if c == nil {
		return nil
	}
	if job != "init-"+c.record.ID+"-tool" && job != "init-"+c.record.ID+"-server" {
		return errors.New("unexpected initialization job")
	}
	b, e := readRegularFile(filepath.Dir(path), filepath.Base(path), 1<<20)
	if e != nil {
		return e
	}
	if c.record.Jobs == nil {
		c.record.Jobs = map[string]string{}
	}
	c.record.Jobs[job] = digestBytes(b)
	if e = writeJSON(c.path, c.record); e != nil {
		return e
	}
	return syncDirectory(filepath.Dir(c.path))
}
func (m *Manager) runInitializationTool(ctx context.Context, bin string, args ...string) (string, error) {
	c := m.initialization
	if c == nil {
		return "", errors.New("initialization tool requires a tracked preparation")
	}
	job := "init-" + c.record.ID + "-tool"
	ctx, cancel := context.WithTimeout(ctx, 5*time.Minute)
	defer cancel()
	// launchd may inherit locale names unsupported by Homebrew's libc tools.
	// Initialization must not depend on the desktop session's language settings.
	env := map[string]string{"LANG": "C", "LC_ALL": "C"}
	if _, e := m.startJob(ctx, job, append([]string{bin}, args...), c.dir, env, false); e != nil {
		return "", e
	}
	if _, ok := c.record.Jobs[job]; !ok {
		return "", errors.New("initialization tool is not recorded as owned")
	}
	for {
		out, e := m.Run(ctx, "launchctl", "print", domain()+"/"+m.label(job))
		if e != nil {
			return "", fmt.Errorf("initialization tool could not be observed: %w", e)
		}
		state := parseJobState(out)
		if !state.Running && state.PID == 0 && state.Runs > 0 && state.LastExitCode != nil {
			if *state.LastExitCode != 0 {
				return "", fmt.Errorf("initialization tool failed with exit code %d; read its managed log", *state.LastExitCode)
			}
			return "", nil
		}
		select {
		case <-ctx.Done():
			return "", ctx.Err()
		case <-time.After(100 * time.Millisecond):
		}
	}
}
func (m *Manager) stopInitializationJobs(ctx context.Context, r initializationRecord) error {
	for _, job := range diagnosticKeys(r.Jobs) {
		path := filepath.Join(m.Root, "jobs", m.label(job)+".plist")
		b, e := readRegularFile(filepath.Dir(path), filepath.Base(path), 1<<20)
		if e == nil && digestBytes(b) != r.Jobs[job] {
			return errors.New("initialization job configuration changed; keep its data for inspection")
		}
		if e != nil && !errors.Is(e, os.ErrNotExist) {
			return e
		}
		if errors.Is(e, os.ErrNotExist) {
			if e = m.verifyShutdownTarget(ctx, domain()+"/"+m.label(job)); e != nil {
				return errors.New("cannot confirm ownership of an initialization job whose configuration is missing")
			}
			continue
		}
		if e = m.stopJob(ctx, job); e != nil {
			return e
		}
		if e = m.verifyShutdownTarget(ctx, domain()+"/"+m.label(job)); e != nil {
			return e
		}
		if e = os.Remove(path); e != nil && !errors.Is(e, os.ErrNotExist) {
			return e
		}
	}
	return nil
}
func (m *Manager) checkNewInstance(ctx context.Context, s State, i Instance) error {
	if e := m.validateDataLocation(s, i); e != nil {
		return e
	}
	if _, ok := s.Instances[i.ID]; ok {
		return errors.New("instance already exists")
	}
	for _, other := range s.Instances {
		if other.Port == i.Port {
			return errors.New("instance port already assigned")
		}
	}
	for _, site := range proxyRoutesFromState(&s) {
		if site.Port == i.Port {
			return errors.New("database port is assigned to an enabled HTTPS site")
		}
	}
	if _, e := os.Lstat(filepath.Join(m.Root, "instances", i.ID)); e == nil {
		return errors.New("instance data already exists; choose another ID")
	} else if !errors.Is(e, os.ErrNotExist) {
		return e
	}
	return freeDatabasePort(i.Port)
}
func (m *Manager) createInstance(ctx context.Context, i Instance) (err error) {
	if e := validID(i.ID); e != nil {
		return e
	}
	if i.Kind != "postgresql" && i.Kind != "mysql" && i.Kind != "mongodb" && i.Kind != "redis" {
		return errors.New("supported engines: mysql, postgresql, redis, mongodb")
	}
	if i.Port < 1024 || i.Port > 65535 {
		return errors.New("database port must be 1024–65535")
	}
	if i.DataDirectory != "" {
		i.DataOwner = randomID()
	}
	keys := []resourceKey{{"database instance", i.ID}, {"TCP port", strconv.Itoa(i.Port)}}
	if i.DataDirectory != "" {
		keys = append(keys, resourceKey{"database locations", "creation"})
	}
	release, e := m.resourceLeases(ctx, keys...)
	if e != nil {
		return e
	}
	defer release()
	packageRelease, e := m.packageLease(ctx, i.Package, false)
	if e != nil {
		return e
	}
	defer packageRelease()
	s, e := m.Read()
	if e != nil {
		return e
	}
	if e = m.checkNewInstance(ctx, s, i); e != nil {
		return e
	}
	pkg, ok := s.Packages[i.Package]
	if !ok {
		return errors.New("database software is not installed")
	}
	if e = pkg.Validate(); e != nil {
		return e
	}
	id := randomID()
	dir, path, data, e := m.initializationPaths(id)
	if e != nil {
		return e
	}
	if e = os.MkdirAll(filepath.Dir(dir), 0700); e != nil {
		return e
	}
	if e = os.Mkdir(dir, 0700); e != nil {
		return e
	}
	owner, e := os.OpenFile(filepath.Join(dir, "owner.lock"), os.O_CREATE|os.O_EXCL|os.O_RDWR, 0600)
	if e != nil {
		return e
	}
	defer owner.Close()
	if e = syscall.Flock(int(owner.Fd()), syscall.LOCK_EX|syscall.LOCK_NB); e != nil {
		return e
	}
	defer syscall.Flock(int(owner.Fd()), syscall.LOCK_UN)
	if e = os.Mkdir(data, 0700); e != nil {
		return e
	}
	info, e := os.Lstat(data)
	if e != nil {
		return e
	}
	identity, e := deletionIdentity(info)
	if e != nil {
		return e
	}
	c := &initializationContext{record: initializationRecord{Schema: 1, ID: id, Instance: i, Identity: identity, Jobs: map[string]string{}, CreatedAt: time.Now()}, path: path, dir: data}
	if e = writeJSON(path, c.record); e != nil {
		return e
	}
	if e = syncDirectory(dir); e != nil {
		return e
	}
	if e = syncDirectory(filepath.Dir(dir)); e != nil {
		return e
	}
	worker := *m
	worker.initialization = c
	worker.operationArtifact(path)
	jobsStopped := false
	defer func() {
		if jobsStopped {
			return
		}
		cleanup, cancel := context.WithTimeout(context.Background(), 35*time.Second)
		defer cancel()
		err = errors.Join(err, worker.stopInitializationJobs(cleanup, c.record))
	}()
	if e = worker.prepareDataLocation(&i); e != nil {
		return e
	}
	c.record.Instance = i
	if e = writeJSON(path, c.record); e != nil {
		return e
	}
	worker.emit("Initializing database " + i.ID)
	if e = worker.initializeInstanceData(ctx, &s, i, data); e != nil {
		return e
	}
	cleanup, cancel := context.WithTimeout(context.Background(), 35*time.Second)
	e = worker.stopInitializationJobs(cleanup, c.record)
	cancel()
	if e != nil {
		return e
	}
	jobsStopped = true
	c.record.Ready = true
	if e = writeJSON(path, c.record); e != nil {
		return e
	}
	if e = syncDirectory(dir); e != nil {
		return e
	}
	worker.emit("Database initialization prepared")
	e = worker.withStateTransaction(ctx, func(latest *State) (func() error, func(), error) {
		if !reflect.DeepEqual(latest.Packages[i.Package], pkg) {
			return nil, nil, errors.New("database software changed during initialization")
		}
		if e := worker.checkNewInstance(ctx, *latest, i); e != nil {
			return nil, nil, e
		}
		root, e := os.OpenRoot(m.Root)
		if e != nil {
			return nil, nil, e
		}
		defer root.Close()
		from, to := filepath.Join("initializations", id, "data"), filepath.Join("instances", i.ID)
		if e = root.Rename(from, to); e != nil {
			return nil, nil, e
		}
		undo := func() error {
			base, e := os.OpenRoot(m.Root)
			if e != nil {
				return e
			}
			defer base.Close()
			if _, e = base.Lstat(from); !errors.Is(e, os.ErrNotExist) {
				return errors.New("initialization rollback path is occupied; data was retained")
			}
			return base.Rename(to, from)
		}
		if e = syncDirectory(dir); e != nil {
			return undo, nil, e
		}
		if e = syncDirectory(filepath.Join(m.Root, "instances")); e != nil {
			return undo, nil, e
		}
		worker.emit("Database initialization data published")
		latest.Instances[i.ID] = i
		return undo, func() { worker.emit("Database initialization committed") }, nil
	})
	if e != nil {
		return e
	}
	if e = syncDirectory(m.Root); e != nil {
		return e
	}
	if e = os.Remove(path); e != nil {
		return e
	}
	return syncDirectory(dir)
}
func initializationIdentity(path string) (string, error) {
	info, e := os.Lstat(path)
	if errors.Is(e, os.ErrNotExist) {
		return "", nil
	}
	if e != nil {
		return "", e
	}
	if !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
		return "", errors.New("initialization data path is not a plain directory")
	}
	return deletionIdentity(info)
}
func (m *Manager) initializationInfo(ctx context.Context, s State, r initializationRecord, b []byte, data string) (InstanceInitializationInfo, error) {
	info := InstanceInitializationInfo{ID: r.ID, InstanceID: r.Instance.ID, Kind: r.Instance.Kind, Package: r.Instance.Package, Port: r.Instance.Port, Path: data, Status: "Incomplete", CanResolve: true, Detail: "Stop any recorded initialization jobs and archive the prepared data. The instance can then be configured again."}
	stage, e := initializationIdentity(data)
	if e != nil {
		return info, e
	}
	canonical := filepath.Join(m.Root, "instances", r.Instance.ID)
	current, e := initializationIdentity(canonical)
	if e != nil {
		return info, e
	}
	archive := filepath.Join(m.Root, "retained-initializations", r.ID)
	retained, e := initializationIdentity(archive)
	if e != nil {
		return info, e
	}
	registered, exists := s.Instances[r.Instance.ID]
	if stage != "" && stage != r.Identity {
		return info, errors.New("initialization data identity changed")
	}
	if current == r.Identity {
		info.Path = canonical
		if exists {
			info.Status = "Published"
			info.Detail = "This data is registered. Finish clears only the initialization record; the database remains intact."
		} else {
			info.Status = "Uncommitted"
			info.Detail = "Data moved but registration did not commit. Reconcile moves it back and archives it, keeping all files."
		}
	}
	if stage == "" && current != r.Identity {
		if retained == r.Identity {
			info.Path = archive
			info.Status = "Archived"
			info.Detail = "The prepared data is already archived. Finish clears only the initialization record and keeps the archive."
		} else {
			info.CanResolve = false
			info.Status = "Needs inspection"
			info.Detail = "The recorded data directory is missing. No other data will be moved."
		}
	}
	if r.Instance.DataDirectory != "" {
		info.Detail += " External database files stay at " + m.DatabaseDataDirectory(r.Instance) + ". Only application metadata is archived; this location cannot be reused automatically."
	}
	fingerprint, _ := json.Marshal(struct {
		Record, Stage, Canonical, Retained string
		Registered                         bool
		Instance                           Instance
	}{digestBytes(b), stage, current, retained, exists, registered})
	info.Revision = digestBytes(fingerprint)
	return info, nil
}
func (m *Manager) PendingInstanceInitializations(ctx context.Context) ([]InstanceInitializationInfo, error) {
	if _, _, _, e := m.initializationPaths("inspect"); e != nil {
		return nil, e
	}
	dirs, e := os.ReadDir(filepath.Join(m.Root, "initializations"))
	if errors.Is(e, os.ErrNotExist) {
		return nil, nil
	}
	if e != nil {
		return nil, e
	}
	s, e := m.Read()
	if e != nil {
		return nil, e
	}
	var out []InstanceInitializationInfo
	for _, d := range dirs {
		if !d.IsDir() {
			continue
		}
		dir, path, data, e := m.initializationPaths(d.Name())
		if e != nil {
			return nil, e
		}
		if e = plainDeletionDirectory(dir); e != nil {
			return nil, e
		}
		r, b, e := readInitialization(path, d.Name())
		if errors.Is(e, os.ErrNotExist) {
			continue
		}
		if e != nil {
			return nil, e
		}
		info, e := m.initializationInfo(ctx, s, r, b, data)
		if e != nil {
			return nil, e
		}
		lease, e := openOperationLease(filepath.Join(dir, "owner.lock"))
		if e == nil {
			closeOperationLease(lease)
		} else if errors.Is(e, syscall.EWOULDBLOCK) {
			info.Busy = true
			info.CanResolve = false
			info.Status = "Initializing"
			info.Detail = "A caller still owns this initialization. Wait or cancel it in that caller."
		} else {
			return nil, e
		}
		out = append(out, info)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].ID < out[j].ID })
	return out, nil
}
func (m *Manager) ResolveInstanceInitialization(ctx context.Context, id, revision string) (string, error) {
	var archive string
	err := m.runOperation(ctx, OperationRequest{Kind: "reconcile-initialization", Resource: id}, "", func(w *Manager) error {
		dir, path, data, e := w.initializationPaths(id)
		if e != nil {
			return e
		}
		if e = plainDeletionDirectory(dir); e != nil {
			return e
		}
		owner, e := openOperationLease(filepath.Join(dir, "owner.lock"))
		if e != nil {
			return errors.New("initialization is still owned by another caller")
		}
		defer closeOperationLease(owner)
		r, b, e := readInitialization(path, id)
		if e != nil {
			return e
		}
		release, e := w.resourceLeases(ctx, resourceKey{"database instance", r.Instance.ID})
		if e != nil {
			return e
		}
		defer release()
		s, e := w.Read()
		if e != nil {
			return e
		}
		info, e := w.initializationInfo(ctx, s, r, b, data)
		if e != nil {
			return e
		}
		if info.Revision != revision || !info.CanResolve {
			return errors.New("initialization changed; refresh its status before reconciling")
		}
		if e = w.stopInitializationJobs(ctx, r); e != nil {
			return e
		}
		e = w.withStateTransaction(ctx, func(latest *State) (func() error, func(), error) {
			now, e := w.initializationInfo(ctx, *latest, r, b, data)
			if e != nil {
				return nil, nil, e
			}
			if now.Revision != revision {
				return nil, nil, errors.New("instance changed while stopping initialization jobs; refresh")
			}
			root, e := os.OpenRoot(w.Root)
			if e != nil {
				return nil, nil, e
			}
			defer root.Close()
			if now.Status == "Published" || now.Status == "Archived" {
				if now.Status == "Archived" {
					archive = now.Path
				}
				return nil, nil, nil
			}
			from := filepath.Join("initializations", id, "data")
			if now.Status == "Uncommitted" {
				if e = w.verifyShutdownTarget(ctx, domain()+"/"+w.label("db-"+r.Instance.ID)); e != nil {
					return nil, nil, e
				}
				from = filepath.Join("instances", r.Instance.ID)
			}
			if e = os.MkdirAll(filepath.Join(w.Root, "retained-initializations"), 0700); e != nil {
				return nil, nil, e
			}
			to := filepath.Join("retained-initializations", id)
			if _, e = root.Lstat(to); !errors.Is(e, os.ErrNotExist) {
				return nil, nil, errors.New("retained initialization path is occupied")
			}
			if e = root.Rename(from, to); e != nil {
				return nil, nil, e
			}
			archive = filepath.Join(w.Root, to)
			w.emit("Database initialization data archived")
			undo := func() error {
				base, e := os.OpenRoot(w.Root)
				if e != nil {
					return e
				}
				defer base.Close()
				return base.Rename(to, from)
			}
			if e = syncDirectory(filepath.Join(w.Root, "retained-initializations")); e != nil {
				return undo, nil, e
			}
			if e = syncDirectory(filepath.Join(w.Root, filepath.Dir(from))); e != nil {
				return undo, nil, e
			}
			return undo, func() {
				w.emit("Database initialization archive committed")
			}, nil
		})
		if e != nil {
			return e
		}
		if e = os.Remove(path); e != nil {
			return e
		}
		return syncDirectory(dir)
	})
	return archive, err
}

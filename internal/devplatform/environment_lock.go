package devplatform

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"reflect"
	"sort"
)

// EnvironmentLock contains declarations only. Credentials and database files
// stay in the local storage root; importing never starts or downloads anything.
type EnvironmentLock struct {
	Schema    int                 `json:"schema"`
	Packages  map[string]Package  `json:"packages"`
	Instances map[string]Instance `json:"instances,omitempty"`
}

func readEnvironmentLock(path string) (EnvironmentLock, error) {
	b, e := readExportFile(filepath.Dir(path), filepath.Base(path), 8<<20)
	if e != nil {
		return EnvironmentLock{}, e
	}
	return parseEnvironmentLock(b)
}

func parseEnvironmentLock(b []byte) (EnvironmentLock, error) {
	var e error
	var lock EnvironmentLock
	if e = json.Unmarshal(b, &lock); e != nil {
		return lock, e
	}
	// Read the initial package-only export format without executing it.
	if lock.Schema == 0 && lock.Packages == nil {
		if e = json.Unmarshal(b, &lock.Packages); e != nil {
			return lock, e
		}
		lock.Schema = 1
	}
	if lock.Schema != 1 || lock.Packages == nil {
		return lock, errors.New("unsupported environment lock schema")
	}
	for id, p := range lock.Packages {
		if e = p.Validate(); e != nil {
			return lock, e
		}
		if id != p.ID() {
			return lock, errors.New("environment lock package identity mismatch")
		}
	}
	for id, i := range lock.Instances {
		if e = validID(id); e != nil {
			return lock, e
		}
		if id != i.ID {
			return lock, errors.New("environment lock instance identity mismatch")
		}
		if p, ok := lock.Packages[i.Package]; !ok || p.Name != i.Kind {
			return lock, errors.New("environment lock instance package mismatch")
		}
	}
	return lock, nil
}

func (m *Manager) projectLockPath(id string) string {
	return filepath.Join(m.Root, "project-locks", id+".json")
}

func (m *Manager) ExportProject(ctx context.Context, id string) error {
	snapshot, err := m.Read()
	if err != nil {
		return err
	}
	project, ok := snapshot.Projects[id]
	if !ok {
		return errors.New("project not found")
	}
	dir, err := canonicalProjectDirectory(project.Directory)
	if err != nil {
		return err
	}
	unlock, err := lockEnvironmentFiles(ctx, dir)
	if err != nil {
		return err
	}
	defer unlock()
	return m.withState(ctx, func(s *State) error {
		p, ok := s.Projects[id]
		if !ok {
			return errors.New("project not found")
		}
		currentDir, err := canonicalProjectDirectory(p.Directory)
		if err != nil {
			return err
		}
		if currentDir != dir {
			return errors.New("project folder changed; retry export with the current location")
		}
		p = p.Clone()
		p.RuntimeHints = nil
		p.DetectionWarnings = nil
		p.Directory = "."
		p.Autostart = false
		lock := EnvironmentLock{Schema: 1, Packages: map[string]Package{}, Instances: map[string]Instance{}}
		addPackage := func(id string) error {
			if id == "" {
				return nil
			}
			pkg, ok := s.Packages[id]
			if !ok {
				return fmt.Errorf("install %s before exporting the environment", id)
			}
			lock.Packages[id] = pkg
			return nil
		}
		for n := range p.Services {
			service := &p.Services[n]
			if e := addPackage(service.Package); e != nil {
				return e
			}
		}
		for _, id := range p.Instances {
			i, ok := s.Instances[id]
			if !ok {
				return fmt.Errorf("instance %s is missing", id)
			}
			if e := addPackage(i.Package); e != nil {
				return e
			}
			lock.Instances[id] = i
		}
		shared, e := makeSharedProject(p, s, &lock)
		if e != nil {
			return e
		}
		return m.publishEnvironmentLocked(ctx, dir, shared, lock)
	})
}

// importEnvironmentLock snapshots the lock in private app storage. Instance IDs
// are scoped to this working copy, so two imports cannot silently share data.
func (m *Manager) importEnvironmentLock(p *Project) error {
	lock, e := readEnvironmentLock(filepath.Join(p.Directory, "devmanager.lock.json"))
	if errors.Is(e, os.ErrNotExist) {
		if len(p.Instances) > 0 {
			return errors.New("shared database declarations require an environment lock; export the source environment first")
		}
		return nil
	}
	if e != nil {
		return e
	}
	instances := map[string]Instance{}
	state, e := m.Read()
	if e != nil {
		return e
	}
	reserved := map[int]bool{}
	for _, i := range state.Instances {
		reserved[i.Port] = true
	}
	for n, id := range p.Instances {
		i, ok := lock.Instances[id]
		if !ok {
			return fmt.Errorf("instance %s is missing from the environment lock", id)
		}
		prefix := id
		if len(prefix) > 40 {
			prefix = prefix[:40]
		}
		i.ID = prefix + "-" + p.ID
		// Ports in a shared lock are suggestions, never a reason to attach to
		// an existing instance or stop another process on this machine.
		i.Port, e = freeInstancePort(i.Port, reserved)
		if e != nil {
			return e
		}
		reserved[i.Port] = true
		instances[i.ID] = i
		p.Instances[n] = i.ID
	}
	lock.Instances = instances
	for _, service := range p.Services {
		if service.Package != "" {
			if _, ok := lock.Packages[service.Package]; !ok {
				return fmt.Errorf("runtime %s is missing from the environment lock", service.Package)
			}
		}
	}
	return writeJSON(m.projectLockPath(p.ID), lock)
}

func freeInstancePort(preferred int, reserved map[int]bool) (int, error) {
	if preferred != 0 && (preferred < 1024 || preferred > 65535) {
		return 0, errors.New("locked database port must be 1024–65535")
	}
	if preferred != 0 && !reserved[preferred] {
		if l, e := net.Listen("tcp4", fmt.Sprintf("127.0.0.1:%d", preferred)); e == nil {
			l.Close()
			return preferred, nil
		}
	}
	for n := 0; n < 32; n++ {
		l, e := net.Listen("tcp4", "127.0.0.1:0")
		if e != nil {
			return 0, e
		}
		port := l.Addr().(*net.TCPAddr).Port
		l.Close()
		if !reserved[port] {
			return port, nil
		}
	}
	return 0, errors.New("could not select a free database port")
}

// PrepareEnvironment downloads only packages resolved by the built-in catalog.
// URLs and executable paths from a repository lock are never used to install.
// Preparation is explicit; project scripts and login jobs are never started.
func (m *Manager) PrepareEnvironment(ctx context.Context, id string) error {
	if e := validID(id); e != nil {
		return e
	}
	release, e := m.resourceLeases(ctx, projectResourceKeys(id)...)
	if e != nil {
		return e
	}
	defer release()
	state, e := m.Read()
	if e != nil {
		return e
	}
	p, ok := state.Projects[id]
	if !ok {
		return errors.New("project not found")
	}
	p = p.Clone()
	if m.projectActive(ctx, p) {
		return errors.New("stop the project before preparing its environment")
	}
	lock, e := readEnvironmentLock(m.projectLockPath(id))
	if e != nil {
		return fmt.Errorf("import an exported environment before preparing it: %w", e)
	}
	resolved := map[string]Package{}
	required := map[string]bool{}
	for _, service := range p.Services {
		if service.Package != "" {
			required[service.Package] = true
		}
	}
	for _, ref := range p.Instances {
		i, ok := lock.Instances[ref]
		if !ok {
			return fmt.Errorf("database %s was added locally; configure it in Databases", ref)
		}
		required[i.Package] = true
	}
	var keys []string
	for key := range required {
		if _, ok := lock.Packages[key]; !ok {
			return fmt.Errorf("package %s is missing from the environment lock", key)
		}
		keys = append(keys, key)
	}
	sort.Strings(keys)
	// Validate every package before starting the first installation.
	for _, key := range keys {
		locked := lock.Packages[key]
		pkg, e := m.ResolvePackage(ctx, locked.Name, locked.Version)
		if e != nil {
			return e
		}
		if pkg.Arch == locked.Arch && (pkg.SHA256 != locked.SHA256 || pkg.Formula != locked.Formula || pkg.Format != locked.Format) {
			return fmt.Errorf("catalog checksum differs from the lock for %s", key)
		}
		resolved[key] = pkg
	}
	for _, key := range keys {
		if e = m.Install(ctx, resolved[key]); e != nil {
			return e
		}
		pkg := resolved[key]
		lock.Packages[pkg.ID()] = pkg
	}
	for n := range p.Services {
		if pkg, ok := resolved[p.Services[n].Package]; ok {
			p.Services[n].Package = pkg.ID()
		}
	}
	for _, ref := range p.Instances {
		i, ok := lock.Instances[ref]
		if !ok {
			return fmt.Errorf("database %s was added locally; configure it in Databases", ref)
		}
		i.Package = resolved[i.Package].ID()
		lock.Instances[ref] = i
		current, e := m.Read()
		if e != nil {
			return e
		}
		if existing, ok := current.Instances[ref]; ok {
			if !reflect.DeepEqual(existing, i) {
				return fmt.Errorf("instance %s differs from its lock; existing data was kept", ref)
			}
			continue
		}
		m.emit("Initializing " + i.ID)
		if e = m.CreateInstance(ctx, i); e != nil {
			return e
		}
	}
	// Do not overwrite settings changed by another GUI/CLI during preparation.
	return m.withState(ctx, func(s *State) error {
		if !reflect.DeepEqual(s.Projects[id], state.Projects[id]) {
			return errors.New("project changed during preparation; retry after reviewing its settings")
		}
		if e := writeJSON(m.projectLockPath(id), lock); e != nil {
			return e
		}
		s.Projects[id] = p
		return nil
	})
}

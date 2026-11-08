package devplatform

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"time"

	"howett.net/plist"
)

func (m *Manager) requirePackageIdle(ctx context.Context, s *State, p Package) error {
	if e := m.requirePackageDatabaseWorkIdle(p.ID()); e != nil {
		return e
	}
	for _, instance := range s.Instances {
		if instance.Package == p.ID() {
			loaded, err := m.packageJobLoaded(ctx, m.label("db-"+instance.ID))
			if err != nil {
				return err
			}
			if loaded {
				return fmt.Errorf("stop database %s before repairing its software", instance.ID)
			}
		}
	}
	files, err := filepath.Glob(filepath.Join(m.Root, "jobs", "*.plist"))
	if err != nil {
		return err
	}
	for _, file := range files {
		f, err := os.Open(file)
		if err != nil {
			return err
		}
		b, readErr := io.ReadAll(io.LimitReader(contextReader{ctx, f}, (1<<20)+1))
		f.Close()
		if readErr != nil {
			return readErr
		}
		if len(b) > 1<<20 {
			return errors.New("launchd job is too large to inspect safely")
		}
		var job map[string]any
		if _, err = plist.Unmarshal(b, &job); err != nil {
			return err
		}
		if !packageValueReferences(job, m.packageDir(p)) {
			continue
		}
		label, _ := job["Label"].(string)
		if !strings.HasPrefix(label, m.label("")) {
			return errors.New("a job outside this workspace references the package; inspect it before repair")
		}
		loaded, err := m.packageJobLoaded(ctx, label)
		if err != nil {
			return err
		}
		if loaded {
			return errors.New("stop the launchd job using this package before repairing it")
		}
	}
	return ctx.Err()
}
func packageValueReferences(value any, path string) bool {
	switch v := value.(type) {
	case string:
		return strings.Contains(v, path)
	case []any:
		for _, item := range v {
			if packageValueReferences(item, path) {
				return true
			}
		}
	case map[string]any:
		for _, item := range v {
			if packageValueReferences(item, path) {
				return true
			}
		}
	}
	return false
}
func (m *Manager) packageJobLoaded(ctx context.Context, label string) (bool, error) {
	ctx, cancel := context.WithTimeout(ctx, time.Second)
	defer cancel()
	_, err := m.Run(ctx, "launchctl", "print", domain()+"/"+label)
	if ctx.Err() != nil {
		return false, ctx.Err()
	}
	if err == nil {
		return true, nil
	}
	if strings.Contains(err.Error(), "Could not find service") || strings.Contains(err.Error(), "Could not find specified service") {
		return false, nil
	}
	return false, fmt.Errorf("could not verify whether the package is in use: %w", err)
}

func (m *Manager) repairPackage(ctx context.Context, id string) error {
	if e := m.init(); e != nil {
		return e
	}
	unlock, e := m.packageLease(ctx, id, true)
	if e != nil {
		return e
	}
	defer unlock()
	state, e := m.Read()
	if e != nil {
		return e
	}
	p, ok := state.Packages[id]
	if !ok {
		return errors.New("package is not installed")
	}
	if p.ID() != id {
		return errors.New("installed package identity mismatch")
	}
	if e = p.Validate(); e != nil {
		return e
	}
	if p.Formula != "" {
		return fmt.Errorf("Homebrew owns this shared installation; stop its users and run brew reinstall %s, then check files again", p.Formula)
	}
	if e = m.requirePackageIdle(ctx, &state, p); e != nil {
		return e
	}
	m.emit("Preparing repair of " + id)
	return m.preparePackageForPublication(ctx, p, true)
}

// Download, extraction and indexing hold only this package's exclusive lease.
// Publication re-reads state and checks every live reference inside the short
// state transaction. A newly started user of the old version prevents repair.
func (m *Manager) preparePackageForPublication(ctx context.Context, p Package, repair bool) error {
	stage, e := m.stagePackage(ctx, p)
	if e != nil {
		return e
	}
	handedOff := false
	defer func() {
		if !handedOff {
			_ = os.RemoveAll(stage.Dir)
		}
	}()
	return m.withStateTransaction(ctx, func(s *State) (func() error, func(), error) {
		current, exists := s.Packages[p.ID()]
		if repair {
			if !exists || !reflect.DeepEqual(current, p) {
				return nil, nil, errors.New("installed package record changed while preparing repair; review it again")
			}
		} else if exists {
			return nil, nil, errors.New("package was registered while preparing installation; review it again")
		}
		if e := m.requirePackageIdle(ctx, s, p); e != nil {
			return nil, nil, e
		}
		// From this point publishPackage owns cleanup and rollback. In
		// particular, never erase an old tree retained after rollback failure.
		handedOff = true
		return m.publishPackage(ctx, s, p, stage, repair)
	})
}

// Only the package tree and its index participate. Database directories are
// outside this transaction. The old tree remains available until state commits.
func (m *Manager) publishPackage(ctx context.Context, s *State, p Package, stage *stagedPackage, repair bool) (undo func() error, commit func(), err error) {
	dest, index := m.packageDir(p), m.integrityPath(p.ID())
	oldIndex := filepath.Join(m.Root, "integrity", ".previous-"+randomID()+".json")
	swapped, renamed, indexMoved, indexWritten := false, false, false, false
	undo = func() error {
		var err error
		if swapped {
			err = exchangePackageDirectories(stage.Dir, dest)
		} else if renamed {
			err = os.Rename(dest, stage.Dir)
		}
		if err != nil {
			return fmt.Errorf("repair rollback needs inspection; old files retained at %s: %w", stage.Dir, err)
		}
		if indexWritten {
			if e := os.Remove(index); e != nil && !errors.Is(e, os.ErrNotExist) {
				return e
			}
		}
		if indexMoved {
			if e := os.Rename(oldIndex, index); e != nil {
				return e
			}
		}
		return os.RemoveAll(stage.Dir)
	}
	commit = func() {
		if e := os.RemoveAll(stage.Dir); e != nil {
			m.emit("Old package files retained for inspection: " + stage.Dir)
		}
		if indexMoved {
			_ = os.Remove(oldIndex)
		}
		verb := "Installed "
		if repair {
			verb = "Repaired "
		}
		m.emit(verb + p.ID())
	}
	if err = ctx.Err(); err != nil {
		return undo, nil, err
	}
	info, statErr := os.Lstat(dest)
	exists := statErr == nil
	if statErr != nil && !errors.Is(statErr, os.ErrNotExist) {
		return undo, nil, statErr
	}
	if exists && (!info.IsDir() || info.Mode()&os.ModeSymlink != 0) {
		return undo, nil, errors.New("installation path is not a plain directory; reveal it for inspection before repair")
	}
	if exists {
		if e := m.checkJavaOwnership(p); e != nil {
			return undo, nil, e
		}
	}
	if exists && !repair {
		var prior Package
		b, e := os.ReadFile(filepath.Join(dest, ".package.json"))
		if e != nil || json.Unmarshal(b, &prior) != nil || prior.ID() != p.ID() || prior.SHA256 != p.SHA256 {
			return undo, nil, errors.New("an unregistered package directory exists; inspect it before retrying")
		}
	}
	if info, e := os.Lstat(index); e == nil {
		if !info.Mode().IsRegular() && info.Mode()&os.ModeSymlink == 0 {
			return undo, nil, errors.New("integrity index path is not a file")
		}
		if err = os.Rename(index, oldIndex); err != nil {
			return undo, nil, err
		}
		indexMoved = true
	} else if !errors.Is(e, os.ErrNotExist) {
		return undo, nil, e
	}
	if exists {
		err = exchangePackageDirectories(stage.Dir, dest)
		swapped = err == nil
	} else {
		err = publishNewPackageDirectory(stage.Dir, dest)
		renamed = err == nil
	}
	if err != nil {
		return undo, nil, err
	}
	if err = atomicWrite(index, stage.Index, 0600); err != nil {
		return undo, nil, err
	}
	indexWritten = true
	if err = ctx.Err(); err != nil {
		return undo, nil, err
	}
	if s.PackageIntegrity == nil {
		s.PackageIntegrity = map[string]string{}
	}
	s.PackageIntegrity[p.ID()] = stage.Digest
	s.Packages[p.ID()] = p
	m.emit("Committing package files for " + p.ID())
	return undo, commit, nil
}

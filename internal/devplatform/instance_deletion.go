package devplatform

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"reflect"
	"syscall"
	"time"
)

type InstanceDeletionPreview struct {
	identity, treeDigest                                 string
	InstanceID, Kind, Package, Path, Fingerprint, Detail string
	Entries, Bytes                                       int64
	References                                           []string
	CanDelete                                            bool
}
type InstanceDeletionInfo struct {
	treeDigest                                                               string
	ID, InstanceID, Kind, OriginalPath, StagedPath, Status, Detail, Revision string
	Entries, Bytes                                                           int64
	CanErase, CanRestore, Busy                                               bool
}
type instanceDeletionRecord struct {
	Committed bool
	Schema    int
	ID        string
	Instance  Instance
	Identity  string
	CreatedAt time.Time
}
type deletionTree struct {
	Identity, Digest string
	Entries, Bytes   int64
	Paths            []string
}

func deletionIdentity(info os.FileInfo) (string, error) {
	stat, ok := info.Sys().(*syscall.Stat_t)
	if !ok {
		return "", errors.New("cannot identify the data directory")
	}
	return fmt.Sprintf("%d:%d", stat.Dev, stat.Ino), nil
}

// Inspect metadata inside an opened directory capability. Links are counted but
// never followed, and deletion later uses the same confinement mechanism.
func scanDeletionTree(ctx context.Context, path string) (deletionTree, error) {
	var tree deletionTree
	info, e := os.Lstat(path)
	if e != nil {
		return tree, e
	}
	if !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
		return tree, errors.New("database data must be a plain directory")
	}
	identity, e := deletionIdentity(info)
	if e != nil {
		return tree, e
	}
	root, e := os.OpenRoot(path)
	if e != nil {
		return tree, e
	}
	defer root.Close()
	opened, e := root.Lstat(".")
	if e != nil {
		return tree, e
	}
	tree.Identity, e = deletionIdentity(opened)
	if e != nil || tree.Identity != identity {
		return tree, errors.New("database data directory changed during inspection")
	}
	device := opened.Sys().(*syscall.Stat_t).Dev
	hash := sha256.New()
	encoder := json.NewEncoder(hash)
	e = fs.WalkDir(root.FS(), ".", func(rel string, d fs.DirEntry, walkErr error) error {
		if walkErr != nil {
			return walkErr
		}
		if e := ctx.Err(); e != nil {
			return e
		}
		info, e := root.Lstat(rel)
		if e != nil {
			return e
		}
		stat, ok := info.Sys().(*syscall.Stat_t)
		if !ok || stat.Dev != device || stat.Uid != uint32(os.Getuid()) {
			return errors.New("data contains a foreign owner or mounted filesystem; inspect it before deletion")
		}
		if !info.IsDir() && !info.Mode().IsRegular() && info.Mode()&os.ModeSymlink == 0 {
			return errors.New("data contains a special file; inspect it before deletion")
		}
		link := ""
		if info.Mode()&os.ModeSymlink != 0 {
			link, e = root.Readlink(rel)
			if e != nil {
				return e
			}
		}
		if e = encoder.Encode(struct {
			Path, Link     string
			Mode           uint32
			Size, Modified int64
			Inode          uint64
		}{rel, link, uint32(info.Mode()), info.Size(), info.ModTime().UnixNano(), stat.Ino}); e != nil {
			return e
		}
		tree.Entries++
		if info.Mode().IsRegular() {
			tree.Bytes += info.Size()
		}
		if tree.Entries > 1000000 {
			return errors.New("data tree exceeds the safe preview limit")
		}
		tree.Paths = append(tree.Paths, rel)
		return nil
	})
	tree.Digest = hex.EncodeToString(hash.Sum(nil))
	return tree, e
}
func plainDeletionDirectory(path string) error {
	info, e := os.Lstat(path)
	if e != nil {
		return e
	}
	if !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
		return errors.New("deletion storage is not a plain directory")
	}
	return nil
}
func (m *Manager) validateDeletionStore() error {
	if !filepath.IsAbs(m.Root) {
		return errors.New("storage root must be absolute")
	}
	for _, name := range []string{"instances", "deletions", "jobs"} {
		e := plainDeletionDirectory(filepath.Join(m.Root, name))
		if e != nil && !errors.Is(e, os.ErrNotExist) {
			return e
		}
	}
	return nil
}
func (m *Manager) instanceDeletionPreview(ctx context.Context, s State, id string) (InstanceDeletionPreview, error) {
	var p InstanceDeletionPreview
	if e := validID(id); e != nil {
		return p, e
	}
	if e := m.validateDeletionStore(); e != nil {
		return p, e
	}
	i, ok := s.Instances[id]
	if !ok {
		return p, errors.New("database instance not found")
	}
	p = InstanceDeletionPreview{InstanceID: id, Kind: i.Kind, Package: i.Package, Path: filepath.Join(m.Root, "instances", id), CanDelete: true, Detail: "Permanently removes this instance record, data, local configuration and credentials. Installed software and backups outside this path are retained."}
	loaded, e := m.packageJobLoaded(ctx, m.label("db-"+id))
	if e != nil {
		p.CanDelete = false
		p.Detail = "Cannot confirm that the database job is stopped. Refresh after checking its status."
	} else if loaded {
		p.CanDelete = false
		p.Detail = "Stop this instance before deleting its data."
	}
	tree, e := scanDeletionTree(ctx, p.Path)
	if e != nil {
		return p, e
	}
	p.Entries = tree.Entries
	p.Bytes = tree.Bytes
	b, _ := json.Marshal(struct {
		Root     string
		Instance Instance
		Preview  InstanceDeletionPreview
		Tree     string
	}{filepath.Clean(m.Root), i, p, tree.Digest + ":" + tree.Identity})
	p.Fingerprint = digestBytes(b)
	p.identity = tree.Identity
	p.treeDigest = tree.Digest
	return p, nil
}
func (m *Manager) PreviewInstanceDeletion(ctx context.Context, id string) (InstanceDeletionPreview, error) {
	s, e := m.Read()
	if e != nil {
		return InstanceDeletionPreview{}, e
	}
	return m.instanceDeletionPreview(ctx, s, id)
}
func (m *Manager) deletionPaths(id string) (dir, record, staged string, e error) {
	if e = validID(id); e != nil {
		return
	}
	if e = m.validateDeletionStore(); e != nil {
		return
	}
	dir = filepath.Join(m.Root, "deletions", id)
	record = filepath.Join(dir, "record.json")
	staged = filepath.Join(dir, "data")
	return
}
func readInstanceDeletion(path, id string) (instanceDeletionRecord, []byte, error) {
	var r instanceDeletionRecord
	b, e := readExportFile(filepath.Dir(path), filepath.Base(path), 65536)
	if e != nil {
		return r, nil, e
	}
	if len(b) > 65536 {
		return r, nil, errors.New("deletion record is too large")
	}
	if e = json.Unmarshal(b, &r); e != nil {
		return r, nil, e
	}
	if r.Schema != 1 || r.ID != id || validID(r.Instance.ID) != nil || r.Identity == "" {
		return r, nil, errors.New("deletion record identity mismatch")
	}
	return r, b, nil
}
func (m *Manager) withDeletionLease(ctx context.Context, id string, create bool, fn func(string, string) error) error {
	if e := ctx.Err(); e != nil {
		return e
	}
	dir, record, staged, e := m.deletionPaths(id)
	if e != nil {
		return e
	}
	if create {
		if e = os.MkdirAll(filepath.Dir(dir), 0700); e != nil {
			return e
		}
		if e = os.Mkdir(dir, 0700); e != nil {
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
		return fn(record, staged)
	}
	if e = plainDeletionDirectory(dir); e != nil {
		return e
	}
	f, e := openOperationLease(filepath.Join(dir, "owner.lock"))
	if errors.Is(e, syscall.EWOULDBLOCK) {
		return errors.New("database deletion is still owned by another caller")
	}
	if e != nil {
		return e
	}
	defer closeOperationLease(f)
	return fn(record, staged)
}
func (m *Manager) deletionInfo(ctx context.Context, s State, recordPath, staged string, r instanceDeletionRecord, b []byte) (InstanceDeletionInfo, error) {
	info := InstanceDeletionInfo{ID: r.ID, InstanceID: r.Instance.ID, Kind: r.Instance.Kind, OriginalPath: filepath.Join(m.Root, "instances", r.Instance.ID), StagedPath: staged}
	original, e := os.Lstat(info.OriginalPath)
	if e != nil && !errors.Is(e, os.ErrNotExist) {
		return info, e
	}
	originalExists := e == nil
	current, registered := s.Instances[r.Instance.ID]
	tree, e := scanDeletionTree(ctx, staged)
	if errors.Is(e, os.ErrNotExist) {
		if originalExists && registered && reflect.DeepEqual(current, r.Instance) {
			identity, err := deletionIdentity(original)
			if err != nil || identity != r.Identity {
				return info, errors.New("original data identity changed")
			}
			info.CanRestore = true
			info.Status = "Not moved"
			info.Detail = "Deletion did not commit. The registered database and original data are retained."
		} else if !registered {
			info.Status = "Finishing"
			info.CanErase = true
			info.Detail = "Staged data is gone. Finish to clear this deletion record."
		} else {
			return info, errors.New("data is missing but its instance is still registered; restore from backup before further changes")
		}
	} else if e != nil {
		return info, e
	} else {
		if tree.Identity != r.Identity {
			return info, errors.New("staged data identity differs from the confirmed directory")
		}
		info.Entries = tree.Entries
		info.Bytes = tree.Bytes
		if registered && !r.Committed {
			if originalExists || !reflect.DeepEqual(current, r.Instance) {
				info.Status = "Restore blocked"
				info.Detail = "Instance removal did not commit, but the original path or instance changed. Keep the staged data; resolve the original path before restoring."
			} else {
				loaded, err := m.packageJobLoaded(ctx, m.label("db-"+r.Instance.ID))
				if err != nil || loaded {
					info.Status = "Stop required"
					info.Detail = "Confirm the original database job is stopped before restoring its data folder."
				} else {
					info.Status = "Restore required"
					info.CanRestore = true
					info.Detail = "Data was moved but instance removal did not commit. Restore the data folder; do not erase it."
				}
			}
		} else if !registered || originalExists {
			if originalExists {
				identity, err := deletionIdentity(original)
				if err == nil && identity == r.Identity {
					return info, errors.New("original and staged paths unexpectedly refer to the same data")
				}
			}
			info.Status = "Ready to erase"
			info.CanErase = true
			info.Detail = "The original instance removal committed. Finish permanently erases only this staged data. Other instances and backups are retained."
		} else {
			info.Status = "Restore blocked"
			info.Detail = "A registered instance has no original data path. Keep the staged data and inspect the current instance before continuing."
		}

	}
	fingerprint, _ := json.Marshal(struct {
		Record     string
		Info       InstanceDeletionInfo
		Tree       string
		Registered bool
		Current    Instance
	}{digestBytes(b), info, tree.Digest, registered, current})
	info.Revision = digestBytes(fingerprint)
	info.treeDigest = tree.Digest
	return info, nil
}
func (m *Manager) PendingInstanceDeletions(ctx context.Context) ([]InstanceDeletionInfo, error) {
	if e := m.validateDeletionStore(); e != nil {
		return nil, e
	}
	dirs, e := os.ReadDir(filepath.Join(m.Root, "deletions"))
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
	var out []InstanceDeletionInfo
	for _, d := range dirs {
		if !d.IsDir() {
			continue
		}
		dir, path, staged, e := m.deletionPaths(d.Name())
		if e != nil {
			return nil, e
		}
		if e = plainDeletionDirectory(dir); e != nil {
			return nil, e
		}
		r, b, e := readInstanceDeletion(path, d.Name())
		if errors.Is(e, os.ErrNotExist) {
			continue
		}
		if e != nil {
			return nil, e
		}
		info, e := m.deletionInfo(ctx, s, path, staged, r, b)
		if e != nil {
			return nil, e
		}
		f, e := openOperationLease(filepath.Join(dir, "owner.lock"))
		if e == nil {
			closeOperationLease(f)
		} else if errors.Is(e, syscall.EWOULDBLOCK) {
			info.Busy = true
			info.CanErase = false
			info.CanRestore = false
		} else {
			return nil, e
		}
		out = append(out, info)
	}
	return out, nil
}
func (m *Manager) DeleteInstanceData(ctx context.Context, id, fingerprint, confirmation string) error {
	if confirmation != id {
		return errors.New("type the exact instance ID to confirm permanent data deletion")
	}
	return m.runOperation(ctx, OperationRequest{Kind: "delete-instance", Resource: id}, "", func(w *Manager) error {
		release, e := w.resourceLeases(ctx, instanceResourceKeys([]string{id})...)
		if e != nil {
			return e
		}
		defer release()
		deletionID := randomID()
		return w.withDeletionLease(ctx, deletionID, true, func(record, staged string) error {
			approvedDigest := ""
			var commitErr error
			e := w.withStateTransaction(ctx, func(s *State) (func() error, func(), error) {
				preview, e := w.instanceDeletionPreview(ctx, *s, id)
				if e != nil {
					return nil, nil, e
				}
				if preview.Fingerprint != fingerprint {
					return nil, nil, errors.New("database or data changed; refresh the deletion preview")
				}
				if !preview.CanDelete {
					return nil, nil, errors.New(preview.Detail)
				}
				tree, e := scanDeletionTree(ctx, preview.Path)
				if e != nil {
					return nil, nil, e
				}
				if tree.Identity != preview.identity || tree.Digest != preview.treeDigest {
					return nil, nil, errors.New("database data changed after review; refresh before deleting")
				}
				approvedDigest = tree.Digest
				r := instanceDeletionRecord{Schema: 1, ID: deletionID, Instance: s.Instances[id], Identity: tree.Identity, CreatedAt: time.Now()}
				if e = writeJSON(record, r); e != nil {
					return nil, nil, e
				}
				if e = syncExportDirectory(filepath.Dir(record)); e != nil {
					return nil, nil, e
				}
				if e = syncExportDirectory(filepath.Dir(filepath.Dir(record))); e != nil {
					return nil, nil, e
				}
				w.operationArtifact(record)
				w.operation.phase("Staging data")
				root, e := os.OpenRoot(w.Root)
				if e != nil {
					return nil, nil, e
				}
				defer root.Close()
				from, to := filepath.Join("instances", id), filepath.Join("deletions", deletionID, "data")
				if e = root.Rename(from, to); e != nil {
					return nil, nil, e
				}
				undo := func() error {
					base, e := os.OpenRoot(w.Root)
					if e != nil {
						return e
					}
					defer base.Close()
					if _, e = base.Lstat(from); !errors.Is(e, os.ErrNotExist) {
						return errors.New("cannot restore deletion data over an existing path")
					}
					return base.Rename(to, from)
				}
				if e = syncExportDirectory(filepath.Join(w.Root, "instances")); e != nil {
					return undo, nil, e
				}
				if e = syncExportDirectory(filepath.Dir(staged)); e != nil {
					return undo, nil, e
				}
				w.emit("Database data staged for deletion")
				delete(s.Instances, id)
				removeInstanceProfiles(s, id)
				return undo, func() {
					commitErr = syncExportDirectory(w.Root)
					if commitErr == nil {
						r.Committed = true
						commitErr = writeJSON(record, r)
					}
					if commitErr == nil {
						commitErr = syncExportDirectory(filepath.Dir(record))
					}
					if commitErr == nil {
						w.emit("Database deletion committed")
					}
				}, nil
			})
			if e != nil {
				return e
			}
			if commitErr != nil {
				return commitErr
			}
			return w.eraseInstanceDeletion(ctx, record, staged, "", approvedDigest)
		})
	})
}
func (m *Manager) eraseInstanceDeletion(ctx context.Context, record, staged, revision, approvedDigest string) error {
	r, b, e := readInstanceDeletion(record, filepath.Base(filepath.Dir(record)))
	if e != nil {
		return e
	}
	s, e := m.Read()
	if e != nil {
		return e
	}
	info, e := m.deletionInfo(ctx, s, record, staged, r, b)
	if e != nil {
		return e
	}
	if revision != "" && info.Revision != revision {
		return errors.New("deletion changed before erasure; refresh its status")
	}
	if approvedDigest != "" && info.treeDigest != approvedDigest {
		return errors.New("staged data changed; review the pending deletion before erasing")
	}
	if !info.CanErase {
		return errors.New("deletion has not committed; restore its data folder first")
	}
	if e = ctx.Err(); e != nil {
		return e
	}
	if e = m.withState(ctx, func(s *State) error {
		if _, exists := s.Instances[r.Instance.ID]; exists {
			if !r.Committed {
				return errors.New("instance removal is not committed; keep the staged data")
			}
			return nil
		}
		loaded, err := m.packageJobLoaded(ctx, m.label("db-"+r.Instance.ID))
		if err != nil {
			return err
		}
		if loaded {
			return errors.New("a database job is still registered; stop it before erasing staged data")
		}
		if !r.Committed {
			r.Committed = true
			if err = writeJSON(record, r); err != nil {
				return err
			}
			if err = syncExportDirectory(filepath.Dir(record)); err != nil {
				return err
			}
		}
		err = os.Remove(filepath.Join(m.Root, "jobs", m.label("db-"+r.Instance.ID)+".plist"))
		if errors.Is(err, os.ErrNotExist) {
			return nil
		}
		return err
	}); e != nil {
		return e
	}
	if e = syncExportDirectory(m.Root); e != nil {
		return e
	}
	m.operation.phase("Erasing data")
	tree, e := scanDeletionTree(ctx, staged)
	if !errors.Is(e, os.ErrNotExist) {
		if e != nil {
			return e
		}
		if tree.Identity != r.Identity || tree.Digest != info.treeDigest {
			return errors.New("staged directory changed; refresh before erasing")
		}
		root, e := os.OpenRoot(staged)
		if e != nil {
			return e
		}
		defer root.Close()
		opened, e := root.Lstat(".")
		if e != nil {
			return e
		}
		identity, _ := deletionIdentity(opened)
		if identity != r.Identity {
			return errors.New("staged directory changed before erasure")
		}
		lastProgress := time.Time{}
		for n := len(tree.Paths) - 1; n >= 0; n-- {
			if e = ctx.Err(); e != nil {
				return e
			}
			if tree.Paths[n] == "." {
				continue
			}
			if e = root.Remove(tree.Paths[n]); e != nil {
				return e
			}
			if lastProgress.IsZero() || time.Since(lastProgress) >= 250*time.Millisecond {
				m.emit("Deleting staged database data")
				lastProgress = time.Now()
			}
		}
		parent, e := os.OpenRoot(filepath.Dir(staged))
		if e != nil {
			return e
		}
		defer parent.Close()
		last, e := parent.Lstat("data")
		if e != nil {
			return e
		}
		identity, _ = deletionIdentity(last)
		if identity != r.Identity {
			return errors.New("staged directory changed during erasure")
		}
		if e = parent.Remove("data"); e != nil {
			return e
		}
	}
	if e = os.Remove(record); e != nil {
		return e
	}
	return syncExportDirectory(filepath.Dir(record))
}
func (m *Manager) FinishInstanceDeletion(ctx context.Context, id, revision, confirmation string) error {
	return m.runOperation(ctx, OperationRequest{Kind: "finish-deletion", Resource: id}, "", func(w *Manager) error {
		return w.withDeletionLease(ctx, id, false, func(record, staged string) error {
			r, b, e := readInstanceDeletion(record, id)
			if e != nil {
				return e
			}
			if confirmation != r.Instance.ID {
				return errors.New("type the original instance ID to confirm permanent erasure")
			}
			s, e := w.Read()
			if e != nil {
				return e
			}
			info, e := w.deletionInfo(ctx, s, record, staged, r, b)
			if e != nil {
				return e
			}
			if info.Revision != revision {
				return errors.New("deletion changed; refresh before continuing")
			}
			return w.eraseInstanceDeletion(ctx, record, staged, revision, "")
		})
	})
}
func (m *Manager) RestoreInstanceDeletion(ctx context.Context, id, revision string) error {
	return m.runOperation(ctx, OperationRequest{Kind: "restore-deletion", Resource: id}, "", func(w *Manager) error {
		return w.withDeletionLease(ctx, id, false, func(record, staged string) error {
			r, _, e := readInstanceDeletion(record, id)
			if e != nil {
				return e
			}
			release, e := w.resourceLeases(ctx, instanceResourceKeys([]string{r.Instance.ID})...)
			if e != nil {
				return e
			}
			defer release()
			return w.withStateTransaction(ctx, func(s *State) (func() error, func(), error) {
				r, b, e := readInstanceDeletion(record, id)
				if e != nil {
					return nil, nil, e
				}
				info, e := w.deletionInfo(ctx, *s, record, staged, r, b)
				if e != nil {
					return nil, nil, e
				}
				if info.Revision != revision || !info.CanRestore {
					return nil, nil, errors.New("deletion changed or cannot be restored; refresh its status")
				}
				if info.Status == "Not moved" {
					return nil, func() { _ = os.Remove(record); _ = syncExportDirectory(filepath.Dir(record)) }, nil
				}
				root, e := os.OpenRoot(w.Root)
				if e != nil {
					return nil, nil, e
				}
				defer root.Close()
				from, to := filepath.Join("deletions", id, "data"), filepath.Join("instances", r.Instance.ID)
				if _, e = root.Lstat(to); !errors.Is(e, os.ErrNotExist) {
					return nil, nil, errors.New("original instance path is occupied")
				}
				if e = root.Rename(from, to); e != nil {
					return nil, nil, e
				}
				undo := func() error {
					base, e := os.OpenRoot(w.Root)
					if e != nil {
						return e
					}
					defer base.Close()
					return base.Rename(to, from)
				}
				return undo, func() { _ = os.Remove(record); _ = syncExportDirectory(filepath.Dir(record)) }, nil
			})
		})
	})
}

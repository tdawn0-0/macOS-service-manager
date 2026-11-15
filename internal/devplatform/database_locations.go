package devplatform

import (
	"errors"
	"os"
	"path/filepath"
	"strings"
)

const dataOwnerFile = ".service-manager-owner"

func (m *Manager) DatabaseDataDirectory(i Instance) string {
	if i.DataDirectory != "" {
		return filepath.Join(i.DataDirectory, "data")
	}
	dir := m.instanceDirectory(i.ID)
	if i.Kind == "redis" {
		return filepath.Join(m.Root, "instances", i.ID)
	}
	return filepath.Join(dir, "data")
}
func pathWithin(path, root string) bool {
	return path == root || strings.HasPrefix(path, root+string(filepath.Separator))
}
func plainPath(path string) error {
	for p := path; ; p = filepath.Dir(p) {
		f, e := os.Lstat(p)
		if e != nil {
			if errors.Is(e, os.ErrNotExist) && p == path {
				continue
			}
			return e
		}
		if f.Mode()&os.ModeSymlink != 0 || !f.IsDir() {
			return errors.New("data location must contain only real directories")
		}
		if filepath.Dir(p) == p {
			break
		}
	}
	return nil
}
func (m *Manager) validateDataLocation(s State, i Instance) error {
	if i.DataDirectory == "" {
		return nil
	}
	path := i.DataDirectory
	home := m.Home
	if home == "" {
		home, _ = os.UserHomeDir()
	}
	appRoot := m.Root
	if p, e := filepath.EvalSymlinks(appRoot); e == nil {
		appRoot = p
	}
	if p, e := filepath.EvalSymlinks(home); e == nil {
		home = p
	}
	if !filepath.IsAbs(path) || filepath.Clean(path) != path || path == "/" || path == home || pathWithin(path, appRoot) || pathWithin(appRoot, path) {
		return errors.New("choose a new absolute data location outside application storage")
	}
	if e := plainPath(path); e != nil {
		return e
	}
	for _, other := range s.Instances {
		if other.ID != i.ID && other.DataDirectory != "" && (pathWithin(path, other.DataDirectory) || pathWithin(other.DataDirectory, path)) {
			return errors.New("data location overlaps another instance")
		}
	}
	for parent := filepath.Dir(path); parent != "/"; parent = filepath.Dir(parent) {
		if _, e := os.Lstat(filepath.Join(parent, dataOwnerFile)); e == nil {
			return errors.New("data location is inside another managed location")
		}
	}
	if _, e := os.Lstat(path); errors.Is(e, os.ErrNotExist) {
		return nil
	} else if e != nil {
		return e
	}
	return m.verifyDataLocation(i)
}
func (m *Manager) verifyDataLocation(i Instance) error {
	if i.DataDirectory == "" {
		return nil
	}
	if i.DataOwner == "" {
		return errors.New("existing data location cannot be adopted")
	}
	if e := plainPath(i.DataDirectory); e != nil {
		return e
	}
	info, e := os.Lstat(i.DataDirectory)
	if e != nil {
		return e
	}
	identity, e := deletionIdentity(info)
	if e != nil {
		return e
	}
	if identity != i.DataIdentity {
		return errors.New("data location identity changed")
	}
	if f, e := os.Lstat(filepath.Join(i.DataDirectory, "data")); e == nil && (!f.IsDir() || f.Mode()&os.ModeSymlink != 0) {
		return errors.New("database data path changed")
	} else if e != nil && !errors.Is(e, os.ErrNotExist) {
		return e
	}
	root, e := os.OpenRoot(i.DataDirectory)
	if e != nil {
		return e
	}
	defer root.Close()
	f, e := root.Lstat(dataOwnerFile)
	if e != nil {
		return e
	}
	if !f.Mode().IsRegular() {
		return errors.New("data ownership marker changed")
	}
	b, e := root.ReadFile(dataOwnerFile)
	if e != nil {
		return e
	}
	if string(b) != m.Root+"\n"+i.ID+"\n"+i.DataOwner+"\n" {
		return errors.New("data location ownership changed")
	}
	return nil
}
func (m *Manager) prepareDataLocation(i *Instance) error {
	if i.DataDirectory == "" {
		return nil
	}
	if e := plainPath(i.DataDirectory); e != nil {
		return e
	}
	if e := os.Mkdir(i.DataDirectory, 0700); e != nil {
		return e
	}
	info, e := os.Lstat(i.DataDirectory)
	if e != nil {
		return e
	}
	i.DataIdentity, e = deletionIdentity(info)
	if e != nil {
		return e
	}
	root, e := os.OpenRoot(i.DataDirectory)
	if e != nil {
		return e
	}
	defer root.Close()
	f, e := root.OpenFile(dataOwnerFile, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
	if e != nil {
		return e
	}
	_, e = f.WriteString(m.Root + "\n" + i.ID + "\n" + i.DataOwner + "\n")
	if e == nil {
		e = f.Sync()
	}
	closeErr := f.Close()
	if e != nil {
		return e
	}
	if closeErr != nil {
		return closeErr
	}
	return syncExportDirectory(i.DataDirectory)
}

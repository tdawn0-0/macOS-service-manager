package devplatform

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"time"
)

// BackupFile is one finished backup in the app's backups folder.
type BackupFile struct {
	Name     string    // file name inside the backups folder
	Path     string    // absolute path
	Database string    // database the backup was taken from
	Size     int64
	Modified time.Time
}

var backupNameRE = regexp.MustCompile(`^.+-[0-9a-f]{16}\.dump$`)

// Backups lists the finished backups of an instance, newest first.
func (m *Manager) Backups(ctx context.Context, id string) ([]BackupFile, error) {
	if e := validID(id); e != nil {
		return nil, e
	}
	s, e := m.Read()
	if e != nil {
		return nil, e
	}
	if _, ok := s.Instances[id]; !ok {
		return nil, errors.New("database instance not found")
	}
	dir := filepath.Join(m.Root, "backups")
	entries, e := os.ReadDir(dir)
	if errors.Is(e, os.ErrNotExist) {
		return nil, nil
	}
	if e != nil {
		return nil, e
	}
	var out []BackupFile
	for _, entry := range entries {
		if e := ctx.Err(); e != nil {
			return nil, e
		}
		name := entry.Name()
		if !backupNameRE.MatchString(name) || !strings.HasPrefix(name, id+"-") {
			continue
		}
		database := strings.TrimSuffix(strings.TrimPrefix(name, id+"-"), name[len(name)-len("-0123456789abcdef.dump"):])
		if database == "" {
			continue
		}
		longer := false
		for other := range s.Instances {
			if other != id && strings.HasPrefix(other, id+"-") && strings.HasPrefix(name, other+"-") {
				longer = true
				break
			}
		}
		if longer {
			continue
		}
		info, e := entry.Info()
		if e != nil || !info.Mode().IsRegular() {
			continue
		}
		out = append(out, BackupFile{Name: name, Path: filepath.Join(dir, name), Database: database, Size: info.Size(), Modified: info.ModTime()})
	}
	sort.Slice(out, func(i, j int) bool {
		if !out[i].Modified.Equal(out[j].Modified) {
			return out[i].Modified.After(out[j].Modified)
		}
		return out[i].Name < out[j].Name
	})
	return out, nil
}

// DeleteBackup permanently removes one finished backup of an instance.
func (m *Manager) DeleteBackup(ctx context.Context, id, name string) error {
	if e := validID(id); e != nil {
		return e
	}
	if name == "" || name != filepath.Base(name) || strings.ContainsAny(name, "/\x00") {
		return errors.New("invalid backup name")
	}
	release, e := m.resourceLeases(ctx, instanceResourceKeys([]string{id})...)
	if e != nil {
		return e
	}
	defer release()
	list, e := m.Backups(ctx, id)
	if e != nil {
		return e
	}
	for _, b := range list {
		if b.Name != name {
			continue
		}
		info, e := os.Lstat(b.Path)
		if e != nil {
			return e
		}
		if !info.Mode().IsRegular() {
			return errors.New("backup is not a regular file")
		}
		if e := os.Remove(b.Path); e != nil {
			return e
		}
		return syncExportDirectory(filepath.Dir(b.Path))
	}
	return errors.New("backup not found for this instance")
}

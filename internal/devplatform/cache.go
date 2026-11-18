package devplatform

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"syscall"
)

// DownloadCacheInfo summarizes cached installation archives in the download directory.
type DownloadCacheInfo struct {
	Directory string
	Files     int
	Bytes     int64
}

// DownloadCacheInfo reports the number and total size of cached archive files.
func (m *Manager) DownloadCacheInfo() (DownloadCacheInfo, error) {
	dir := m.downloadDirectory()
	info := DownloadCacheInfo{Directory: dir}
	entries, err := os.ReadDir(dir)
	if errors.Is(err, os.ErrNotExist) {
		return info, nil
	}
	if err != nil {
		return info, err
	}
	for _, entry := range entries {
		if entry.IsDir() || entry.Type()&os.ModeSymlink != 0 {
			continue
		}
		stat, err := os.Lstat(filepath.Join(dir, entry.Name()))
		if err != nil || !stat.Mode().IsRegular() {
			continue
		}
		info.Files++
		info.Bytes += stat.Size()
	}
	return info, nil
}

// ClearDownloadCache deletes cached archive files while installed packages and
// databases remain untouched. It refuses while an active package operation holds a lease.
func (m *Manager) ClearDownloadCache(ctx context.Context) (DownloadCacheInfo, error) {
	if err := ctx.Err(); err != nil {
		return DownloadCacheInfo{}, err
	}
	if err := m.ensureNoActivePackageLock(); err != nil {
		return DownloadCacheInfo{}, err
	}
	dir := m.downloadDirectory()
	removed := DownloadCacheInfo{Directory: dir}
	entries, err := os.ReadDir(dir)
	if errors.Is(err, os.ErrNotExist) {
		return removed, nil
	}
	if err != nil {
		return removed, err
	}
	root, err := os.OpenRoot(dir)
	if errors.Is(err, os.ErrNotExist) {
		return removed, nil
	}
	if err != nil {
		return removed, err
	}
	defer root.Close()
	for _, entry := range entries {
		if err := ctx.Err(); err != nil {
			return removed, err
		}
		name := entry.Name()
		stat, err := root.Lstat(name)
		if err != nil || !stat.Mode().IsRegular() {
			continue
		}
		if err := root.Remove(name); err != nil && !errors.Is(err, os.ErrNotExist) {
			return removed, err
		}
		removed.Files++
		removed.Bytes += stat.Size()
	}
	if err := syncDirectory(dir); err != nil {
		return removed, err
	}
	return removed, nil
}

func (m *Manager) ensureNoActivePackageLock() error {
	lockDir := filepath.Join(m.Root, "package-locks")
	entries, err := os.ReadDir(lockDir)
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	if err != nil {
		return err
	}
	for _, entry := range entries {
		name := entry.Name()
		if entry.IsDir() || !safeRelative(name) || filepath.Base(name) != name {
			continue
		}
		path := filepath.Join(lockDir, name)
		fd, err := syscall.Open(path, syscall.O_RDONLY|syscall.O_NOFOLLOW|syscall.O_NONBLOCK, 0)
		if err != nil {
			if errors.Is(err, os.ErrNotExist) {
				continue
			}
			return err
		}
		f := os.NewFile(uintptr(fd), name)
		stat, statErr := f.Stat()
		if statErr != nil || !stat.Mode().IsRegular() {
			f.Close()
			continue
		}
		err = syscall.Flock(int(f.Fd()), syscall.LOCK_SH|syscall.LOCK_NB)
		if err == nil {
			_ = syscall.Flock(int(f.Fd()), syscall.LOCK_UN)
			f.Close()
			continue
		}
		f.Close()
		if errors.Is(err, syscall.EWOULDBLOCK) {
			return &PackageBusyError{PackageID: name}
		}
		return err
	}
	return nil
}

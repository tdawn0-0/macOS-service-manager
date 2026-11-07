package devplatform

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"time"
)

// CLI exec holds a shared lease; replacement/uninstall take an exclusive one.
// launchd jobs are checked separately because no resident supervisor holds a
// lease on their behalf. Preparation holds a package lease without the state
// lock. State holders may only try exclusive leases without waiting, so an
// operation waiting to publish cannot deadlock against uninstall.
type PackageBusyError struct{ PackageID string }

func (e *PackageBusyError) Error() string {
	return fmt.Sprintf("package %s is in use by another operation or CLI command; wait for it to finish", e.PackageID)
}

func (m *Manager) packageLease(ctx context.Context, id string, exclusive bool) (func(), error) {
	if !filepath.IsAbs(m.Root) {
		return nil, errors.New("storage root must be absolute")
	}
	if !safeRelative(id) || filepath.Base(id) != id || !strings.Contains(id, "@") {
		return nil, errors.New("invalid package identity for operation")
	}
	dir := filepath.Join(m.Root, "package-locks")
	if info, e := os.Lstat(dir); e == nil {
		if !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
			return nil, errors.New("package lock store must be a plain directory")
		}
	} else if !errors.Is(e, os.ErrNotExist) {
		return nil, e
	}
	if err := os.MkdirAll(dir, 0700); err != nil {
		return nil, err
	}
	f, err := os.OpenFile(filepath.Join(m.Root, "package-locks", id), os.O_CREATE|os.O_RDWR|syscall.O_NOFOLLOW|syscall.O_NONBLOCK, 0600)
	if err != nil {
		return nil, err
	}
	if info, e := f.Stat(); e != nil || !info.Mode().IsRegular() {
		f.Close()
		return nil, errors.New("package lease is not a regular file")
	}
	waitingAnnounced := false
	mode := syscall.LOCK_SH
	if exclusive {
		mode = syscall.LOCK_EX
	}
	for {
		if err = ctx.Err(); err != nil {
			f.Close()
			return nil, err
		}
		err = syscall.Flock(int(f.Fd()), mode|syscall.LOCK_NB)
		if err == nil {
			return func() { syscall.Flock(int(f.Fd()), syscall.LOCK_UN); f.Close() }, nil
		}
		if err != syscall.EWOULDBLOCK {
			f.Close()
			return nil, err
		}
		if exclusive {
			f.Close()
			return nil, &PackageBusyError{PackageID: id}
		}
		if !waitingAnnounced {
			m.emit("Waiting for package " + id)
			waitingAnnounced = true
		}
		select {
		case <-ctx.Done():
			f.Close()
			return nil, ctx.Err()
		case <-time.After(100 * time.Millisecond):
		}
	}
}

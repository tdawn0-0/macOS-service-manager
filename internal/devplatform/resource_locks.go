package devplatform

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"syscall"
)

type ResourceBusyError struct{ Kind, ID string }

func (e *ResourceBusyError) Error() string {
	return fmt.Sprintf("%s %s is reserved by another operation; finish or cancel it in its caller and retry", e.Kind, e.ID)
}

type resourceKey struct{ Kind, ID string }

// Always try, never wait while holding another resource or the state lock.
func (m *Manager) resourceLeases(ctx context.Context, keys ...resourceKey) (func(), error) {
	if !filepath.IsAbs(m.Root) {
		return nil, errors.New("storage root must be absolute")
	}
	dir := filepath.Join(m.Root, "resource-locks")
	if info, e := os.Lstat(dir); e == nil {
		if !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
			return nil, errors.New("resource lock store must be a plain directory")
		}
	} else if !errors.Is(e, os.ErrNotExist) {
		return nil, e
	}
	if e := os.MkdirAll(dir, 0700); e != nil {
		return nil, e
	}
	sort.Slice(keys, func(i, j int) bool {
		return strings.ToLower(keys[i].Kind)+":"+keys[i].ID < strings.ToLower(keys[j].Kind)+":"+keys[j].ID
	})
	var files []*os.File
	release := func() {
		for n := len(files) - 1; n >= 0; n-- {
			syscall.Flock(int(files[n].Fd()), syscall.LOCK_UN)
			files[n].Close()
		}
	}
	prior := ""
	for _, key := range keys {
		if e := ctx.Err(); e != nil {
			release()
			return nil, e
		}
		value := key.Kind + ":" + key.ID
		if value == prior {
			continue
		}
		prior = value
		f, e := os.OpenFile(filepath.Join(dir, digestBytes([]byte(value))), os.O_CREATE|os.O_RDWR|syscall.O_NOFOLLOW|syscall.O_NONBLOCK, 0600)
		if e != nil {
			release()
			return nil, e
		}
		info, e := f.Stat()
		if e != nil || !info.Mode().IsRegular() {
			f.Close()
			release()
			return nil, errors.New("resource lease must be a regular file")
		}
		e = syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB)
		if e != nil {
			f.Close()
			release()
			if errors.Is(e, syscall.EWOULDBLOCK) {
				return nil, &ResourceBusyError{Kind: key.Kind, ID: key.ID}
			}
			return nil, e
		}
		files = append(files, f)
		if key.Kind == "project" || key.Kind == "database instance" {
			if e := m.requireCommandResourceIdle(key); e != nil {
				release()
				return nil, e
			}
		}
		if key.Kind == "database instance" {
			if e := m.requireDatabaseWorkIdle(key.ID); e != nil {
				release()
				return nil, e
			}
		}
	}
	return release, nil
}

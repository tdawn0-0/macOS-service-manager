package devplatform

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"
)

// A bundle executable's signature also seals Info.plist and bundle resources.
// Copying only its Mach-O produces an invalid standalone signature. Keep the
// complete signed envelope, shared by all helper roles and addressed by version.
// Old snapshots remain available to existing jobs after the original app moves.
func (m *Manager) bundleHelperExecutable(source, digest string) (string, bool, error) {
	macos := filepath.Dir(source)
	contents := filepath.Dir(macos)
	bundle := filepath.Dir(contents)
	if filepath.Base(macos) != "MacOS" || filepath.Base(contents) != "Contents" {
		return "", false, nil
	}
	if !strings.EqualFold(filepath.Ext(bundle), ".app") {
		if _, err := os.Stat(filepath.Join(contents, "Info.plist")); err != nil {
			return "", false, nil
		}
	}
	ctx, cancel := context.WithTimeout(m.Context, 30*time.Second)
	defer cancel()
	verify := func(path string) error {
		if e := plainDeletionDirectory(path); e != nil {
			return e
		}
		if _, e := m.Run(ctx, "/usr/bin/codesign", "--verify", "--deep", "--strict", path); e != nil {
			return fmt.Errorf("signed helper bundle verification failed: %w", e)
		}
		return nil
	}
	parent := filepath.Join(m.Root, "helpers", "bundles")
	if e := os.MkdirAll(parent, 0700); e != nil {
		return "", true, e
	}
	if e := plainDeletionDirectory(parent); e != nil {
		return "", true, e
	}
	dest := filepath.Join(parent, digest)
	program := filepath.Join(dest, "Contents", "MacOS", filepath.Base(source))
	if _, e := os.Lstat(dest); e == nil {
		if !hasDigest(program, digest) {
			return "", true, errors.New("cached helper executable changed; keep its bundle for inspection")
		}
		return program, true, verify(dest)
	} else if !errors.Is(e, os.ErrNotExist) {
		return "", true, e
	}
	if e := verify(bundle); e != nil {
		return "", true, e
	}
	stage, e := os.MkdirTemp(parent, ".bundle-stage-")
	if e != nil {
		return "", true, e
	}
	defer os.RemoveAll(stage)
	snapshot := filepath.Join(stage, "snapshot")
	if _, e = m.Run(ctx, "/usr/bin/ditto", "--noqtn", bundle, snapshot); e != nil {
		return "", true, fmt.Errorf("prepare signed helper bundle: %w", e)
	}
	if e = verify(snapshot); e != nil {
		return "", true, e
	}
	if !hasDigest(filepath.Join(snapshot, "Contents", "MacOS", filepath.Base(source)), digest) {
		return "", true, errors.New("app executable changed while preparing its helper")
	}
	if e = os.Rename(snapshot, dest); e != nil {
		// Independent projects may publish the same immutable snapshot together.
		if !hasDigest(program, digest) {
			return "", true, e
		}
		if e = verify(dest); e != nil {
			return "", true, e
		}
	}
	return program, true, syncDirectory(parent)
}

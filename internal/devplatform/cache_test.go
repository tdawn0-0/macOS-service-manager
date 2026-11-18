package devplatform

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"testing"
)

func TestDownloadCacheInfoAndClear(t *testing.T) {
	m := testManager(t)

	info, err := m.DownloadCacheInfo()
	if err != nil {
		t.Fatal(err)
	}
	if info.Directory != m.downloadDirectory() || info.Files != 0 || info.Bytes != 0 {
		t.Fatalf("unexpected empty cache info: %+v", info)
	}

	if err := os.MkdirAll(m.downloadDirectory(), 0o700); err != nil {
		t.Fatal(err)
	}
	aPath := filepath.Join(m.downloadDirectory(), "archive-a")
	bPath := filepath.Join(m.downloadDirectory(), "archive-b")
	if err := os.WriteFile(aPath, []byte("12345"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(bPath, []byte("67890123"), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Join(m.downloadDirectory(), "nested-dir"), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(aPath, filepath.Join(m.downloadDirectory(), "link-a")); err != nil {
		t.Fatal(err)
	}

	// Shared package lease (e.g. a running database) must not block clearing download archives.
	sharedUnlock, err := m.packageLease(context.Background(), "mysql@8.4.4-arm64", false)
	if err != nil {
		t.Fatal(err)
	}
	info, err = m.DownloadCacheInfo()
	if err != nil {
		t.Fatal(err)
	}
	if info.Files != 2 || info.Bytes != 13 {
		t.Fatalf("unexpected cache info: %+v", info)
	}
	sharedUnlock()

	// Exclusive package lease (active install/repair/uninstall) must refuse clearing.
	exclUnlock, err := m.packageLease(context.Background(), "node@22.14.0-arm64", true)
	if err != nil {
		t.Fatal(err)
	}
	_, err = m.ClearDownloadCache(context.Background())
	var busyErr *PackageBusyError
	if !errors.As(err, &busyErr) || busyErr.PackageID != "node@22.14.0-arm64" {
		t.Fatalf("expected PackageBusyError, got %v", err)
	}
	exclUnlock()

	// Installed package directories must remain intact after clearing cache.
	pkgFile := filepath.Join(m.Root, "packages", "node@22.14.0-arm64", "bin", "node")
	if err := os.MkdirAll(filepath.Dir(pkgFile), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(pkgFile, []byte("binary"), 0o700); err != nil {
		t.Fatal(err)
	}

	removed, err := m.ClearDownloadCache(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if removed.Files != 2 || removed.Bytes != 13 {
		t.Fatalf("unexpected removed summary: %+v", removed)
	}
	after, err := m.DownloadCacheInfo()
	if err != nil {
		t.Fatal(err)
	}
	if after.Files != 0 || after.Bytes != 0 {
		t.Fatalf("expected empty cache after clear, got %+v", after)
	}
	if _, err := os.Stat(pkgFile); err != nil {
		t.Fatalf("installed package affected by cache clear: %v", err)
	}
}

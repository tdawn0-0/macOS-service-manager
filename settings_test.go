package main

import (
	"context"
	"os"
	"path/filepath"
	"testing"

	"devbase/internal/devplatform"
	"devbase/internal/privhelper"
)

func TestSettingsPageConsolidatesPrivilegedHelperAndWorkspaceSettings(t *testing.T) {
	t.Setenv("MSM_DEV_HOME", t.TempDir())
	h := newHarness(t)
	h.tt.SetSize(1120, 900)
	h.a.dev.loaded = true

	// Clicking Settings opens navSettings with all three system/workspace cards.
	h.click("Settings")
	h.tt.Frame()
	if h.a.nav != navSettings {
		t.Fatalf("expected nav %q, got %q", navSettings, h.a.nav)
	}
	h.wantText("Settings", "Privileged helper", "macOS permissions", "Workspace", "Automatic log rotation")

	// With helper installed, refreshing Settings updates badge to Installed.
	privhelper.InstallTestHelper(t, privhelper.ServerConfig{})
	h.click("Refresh status")
	h.tt.Frame()
	if !h.a.helperReady() {
		t.Fatalf("expected helperReady(), status=%+v", h.a.helperStatus)
	}
	h.wantText("Installed", "Remove helper")
}

func TestSettingsDownloadCacheDisplayAndClear(t *testing.T) {
	root := t.TempDir()
	downloads := filepath.Join(root, "downloads")
	if err := os.MkdirAll(downloads, 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(downloads, "cached-node"), make([]byte, 2048), 0o600); err != nil {
		t.Fatal(err)
	}

	h := newHarness(t)
	h.tt.SetSize(1120, 900)
	h.a.dev.loaded = true
	h.a.dev.packagePreview = &devplatform.PackageDetails{Cached: true}
	h.a.dev.managerFactory = func() *devplatform.Manager {
		m := devplatform.New(root)
		m.Run = func(context.Context, string, ...string) (string, error) { return "", nil }
		return m
	}

	h.click("Settings")
	h.tt.Frame()
	h.wantText("2.0 KiB · 1 archive", "Clear download cache")

	h.click("Clear download cache")
	h.tt.Frame()
	h.wantText("Clear download cache?", "Clear Cache")
	h.click("Clear Cache")
	h.tt.Frame()

	if _, err := os.Stat(filepath.Join(downloads, "cached-node")); !os.IsNotExist(err) {
		t.Fatalf("expected cached file removed, stat err=%v", err)
	}
	if h.a.cacheInfo.Files != 0 || h.a.dev.packagePreview.Cached {
		t.Fatalf("expected cacheInfo cleared and preview.Cached=false, got cacheInfo=%+v cached=%v", h.a.cacheInfo, h.a.dev.packagePreview.Cached)
	}
	h.wantText("0 B")
}

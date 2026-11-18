package main

import (
	"context"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"howett.net/plist"

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

func TestSettingsStartAtLoginInMenuBar(t *testing.T) {
	t.Setenv("MSM_DEV_HOME", t.TempDir())
	h := newHarness(t)
	h.tt.SetSize(1120, 900)
	h.a.dev.loaded = true
	h.a.loginProgram = "/Applications/Devbase.app/Contents/MacOS/Devbase"
	var launchctlCalls []string
	h.a.loginRunner = func(args ...string) error {
		launchctlCalls = append(launchctlCalls, strings.Join(args, " "))
		return nil
	}

	h.click("Settings")
	h.tt.Frame()
	h.wantText("Start at login in menu bar")
	if h.a.loginAtLogin {
		t.Fatal("expected start at login initially false")
	}

	plistPath := filepath.Join(h.a.loginAgentDir, "xyz.jyeu.devbase.login.plist")
	h.click("Start at login in menu bar")
	if !h.a.loginAtLogin {
		t.Fatal("expected start at login enabled after toggle")
	}
	data, err := os.ReadFile(plistPath)
	if err != nil {
		t.Fatalf("expected login agent plist at %s: %v", plistPath, err)
	}
	var parsed struct {
		Label            string   `plist:"Label"`
		ProgramArguments []string `plist:"ProgramArguments"`
		RunAtLoad        bool     `plist:"RunAtLoad"`
	}
	if _, err := plist.Unmarshal(data, &parsed); err != nil {
		t.Fatalf("unmarshal plist: %v", err)
	}
	if parsed.Label != loginAgentLabel || !parsed.RunAtLoad || !slices.Equal(parsed.ProgramArguments, []string{"/Applications/Devbase.app/Contents/MacOS/Devbase", "--background"}) {
		t.Fatalf("unexpected plist contents: %+v", parsed)
	}
	if len(launchctlCalls) != 1 || !strings.HasPrefix(launchctlCalls[0], "enable ") {
		t.Fatalf("unexpected launchctl calls: %v", launchctlCalls)
	}

	// Refreshing Settings picks up the enabled plist state.
	h.a.loginAtLogin = false
	h.click("Refresh status")
	h.tt.Frame()
	if !h.a.loginAtLogin {
		t.Fatal("refreshSettings did not detect enabled login agent plist")
	}

	// Toggling off removes the plist and boots out via injected runner.
	h.click("Start at login in menu bar")
	if h.a.loginAtLogin {
		t.Fatal("expected start at login disabled after second toggle")
	}
	if _, err := os.Stat(plistPath); !os.IsNotExist(err) {
		t.Fatalf("expected plist removed, stat err=%v", err)
	}
	if len(launchctlCalls) != 2 || !strings.HasPrefix(launchctlCalls[1], "bootout ") {
		t.Fatalf("unexpected launchctl calls after disable: %v", launchctlCalls)
	}

	// Bundle Info.plist executable resolution in isolated TempDir.
	bundle := filepath.Join(t.TempDir(), "Devbase.app")
	if err := os.MkdirAll(filepath.Join(bundle, "Contents", "MacOS"), 0o755); err != nil {
		t.Fatal(err)
	}
	infoPlist, err := plist.Marshal(map[string]any{"CFBundleExecutable": "Devbase"}, plist.XMLFormat)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(bundle, "Contents", "Info.plist"), infoPlist, 0o644); err != nil {
		t.Fatal(err)
	}
	wantBin := filepath.Join(bundle, "Contents", "MacOS", "Devbase")
	if err := os.WriteFile(wantBin, []byte("#!/bin/sh\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	gotBin, err := resolveLoginExecutable(bundle, "")
	if err != nil || gotBin != wantBin {
		t.Fatalf("resolveLoginExecutable = %q, %v; want %q", gotBin, err, wantBin)
	}
}

func TestSettingsCLICardInstallAndUninstall(t *testing.T) {
	t.Setenv("MSM_DEV_HOME", t.TempDir())
	h := newHarness(t)
	h.tt.SetSize(1120, 1200)
	h.a.dev.loaded = true

	fakeBinDir := filepath.Join(t.TempDir(), "bin")
	h.a.cliBinDir = fakeBinDir
	h.a.loginProgram = "/Applications/Devbase.app/Contents/MacOS/Devbase"
	var copied string
	h.a.copy = func(text string) {
		copied = text
	}

	h.click("Settings")
	h.tt.Frame()
	h.wantText("Command-line interface", "Install devbase CLI", "Copy AI schema command")

	h.click("Copy AI schema command")
	h.tt.Frame()
	if copied != "devbase schema" {
		t.Fatalf("expected copied command %q, got %q", "devbase schema", copied)
	}

	h.click("Install devbase CLI")
	h.tt.Frame()
	wrapperPath := filepath.Join(fakeBinDir, "devbase")
	data, err := os.ReadFile(wrapperPath)
	if err != nil {
		t.Fatalf("expected installed CLI wrapper at %s: %v", wrapperPath, err)
	}
	if !strings.Contains(string(data), "/Applications/Devbase.app/Contents/MacOS/Devbase") {
		t.Fatalf("wrapper did not point to target binary: %s", string(data))
	}
	h.wantText("Installed", "Remove devbase CLI")

	h.click("Remove devbase CLI")
	h.tt.Frame()
	if _, err := os.Stat(wrapperPath); !os.IsNotExist(err) {
		t.Fatalf("expected CLI wrapper removed, stat err=%v", err)
	}
	h.wantText("Not installed", "Install devbase CLI")
}

func TestShouldRunPublicCLI(t *testing.T) {
	for _, guiArgs := range [][]string{
		nil,
		{},
		{"--background"},
		{"-psn_0_12345"},
		{"-psn_0_12345", "--background"},
		{"-ApplePersistenceIgnoreState", "YES"},
		{"gui"},
		{"open"},
		{"-psn_0_9", "gui", "--background"},
	} {
		if got, ok := shouldRunPublicCLI(guiArgs); ok || len(got) != 0 {
			t.Errorf("shouldRunPublicCLI(%v) = (%v, %t); want GUI (false)", guiArgs, got, ok)
		}
	}

	for _, tc := range []struct {
		in   []string
		want []string
	}{
		{[]string{"status", "--json"}, []string{"status", "--json"}},
		{[]string{"-psn_0_1", "schema"}, []string{"schema"}},
		{[]string{"--help"}, []string{"--help"}},
		{[]string{"--json", "db", "list"}, []string{"--json", "db", "list"}},
	} {
		got, ok := shouldRunPublicCLI(tc.in)
		if !ok || !slices.Equal(got, tc.want) {
			t.Errorf("shouldRunPublicCLI(%v) = (%v, %t); want (%v, true)", tc.in, got, ok, tc.want)
		}
	}
}

package main

import (
	"testing"

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

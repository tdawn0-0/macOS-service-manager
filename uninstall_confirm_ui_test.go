package main

import (
	"testing"

	"github.com/egoist/mygo/ui"
)

// Uninstall sits among everyday actions, so it only asks: the version stays
// until the dialog is confirmed, and Cancel keeps it.
func TestUninstallAsksFirst(t *testing.T) {
	a := designFixture(t)
	a.dev.tab = devTabSoftware
	tt := ui.NewTester(a.view, 1120, 720)
	tt.Frame()
	if e := tt.Click("Uninstall"); e != nil {
		t.Fatal(e)
	}
	tt.Frame()
	if !a.ask.open || a.ask.button != "Uninstall Version" || a.ask.run == nil {
		t.Fatalf("Uninstall did not ask first: %+v", a.ask)
	}
	if !tt.HasText("Uninstall Node.js 22.14.0?") {
		t.Fatal("confirmation does not name the version")
	}
	if _, ok := a.dev.state.Packages["node@22.14.0-arm64"]; !ok {
		t.Fatal("version removed before confirming")
	}
	if e := tt.Click("Cancel"); e != nil {
		t.Fatal(e)
	}
	tt.Frame()
	if a.ask.open {
		t.Fatal("Cancel left the confirmation open")
	}
	if _, ok := a.dev.state.Packages["node@22.14.0-arm64"]; !ok {
		t.Fatal("Cancel removed the version")
	}
}

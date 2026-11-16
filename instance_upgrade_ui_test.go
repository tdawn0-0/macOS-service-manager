package main

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"devbase/internal/devplatform"
	"github.com/egoist/mygo/ui"
)

func upgradeUIFixture(t *testing.T) *app {
	t.Helper()
	a := designFixture(t)
	for _, v := range []string{"8.4.13"} {
		p := devplatform.Package{Name: "mysql", Version: v, Arch: "arm64", License: "See included license"}
		a.dev.state.Packages[p.ID()] = p
	}
	a.dev.state.Instances["shop"] = devplatform.Instance{ID: "shop", Kind: "mysql", Package: "mysql@8.4.4-arm64", Port: 3306, Persistent: true}
	a.dev.statuses["instance:shop"] = "Stopped"
	a.openInstance("shop")
	return a
}

func TestUIInstancePatchUpgrade(t *testing.T) {
	const label = "Upgrade to 8.4.13…"
	t.Run("offers-newest", func(t *testing.T) {
		a := upgradeUIFixture(t)
		a.dev.statuses["instance:shop"] = "Running"
		tt := ui.NewTester(a.view, 1120, 1600)
		tt.Frame()
		if !tt.HasText(label) {
			t.Fatal("upgrade control not shown")
		}
		_ = tt.Click(label)
		tt.Frame()
		if a.ask.open {
			t.Fatal("running instance opened the confirmation")
		}
	})
	t.Run("asks-first", func(t *testing.T) {
		a := upgradeUIFixture(t)
		tt := ui.NewTester(a.view, 1120, 1600)
		tt.Frame()
		if e := tt.Click(label); e != nil {
			t.Fatal(e)
		}
		tt.Frame()
		if !a.ask.open || a.ask.button != "Upgrade Instance" || a.ask.run == nil {
			t.Fatalf("did not ask first: %+v", a.ask)
		}
		if !strings.Contains(a.ask.message, "no backup is taken") || !strings.Contains(a.ask.message, "8.4.4") {
			t.Fatalf("message = %q", a.ask.message)
		}
		if got := a.dev.state.Instances["shop"].Package; got != "mysql@8.4.4-arm64" {
			t.Fatalf("package changed before confirming: %s", got)
		}
		if e := tt.Click("Cancel"); e != nil {
			t.Fatal(e)
		}
		tt.Frame()
		if a.ask.open {
			t.Fatal("Cancel left the confirmation open")
		}
		if got := a.dev.state.Instances["shop"].Package; got != "mysql@8.4.4-arm64" {
			t.Fatalf("Cancel changed the package: %s", got)
		}
	})
	t.Run("runs-core-operation", func(t *testing.T) {
		a := upgradeUIFixture(t)
		a.spawn = func(f func()) { f() }
		a.post = func(f func()) { f() }
		a.dev.managerFactory = func() *devplatform.Manager {
			m := devplatform.New(t.TempDir())
			m.Run = func(context.Context, string, ...string) (string, error) {
				return "", errors.New("Could not find service")
			}
			return m
		}
		tt := ui.NewTester(a.view, 1120, 1600)
		tt.Frame()
		if e := tt.Click(label); e != nil {
			t.Fatal(e)
		}
		tt.Frame()
		a.dev.output = ""
		a.ask.run()
		// Asserted: the core method was called and its result surfaced. The
		// temporary manager has no such instance, so the core's refusal shows.
		if !strings.Contains(a.dev.output, "instance not found") && !strings.Contains(a.dev.output, "Upgraded shop to 8.4.13") {
			t.Fatalf("output = %q", a.dev.output)
		}
	})
	t.Run("no-control-without-target", func(t *testing.T) {
		a := upgradeUIFixture(t)
		a.openInstance("atlas-postgres")
		tt := ui.NewTester(a.view, 1120, 1600)
		tt.Frame()
		if tt.HasText("Upgrade to") {
			t.Fatal("PostgreSQL page offers an upgrade")
		}
		delete(a.dev.state.Packages, "mysql@8.4.13-arm64")
		a.openInstance("shop")
		tt.Frame()
		if tt.HasText("Upgrade to") {
			t.Fatal("instance without a newer package offers an upgrade")
		}
	})
	t.Run("software-tag", func(t *testing.T) {
		if devplatform.NewerListedPatch("8.4.4", []string{"8.4.13", "8.4.4"}) != "8.4.13" {
			t.Fatal("NewerListedPatch")
		}
		want := devplatform.NewerListedPatch("8.4.4", devplatform.AvailableVersions("mysql"))
		a := upgradeUIFixture(t)
		a.dev.tab = devTabSoftware
		a.dev.software = "mysql"
		tt := ui.NewTester(a.view, 1120, 1600)
		tt.Frame()
		if got := tt.HasText(want + " available"); want != "" && !got {
			t.Fatalf("tag %q missing", want+" available")
		}
		if want == "" && tt.HasText(" available") {
			t.Fatal("tag shown without a newer listed patch")
		}
	})
	// Real success path: a manager root on disk holding two MySQL versions.
	upgradeDiskFixture := func(t *testing.T, extraInstance bool) (*app, *ui.Tester, *devplatform.Manager) {
		t.Helper()
		a := designFixture(t)
		a.spawn = func(f func()) { f() }
		a.post = func(f func()) { f() }
		m := devplatform.New(t.TempDir())
		m.Run = func(context.Context, string, ...string) (string, error) {
			return "", errors.New("Could not find service")
		}
		for _, d := range []string{"", "packages", "instances", "jobs", "logs", "backups"} {
			if e := os.MkdirAll(filepath.Join(m.Root, d), 0700); e != nil {
				t.Fatal(e)
			}
		}
		state := a.dev.state
		state.Packages = map[string]devplatform.Package{}
		state.Instances = map[string]devplatform.Instance{}
		for _, v := range []string{"8.4.4", "8.4.13"} {
			p := devplatform.Package{Name: "mysql", Version: v, Arch: "arm64", License: "See included license", Binaries: map[string]string{"mysqld": "bin/mysqld"}}
			dir := filepath.Join(m.PackageDirectory(p), "bin")
			if e := os.MkdirAll(dir, 0755); e != nil {
				t.Fatal(e)
			}
			if e := os.WriteFile(filepath.Join(dir, "mysqld"), []byte("#!/bin/sh\nexit 0\n"), 0755); e != nil {
				t.Fatal(e)
			}
			state.Packages[p.ID()] = p
		}
		state.Instances["shop"] = devplatform.Instance{ID: "shop", Kind: "mysql", Package: "mysql@8.4.4-arm64", Port: 3306, Persistent: true}
		if extraInstance {
			state.Instances["blog"] = devplatform.Instance{ID: "blog", Kind: "mysql", Package: "mysql@8.4.4-arm64", Port: 3307, Persistent: true}
		}
		raw, e := json.Marshal(state)
		if e != nil {
			t.Fatal(e)
		}
		if e = os.WriteFile(filepath.Join(m.Root, "state.json"), raw, 0600); e != nil {
			t.Fatal(e)
		}
		if e = os.WriteFile(filepath.Join(m.Root, "logs", "db-shop.err.log"), []byte("[System] ready for connections\n[System] Shutdown complete\n"), 0600); e != nil {
			t.Fatal(e)
		}
		a.dev.managerFactory = func() *devplatform.Manager { return m }
		a.dev.state = state
		a.dev.statuses["instance:shop"] = "Stopped"
		a.openInstance("shop")
		tt := ui.NewTester(a.view, 1120, 1600)
		tt.Frame()
		return a, tt, m
	}
	t.Run("offers-uninstall-after-upgrade", func(t *testing.T) {
		a, tt, m := upgradeDiskFixture(t, false)
		if e := tt.Click(label); e != nil {
			t.Fatal(e)
		}
		tt.Frame()
		if !a.ask.open || a.ask.button != "Upgrade Instance" {
			t.Fatalf("no upgrade confirmation: %+v", a.ask)
		}
		a.ask.open = false // the dialog closes itself when confirmed
		a.ask.run()
		s, e := m.Read()
		if e != nil {
			t.Fatal(e)
		}
		if got := s.Instances["shop"].Package; got != "mysql@8.4.13-arm64" {
			t.Fatalf("saved package = %s (output %q)", got, a.dev.output)
		}
		if !a.ask.open || !strings.HasPrefix(a.ask.title, "Uninstall ") || !strings.Contains(a.ask.title, "8.4.4") || !strings.Contains(a.ask.message, "No instance uses") {
			t.Fatalf("no uninstall offer: %+v", a.ask)
		}
		if _, ok := s.Packages["mysql@8.4.4-arm64"]; !ok {
			t.Fatal("old package removed without confirmation")
		}
		tt.Frame()
		if e := tt.Click("Cancel"); e != nil {
			t.Fatal(e)
		}
		tt.Frame()
		if a.ask.open {
			t.Fatal("Cancel left the question open")
		}
		s, e = m.Read()
		if e != nil {
			t.Fatal(e)
		}
		if _, ok := s.Packages["mysql@8.4.4-arm64"]; !ok {
			t.Fatal("Cancel uninstalled the old package")
		}
	})
	t.Run("no-offer-when-still-used", func(t *testing.T) {
		a, tt, m := upgradeDiskFixture(t, true)
		if e := tt.Click(label); e != nil {
			t.Fatal(e)
		}
		tt.Frame()
		a.ask.open = false // the dialog closes itself when confirmed
		a.ask.run()
		s, e := m.Read()
		if e != nil {
			t.Fatal(e)
		}
		if got := s.Instances["shop"].Package; got != "mysql@8.4.13-arm64" {
			t.Fatalf("saved package = %s (output %q)", got, a.dev.output)
		}
		if a.ask.open {
			t.Fatalf("offered to uninstall a version another instance uses: %+v", a.ask)
		}
	})
}

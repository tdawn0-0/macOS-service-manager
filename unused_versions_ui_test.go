package main

import (
	"testing"

	"devbase/internal/devplatform"
	"github.com/egoist/mygo/ui"
)

func TestUIUnusedDatabaseVersions(t *testing.T) {
	pkg := func(name, version string) devplatform.Package {
		return devplatform.Package{Name: name, Version: version, Arch: "arm64"}
	}
	s := devplatform.State{Packages: map[string]devplatform.Package{}, Instances: map[string]devplatform.Instance{}}
	for _, p := range []devplatform.Package{pkg("mysql", "8.4.4"), pkg("mysql", "8.4.13"), pkg("node", "22.1.0"), pkg("java", "21.0.1")} {
		s.Packages[p.ID()] = p
	}
	s.Instances["shop"] = devplatform.Instance{ID: "shop", Kind: "mysql", Package: "mysql@8.4.13-arm64"}
	for id, want := range map[string]bool{
		"mysql@8.4.4-arm64":  true,
		"mysql@8.4.13-arm64": false,
		"node@22.1.0-arm64":  false,
		"java@21.0.1-arm64":  false,
		"mysql@9.9.9-arm64":  false,
		"unknown":            false,
	} {
		if got := packageUnused(s, id); got != want {
			t.Errorf("packageUnused(%q) = %v, want %v", id, got, want)
		}
	}

	a := designFixture(t)
	p := pkg("mysql", "8.4.4")
	a.dev.state.Packages[p.ID()] = p
	a.dev.tab = devTabSoftware
	a.dev.software = "mysql"
	tt := ui.NewTester(a.view, 1120, 1600)
	tt.Frame()
	if !tt.HasText("Unused") {
		t.Fatal("unreferenced MySQL version is not marked")
	}
	a.dev.software = "postgresql"
	tt.Frame()
	if tt.HasText("Unused") {
		t.Fatal("PostgreSQL version used by an instance is marked")
	}
}

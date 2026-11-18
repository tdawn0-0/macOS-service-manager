package main

import (
	"devbase/internal/devplatform"
	"fmt"
	"github.com/egoist/mygo/ui"
	"testing"
)

func designFixture(t *testing.T) *app {
	t.Helper()
	a := newAppWith(nil, nil, t.TempDir())
	a.nav = navDevelopment
	a.copy = func(string) {}
	a.reveal = func(string) {}
	a.dev.loaded = true
	s := devplatform.State{SchemaVersion: devplatform.Schema, Packages: map[string]devplatform.Package{}, Instances: map[string]devplatform.Instance{}, ProxyRules: []devplatform.ProxyRule{{Domain: "atlas.devbase.localhost", LocalPort: 3000}}, ProxyListenerPort: 443}
	for _, pair := range [][2]string{{"node", "22.14.0"}, {"mysql", "8.4.4"}, {"postgresql", "17.11_1"}, {"redis", "8.2.11"}, {"mongodb", "8.0.5"}} {
		p := devplatform.Package{Name: pair[0], Version: pair[1], Arch: "arm64", License: "See included license"}
		if p.Name == "postgresql" {
			p.Formula = "postgresql@17"
			p.Format = "homebrew"
		}
		if p.Name == "redis" {
			p.Formula = "redis@8.2"
			p.Format = "homebrew"
		}
		s.Packages[p.ID()] = p
	}
	s.Instances["atlas-postgres"] = devplatform.Instance{ID: "atlas-postgres", Kind: "postgresql", Package: "postgresql@17.11_1-arm64", Port: 5432, Persistent: true}
	a.dev.state = s
	a.dev.statuses = map[string]string{"instance:atlas-postgres": "Running"}
	a.dev.software = "node"
	a.dev.version = "22.14.0"
	a.dev.dbID = "atlas-postgres"
	a.dev.dbKind = "postgresql"
	a.dev.dbPackage = "postgresql@17.11_1-arm64"
	a.dev.dbPort = "5432"
	a.dev.loadProxyDraft(s)
	a.dev.nodeReleasesTried = true
	a.dev.nodeShell = 2
	a.dev.state.NodeDefault = "node@22.14.0-arm64"
	a.dev.output = "Workspace ready"
	return a
}
func TestDevelopmentDesignLayouts(t *testing.T) {
	for _, size := range [][2]int{{1120, 720}, {900, 520}} {
		for tab, name := range devSections {
			for _, dark := range []bool{false, true} {
				t.Run(fmt.Sprintf("%s-%dx%d-dark-%t", name, size[0], size[1], dark), func(t *testing.T) {
					a := designFixture(t)
					a.dev.tab = tab
					tt := ui.NewTester(a.view, size[0], size[1])
					tt.SetDark(dark)
					tt.Frame()
					if tt.Image().Bounds().Dx() == 0 {
						t.Fatal("empty development pane")
					}
					saveShot(t, fmt.Sprintf("%s-%dx%d-dark-%t", name, size[0], size[1], dark), tt.Image())
				})
			}
		}
	}
}

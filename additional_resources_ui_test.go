package main

import (
	"context"
	"encoding/json"
	"fmt"
	"image/png"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/egoist/mygo/ui"
	"devbase/internal/devplatform"
)

func resource018Screenshot(t *testing.T, tt *ui.Tester, name string) {
	t.Helper()
	dir := os.Getenv("MSM_ADDITIONAL_SHOTS")
	if dir == "" {
		return
	}
	if e := os.MkdirAll(dir, 0755); e != nil {
		t.Fatal(e)
	}
	f, e := os.Create(filepath.Join(dir, name+".png"))
	if e != nil {
		t.Fatal(e)
	}
	defer f.Close()
	if e = png.Encode(f, tt.Image()); e != nil {
		t.Fatal(e)
	}
}
func TestUIAdditionalRuntimes(t *testing.T) {
	for _, name := range []string{"java", "go", "python"} {
		for _, size := range [][2]int{{900, 520}, {1120, 720}} {
			for _, dark := range []bool{false, true} {
				testName := fmt.Sprintf("%s-%dx%d-dark-%t", name, size[0], size[1], dark)
				t.Run(testName, func(t *testing.T) {
					a := designFixture(t)
					a.dev.runtimeShell = map[string]int{"java": 2, "go": 2}
					a.dev.software = name
					a.dev.version = "21.0.13_9.0.LTS"
					a.dev.runtimeVersions = map[string][]string{"java": {"21.0.13_9.0.LTS", "25.0.1_8.0.LTS"}, "go": {"1.27.3", "1.26.8"}}
					p := devplatform.Package{Name: name, Version: a.dev.version, Arch: "arm64"}
					if name == "go" {
						p.Version = "1.27.3"
						a.dev.version = p.Version
					}
					if name != "python" {
						p.License = "BSD-3-Clause"
						if name == "java" {
							p.License = "GPL-2.0 with Classpath Exception"
						}
						a.dev.state.Packages[p.ID()] = p
					}
					a.dev.state.RuntimeDefaults = map[string]string{name: p.ID()}
					if name == "java" {
						a.dev.javaLoaded = true
						a.dev.javaInstallations = []devplatform.JavaInstallation{{ID: "java-fixture", PackageID: p.ID(), Managed: true, CanUse: true, Name: "Temurin", Vendor: "Eclipse Adoptium", Version: p.Version, Arch: "aarch64", Path: "/Users/developer/Library/Java/JavaVirtualMachines/temurin-21.0.13", Home: "/Users/developer/Library/Java/JavaVirtualMachines/temurin-21.0.13/Contents/Home"}}
					}
					a.dev.uvPin = "cpython-3.12.12-macos-aarch64-none"
					a.dev.uvDescription = "uv 0.9.10 · /Users/developer/.local/bin/uv"
					a.dev.uvInstalled = []devplatform.UVPython{{Key: "cpython-3.12.12-macos-aarch64-none", Version: "3.12.12", Path: "/Users/developer/.local/share/uv/python/cpython-3.12.12-macos-aarch64-none/bin/python3.12"}}
					a.dev.uvAvailable = []devplatform.UVPython{{Key: "cpython-3.13.9-macos-aarch64-none", Version: "3.13.9", Arch: "aarch64", OS: "macos"}}
					a.dev.uvVersion = a.dev.uvAvailable[0].Key
					tt := ui.NewTester(a.view, size[0], size[1])
					tt.SetDark(dark)
					tt.Frame()
					shown := strings.Join(tt.Texts(), "\n")
					labels := []string{"Refresh available versions", "Exact version", "Default", "Enable in terminal"}
					if name == "python" {
						labels = []string{"Refresh Python versions", "Install Python with uv", "Set uv default 3.12.12", "Uninstall Python 3.12.12"}
					}
					for _, label := range labels {
						if !strings.Contains(shown, label) {
							t.Fatal("missing", label, shown)
						}
					}
					if name != "python" {
						title := softwareTitle(name) + " · " + p.Version
						if name == "java" {
							title = a.dev.javaInstallations[0].Label()
							for n := 0; n < 8; n++ {
								r, ok := tt.Find(title)
								if ok && r.W > 0 && r.Y >= 50 && r.Y+r.H < float32(size[1]-45) {
									break
								}
								tt.Scroll(float32(size[0]-20), float32(size[1]/2), 0, 100)
								tt.Frame()
							}
						}
						r, ok := tt.Find(title)
						if !ok || r.W < 100 || r.X+r.W > float32(size[0]) {
							t.Fatal("installed version clipped", r)
						}
					}
					resource018Screenshot(t, tt, testName)
					// Every final action remains reachable in the narrow native window.
					last := "Enable in terminal"
					if name == "python" {
						last = "Uninstall Python 3.12.12"
					}
					for n := 0; n < 12; n++ {
						r, ok := tt.Find(last)
						if ok && r.Y >= 50 && r.Y+r.H < float32(size[1]-45) {
							break
						}
						tt.Scroll(float32(size[0]-20), float32(size[1]/2), 0, 140)
						tt.Frame()
					}
					r, ok := tt.Find(last)
					if !ok || r.Y < 50 || r.Y+r.H > float32(size[1]-45) {
						t.Fatal("action not reachable", last, r)
					}
				})
			}
		}
	}
}
func TestUIDatabaseLocations(t *testing.T) {
	t.Run("create-keeps-custom-location", func(t *testing.T) {
		h := newDevUIHarnessSize(t, 900, 520, false)
		m := h.a.dev.managerFactory()
		s, e := m.Read()
		if e != nil {
			t.Fatal(e)
		}
		p := devplatform.Package{Name: "redis", Version: "7.4.7", Arch: "arm64", URL: "https://example.invalid/fixture.zip", SHA256: strings.Repeat("a", 64), Format: "zip", Binaries: map[string]string{"redis-server": "bin/redis-server"}}
		path := filepath.Join(m.PackageDirectory(p), "bin", "redis-server")
		os.MkdirAll(filepath.Dir(path), 0700)
		os.WriteFile(path, []byte("#!/bin/sh\nexit 0\n"), 0700)
		s.Packages[p.ID()] = p
		b, _ := json.Marshal(s)
		if e = os.WriteFile(filepath.Join(h.root, "state.json"), b, 0600); e != nil {
			t.Fatal(e)
		}
		h.a.dev.state = s
		base, e := filepath.EvalSymlinks(t.TempDir())
		if e != nil {
			t.Fatal(e)
		}
		location := filepath.Join(base, "my database")
		h.click("Databases")
		h.click("New database instance")
		h.fill("Instance name", "external-redis")
		h.choose("Database engine", "redis")
		h.fill("Data location", location)
		h.fill("Local port", fmt.Sprint(freeUIPort(t)))
		h.click("Create instance")
		saved, e := m.Read()
		if e != nil {
			t.Fatal(e)
		}
		i, ok := saved.Instances["external-redis"]
		if !ok || i.DataDirectory != location || i.DataOwner == "" {
			t.Fatal("UI did not submit custom location", h.a.dev.output, i)
		}
		if e = m.InstanceAction(context.Background(), i.ID, "remove"); e != nil {
			t.Fatal(e)
		}
		if _, e = os.Stat(filepath.Join(location, "data")); e != nil {
			t.Fatal("Remove erased external data", e)
		}
	})
	for _, size := range [][2]int{{900, 520}, {1120, 720}} {
		for _, dark := range []bool{false, true} {
			testName := fmt.Sprintf("database-location-%dx%d-dark-%t", size[0], size[1], dark)
			t.Run(testName, func(t *testing.T) {
				a := designFixture(t)
				a.dev.tab = devTabDatabases
				a.dev.dbID = "new-pg18"
				a.dev.dbNewDraft = true
				a.dev.dbKind = "postgresql"
				a.dev.dbDataDirectory = "/Volumes/Development/Databases/postgresql-18"
				a.dev.dbPort = "55433"
				p := devplatform.Package{Name: "postgresql", Version: "18.6_1", Arch: "arm64", Format: "homebrew", Formula: "postgresql@18"}
				a.dev.state.Packages[p.ID()] = p
				a.dev.dbPackage = p.ID()
				tt := ui.NewTester(a.view, size[0], size[1])
				tt.SetDark(dark)
				tt.Frame()
				shown := strings.Join(tt.Texts(), "\n")
				for _, label := range []string{"Data location", "Database version", "Local port", "Create instance"} {
					if !strings.Contains(shown, label) {
						t.Fatal(label, shown)
					}
				}
				resource018Screenshot(t, tt, testName)
				for n := 0; n < 12; n++ {
					r, ok := tt.Find("Create instance")
					if ok && r.Y >= 50 && r.Y+r.H < float32(size[1]-45) {
						break
					}
					tt.Scroll(float32(size[0]-20), float32(size[1]/2), 0, 140)
					tt.Frame()
				}
				r, ok := tt.Find("Create instance")
				if !ok || r.Y < 50 || r.Y+r.H > float32(size[1]-45) {
					t.Fatal("create not reachable", r)
				}
			})
		}
	}
}

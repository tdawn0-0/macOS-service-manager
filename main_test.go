package main

import (
	"errors"
	"image/png"
	"os"
	"slices"
	"testing"

	"github.com/egoist/mygo/ui"

	"macos-service-manager/internal/brew"
	"macos-service-manager/internal/launchd"
)

type fakeBrew struct {
	services []brew.Service
	err      error
	calls    []string
}

func (f *fakeBrew) List() ([]brew.Service, error) { return slices.Clone(f.services), f.err }

func (f *fakeBrew) Info(name string) (*brew.Info, error) {
	log := "/tmp/" + name + ".log"
	return &brew.Info{Name: name, ServiceName: "homebrew.mxcl." + name, Status: "started", LogPath: &log}, nil
}

func (f *fakeBrew) Manage(name string, c brew.Command) error {
	f.calls = append(f.calls, string(c)+" "+name)
	for i := range f.services {
		if f.services[i].Name == name {
			f.services[i].Status = map[bool]string{true: "started", false: "stopped"}[c == brew.Run || c == brew.Start]
		}
	}
	return nil
}

type fakeLaunchd struct {
	services []launchd.Service
	calls    []string
}

func (f *fakeLaunchd) List() ([]launchd.Service, error) { return slices.Clone(f.services), nil }

func (f *fakeLaunchd) Info(path string) (*launchd.Info, error) {
	prog := "/bin/true"
	return &launchd.Info{Service: f.services[0], Program: &prog}, nil
}

func (f *fakeLaunchd) Manage(label, path string, c launchd.Command) error {
	f.calls = append(f.calls, string(c)+" "+label)
	f.services[0].Loaded = c == launchd.Load
	return nil
}

// testApp returns an app whose background work runs at once, on fake
// backends, with the tester rendering it.
func testApp(t *testing.T, b *fakeBrew, l *fakeLaunchd) (*app, *ui.Tester, *[]string) {
	t.Helper()
	a := newApp()
	a.brew, a.launchd = b, l
	a.post = func(f func()) { f() }
	a.spawn = func(f func()) { f() }
	var copied []string
	a.copy = func(s string) { copied = append(copied, s) }
	a.refreshAll()
	return a, ui.NewTester(a.view, 800, 600), &copied
}

func redis() []brew.Service {
	user := "jyeu"
	return []brew.Service{{Name: "redis", Status: "stopped", User: &user}}
}

func agent() []launchd.Service {
	return []launchd.Service{{Label: "com.example.sync", Path: "/Library/LaunchAgents/sync.plist", Domain: launchd.DomainSystem}}
}

func TestBrewListAndRun(t *testing.T) {
	b := &fakeBrew{services: redis()}
	a, tt, _ := testApp(t, b, &fakeLaunchd{services: agent()})
	tt.Frame()
	for _, want := range []string{"Homebrew Services", "redis", "stopped", "jyeu", "Run"} {
		if !tt.HasText(want) {
			t.Errorf("missing %q in %q", want, tt.Texts())
		}
	}
	if err := tt.Click("Run"); err != nil {
		t.Fatal(err)
	}
	if !slices.Equal(b.calls, []string{"run redis"}) {
		t.Fatalf("calls = %v", b.calls)
	}
	tt.Frame()
	if !tt.HasText("started") || !tt.HasText("Stop") || !tt.HasText("redis: run") {
		t.Errorf("after Run: %q", tt.Texts())
	}
	if len(a.brewTab.busy) != 0 {
		t.Errorf("busy = %v", a.brewTab.busy)
	}
}

func TestBrewError(t *testing.T) {
	b := &fakeBrew{err: errors.New("brew not found")}
	_, tt, _ := testApp(t, b, &fakeLaunchd{services: agent()})
	tt.Frame()
	if !tt.HasText("brew not found") || !tt.HasText("Retry") {
		t.Fatalf("texts = %q", tt.Texts())
	}
	b.err, b.services = nil, redis()
	if err := tt.Click("Retry"); err != nil {
		t.Fatal(err)
	}
	tt.Frame()
	if !tt.HasText("redis") || tt.HasText("brew not found") {
		t.Errorf("after retry: %q", tt.Texts())
	}
}

func TestBrewDetail(t *testing.T) {
	b := &fakeBrew{services: redis()}
	a, tt, copied := testApp(t, b, &fakeLaunchd{services: agent()})
	tt.Frame()
	if err := tt.Click("Details"); err != nil {
		t.Fatal(err)
	}
	tt.Frame()
	for _, want := range []string{"service_name", "homebrew.mxcl.redis", "Open log", "Restart", "Kill", "Close"} {
		if !tt.HasText(want) {
			t.Errorf("detail lacks %q: %q", want, tt.Texts())
		}
	}
	if err := tt.Click("Copy"); err != nil {
		t.Fatal(err)
	}
	if !slices.Equal(*copied, []string{"redis"}) {
		t.Errorf("copied = %v, want the first row's value", *copied)
	}
	if err := tt.Click("Restart"); err != nil {
		t.Fatal(err)
	}
	if !slices.Equal(b.calls, []string{"restart redis"}) {
		t.Errorf("calls = %v", b.calls)
	}
	if err := tt.Click("Close"); err != nil {
		t.Fatal(err)
	}
	if a.detail.open {
		t.Error("Close should close the dialog")
	}
}

func TestLaunchdTab(t *testing.T) {
	l := &fakeLaunchd{services: agent()}
	_, tt, _ := testApp(t, &fakeBrew{services: redis()}, l)
	tt.Frame()
	if err := tt.Click("LaunchAgents"); err != nil {
		t.Fatal(err)
	}
	tt.Frame()
	for _, want := range []string{"com.example.sync", "unloaded", "system", "Load"} {
		if !tt.HasText(want) {
			t.Errorf("missing %q in %q", want, tt.Texts())
		}
	}
	if err := tt.Click("Load"); err != nil {
		t.Fatal(err)
	}
	if !slices.Equal(l.calls, []string{"load com.example.sync"}) {
		t.Fatalf("calls = %v", l.calls)
	}
	tt.Frame()
	if !tt.HasText("Unload") {
		t.Errorf("after Load: %q", tt.Texts())
	}
}

func TestScreenshots(t *testing.T) {
	dir := os.Getenv("MSM_SHOTS")
	if dir == "" {
		t.Skip("set MSM_SHOTS to a directory to write screenshots")
	}
	user := "jyeu"
	b := &fakeBrew{services: []brew.Service{
		{Name: "postgresql@16", Status: "started", User: &user},
		{Name: "redis", Status: "stopped"},
		{Name: "nginx", Status: "error", User: &user},
		{Name: "mysql", Status: "scheduled"},
	}}
	l := &fakeLaunchd{services: agent()}
	a, tt, _ := testApp(t, b, l)
	shot := func(name string) {
		tt.Frame()
		f, err := os.Create(dir + "/" + name + ".png")
		if err != nil {
			t.Fatal(err)
		}
		defer f.Close()
		if err := png.Encode(f, tt.Image()); err != nil {
			t.Fatal(err)
		}
	}
	shot("brew")
	a.openBrewDetail("postgresql@16")
	shot("brew-detail")
	a.detail.open = false
	tt.Frame()
	if err := tt.Click("LaunchAgents"); err != nil {
		t.Fatal(err)
	}
	shot("launchd")
}

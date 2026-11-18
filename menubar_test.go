package main

import (
	"bytes"
	"fmt"
	"image/png"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/egoist/mygo/ui"

	"devbase/internal/launchd"
	"devbase/internal/service"
)

func TestMenubarPanelTogglesServices(t *testing.T) {
	h := newHarness(t)
	opened := 0
	panel := ui.NewTester(func(c *ui.Context) { h.a.menubarView(c, func() { opened++ }) }, 320, 400)
	panel.Frame()
	for _, w := range []string{"Devbase", "1 running", "Homebrew", "postgresql", "redis", "User Agents", "com.example.sync", "Open Devbase"} {
		if !panel.HasText(w) {
			t.Errorf("panel lacks %q: %q", w, panel.Texts())
		}
	}
	if panel.HasText("CleanShot X") {
		t.Error("login items have no switch to show in the panel")
	}
	if err := panel.Click("Open Devbase"); err != nil || opened != 1 {
		t.Errorf("open: %v %d", err, opened)
	}
	rect, ok := panel.Find("redis")
	if !ok {
		t.Fatal("no redis row")
	}
	// The switch is at the right end of the row.
	panel.ClickAt(290, rect.Y+rect.H/2)
	panel.Frame()
	if !slices.Equal(h.brew.calls, []string{"load redis"}) {
		t.Errorf("a switch turned on starts the service: calls = %v", h.brew.calls)
	}
}

func TestARowsMenuListsWhatCanBeDone(t *testing.T) {
	h := newHarness(t)
	h.a.providers[service.UserAgent] = launchd.UserAgents()
	path := filepath.Join(t.TempDir(), "com.example.sync.plist")
	h.agents.items[0].Path = path
	h.a.sources[service.UserAgent].items = h.agents.items
	h.click("User Agents")
	if err := h.tt.RightClick("com.example.sync"); err != nil {
		t.Fatal(err)
	}
	h.tt.Frame()
	menu := fmt.Sprint(h.tt.Menu())
	for _, want := range []string{"Start", "Unload", "Edit…", "Reveal in Finder", "Copy Path", "Remove…"} {
		if !strings.Contains(menu, want) {
			t.Errorf("the menu lacks %q: %s", want, menu)
		}
	}
	if err := h.tt.ChooseMenuItem("Copy Path"); err != nil {
		t.Fatal(err)
	}
	h.tt.Frame()
	if !slices.Equal(h.copied, []string{path}) {
		t.Errorf("copied = %v", h.copied)
	}
}

func TestSwitchAction(t *testing.T) {
	for _, c := range []struct {
		actions []service.Action
		on      bool
		want    service.Action
	}{
		{[]service.Action{service.Start}, true, service.Start},
		{[]service.Action{service.Load}, true, service.Load},
		{[]service.Action{service.Stop, service.Restart}, true, service.Restart}, // a service in error
		{[]service.Action{service.Stop, service.Restart}, false, service.Stop},
		{[]service.Action{service.Start, service.Unload}, false, service.Unload}, // a scheduled job
		{nil, true, ""},
	} {
		if got := switchAction(c.actions, c.on); got != c.want {
			t.Errorf("switchAction(%v, %v) = %q, want %q", c.actions, c.on, got, c.want)
		}
	}
}

// macOS tints a template image by its alpha, at the height of the menu bar.
func TestMenubarIconIsATemplate(t *testing.T) {
	img, err := png.Decode(bytes.NewReader(menubarIcon))
	if err != nil {
		t.Fatal(err)
	}
	if b := img.Bounds(); b.Dx() != 36 || b.Dy() != 36 {
		t.Errorf("size %v, want 36×36", b.Size())
	}
	drawn := 0
	for y := range img.Bounds().Dy() {
		for x := range img.Bounds().Dx() {
			r, g, b, a := img.At(x, y).RGBA()
			if r|g|b != 0 {
				t.Fatalf("(%d, %d) is not black", x, y)
			}
			if a > 0 {
				drawn++
			}
		}
	}
	if drawn == 0 {
		t.Error("the icon is empty")
	}
}

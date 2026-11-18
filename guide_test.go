package main

import (
	"image"
	"slices"
	"testing"
	"time"

	"github.com/egoist/mygo/ui"

	"devbase/internal/service"
	"devbase/internal/system"
)

// restrictedLogin is a login source that macOS lets read only in part.
type restrictedLogin struct {
	*loginFake
	limit  *service.Restriction
	needs  bool
	lists  int
	opened int
	reads  int
}

func (r *restrictedLogin) List() ([]service.Item, error) {
	r.lists++
	return r.loginFake.List()
}

func (r *restrictedLogin) Restriction() *service.Restriction { return r.limit }

func (r *restrictedLogin) NeedsAccess() bool { return r.needs }

func (r *restrictedLogin) OpenSettings() error {
	r.opened++
	return nil
}

func (r *restrictedLogin) ReadOnce() error {
	r.reads++
	r.limit = nil
	return nil
}

// restrict makes the harness's login source one that macOS restricts, and
// counts how the guide's window is shown, closed and moved.
func (h *harness) restrict() (login *restrictedLogin, shown, hidden, joined *int) {
	login = &restrictedLogin{loginFake: h.login, needs: true, limit: &service.Restriction{Text: "Helpers are not listed", Settings: true}}
	h.a.providers[service.LoginItem] = login
	shown, hidden, joined = new(int), new(int), new(int)
	h.a.showGuide = func() { *shown++ }
	h.a.hideGuide = func() { *hidden++ }
	h.a.joinGuide = func() { *joined++ }
	return login, shown, hidden, joined
}

func TestARestrictedSourceOffersWaysToListTheRest(t *testing.T) {
	h := newHarness(t)
	login, shown, _, _ := h.restrict()
	h.a.refresh(service.LoginItem)
	h.tt.Frame()
	h.wantNoText("Helpers are not listed")

	h.click("Login Items")
	h.wantText("Helpers are not listed", "Allow Full Disk Access…", "Read Once with Password", "CleanShot X")
	if login.reads != 0 {
		t.Errorf("read %d times before the user asked", login.reads)
	}

	h.click("Allow Full Disk Access…")
	if *shown != 1 || !h.a.guide.open {
		t.Errorf("the guide: shown %d, %+v", *shown, h.a.guide)
	}
	h.a.closeGuide()
	h.tt.Frame()
	h.click("Read Once with Password")
	if login.reads != 1 {
		t.Errorf("read %d times", login.reads)
	}
	h.wantNoText("Helpers are not listed", "Read Once with Password")
}

func TestARestrictionWithoutASettingOffersOnlyThePassword(t *testing.T) {
	h := newHarness(t)
	h.a.providers[service.LoginItem] = &restrictedLogin{loginFake: h.login, limit: &service.Restriction{Text: "Helpers are not listed"}}
	h.a.refresh(service.LoginItem)
	h.click("Login Items")
	h.wantText("Read Once with Password")
	h.wantNoText("Allow Full Disk Access…")
}

func TestTheGuideLeadsToSettingsAndClosesOnceAllowed(t *testing.T) {
	h := newHarness(t)
	login, shown, hidden, joined := h.restrict()
	h.a.appPath = "/Applications/Devbase.app"
	h.a.openGuide(service.LoginItem)
	if *shown != 1 || !h.a.guide.open || h.a.guide.src != service.LoginItem {
		t.Fatalf("shown %d, %+v", *shown, h.a.guide)
	}
	// What may ask for a permission waits for the user's answer to the guide.
	h.a.refreshAll()
	if login.lists != 0 {
		t.Errorf("listed %d times with the guide open", login.lists)
	}
	page := ui.NewTester(h.a.guideView, guideWidth, guideHeight)
	page.Frame()
	for _, want := range []string{"Allow Full Disk Access", "Allow…", "Close"} {
		if !page.HasText(want) {
			t.Errorf("missing %q in %q", want, page.Texts())
		}
	}

	if err := page.Click("Allow…"); err != nil {
		t.Fatal(err)
	}
	if login.opened != 1 || *joined != 1 || !h.a.guide.joined {
		t.Errorf("settings opened %d times, joined %d, %+v", login.opened, *joined, h.a.guide)
	}
	page.SetSize(sidecarWidth, sidecarHeight)
	page.Frame()
	if !h.a.guide.open {
		t.Error("the guide stays open for the drag")
	}
	for _, want := range []string{"Drag into the list", "Open Settings", "Close"} {
		if !page.HasText(want) {
			t.Errorf("missing %q in %q", want, page.Texts())
		}
	}
	if _, ok := page.Find("Drag Devbase into the list"); !ok {
		t.Errorf("no icon to drag in %q", page.Texts())
	}

	// System Settings shows.
	h.a.guide.settingsOpen = true
	page.Frame()
	if !page.HasText("Waiting for access") || page.HasText("Open Settings") {
		t.Errorf("with settings open: %q", page.Texts())
	}

	// macOS allows it while the page is open: the page says so, and closes.
	login.needs = false
	h.a.guide.checked = time.Time{}
	page.Frame()
	page.Frame()
	if !page.HasText("Full Disk Access is on") || !h.a.guide.open {
		t.Errorf("once allowed: %q, open %v", page.Texts(), h.a.guide.open)
	}
	h.a.guide.allowedAt = time.Now().Add(-2 * guideDoneAfter)
	page.Frame()
	// Closing reads the source it held back.
	if h.a.guide.open || *hidden != 1 || login.lists != 1 {
		t.Errorf("closing: open %v, hidden %d, lists %d", h.a.guide.open, *hidden, login.lists)
	}
}

func TestTheIconDragsAsTheAppsBundle(t *testing.T) {
	h := newHarness(t)
	h.a.appPath = "/Applications/Devbase.app"
	data, err := h.a.guideDragData()
	if err != nil {
		t.Fatal(err)
	}
	if files, err := data.Files(); err != nil || !slices.Equal(files, []string{h.a.appPath}) {
		t.Errorf("files = %v, %v", files, err)
	}
	if p := dragPreview(); p == nil || p.Bounds().Dx() != 72 {
		t.Errorf("preview = %v", p)
	}
	if guideIcon() == nil {
		t.Error("no icon")
	}

	// A build outside a bundle has nothing to drag.
	h.restrict()
	h.a.openGuide(service.LoginItem)
	h.a.guide.joined = true
	h.a.appPath = ""
	page := ui.NewTester(h.a.guideView, sidecarWidth, sidecarHeight)
	page.Frame()
	if _, ok := page.Find("Drag Devbase into the list"); ok || !page.HasText("Add this app with the + button") {
		t.Errorf("without a bundle: %q", page.Texts())
	}
}

func TestTheOverlayShowsTheWayThenTheWindowToDropOn(t *testing.T) {
	h := newHarness(t)
	h.a.guide = guideState{open: true, joined: true, src: service.LoginItem, settingsOpen: true, settings: system.Rect{X: 100, Y: 100, Width: 700, Height: 600}}
	page := ui.NewTester(h.a.overlayView, 1400, 900)
	page.Frame()
	if !page.HasText("Drop here") {
		t.Errorf("before the drag: %q", page.Texts())
	}
	h.a.guide.dragging, h.a.guide.dragged = true, true
	page.Frame()
	page.Advance(time.Second)
	page.Frame()
	if !page.HasText("Drop it in the list to allow access") || page.HasText("Drop here") {
		t.Errorf("during the drag: %q", page.Texts())
	}
	h.a.guide.settingsOpen = false
	page.Frame()
	if page.HasText("Drop it in the list to allow access") {
		t.Errorf("without the window: %q", page.Texts())
	}
}

func TestAFlightArcsToTheDropAndStops(t *testing.T) {
	from, to := image.Pt(1000, 200), image.Pt(400, 500)
	start := flightAt(from, to, 0)
	if !start.flying || start.x != 1000 || start.y != 200 || start.opacity != 0 {
		t.Errorf("start = %+v", start)
	}
	mid := flightAt(from, to, flightTime/2)
	if mid.y >= 350 {
		t.Errorf("the middle of the way is lifted: %+v", mid)
	}
	if mid.opacity < 0.9 || mid.scale >= 1 || mid.scale <= 0.8 {
		t.Errorf("mid = %+v", mid)
	}
	landed := flightAt(from, to, flightTime+rippleTime/2)
	if landed.flying || landed.ripple <= 0 || landed.ripple >= 1 || landed.x != 400 {
		t.Errorf("landed = %+v", landed)
	}
	if again := flightAt(from, to, flightTime+flightPause); !again.flying {
		t.Errorf("a next flight: %+v", again)
	}
	if end := flightAt(from, to, flightsShown*(flightTime+flightPause)); !end.done || end.flying {
		t.Errorf("end = %+v", end)
	}
}

func TestSidecarOrigin(t *testing.T) {
	area := system.Rect{X: 0, Y: 30, Width: 1920, Height: 1000}
	for _, test := range []struct {
		name string
		s    system.Rect
		x, y int
	}{
		{"at its right", system.Rect{X: 500, Y: 200, Width: 700, Height: 600}, 1214, 200},
		{"at its left", system.Rect{X: 1000, Y: 200, Width: 700, Height: 600}, 698, 200},
		{"over its corner", system.Rect{X: 100, Y: 200, Width: 1700, Height: 600}, 1618, 200},
		{"on the screen, above", system.Rect{X: 500, Y: -40, Width: 700, Height: 600}, 1214, 30},
		{"on the screen, below", system.Rect{X: 500, Y: 900, Width: 700, Height: 600}, 1214, 700},
	} {
		if x, y := sidecarOrigin(test.s, area, sidecarWidth, sidecarHeight); x != test.x || y != test.y {
			t.Errorf("%s: (%d, %d), want (%d, %d)", test.name, x, y, test.x, test.y)
		}
	}
}

func TestBundleOf(t *testing.T) {
	for exe, want := range map[string]string{
		"/Applications/Devbase.app/Contents/MacOS/Devbase": "/Applications/Devbase.app",
		"/Users/me/go/bin/msm":                             "",
		"/tmp/x.app/msm":                                   "",
	} {
		if got := bundleOf(exe); got != want {
			t.Errorf("bundleOf(%q) = %q, want %q", exe, got, want)
		}
	}
}

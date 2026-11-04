package main

import (
	"image"
	"image/color"
	"image/png"
	"math"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/egoist/mygo/ui"

	"macos-service-manager/internal/service"
)

// TestReadmeShots draws the README's pictures from made-up items, so that no
// machine's own jobs or paths end up in them:
//
//	MSM_README_SHOTS=docs/images go test -run TestReadmeShots .
func TestReadmeShots(t *testing.T) {
	dir := os.Getenv("MSM_README_SHOTS")
	if dir == "" {
		t.Skip("set MSM_README_SHOTS to a directory to write the README's pictures")
	}
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	for theme, dark := range map[string]bool{"light": false, "dark": true} {
		t.Run(theme, func(t *testing.T) {
			save := func(tt *ui.Tester, name string, controls bool) {
				t.Helper()
				tt.SetDark(dark)
				tt.SetScale(readmeScale)
				tt.Frame()
				f, err := os.Create(filepath.Join(dir, name+"-"+theme+".png"))
				if err != nil {
					t.Fatal(err)
				}
				defer f.Close()
				enc := png.Encoder{CompressionLevel: png.BestCompression}
				if err := enc.Encode(f, framed(tt.Image(), dark, controls)); err != nil {
					t.Fatal(err)
				}
			}

			h := readmeHarness(t)
			h.tt.SetSize(1040, 720)
			h.tt.SetDark(dark)
			h.tt.SetScale(readmeScale)
			h.tt.Frame()
			h.click("postgresql@17")
			save(h.tt, "window", true)

			h.click("Daemons")
			h.click("New Job…")
			e := &h.a.editor
			e.label, e.args = "com.example.nightly-backup", "/usr/local/bin/backup --all >> /var/log/backup.log 2>&1"
			e.when, e.minute, e.hour, e.weekday = whenCalendar, "0", "3", "1-5"
			save(h.tt, "new-job", true)
			h.click("Cancel")

			save(ui.NewTester(func(c *ui.Context) { h.a.menubarView(c, func() {}) }, 320, h.a.panelHeight()), "menu-bar", false)

			h.a.appPath = "/Applications/Service Manager.app"
			h.a.guide = guideState{open: true, src: service.LoginItem, welcome: true, checked: time.Now()}
			save(ui.NewTester(h.a.guideView, guideWidth, guideHeight), "welcome", false)
			h.a.guide = guideState{}
		})
	}
}

// readmeHarness is the app with a plausible set of items in every source.
func readmeHarness(t *testing.T) *harness {
	t.Helper()
	h := newHarness(t)
	h.a.toasts = nil
	const agents = "/Users/me/Library/LaunchAgents/"
	on, off := service.AutostartOn, service.AutostartOff
	h.brew.items = []service.Item{
		{ID: "brew:nginx", Source: service.Brew, Name: "nginx", State: service.StateFailed, Autostart: off},
		{ID: "brew:php", Source: service.Brew, Name: "php", State: service.StateRunning, PID: 407, Autostart: on, Admin: true, Owner: "root", Path: "/Library/LaunchDaemons/homebrew.mxcl.php.plist"},
		{ID: "brew:postgresql@17", Source: service.Brew, Name: "postgresql@17", State: service.StateRunning, PID: 4242, Autostart: on, Owner: "me", Path: agents + "homebrew.mxcl.postgresql@17.plist"},
		{ID: "brew:redis", Source: service.Brew, Name: "redis", State: service.StateStopped, Autostart: off},
		{ID: "brew:xray", Source: service.Brew, Name: "xray", State: service.StateScheduled, Autostart: on},
	}
	h.brew.detail = service.Detail{Rows: []service.Row{
		{Group: "Status", Key: "Process ID", Value: "4242"},
		{Group: "Status", Key: "Runs as", Value: "me"},
		{Group: "Status", Key: "Service name", Value: "homebrew.mxcl.postgresql@17"},
		{Group: "Program", Key: "Command", Value: "/opt/homebrew/opt/postgresql@17/bin/postgres -D /opt/homebrew/var/postgresql@17"},
		{Group: "Program", Key: "Working directory", Value: "/opt/homebrew"},
		{Group: "Files", Key: "Property list", Value: agents + "homebrew.mxcl.postgresql@17.plist"},
		{Group: "Files", Key: "Output log", Value: "/opt/homebrew/var/log/postgresql@17.log"},
	}, Logs: []service.LogFile{{Label: "Output", Path: filepath.Join(t.TempDir(), "postgresql@17.log")}}}
	h.login.items = []service.Item{
		{ID: "login-item:1", Source: service.LoginItem, Name: "Rectangle", Owner: "Rectangle", Kind: "App", State: service.StateRunning, PID: 1368, Path: "/Applications/Rectangle.app", Autostart: on},
		{ID: "login-item:2", Source: service.LoginItem, Name: "Mac Mouse Fix Helper", Owner: "Mac Mouse Fix", Kind: "Background helper", State: service.StateRunning, PID: 2173, Path: "/Applications/Mac Mouse Fix.app/Contents/Library/LoginItems/Mac Mouse Fix Helper.app", Autostart: on},
	}
	h.agents.items = []service.Item{
		{ID: "user-agent:a", Source: service.UserAgent, Name: "com.example.backup", State: service.StateScheduled, Autostart: on, Path: agents + "com.example.backup.plist"},
		{ID: "user-agent:b", Source: service.UserAgent, Name: "com.google.GoogleUpdater.wake", State: service.StateStopped, Autostart: off, Path: agents + "com.google.GoogleUpdater.wake.plist"},
		{ID: "user-agent:c", Source: service.UserAgent, Name: "io.mutagen.mutagen", State: service.StateRunning, PID: 47313, Autostart: on, Path: agents + "io.mutagen.mutagen.plist"},
	}
	h.a.providers[service.SystemAgent] = &fake{src: service.SystemAgent, items: []service.Item{
		{ID: "system-agent:a", Source: service.SystemAgent, Name: "com.google.keystone.agent", State: service.StateLoaded, Autostart: on, Path: "/Library/LaunchAgents/com.google.keystone.agent.plist"},
	}}
	h.a.providers[service.Daemon] = &fake{src: service.Daemon, items: []service.Item{
		{ID: "daemon:a", Source: service.Daemon, Name: "com.docker.vmnetd", State: service.StateRunning, PID: 512, Autostart: on, Admin: true, Path: "/Library/LaunchDaemons/com.docker.vmnetd.plist"},
		{ID: "daemon:b", Source: service.Daemon, Name: "org.wireshark.ChmodBPF", State: service.StateLoaded, Autostart: on, Admin: true, Path: "/Library/LaunchDaemons/org.wireshark.ChmodBPF.plist"},
	}}
	h.a.refreshAll()
	return h
}

const readmeScale = 2

// framed sets a picture of a window on a transparent ground, with the corners,
// edge and shadow that macOS gives a window, and the controls that it alone draws.
func framed(src *image.RGBA, dark, controls bool) *image.NRGBA {
	const scale = float64(readmeScale)
	margin := int(30 * scale)
	radius, drop, blur := 12*scale, 8*scale, 28*scale
	shadow, edge := 0.16, color.NRGBA{0, 0, 0, 34}
	if dark {
		shadow, edge = 0.34, color.NRGBA{255, 255, 255, 34}
	}
	b := src.Bounds()
	w, h := float64(b.Dx()), float64(b.Dy())
	out := image.NewNRGBA(image.Rect(0, 0, b.Dx()+2*margin, b.Dy()+2*margin))
	// Red, yellow and green, at the place they have under a title bar this high.
	lights := []struct {
		x   float64
		col [3]float64
	}{{20, [3]float64{255, 95, 87}}, {40, [3]float64{254, 188, 46}}, {60, [3]float64{40, 200, 64}}}
	for y := range out.Bounds().Dy() {
		for x := range out.Bounds().Dx() {
			px, py := float64(x-margin)+.5, float64(y-margin)+.5
			d := roundedDistance(px, py, w, h, radius)
			cover := clamp01(.5 - d)
			dim := 1 - clamp01(roundedDistance(px, py-drop, w, h, radius)/blur)
			under := shadow * dim * dim
			alpha := cover + under*(1-cover)
			if alpha <= 0 {
				continue
			}
			var rgb [3]float64
			if cover > 0 {
				c := src.RGBAAt(min(max(x-margin, 0), b.Dx()-1), min(max(y-margin, 0), b.Dy()-1))
				rgb = [3]float64{float64(c.R), float64(c.G), float64(c.B)}
				if controls {
					for _, l := range lights {
						mix(&rgb, l.col, clamp01(6*scale+.5-math.Hypot(px-l.x*scale, py-float64(barHeight)/2*scale)))
					}
				}
				mix(&rgb, [3]float64{float64(edge.R), float64(edge.G), float64(edge.B)}, float64(edge.A)/255*clamp01(d+scale+.5))
			}
			k := cover / alpha
			out.SetNRGBA(x, y, color.NRGBA{uint8(rgb[0]*k + .5), uint8(rgb[1]*k + .5), uint8(rgb[2]*k + .5), uint8(alpha*255 + .5)})
		}
	}
	return out
}

// roundedDistance is how far a point is outside a rounded rectangle at the
// origin, negative inside it.
func roundedDistance(x, y, w, h, r float64) float64 {
	dx, dy := math.Abs(x-w/2)-(w/2-r), math.Abs(y-h/2)-(h/2-r)
	return math.Hypot(max(dx, 0), max(dy, 0)) + min(max(dx, dy), 0) - r
}

func clamp01(v float64) float64 { return min(max(v, 0), 1) }

func mix(rgb *[3]float64, to [3]float64, by float64) {
	for i := range rgb {
		rgb[i] += (to[i] - rgb[i]) * by
	}
}

package main

import (
	"context"
	"image/color"
	"testing"
	"time"

	"github.com/egoist/mygo"
	"github.com/egoist/mygo/ui"

	"devbase/internal/devplatform"
)

func TestMainWindowVibrancy(t *testing.T) {
	if got := mainWindowVibrancy(false); got != mygo.VibrancyGlass {
		t.Fatalf("mainWindowVibrancy(false) = %q, want %q", got, mygo.VibrancyGlass)
	}
	if got := mainWindowVibrancy(true); got != mygo.VibrancyNone {
		t.Fatalf("mainWindowVibrancy(true) = %q, want %q", got, mygo.VibrancyNone)
	}
}

func TestGlassToneOpaqueSurfaces(t *testing.T) {
	for _, tc := range []struct {
		name string
		dark bool
	}{
		{name: "light", dark: false},
		{name: "dark", dark: true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			g := glassOf(&ui.Theme{Dark: tc.dark})
			for _, surface := range []struct {
				field string
				color ui.Color
			}{
				{"sidebarSolid", g.sidebarSolid},
				{"pane", g.pane},
				{"sheet", g.sheet},
				{"panel", g.panel},
			} {
				if surface.color.A != 255 {
					t.Fatalf("%s (%s) alpha = %d, want 255", tc.name, surface.field, surface.color.A)
				}
			}
		})
	}
}

func TestSidebarGlassRendering(t *testing.T) {
	t.Setenv("MSM_DEV_HOME", t.TempDir())

	views := []struct {
		name  string
		setup func(a *app)
	}{
		{
			name: "dev-software",
			setup: func(a *app) {
				a.nav = navDevelopment
				a.dev.tab = devTabSoftware
				a.dev.software = "node"
			},
		},
		{
			name: "dev-https",
			setup: func(a *app) {
				a.nav = navDevelopment
				a.dev.tab = devTabHTTPS
			},
		},
		{
			name: "settings",
			setup: func(a *app) {
				a.nav = navSettings
			},
		},
		{
			name: "inspector",
			setup: func(a *app) {
				a.nav = navAll
			},
		},
	}

	themes := []struct {
		name string
		dark bool
	}{
		{name: "light", dark: false},
		{name: "dark", dark: true},
	}

	for _, th := range themes {
		for _, vw := range views {
			t.Run(th.name+"/"+vw.name, func(t *testing.T) {
				root := t.TempDir()
				home := t.TempDir()
				h := newHarness(t)
				h.a.dev.loaded = true
				h.a.dev.statusLast = time.Now()
				h.a.dev.nodeReleasesTried = true
				h.a.dev.authorityStatus = &devplatform.AuthorityStatus{}
				h.a.settingsLoaded = true
				h.a.dev.managerFactory = func() *devplatform.Manager {
					m := devplatform.New(root)
					m.Home = home
					m.Run = func(context.Context, string, ...string) (string, error) { return "", nil }
					return m
				}
				vw.setup(h.a)
				h.tt.SetDark(th.dark)

				wantSolid := glassOf(&ui.Theme{Dark: th.dark}).sidebarSolid
				wantRGB := color.RGBA{R: wantSolid.R, G: wantSolid.G, B: wantSolid.B, A: 255}

				// Active Liquid Glass mode: sidebar blank region is transparent (A == 0),
				// while the entire right-hand workspace is opaque (A == 255).
				h.a.fullScreen = false
				h.tt.SetPreferences(ui.Preferences{})
				h.tt.SetVibrancy(true)
				h.tt.Frame()
				img := h.tt.Image()
				if px := img.RGBAAt(100, 20); px.A != 0 {
					t.Fatalf("glass active sidebar (100,20) alpha = %d, want 0 (rgba=%+v)", px.A, px)
				}
				for _, pt := range [][2]int{{400, 20}, {210, 20}, {210, 300}, {700, 300}, {400, 660}} {
					if px := img.RGBAAt(pt[0], pt[1]); px.A != 255 {
						t.Fatalf("right workspace (%d,%d) alpha = %d, want 255 (rgba=%+v)", pt[0], pt[1], px.A, px)
					}
				}

				// Fullscreen fallback: sidebar paints solid opaque fill.
				h.a.fullScreen = true
				h.tt.Frame()
				img = h.tt.Image()
				if px := img.RGBAAt(100, 20); px != wantRGB {
					t.Fatalf("fullscreen sidebar (100,20) = %+v, want %+v", px, wantRGB)
				}

				// Reduce Transparency fallback: sidebar paints solid opaque fill.
				h.a.fullScreen = false
				h.tt.SetPreferences(ui.Preferences{ReduceTransparency: true})
				h.tt.Frame()
				img = h.tt.Image()
				if px := img.RGBAAt(100, 20); px != wantRGB {
					t.Fatalf("reduce-transparency sidebar (100,20) = %+v, want %+v", px, wantRGB)
				}

				// Non-vibrant surface fallback: sidebar paints solid opaque fill.
				h.tt.SetPreferences(ui.Preferences{})
				h.tt.SetVibrancy(false)
				h.tt.Frame()
				img = h.tt.Image()
				if px := img.RGBAAt(100, 20); px != wantRGB {
					t.Fatalf("non-vibrant sidebar (100,20) = %+v, want %+v", px, wantRGB)
				}
			})
		}
	}
}

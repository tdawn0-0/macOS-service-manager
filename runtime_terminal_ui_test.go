package main

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/egoist/mygo/ui"
)

func TestUIRuntimeTerminal(t *testing.T) {
	t.Run("states", func(t *testing.T) {
		a := designFixture(t)
		a.dev.tab = devTabSoftware
		a.dev.software = "node"
		copied := ""
		a.copy = func(value string) { copied = value }
		tt := ui.NewTester(a.view, 1120, 720)
		tt.Frame()
		shown := strings.Join(tt.Texts(), "\n")
		for _, label := range []string{"Installed", "Default", "Terminal", "Copy executable path", "Enable in terminal"} {
			if !strings.Contains(shown, label) {
				t.Fatalf("missing %s: %s", label, shown)
			}
		}
		if strings.Contains(shown, "PHP") {
			t.Fatal("PHP control remains")
		}
		if err := tt.Click("Copy executable path"); err != nil {
			t.Fatal(err)
		}
		tt.Frame()
		if copied != "" || !strings.Contains(a.dev.output, "unavailable") {
			t.Fatalf("invalid executable was copied: %q %q", copied, a.dev.output)
		}
		// Make a real arm64 executable available at the managed package path.
		p := a.dev.state.Packages["node@22.14.0-arm64"]
		p.Binaries = map[string]string{"node": "bin/node"}
		a.dev.state.Packages[p.ID()] = p
		path := filepath.Join(a.devManager().PackageDirectory(p), "bin", "node")
		if err := os.MkdirAll(filepath.Dir(path), 0700); err != nil {
			t.Fatal(err)
		}
		data, err := os.ReadFile("/bin/echo")
		if err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, data, 0700); err != nil {
			t.Fatal(err)
		}
		if err := tt.Click("Copy executable path"); err != nil {
			t.Fatal(err)
		}
		if copied != path {
			t.Fatalf("copy = %q, want %q; UI: %s", copied, path, a.dev.output)
		}
	})
	t.Run("layouts", func(t *testing.T) {
		for _, tc := range []struct {
			dark          bool
			width, height int
		}{{false, 900, 520}, {true, 900, 520}, {false, 1120, 720}, {true, 1120, 720}} {
			a := designFixture(t)
			a.dev.tab = devTabSoftware
			a.dev.software = "node"
			a.dev.output = "A long install path: /very/long/example/with/multiple/nested/directories/node@22.14.0-arm64/bin/node"
			tt := ui.NewTester(a.view, tc.width, tc.height)
			tt.SetDark(tc.dark)
			tt.Frame()
			if _, ok := tt.Find("Copy executable path"); !ok {
				t.Fatal("copy action not accessible")
			}
			if tc.height == 520 {
				tt.Scroll(float32(tc.width-20), float32(tc.height/2), 0, 420)
				tt.Frame()
			}
			if rect, ok := tt.Find("Enable in terminal"); !ok || rect.Y < 50 || rect.Y+rect.H > float32(tc.height-45) {
				t.Fatal("terminal action not reachable after scrolling")
			}
			saveShot(t, fmt.Sprintf("%s-%dx%d", map[bool]string{false: "light", true: "dark"}[tc.dark], tc.width, tc.height), tt.Image())
		}
	})
}

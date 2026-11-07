package main

import (
	"fmt"
	"github.com/egoist/mygo/ui"
	"image/png"
	"os"
	"path/filepath"
	"reflect"
	"testing"
	"time"
)

func navigationFixture(t *testing.T, width, height int, dark bool) (*app, *ui.Tester) {
	t.Helper()
	a := designFixture(t)
	a.dev.statusLast = time.Now().Add(time.Hour)
	a.dev.tab = 0
	a.dev.projectTab = 1
	a.spawn = func(func()) { t.Fatal("navigation spawned backend work") }
	tt := ui.NewTester(a.view, width, height)
	tt.SetDark(dark)
	tt.Frame()
	return a, tt
}
func navigationClick(t *testing.T, tt *ui.Tester, label string) {
	t.Helper()
	if e := tt.Click(label); e != nil {
		t.Fatal(e)
	}
	tt.Frame()
}
func TestProjectNavigationKeepsCurrentDraft(t *testing.T) {
	for _, size := range [][2]int{{900, 520}, {1120, 720}} {
		for _, dark := range []bool{false, true} {
			t.Run(fmt.Sprintf("%dx%d-dark-%t", size[0], size[1], dark), func(t *testing.T) {
				a, tt := navigationFixture(t, size[0], size[1], dark)
				saved := a.dev.state.Projects[a.dev.id].Clone()
				a.dev.draft.Name = "Uncommitted"
				a.dev.arguments = "node unsaved.js"
				a.dev.environment = "MODE=unsaved"
				tt.Frame()
				navigationClick(t, tt, "Project Atlas API")
				if a.dev.draft.Name != "Uncommitted" || a.dev.arguments != "node unsaved.js" || a.dev.environment != "MODE=unsaved" || a.dev.projectTab != 1 {
					t.Fatal("current row discarded editor state")
				}
				if !reflect.DeepEqual(a.dev.state.Projects[a.dev.id], saved) {
					t.Fatal("navigation saved draft")
				}
			})
		}
	}
}

func TestProjectNavigationConfirmsDiscard(t *testing.T) {
	for _, size := range [][2]int{{900, 520}, {1120, 720}} {
		for _, dark := range []bool{false, true} {
			for _, decision := range []string{"Cancel", "Escape", "Discard changes"} {
				t.Run(fmt.Sprintf("%dx%d-dark-%t-%s", size[0], size[1], dark, decision), func(t *testing.T) {
					a, tt := navigationFixture(t, size[0], size[1], dark)
					saved := a.dev.state.Projects["atlas"].Clone()
					a.dev.arguments = "node unsaved.js"
					a.dev.environment = "MODE=unsaved"
					tt.Frame()
					navigationClick(t, tt, "Project Storefront")
					if !a.ask.open || a.dev.id != "atlas" {
						t.Fatal("dirty navigation skipped confirmation")
					}
					if dir := os.Getenv("MSM_NAV_SHOTS"); dir != "" && decision == "Discard changes" && size[0] == 900 {
						if e := os.MkdirAll(dir, 0700); e != nil {
							t.Fatal(e)
						}
						f, e := os.Create(filepath.Join(dir, fmt.Sprintf("navigation-dark-%t.png", dark)))
						if e != nil {
							t.Fatal(e)
						}
						e = png.Encode(f, tt.Image())
						ce := f.Close()
						if e != nil || ce != nil {
							t.Fatal(e, ce)
						}
					}
					if _, ok := tt.Find("Discard changes"); !ok {
						t.Fatal("discard control unavailable")
					}
					if _, ok := tt.Find("Cancel"); !ok {
						t.Fatal("cancel control unavailable")
					}
					if decision == "Escape" {
						tt.Key(0, ui.KeyEscape)
						tt.Frame()
					} else {
						navigationClick(t, tt, decision)
					}
					if decision == "Discard changes" {
						if a.dev.id != "storefront" {
							t.Fatal("confirmation failed to navigate")
						}
					} else {
						if a.dev.id != "atlas" || a.dev.arguments != "node unsaved.js" || a.dev.environment != "MODE=unsaved" || a.ask.open {
							t.Fatal("cancel discarded editor state or kept dialog open")
						}
					}
					if !reflect.DeepEqual(saved, a.dev.state.Projects["atlas"]) {
						t.Fatal("navigation mutated saved state")
					}
				})
			}
		}
	}
}
func TestProjectNavigationStaleTarget(t *testing.T) {
	for _, change := range []string{"deleted", "source-changed", "target-updated", "busy", "clean", "long-name"} {
		t.Run(change, func(t *testing.T) {
			a, tt := navigationFixture(t, 900, 520, true)
			if change == "clean" {
				a.requestProjectSelection("storefront")
				if a.ask.open || a.dev.id != "storefront" {
					t.Fatal("clean switch prompted")
				}
				return
			}
			a.dev.draft.Name = "Unsaved"
			target := "storefront"
			if change == "long-name" {
				target = "unicode"
			}
			a.requestProjectSelection(target)
			tt.Frame()
			if !a.ask.open {
				t.Fatal("missing confirmation")
			}
			callback := a.ask.run
			switch change {
			case "deleted":
				delete(a.dev.state.Projects, target)
			case "source-changed":
				a.dev.id = "unicode"
			case "target-updated":
				p := a.dev.state.Projects[target]
				p.Name = "New saved name"
				a.dev.state.Projects[target] = p
			case "busy":
				a.dev.busy = true
			}
			if change == "long-name" {
				navigationClick(t, tt, "Discard changes")
				if a.dev.id != "unicode" {
					t.Fatal("long project name blocked controls")
				}
				return
			}
			callback()
			if change == "target-updated" {
				if a.dev.id != target || a.dev.draft.Name != "New saved name" {
					t.Fatal("used stale saved target")
				}
			} else if a.dev.draft.Name != "Unsaved" || a.dev.id == "storefront" {
				t.Fatal("stale callback discarded draft")
			}
		})
	}
}

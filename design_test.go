package main

import (
	"strings"
	"testing"

	"github.com/egoist/mygo/ui"
)

// The workspace's sections are items of the sidebar: one level of navigation,
// with no second row of tabs under the title.
func TestDevelopmentSectionsAreSidebarItems(t *testing.T) {
	a := designFixture(t)
	tt := ui.NewTester(a.view, 1120, 720)
	tt.Frame()
	for tab, name := range devSections {
		if err := tt.Click(name); err != nil {
			t.Fatalf("click %s: %v", name, err)
		}
		tt.Frame()
		if a.nav != navDevelopment || a.dev.tab != tab {
			t.Fatalf("%s opened nav %q tab %d", name, a.nav, a.dev.tab)
		}
	}
	if err := tt.Click("Homebrew"); err != nil {
		t.Fatal(err)
	}
	tt.Frame()
	if a.nav == navDevelopment {
		t.Fatal("a source left the workspace open")
	}
	if err := tt.Click("Software"); err != nil {
		t.Fatal(err)
	}
	tt.Frame()
	if a.nav != navDevelopment || a.dev.tab != 0 {
		t.Fatalf("back in the workspace: nav %q tab %d", a.nav, a.dev.tab)
	}
}

// Screens say what they hold and nothing about themselves: no paragraphs of
// help. What a control needs said waits in its tooltip.
func TestDevelopmentScreensCarryNoParagraphs(t *testing.T) {
	for tab, name := range devSections {
		a := designFixture(t)
		a.dev.tab = tab
		tt := ui.NewTester(a.view, 1120, 1400)
		tt.Frame()
		for _, line := range tt.Texts() {
			if len(line) > 140 && !strings.Contains(line, "/") && !strings.Contains(line, "\n") {
				t.Errorf("%s shows a paragraph: %q", name, line)
			}
		}
	}
}

// The actions of the title bar are icons, and an icon is named for assistive
// technology and for agents by the label it keeps.
func TestDevelopmentBarActionsKeepTheirNames(t *testing.T) {
	a := designFixture(t)
	tt := ui.NewTester(a.view, 900, 520)
	tt.Frame()
	for _, name := range []string{"Refresh", "Tasks", "Diagnostics"} {
		r, ok := tt.Find(name)
		if !ok || r.W <= 0 || r.H <= 0 {
			t.Errorf("%q is not in the bar: %v", name, tt.Texts())
		}
	}
}

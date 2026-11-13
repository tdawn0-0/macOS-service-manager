package main

import (
	"context"
	"encoding/json"
	"errors"
	"github.com/egoist/mygo/ui"
	"image/png"
	"devbase/internal/devplatform"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestStatusPollCannotOverwriteAnOperationOrUnsavedDraft(t *testing.T) {
	a := designFixture(t)
	a.dev.proxyDraft = []proxyRuleDraft{{Domain: "unsaved.test", LocalPort: "3000"}}
	a.dev.proxyDraftLoaded = true
	a.dev.statusLast = time.Now().Add(-time.Minute)
	var work []func()
	a.spawn = func(f func()) { work = append(work, f) }
	a.post = func(f func()) { f() }
	a.dev.managerFactory = func() *devplatform.Manager {
		m := devplatform.New(t.TempDir())
		m.Run = func(context.Context, string, ...string) (string, error) { return "", errors.New("not found") }
		return m
	}
	a.pollDevStatus()
	if len(work) != 1 || !a.dev.statusPolling {
		t.Fatal("poll did not start")
	}
	// A user action started and completed after the observation was queued.
	a.dev.taskSequence++
	a.dev.statuses["instance:atlas-postgres"] = "Starting"
	work[0]()
	if a.dev.statusPolling || a.dev.statuses["instance:atlas-postgres"] != "Starting" || a.dev.proxyDraft[0].Domain != "unsaved.test" {
		t.Fatal("stale status poll replaced an operation or draft")
	}
	a.dev.statusLast = time.Now().Add(-time.Minute)
	a.pollDevStatus()
	if len(work) != 2 {
		t.Fatal("poll did not become available again")
	}
	work[1]()
	if a.dev.statuses["instance:atlas-postgres"] != "Stopped" || a.dev.proxyDraft[0].Domain != "unsaved.test" {
		t.Fatal("fresh poll failed or replaced the form")
	}
}

func TestQuitPreviewReopenAndBusyDismissal(t *testing.T) {
	a := designFixture(t)
	var work []func()
	a.spawn = func(f func()) { work = append(work, f) }
	a.post = func(f func()) { f() }
	root := t.TempDir()
	rawState, err := json.MarshalIndent(a.dev.state, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "state.json"), rawState, 0600); err != nil {
		t.Fatal(err)
	}
	a.dev.managerFactory = func() *devplatform.Manager { return devplatform.New(root) }
	a.requestQuit()
	a.quit.open = false
	a.requestQuit()
	if len(work) != 2 {
		t.Fatal("reopening quit did not schedule a new preview")
	}
	work[0]()
	if !a.quit.loading {
		t.Fatal("old preview completed the newer request")
	}
	work[1]()
	if a.quit.loading || a.quit.plan == nil {
		t.Fatal("reopened quit preview remained stuck")
	}
	quits := 0
	a.quitApp = func() { quits++ }
	a.dev.busy = true
	tt := ui.NewTester(a.view, 900, 520)
	tt.Frame()
	if err := tt.Click("Keep services and quit"); err != nil {
		t.Fatal(err)
	}
	tt.Frame()
	if quits != 0 || a.quit.allow {
		t.Fatal("quit interrupted an active operation")
	}
	a.quit.stopping = true
	tt.Key(0, ui.KeyEscape)
	tt.Frame()
	if !a.quit.open || quits != 0 {
		t.Fatal("dismissal hid an in-progress shutdown")
	}
	a.quit.stopping = false
	a.dev.busy = false
}

func TestDevelopmentPanes(t *testing.T) {
	for tab := 0; tab < 3; tab++ {
		for _, dark := range []bool{false, true} {
			a := newAppWith(nil, nil, t.TempDir())
			a.nav = "development"
			a.dev.loaded = true
			a.dev.tab = tab
			a.dev.state = devplatform.State{SchemaVersion: devplatform.Schema, ProxyRules: []devplatform.ProxyRule{}, ProxyListenerPort: 443, Packages: map[string]devplatform.Package{}, Instances: map[string]devplatform.Instance{}}
			tt := ui.NewTester(a.view, 1120, 720)
			tt.SetDark(dark)
			tt.Frame()
			if tt.Image().Bounds().Dx() == 0 {
				t.Fatal("empty development pane")
			}
			if dir := os.Getenv("MSM_DEV_SHOTS"); dir != "" {
				os.MkdirAll(dir, 0755)
				name := []string{"software", "databases", "https"}[tab]
				if dark {
					name += "-dark"
				}
				f, e := os.Create(filepath.Join(dir, name+".png"))
				if e != nil {
					t.Fatal(e)
				}
				if e = png.Encode(f, tt.Image()); e != nil {
					t.Fatal(e)
				}
				f.Close()
			}
		}
	}
}

func TestDevelopmentHasNoJSONEditor(t *testing.T) {
	for tab := 0; tab < 3; tab++ {
		a := newAppWith(nil, nil, t.TempDir())
		a.nav = "development"
		a.dev.loaded = true
		a.dev.tab = tab
		tt := ui.NewTester(a.view, 1120, 900)
		tt.Frame()
		for _, text := range tt.Texts() {
			if strings.Contains(strings.ToLower(text), "json") {
				t.Fatalf("tab %d exposes JSON: %s", tab, text)
			}
		}
	}
}
func TestProxyDraftDoesNotMutateSavedConfiguration(t *testing.T) {
	s := devplatform.State{ProxyListenerPort: 443, ProxyRules: []devplatform.ProxyRule{{Domain: "app.test", LocalPort: 3000}}}
	d := developmentState{state: s}
	d.loadProxyDraft(s)
	d.proxyDraft[0].Domain = "changed.test"
	d.proxyListenerPort = "8443"
	if s.ProxyRules[0].Domain != "app.test" || s.ProxyListenerPort != 443 || !d.proxyDraftDirty() {
		t.Fatal("editing draft changed saved configuration")
	}
}

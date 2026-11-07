package main

import (
	"context"
	"errors"
	"github.com/egoist/mygo/ui"
	"image/png"
	"macos-service-manager/internal/devplatform"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestStatusPollCannotOverwriteAnOperationOrUnsavedDraft(t *testing.T) {
	a := designFixture(t)
	a.dev.draft.Name = "Unsaved project name"
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
	a.dev.statuses["project:atlas"] = "Starting"
	work[0]()
	if a.dev.statusPolling || a.dev.statuses["project:atlas"] != "Starting" || a.dev.draft.Name != "Unsaved project name" {
		t.Fatal("stale status poll replaced an operation or draft")
	}
	a.dev.statusLast = time.Now().Add(-time.Minute)
	a.pollDevStatus()
	if len(work) != 2 {
		t.Fatal("poll did not become available again")
	}
	work[1]()
	if a.dev.statuses["project:atlas"] != "Stopped" || a.dev.draft.Name != "Unsaved project name" {
		t.Fatal("fresh poll failed or replaced the form")
	}
}

func TestQuitPreviewReopenAndBusyDismissal(t *testing.T) {
	a := designFixture(t)
	var work []func()
	a.spawn = func(f func()) { work = append(work, f) }
	a.post = func(f func()) { f() }
	root := t.TempDir()
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
	for tab := 0; tab < 4; tab++ {
		for _, dark := range []bool{false, true} {
			a := newAppWith(nil, nil, t.TempDir())
			a.nav = "development"
			a.dev.loaded = true
			a.dev.tab = tab
			a.dev.state = devplatform.State{Projects: map[string]devplatform.Project{}, Packages: map[string]devplatform.Package{}, Instances: map[string]devplatform.Instance{}}
			tt := ui.NewTester(a.view, 1120, 720)
			tt.SetDark(dark)
			tt.Frame()
			if tt.Image().Bounds().Dx() == 0 {
				t.Fatal("empty development pane")
			}
			if dir := os.Getenv("MSM_DEV_SHOTS"); dir != "" {
				os.MkdirAll(dir, 0755)
				name := []string{"projects", "runtimes", "databases", "https"}[tab]
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
	for tab := 0; tab < 4; tab++ {
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
func TestServiceFormPreservesArgumentsAndEnvironment(t *testing.T) {
	d := developmentState{draft: devplatform.Project{ID: "demo", Services: []devplatform.Service{{ID: "web", Command: []string{"node", "old.js"}}}}}
	d.selectService(d.draft.Services[0])
	d.arguments = "node\nserver with spaces.js\n--title=hello world"
	d.environment = "NODE_ENV=development\nGREETING=hello world"
	d.health = "tcp://127.0.0.1:3000"
	if e := d.applyService(); e != nil {
		t.Fatal(e)
	}
	s := d.draft.Services[0]
	if len(s.Command) != 3 || s.Command[1] != "server with spaces.js" || s.Environment["GREETING"] != "hello world" {
		t.Fatal(s)
	}
}
func TestProjectFormDoesNotMutateSavedConfiguration(t *testing.T) {
	original := devplatform.Project{ID: "demo", Services: []devplatform.Service{{ID: "web", Command: []string{"node", "old.js"}}}, Instances: []string{"db"}, Sites: []devplatform.Site{{Proxy: &devplatform.ProxyOptions{ConnectTimeoutSeconds: 10}}}}
	d := developmentState{}
	d.selectProject(original)
	d.arguments = "node\nnew.js"
	if e := d.applyService(); e != nil {
		t.Fatal(e)
	}
	d.draft.Instances[0] = "other"
	d.draft.Sites[0].Proxy.ConnectTimeoutSeconds = 20
	if original.Services[0].Command[1] != "old.js" || original.Instances[0] != "db" || original.Sites[0].Proxy.ConnectTimeoutSeconds != 10 {
		t.Fatal("editing the draft mutated the saved project")
	}
}

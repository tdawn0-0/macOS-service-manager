package main

import (
	"context"
	"errors"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"devbase/internal/launchd"
	"devbase/internal/privhelper"
	"devbase/internal/service"
	"devbase/internal/system"
)

func TestNewLaunchAgent(t *testing.T) {
	old := system.Run
	defer func() { system.Run = old }()
	system.Run = func(string, ...string) (string, error) { return "", nil } // plutil -lint

	h := newHarness(t)
	h.click("User Agents")
	h.click("New Job…")
	h.wantText("New Job", "Label", "Command", "Save", "User Agent", "System Agent", "Daemon", "As a service")

	// Invalid: the error shows, and nothing is written.
	h.click("Save")
	h.wantText("the label may hold letters")
	if entries, _ := os.ReadDir(h.a.agentDir); len(entries) != 0 {
		t.Fatalf("an invalid agent was written: %v", entries)
	}

	if err := h.tt.Click("Label"); err != nil {
		t.Fatal(err)
	}
	h.tt.Type("com.example.job")
	if err := h.tt.Click("Command"); err != nil {
		t.Fatal(err)
	}
	h.tt.Type("sleep 1 && echo done")
	h.tt.Frame()
	h.click("Save")

	path := filepath.Join(h.a.agentDir, "com.example.job.plist")
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("the agent was not written: %v\ntexts %q", err, h.tt.Texts())
	}
	spec, err := launchd.LoadSpec(path)
	if err != nil {
		t.Fatal(err)
	}
	if spec.Label != "com.example.job" || !slices.Equal(spec.Args, []string{"/bin/sh", "-c", "sleep 1 && echo done"}) || !spec.RunAtLoad {
		t.Errorf("spec = %+v\n%s", spec, data)
	}
	if !strings.HasSuffix(spec.StdOut, "/Library/Logs/com.example.job.log") {
		t.Errorf("output goes to the default log: %q", spec.StdOut)
	}
	if h.a.editor.open {
		t.Errorf("the dialog should close; error: %q", h.a.editor.err)
	}
	if !slices.Contains(h.agents.calls, "load com.example.job") {
		t.Errorf("a new agent is loaded: %v", h.agents.calls)
	}
}

// rootJobs is a source of jobs that only root can write, which remembers them.
type rootJobs struct {
	fake
	made   []launchd.Spec
	loads  []bool
	edited []string
	dir    string
	err    error
}

func (r *rootJobs) SaveAsRoot(s launchd.Spec, existing string, load bool) (string, error) {
	r.made, r.loads, r.edited = append(r.made, s), append(r.loads, load), append(r.edited, existing)
	if r.err != nil {
		return "", r.err
	}
	return filepath.Join(r.dir, s.Label+".plist"), nil
}

func (h *harness) typeInto(label, text string) {
	h.t.Helper()
	if err := h.tt.Click(label); err != nil {
		h.t.Fatalf("click %q: %v\ntexts: %q", label, err, h.tt.Texts())
	}
	h.tt.Type(text)
	h.tt.Frame()
}

func TestANewDaemonGoesToRootWithItsUser(t *testing.T) {
	old := system.Run
	defer func() { system.Run = old }()
	system.Run = func(string, ...string) (string, error) { return "", nil }

	h := newHarness(t)
	h.tt.SetSize(1080, 1500)
	jobs := &rootJobs{fake: fake{src: service.Daemon}, dir: "/Library/LaunchDaemons"}
	h.a.providers[service.Daemon] = jobs
	h.click("Daemons")
	h.click("New Job…")
	if h.a.editor.source() != service.Daemon {
		t.Fatalf("the form opens on the list's kind: %v", h.a.editor.source())
	}
	h.wantText("Starts at boot", "Runs as", "User", "Group", "Root directory")

	h.typeInto("Label", "com.example.nightly")
	h.typeInto("Command", "/usr/local/bin/backup")
	h.typeInto("User", "_backup")
	h.click("Save")
	h.wantText("Create system job com.example.nightly?")
	h.click("Create Job")
	if len(jobs.made) != 1 || !jobs.loads[0] {
		t.Fatalf("made %+v, loads %v, error %q", jobs.made, jobs.loads, h.a.editor.err)
	}
	spec := jobs.made[0]
	if spec.UserName != "_backup" || spec.Label != "com.example.nightly" {
		t.Errorf("spec = %+v", spec)
	}
	// A log root could write, but this user may not.
	if spec.StdOut != "" {
		t.Errorf("a daemon with a user has no default log: %q", spec.StdOut)
	}
	if h.a.editor.open || h.a.nav != string(service.Daemon) {
		t.Errorf("open %v, nav %q", h.a.editor.open, h.a.nav)
	}
	if entries, _ := os.ReadDir(h.a.agentDir); len(entries) != 0 {
		t.Errorf("nothing goes to the user's agents: %v", entries)
	}

	// As root, its log goes where root logs.
	h.a.newJob()
	h.a.editor.label, h.a.editor.args = "com.example.root", "/bin/date"
	got, err := h.a.editor.spec()
	if err != nil || got.StdOut != "/Library/Logs/com.example.root.log" {
		t.Errorf("spec = %+v, %v", got, err)
	}
}

func TestASystemAgentHasNoUserAndNoDefaultLog(t *testing.T) {
	h := newHarness(t)
	jobs := &rootJobs{fake: fake{src: service.SystemAgent}, dir: "/Library/LaunchAgents"}
	h.a.providers[service.SystemAgent] = jobs
	h.click("System Agents")
	h.click("New Job…")
	h.wantText("Starts at login", "Asks for in-app confirmation before saving")
	h.wantNoText("Runs as", "Root directory", "if no log file is set", "password")
	h.a.editor.label, h.a.editor.args = "com.example.all", "/bin/date"
	got, err := h.a.editor.spec()
	if err != nil || got.StdOut != "" {
		t.Errorf("spec = %+v, %v", got, err)
	}
}

func TestAFailureToCreateARootJobKeepsTheForm(t *testing.T) {
	h := newHarness(t)
	h.a.providers[service.Daemon] = &rootJobs{fake: fake{src: service.Daemon}, err: errors.New("authorization canceled")}
	h.click("Daemons")
	h.click("New Job…")
	h.typeInto("Label", "com.example.nightly")
	h.typeInto("Command", "/bin/date")
	h.click("Save")
	h.wantText("Create system job com.example.nightly?")
	h.click("Create Job")
	if !h.a.editor.open || !h.tt.HasText("authorization canceled") {
		t.Errorf("open %v, %q", h.a.editor.open, h.tt.Texts())
	}
}

func TestTheFormsScheduleModes(t *testing.T) {
	h := newHarness(t)
	h.tt.SetSize(1080, 1500)
	h.click("User Agents")
	h.click("New Job…")
	e := &h.a.editor
	e.label, e.args = "com.example.job", "/bin/date"

	// As a service: restarting after a crash runs it at load, whatever the switch.
	h.click("After a crash")
	spec, err := e.spec()
	if err != nil || !spec.RestartOnCrash || spec.KeepAlive {
		t.Errorf("spec = %+v, %v", spec, err)
	}
	h.tt.Frame()
	e.mount, e.queue = true, "/tmp/a, /tmp/b"

	// A schedule drops what only a service does, which it no longer shows.
	h.click("At an interval")
	h.wantText("Run every (seconds)", "Also starts at login")
	h.wantNoText("Queue directories", "Restarts")
	if e.restart != restartNever || e.mount || e.queue != "" || e.runAtLoad {
		t.Errorf("after choosing an interval: %+v", e)
	}
	if _, err := e.spec(); err == nil {
		t.Error("an interval needs its seconds")
	}
	e.interval = "90"
	if spec, err := e.spec(); err != nil || spec.StartInterval != 90 {
		t.Errorf("spec = %+v, %v", spec, err)
	}

	h.click("On a calendar")
	h.wantText("Minute", "Hour", "Day", "Month", "Weekday")
	if _, err := e.spec(); err == nil {
		t.Error("a calendar needs a field")
	}
	e.hour, e.weekday = "9-17", "1-5"
	h.tt.Frame()
	spec, err = e.spec()
	if err != nil || spec.StartInterval != 0 || spec.Calendar.Hour != "9-17" || spec.Calendar.Weekday != "1-5" {
		t.Errorf("spec = %+v, %v", spec, err)
	}
	e.minute = "61"
	if _, err := e.spec(); err == nil || !strings.Contains(err.Error(), "0-59") {
		t.Errorf("err = %v", err)
	}
}

func TestEditingKeepsWhatTheFormDoesNotShow(t *testing.T) {
	old := system.Run
	defer func() { system.Run = old }()
	system.Run = func(string, ...string) (string, error) { return "", nil }

	h := newHarness(t)
	path := filepath.Join(h.a.agentDir, "com.example.mixed.plist")
	if err := os.WriteFile(path, []byte(`<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>Label</key><string>com.example.mixed</string>
<key>ProgramArguments</key><array><string>/bin/sh</string><string>-c</string><string>date</string></array>
<key>StartInterval</key><integer>60</integer>
<key>KeepAlive</key><true/>
<key>StartOnMount</key><true/>
<key>QueueDirectories</key><array><string>/tmp/q</string></array>
<key>Nice</key><integer>5</integer>
</dict></plist>`), 0o644); err != nil {
		t.Fatal(err)
	}
	h.a.editJob(service.Item{Name: "com.example.mixed", Path: path, Source: service.UserAgent})
	e := &h.a.editor
	if e.command != commandShell || e.args != "date" || e.when != whenInterval || e.restart != restartAlways || !e.mount || e.queue != "/tmp/q" {
		t.Fatalf("loaded: %+v", e)
	}
	spec, err := e.spec()
	if err != nil {
		t.Fatal(err)
	}
	if !spec.KeepAlive || !spec.StartOnMount || len(spec.QueueDirs) != 1 || spec.StartInterval != 60 {
		t.Errorf("an edit without a change drops nothing: %+v", spec)
	}
}

// rootJobFile writes a daemon's file, as its installer might have.
func rootJobFile(t *testing.T) (dir, path string) {
	t.Helper()
	dir = t.TempDir()
	path = filepath.Join(dir, "com.vendor.helper.plist")
	if err := os.WriteFile(path, []byte(`<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>Label</key><string>com.vendor.helper</string>
<key>Program</key><string>/opt/vendor/helper</string>
<key>UserName</key><string>_vendor</string>
<key>MachServices</key><dict><key>com.vendor.helper</key><true/></dict>
</dict></plist>`), 0o644); err != nil {
		t.Fatal(err)
	}
	return dir, path
}

func TestEditingARootJobSaysWhatChangesBeforeSaving(t *testing.T) {
	old := system.Run
	defer func() { system.Run = old }()
	system.Run = func(string, ...string) (string, error) { return "", nil }

	dir, path := rootJobFile(t)
	h := newHarness(t)
	h.tt.SetSize(1080, 1500)
	jobs := &rootJobs{fake: fake{src: service.Daemon}, dir: dir}
	h.a.providers[service.Daemon] = jobs
	h.a.editJob(service.Item{Name: "com.vendor.helper", Path: path, Source: service.Daemon, State: service.StateRunning})
	h.tt.Frame()
	h.wantText("Edit Job", "Asks for in-app confirmation before saving", "Runs as", "Reload it after saving")
	if got := h.a.editor.user; got != "_vendor" {
		t.Errorf("user = %q", got)
	}

	// Nothing to save: no question and no restart.
	h.click("Save")
	if h.a.editor.open || h.a.ask.open || len(jobs.made) != 0 {
		t.Fatalf("open %v, ask %v, made %v", h.a.editor.open, h.a.ask.open, jobs.made)
	}

	h.a.editJob(service.Item{Name: "com.vendor.helper", Path: path, Source: service.Daemon, State: service.StateRunning})
	h.tt.Frame()
	h.a.editor.user = "_other"
	h.a.editor.runAtLoad = true
	h.click("Save")
	h.wantText("Save changes to com.vendor.helper?", "This changes RunAtLoad, UserName.", "restarts it if it runs", "without a macOS password prompt")
	if len(jobs.made) != 0 {
		t.Fatal("saved before the answer")
	}
	// Cancel, left of the answer and above the form's own, goes back to the form
	// with what was typed.
	answer, ok := h.tt.Find("Save Changes")
	if !ok {
		t.Fatal("no answer")
	}
	h.tt.ClickAt(answer.X-20, answer.Y+answer.H/2)
	h.tt.Frame()
	if h.a.ask.open || !h.a.editor.open || h.a.editor.user != "_other" || len(jobs.made) != 0 {
		t.Fatalf("ask %v, form %v, made %v", h.a.ask.open, h.a.editor.open, jobs.made)
	}

	h.click("Save")
	h.click("Save Changes")
	if len(jobs.made) != 1 || jobs.edited[0] != path || !jobs.loads[0] || jobs.made[0].UserName != "_other" {
		t.Fatalf("made %+v, edited %v, loads %v, error %q", jobs.made, jobs.edited, jobs.loads, h.a.editor.err)
	}
	if h.a.editor.open {
		t.Error("the form stays open")
	}
}

func TestAJobSavedButNotLoadedIsEditedNext(t *testing.T) {
	old := system.Run
	defer func() { system.Run = old }()
	system.Run = func(string, ...string) (string, error) { return "", nil }

	h := newHarness(t)
	jobs := &savedNotLoaded{rootJobs{fake: fake{src: service.Daemon}, dir: "/Library/LaunchDaemons"}}
	h.a.providers[service.Daemon] = jobs
	h.click("Daemons")
	h.click("New Job…")
	h.a.editor.label, h.a.editor.args = "com.example.d", "/bin/date"
	h.tt.Frame()
	h.click("Save")
	h.wantText("Create system job com.example.d?")
	h.click("Create Job")
	h.wantText("saved, but it did not load")
	if want := "/Library/LaunchDaemons/com.example.d.plist"; h.a.editor.existing != want {
		t.Errorf("existing = %q, want %q", h.a.editor.existing, want)
	}
}

// savedNotLoaded saves a job and fails to load it.
type savedNotLoaded struct{ rootJobs }

func (s *savedNotLoaded) SaveAsRoot(spec launchd.Spec, existing string, load bool) (string, error) {
	path, _ := s.rootJobs.SaveAsRoot(spec, existing, load)
	return path, errors.New("saved, but it did not load: Input/output error")
}

func TestAnyKindOfJobCanBeEditedAndRemoved(t *testing.T) {
	for _, c := range []struct {
		src  service.Source
		nav  string
		list func() *launchd.Provider
	}{
		{service.SystemAgent, "System Agents", launchd.SystemAgents},
		{service.Daemon, "Daemons", launchd.Daemons},
	} {
		h := newHarness(t)
		item := service.Item{ID: string(c.src) + ":/x/com.example.job.plist", Source: c.src, Name: "com.example.job", Path: "/x/com.example.job.plist", State: service.StateLoaded}
		h.a.providers[c.src] = c.list()
		h.a.sources[c.src].items = []service.Item{item}
		h.click(c.nav)
		if err := h.tt.RightClick("com.example.job"); err != nil {
			t.Fatal(err)
		}
		h.tt.Frame()
		menu := fmt.Sprint(h.tt.Menu())
		for _, want := range []string{"Edit…", "Remove…"} {
			if !strings.Contains(menu, want) {
				t.Errorf("%s: the menu lacks %q: %s", c.src, want, menu)
			}
		}
		if err := h.tt.ChooseMenuItem("Remove…"); err != nil {
			t.Fatal(err)
		}
		h.tt.Frame()
		h.wantText("Remove com.example.job?", "not an undo", "may stop working")
		h.click("Cancel")
		if h.a.ask.open {
			t.Errorf("%s: the question stays open", c.src)
		}
	}
}

func TestPrivilegedHelperCreatesAndEditsRootJobWithInAppConfirmationAndNoPassword(t *testing.T) {
	oldRun := system.Run
	defer func() { system.Run = oldRun }()
	system.Run = func(string, ...string) (string, error) { return "", nil }
	adminCalled := false

	sockDir, err := os.MkdirTemp("/tmp", "msm-ph-")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.RemoveAll(sockDir) })
	sockPath := filepath.Join(sockDir, "h.sock")

	managedDir := t.TempDir()
	helperDir := t.TempDir()
	stagedExe := filepath.Join(helperDir, "devbase-helper")
	if err := os.WriteFile(stagedExe, []byte("bin"), 0755); err != nil {
		t.Fatal(err)
	}
	helperPlistPath := filepath.Join(t.TempDir(), privhelper.Label+".plist")
	ownerUID := os.Geteuid()
	plistXML := fmt.Sprintf(`<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>Label</key><string>%s</string>
<key>ProgramArguments</key><array><string>%s</string><string>--dev</string><string>helper</string><string>--owner-uid</string><string>%d</string></array>
<key>Sockets</key><dict><key>%s</key><dict><key>SockPathName</key><string>%s</string><key>SockPathMode</key><integer>438</integer></dict></dict>
</dict></plist>`, privhelper.Label, stagedExe, ownerUID, privhelper.SocketName, sockPath)
	if err := os.WriteFile(helperPlistPath, []byte(plistXML), 0644); err != nil {
		t.Fatal(err)
	}

	ln, err := net.Listen("unix", sockPath)
	if err != nil {
		t.Fatal(err)
	}
	if ul, ok := ln.(*net.UnixListener); ok {
		ul.SetUnlinkOnClose(false)
	}
	var launchctlCalls [][]string
	srv := privhelper.NewServer(privhelper.ServerConfig{
		OwnerUID:           ownerUID,
		ManagedLaunchdDirs: []string{managedDir},
		Run: func(_ context.Context, name string, args ...string) ([]byte, error) {
			launchctlCalls = append(launchctlCalls, append([]string{name}, args...))
			return nil, nil
		},
	})
	ctx, cancel := context.WithCancel(context.Background())
	srvDone := make(chan error, 1)
	go func() { srvDone <- srv.Serve(ctx, ln) }()
	t.Cleanup(func() {
		cancel()
		_ = ln.Close()
		<-srvDone
	})

	client := privhelper.NewClient(privhelper.ClientConfig{
		SocketPath: sockPath,
		PlistPath:  helperPlistPath,
		HelperDir:  helperDir,
		Expected: privhelper.BinaryInfo{
			Executable: "/Applications/Devbase.app/Contents/MacOS/Devbase",
			OwnerUID:   ownerUID,
		},
		RunAdmin: func(string, ...string) (string, error) {
			adminCalled = true
			return "", errors.New("unexpected password prompt while the privileged helper is ready")
		},
	})
	restoreClient := privhelper.SetDefaultForTest(client)
	defer restoreClient()

	h := newHarness(t)
	prov := launchd.Daemons()
	prov.Dir = managedDir
	h.a.providers[service.Daemon] = prov
	h.a.refreshHelperStatus()
	if !h.a.helperReady() {
		t.Fatalf("expected helperReady(), status=%+v", h.a.helperStatus)
	}

	h.click("Daemons")
	h.click("New Job…")
	h.wantText("Asks for in-app confirmation before saving")
	h.typeInto("Label", "com.example.helperjob")
	h.typeInto("Command", "/bin/date")
	h.click("Save")

	if !h.a.ask.open {
		t.Fatalf("expected in-app secondary confirmation on create")
	}
	h.wantText("Create system job com.example.helperjob?", "without a macOS password prompt")
	h.click("Create Job")

	plistPath := filepath.Join(managedDir, "com.example.helperjob.plist")
	if _, err := os.Stat(plistPath); err != nil {
		t.Fatalf("expected helper to write plist at %s: %v", plistPath, err)
	}
	if adminCalled {
		t.Fatal("creating a root job asked for a password although the helper is ready")
	}

	h.a.sources[service.Daemon].items = []service.Item{{
		ID:     "daemon:" + plistPath,
		Source: service.Daemon,
		Name:   "com.example.helperjob",
		Path:   plistPath,
		State:  service.StateLoaded,
		Admin:  true,
	}}
	h.tt.Frame()
	if err := h.tt.RightClick("com.example.helperjob"); err != nil {
		t.Fatal(err)
	}
	h.tt.Frame()
	if err := h.tt.ChooseMenuItem("Edit…"); err != nil {
		t.Fatal(err)
	}
	h.tt.Frame()
	h.typeInto("Command", "/bin/echo")
	h.click("Save")

	if !h.a.ask.open {
		t.Fatalf("expected in-app secondary confirmation on edit")
	}
	h.wantText("Save changes to com.example.helperjob?", "without a macOS password prompt")
	h.click("Save Changes")

	if h.a.editor.open {
		t.Fatalf("expected editor to close after edit, err=%q", h.a.editor.err)
	}
	updated, err := os.ReadFile(plistPath)
	if err != nil || !strings.Contains(string(updated), "/bin/echo") {
		t.Fatalf("expected updated plist to contain /bin/echo, got %q, err=%v", string(updated), err)
	}
	if adminCalled {
		t.Fatal("editing a root job asked for a password although the helper is ready")
	}

	if err := h.tt.RightClick("com.example.helperjob"); err != nil {
		t.Fatal(err)
	}
	h.tt.Frame()
	if err := h.tt.ChooseMenuItem("Remove…"); err != nil {
		t.Fatal(err)
	}
	h.tt.Frame()
	h.wantText("Remove com.example.helperjob?")
	h.click("Cancel")
	_ = launchctlCalls
}

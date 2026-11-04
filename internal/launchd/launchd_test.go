package launchd

import (
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"howett.net/plist"

	"macos-service-manager/internal/service"
	"macos-service-manager/internal/system"
)

const printOut = `gui/501 = {
	type = login
	services = {
		   409      - 	com.example.running
		     0      - 	com.example.idle
		     0   (pe) 	com.example.pending
		     0      1 	com.example.failed
		     0    -15 	com.example.signaled
	}

	unmanaged processes = {
		   111      - 	not.a.service
	}

	disabled services = {
		"com.example.off" => disabled
		"com.example.idle" => enabled
	}
}`

func TestParsePrint(t *testing.T) {
	st := ParsePrint(printOut)
	if j, ok := st.Jobs["com.example.running"]; !ok || j.PID != 409 {
		t.Errorf("running: %+v %v", j, ok)
	}
	if j, ok := st.Jobs["com.example.idle"]; !ok || j.PID != 0 {
		t.Errorf("idle: %+v %v", j, ok)
	}
	if _, ok := st.Jobs["com.example.pending"]; !ok {
		t.Error("a job with a (pe) status is loaded")
	}
	if _, ok := st.Jobs["not.a.service"]; ok {
		t.Error("an unmanaged process is not a job")
	}
	if !st.Disabled["com.example.off"] || st.Disabled["com.example.idle"] {
		t.Errorf("disabled = %v", st.Disabled)
	}
	for label, want := range map[string]service.State{
		"com.example.running":  service.StateRunning,
		"com.example.idle":     service.StateLoaded,
		"com.example.pending":  service.StateLoaded,
		"com.example.failed":   service.StateFailed,
		"com.example.signaled": service.StateLoaded, // how a job is stopped
		"com.example.missing":  service.StateStopped,
	} {
		if got, _ := st.Of(label); got != want {
			t.Errorf("Of(%s) = %v, want %v", label, got, want)
		}
	}
}

func TestDisabledAndScheduled(t *testing.T) {
	p := &Provider{Config{service.UserAgent, "/x", "gui/501", false}}
	on, off := true, false
	st := ParsePrint(printOut)
	// The file's key counts only where launchd holds no override.
	if it := p.item("/x/a", &file{Label: "com.example.new", Disabled: &on}, st); it.Autostart != service.AutostartOff {
		t.Errorf("Disabled in the file: %+v", it)
	}
	if it := p.item("/x/a", &file{Label: "com.example.idle", Disabled: &on}, st); it.Autostart != service.AutostartOn {
		t.Errorf("an override beats the file: %+v", it)
	}
	if it := p.item("/x/a", &file{Label: "com.example.off", Disabled: &off}, st); it.Autostart != service.AutostartOff {
		t.Errorf("an override beats the file: %+v", it)
	}
	interval := 60
	if it := p.item("/x/a", &file{Label: "com.example.idle", StartInterval: &interval}, st); it.State != service.StateScheduled {
		t.Errorf("a loaded job with an interval is scheduled: %+v", it)
	}
}

const xmlPlist = `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>com.example.running</string>
  <key>ProgramArguments</key><array><string>/bin/echo</string><string>hi</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>Crashed</key><true/></dict>
  <key>StartInterval</key><integer>60</integer>
  <key>EnvironmentVariables</key><dict><key>B</key><string>2</string><key>A</key><string>1</string></dict>
  <key>StandardOutPath</key><string>/tmp/x.log</string>
  <key>StandardErrorPath</key><string>/tmp/x.log</string>
</dict></plist>`

func testProvider(t *testing.T) *Provider {
	t.Helper()
	dir := t.TempDir()
	write := func(name string, data []byte) {
		if err := os.WriteFile(filepath.Join(dir, name), data, 0o644); err != nil {
			t.Fatal(err)
		}
	}
	write("running.plist", []byte(xmlPlist))
	bin, err := plist.Marshal(map[string]any{"Label": "com.example.off", "Program": "/bin/true"}, plist.BinaryFormat)
	if err != nil {
		t.Fatal(err)
	}
	write("off.plist", bin)
	write("nolabel.plist", []byte(`<?xml version="1.0"?><plist version="1.0"><dict><key>X</key><string>y</string></dict></plist>`))
	write("broken.plist", []byte("not a plist"))
	write("readme.txt", []byte("ignored"))
	return &Provider{Config{service.UserAgent, dir, "gui/501", false}}
}

func stubRun(t *testing.T, out string) *[][]string {
	t.Helper()
	old, oldAdmin := system.Run, system.RunAdmin
	t.Cleanup(func() { system.Run, system.RunAdmin = old, oldAdmin })
	var calls [][]string
	system.Run = func(name string, args ...string) (string, error) {
		calls = append(calls, append([]string{name}, args...))
		return out, nil
	}
	system.RunAdmin = func(name string, args ...string) (string, error) {
		calls = append(calls, append([]string{"ADMIN", name}, args...))
		return "", nil
	}
	return &calls
}

func TestList(t *testing.T) {
	p := testProvider(t)
	calls := stubRun(t, printOut)
	items, err := p.List()
	if err != nil {
		t.Fatal(err)
	}
	if !slices.EqualFunc(*calls, [][]string{{"launchctl", "print", "gui/501"}}, slices.Equal) {
		t.Errorf("calls = %v", *calls)
	}
	if len(items) != 2 {
		t.Fatalf("want 2 items (files without a label are left out), got %+v", items)
	}
	off, running := items[0], items[1]
	if off.Name != "com.example.off" || off.State != service.StateStopped || off.Autostart != service.AutostartOff {
		t.Errorf("off = %+v", off)
	}
	if running.State != service.StateRunning || running.PID != 409 || running.Autostart != service.AutostartOn {
		t.Errorf("running = %+v", running)
	}
}

func TestDetail(t *testing.T) {
	p := testProvider(t)
	stubRun(t, printOut)
	items, _ := p.List()
	d, err := p.Detail(items[1])
	if err != nil {
		t.Fatal(err)
	}
	rows := map[string]string{}
	for _, r := range d.Rows {
		rows[r.Key] = r.Value
	}
	for k, v := range map[string]string{
		"Command": "/bin/echo hi", "Runs": "When loaded, kept alive on conditions, every 60 s",
		"A": "1", "B": "2", "Process ID": "409",
	} {
		if rows[k] != v {
			t.Errorf("row %q = %q, want %q", k, rows[k], v)
		}
	}
	if len(d.Logs) != 1 || d.Logs[0].Path != "/tmp/x.log" {
		t.Errorf("one log file, as both paths are the same: %+v", d.Logs)
	}
	if _, err := p.Detail(service.Item{Path: filepath.Join(p.Dir, "missing.plist")}); err == nil || !strings.Contains(err.Error(), "not found") {
		t.Errorf("missing file: %v", err)
	}
}

func TestActions(t *testing.T) {
	user := &Provider{Config{Source: service.UserAgent}}
	sys := &Provider{Config{Source: service.SystemAgent}}
	for _, c := range []struct {
		p    *Provider
		it   service.Item
		want []service.Action
	}{
		{user, service.Item{State: service.StateRunning, Autostart: service.AutostartOn},
			[]service.Action{service.Stop, service.Restart, service.Unload, service.Disable, service.Remove}},
		{user, service.Item{State: service.StateLoaded, Autostart: service.AutostartOn},
			[]service.Action{service.Start, service.Unload, service.Disable, service.Remove}},
		{user, service.Item{State: service.StateFailed, Autostart: service.AutostartOn},
			[]service.Action{service.Start, service.Unload, service.Disable, service.Remove}},
		{sys, service.Item{State: service.StateStopped, Autostart: service.AutostartOff},
			[]service.Action{service.Load, service.Enable, service.Remove}},
	} {
		if got := c.p.Actions(c.it); !slices.Equal(got, c.want) {
			t.Errorf("Actions(%v) = %v, want %v", c.it.State, got, c.want)
		}
	}
}

func TestDo(t *testing.T) {
	calls := stubRun(t, "")
	user := &Provider{Config{service.UserAgent, "/x", "gui/501", false}}
	daemon := &Provider{Config{service.Daemon, "/x", "system", true}}
	it := service.Item{Name: "lbl", Path: "/x/lbl.plist"}
	off := service.Item{Name: "lbl", Path: "/x/lbl.plist", Autostart: service.AutostartOff}
	for _, c := range []struct {
		p    *Provider
		it   service.Item
		a    service.Action
		want [][]string
	}{
		{user, it, service.Load, [][]string{{"launchctl", "bootstrap", "gui/501", "/x/lbl.plist"}}},
		{user, off, service.Load, [][]string{{"launchctl", "enable", "gui/501/lbl"}, {"launchctl", "bootstrap", "gui/501", "/x/lbl.plist"}}},
		{user, it, service.Unload, [][]string{{"launchctl", "bootout", "gui/501/lbl"}}},
		{user, it, service.Start, [][]string{{"launchctl", "kickstart", "gui/501/lbl"}}},
		{user, it, service.Restart, [][]string{{"launchctl", "kickstart", "-k", "gui/501/lbl"}}},
		{user, it, service.Stop, [][]string{{"launchctl", "kill", "SIGTERM", "gui/501/lbl"}}},
		{user, it, service.Disable, [][]string{{"launchctl", "disable", "gui/501/lbl"}}},
		{daemon, it, service.Unload, [][]string{{"ADMIN", "launchctl", "bootout", "system/lbl"}}},
		// Two commands as root run under one authentication.
		{daemon, off, service.Load, [][]string{{"ADMIN", "/bin/sh", "-c",
			"'launchctl' 'enable' 'system/lbl' && 'launchctl' 'bootstrap' 'system' '/x/lbl.plist'"}}},
	} {
		*calls = nil
		if err := c.p.Do(c.it, c.a); err != nil {
			t.Fatal(err)
		}
		if !slices.EqualFunc(*calls, c.want, slices.Equal) {
			t.Errorf("%s: calls %q, want %q", c.a, *calls, c.want)
		}
	}
}

func TestDoAllAsksOnce(t *testing.T) {
	calls := stubRun(t, "")
	daemon := &Provider{Config{service.Daemon, "/x", "system", true}}
	items := []service.Item{{Name: "a"}, {Name: "b"}}
	if err := daemon.DoAll(items, service.Start); err != nil {
		t.Fatal(err)
	}
	want := [][]string{{"ADMIN", "/bin/sh", "-c", "'launchctl' 'kickstart' 'system/a' && 'launchctl' 'kickstart' 'system/b'"}}
	if !slices.EqualFunc(*calls, want, slices.Equal) {
		t.Errorf("calls %q, want %q", *calls, want)
	}
}

func TestRemoveMovesFileToTrash(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	if err := os.MkdirAll(filepath.Join(home, ".Trash"), 0o755); err != nil {
		t.Fatal(err)
	}
	stubRun(t, "")
	p := testProvider(t)
	path := filepath.Join(p.Dir, "running.plist")
	it := service.Item{Name: "com.example.running", Path: path, State: service.StateRunning}
	if err := p.Do(it, service.Remove); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(path); !os.IsNotExist(err) {
		t.Error("the file should be gone")
	}
	if _, err := os.Stat(filepath.Join(home, ".Trash", "running.plist")); err != nil {
		t.Errorf("not in the Trash: %v", err)
	}
}

func TestConfirm(t *testing.T) {
	user := &Provider{Config{Source: service.UserAgent}}
	daemon := &Provider{Config{Source: service.Daemon, Elevated: true}}
	it := service.Item{Name: "x", Path: "/a/x.plist"}
	if user.Confirm(it, service.Remove) == "" || daemon.Confirm(it, service.Unload) == "" || daemon.Confirm(it, service.Disable) == "" {
		t.Error("removing, and unloading or disabling a daemon, ask first")
	}
	for _, p := range []*Provider{daemon, {Config{Source: service.SystemAgent}}} {
		msg := p.Confirm(it, service.Remove)
		for _, want := range []string{"password", "may stop working", "not an undo", "Put Back cannot return it to the a folder", "reinstall"} {
			if !strings.Contains(msg, want) {
				t.Errorf("%s lacks %q: %q", p.Config.Source, want, msg)
			}
		}
	}
	if strings.Contains(user.Confirm(it, service.Remove), "password") {
		t.Error("a user agent needs no password")
	}
	if user.Confirm(it, service.Stop) != "" || user.Confirm(it, service.Unload) != "" || daemon.Confirm(it, service.Start) != "" {
		t.Error("ordinary actions do not ask")
	}
}

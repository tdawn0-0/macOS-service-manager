package brew

import (
	"errors"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"macos-service-manager/internal/service"
	"macos-service-manager/internal/system"
)

const listJSON = `[
  {"name":"postgresql@16","status":"started","user":"jyeu","file":"/a.plist","exit_code":0},
  {"name":"redis","status":"none","user":null,"file":"/opt/homebrew/opt/redis/sh.brew.redis.plist","exit_code":null},
  {"name":"nginx","status":"error","user":"jyeu","file":"/n.plist","exit_code":1},
  {"name":"mysql","status":"stopped","user":"jyeu","file":"/m.plist","exit_code":0},
  {"name":"cron","status":"scheduled","user":"jyeu","file":"/c.plist","exit_code":0},
  {"name":"php","status":"none","user":"root","file":"/Users/x/Library/LaunchAgents/sh.brew.php.plist","exit_code":null}
]`

const systemPrint = `system = {
	services = {
		     407      - 	homebrew.mxcl.php
	}
}`

func stub(t *testing.T, out string, fail map[string]error) *[][]string {
	t.Helper()
	oldRun, oldAdmin := system.Run, system.RunAdmin
	t.Cleanup(func() { system.Run, system.RunAdmin = oldRun, oldAdmin })
	var calls [][]string
	system.Run = func(name string, args ...string) (string, error) {
		call := append([]string{name}, args...)
		calls = append(calls, call)
		if name == "launchctl" {
			return systemPrint, nil
		}
		if err := fail[call[2]]; err != nil {
			return "", err
		}
		return out, nil
	}
	system.RunAdmin = func(name string, args ...string) (string, error) {
		calls = append(calls, append([]string{"ADMIN", name}, args...))
		return "", nil
	}
	return &calls
}

// provider has the property lists named: those with a slash, of the daemons.
func provider(t *testing.T, plists ...string) *Provider {
	t.Helper()
	p := &Provider{AgentDir: t.TempDir(), DaemonDir: t.TempDir()}
	for _, name := range plists {
		dir := p.AgentDir
		if d, base, ok := strings.Cut(name, "/"); ok && d == "daemon" {
			dir, name = p.DaemonDir, base
		}
		if err := os.WriteFile(filepath.Join(dir, name), nil, 0o644); err != nil {
			t.Fatal(err)
		}
	}
	return p
}

func TestList(t *testing.T) {
	stub(t, listJSON, nil)
	// postgresql is registered the new way, mysql the old way; php as root.
	p := provider(t, "sh.brew.postgresql@16.plist", "homebrew.mxcl.mysql.plist", "sh.brew.other.plist", "daemon/homebrew.mxcl.php.plist")
	items, err := p.List()
	if err != nil {
		t.Fatal(err)
	}
	type row struct {
		name  string
		state service.State
		auto  service.Autostart
		owner string
		admin bool
		pid   int
	}
	var got []row
	for _, it := range items {
		got = append(got, row{it.Name, it.State, it.Autostart, it.Owner, it.Admin, it.PID})
	}
	want := []row{
		{"postgresql@16", service.StateRunning, service.AutostartOn, "jyeu", false, 0},
		{"redis", service.StateStopped, service.AutostartOff, "", false, 0},
		{"nginx", service.StateFailed, service.AutostartOff, "jyeu", false, 0},
		{"mysql", service.StateLoaded, service.AutostartOn, "jyeu", false, 0},
		{"cron", service.StateScheduled, service.AutostartOff, "jyeu", false, 0},
		// `brew services` says none: launchd knows better.
		{"php", service.StateRunning, service.AutostartOn, "root", true, 407},
	}
	if !slices.Equal(got, want) {
		t.Errorf("got  %+v\nwant %+v", got, want)
	}
	if items[0].ID != "brew:postgresql@16" || items[0].Source != service.Brew {
		t.Errorf("item = %+v", items[0])
	}
	if items[5].Path != filepath.Join(p.DaemonDir, "homebrew.mxcl.php.plist") {
		t.Errorf("the path of a root service is its property list: %q", items[5].Path)
	}
	if items[0].Path != filepath.Join(p.AgentDir, "sh.brew.postgresql@16.plist") {
		t.Errorf("path = %q", items[0].Path)
	}
}

func TestListErrors(t *testing.T) {
	stub(t, "", map[string]error{"list": errors.New("brew: command not found")})
	if _, err := provider(t).List(); err == nil || err.Error() != "brew: command not found" {
		t.Errorf("err = %v", err)
	}
	stub(t, "nope", nil)
	if _, err := provider(t).List(); err == nil {
		t.Error("invalid JSON should fail")
	}
}

func TestDetail(t *testing.T) {
	stub(t, `[{"name":"redis","service_name":"sh.brew.redis","running":true,"loaded":true,
	  "schedulable":false,"pid":42,"exit_code":null,"user":"jyeu","status":"started",
	  "file":"/r.plist","command":"redis-server","working_dir":null,"root_dir":null,
	  "log_path":"/var/log/redis.log","error_log_path":"/var/log/redis.err","interval":null,"cron":null}]`, nil)
	d, err := provider(t).Detail(service.Item{Name: "redis", State: service.StateRunning, Path: "/r.plist"})
	if err != nil {
		t.Fatal(err)
	}
	rows := map[string]string{}
	for _, r := range d.Rows {
		rows[r.Key] = r.Value
	}
	if rows["Process ID"] != "42" || rows["Command"] != "redis-server" || rows["Property list"] != "/r.plist" {
		t.Errorf("rows = %v", rows)
	}
	if _, ok := rows["Working directory"]; ok {
		t.Error("values that are not set are left out")
	}
	if !slices.Equal(d.Logs, []service.LogFile{{Label: "Output", Path: "/var/log/redis.log"}, {Label: "Errors", Path: "/var/log/redis.err"}}) {
		t.Errorf("logs = %+v", d.Logs)
	}
	stub(t, `[]`, nil)
	if _, err := provider(t).Detail(service.Item{Name: "x"}); err == nil {
		t.Error("no info should fail")
	}
}

func TestActionsAndConfirm(t *testing.T) {
	p := provider(t)
	running := service.Item{Name: "redis", State: service.StateRunning, Autostart: service.AutostartOff}
	stopped := service.Item{Name: "redis", State: service.StateStopped, Autostart: service.AutostartOn}
	root := service.Item{Name: "php", State: service.StateRunning, Autostart: service.AutostartOn, Admin: true, Path: "/x/homebrew.mxcl.php.plist"}
	if got := p.Actions(running); !slices.Equal(got, []service.Action{service.Stop, service.Restart, service.Enable}) {
		t.Errorf("running: %v", got)
	}
	if got := p.Actions(stopped); !slices.Equal(got, []service.Action{service.Start, service.Disable}) {
		t.Errorf("stopped: %v", got)
	}
	// Turning the autostart on or off interrupts nothing, so it asks nothing.
	for _, it := range []service.Item{running, stopped} {
		if msg := p.Confirm(it, service.Enable); msg != "" {
			t.Errorf("enable asks: %q", msg)
		}
		if msg := p.Confirm(it, service.Disable); msg != "" {
			t.Errorf("disable asks: %q", msg)
		}
	}
	// Deleting a root service's property list is another matter.
	if msg := p.Confirm(root, service.Disable); msg == "" {
		t.Error("unregistering a root service asks first")
	}
}

func TestDo(t *testing.T) {
	stopped := service.Item{Name: "redis", State: service.StateStopped, Autostart: service.AutostartOff}
	stoppedOn := service.Item{Name: "redis", State: service.StateStopped, Autostart: service.AutostartOn}
	loaded := service.Item{Name: "redis", State: service.StateLoaded, Autostart: service.AutostartOn}
	running := service.Item{Name: "redis", State: service.StateRunning, Autostart: service.AutostartOff}
	runningOn := service.Item{Name: "redis", State: service.StateRunning, Autostart: service.AutostartOn}
	for _, c := range []struct {
		name string
		it   service.Item
		a    service.Action
		want []string
	}{
		// Starting and stopping leave the autostart as it is.
		{"start unregistered", stopped, service.Start, []string{"run", "redis"}},
		{"start registered", stoppedOn, service.Start, []string{"start", "redis"}},
		{"start loaded", loaded, service.Start, []string{"restart", "redis"}},
		{"stop unregistered", running, service.Stop, []string{"stop", "redis"}},
		{"stop registered", runningOn, service.Stop, []string{"stop", "--keep", "redis"}},
		{"restart", running, service.Restart, []string{"restart", "redis"}},
	} {
		calls := stub(t, "", nil)
		if err := provider(t).Do(c.it, c.a); err != nil {
			t.Fatal(err)
		}
		if len(*calls) != 1 || !slices.Equal((*calls)[0], append([]string{"brew", "services"}, c.want...)) {
			t.Errorf("%s: ran %v, want %v", c.name, *calls, c.want)
		}
	}
}

func TestAutostartMovesThePropertyListOnly(t *testing.T) {
	calls := stub(t, "", nil)
	p := provider(t)
	source := filepath.Join(t.TempDir(), "sh.brew.redis.plist")
	if err := os.WriteFile(source, []byte("the plist"), 0o644); err != nil {
		t.Fatal(err)
	}
	dest := filepath.Join(p.AgentDir, "sh.brew.redis.plist")
	it := service.Item{Name: "redis", State: service.StateRunning, Path: source, Autostart: service.AutostartOff}

	if err := p.Do(it, service.Enable); err != nil {
		t.Fatal(err)
	}
	if data, err := os.ReadFile(dest); err != nil || string(data) != "the plist" {
		t.Errorf("registered: %q, %v", data, err)
	}
	it.Autostart, it.Path = service.AutostartOn, dest
	if err := p.Do(it, service.Disable); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(dest); !os.IsNotExist(err) {
		t.Errorf("unregistered: %v", err)
	}
	if len(*calls) != 0 {
		t.Errorf("no command runs, so the service is not interrupted: %v", *calls)
	}
	// The file an old Homebrew named goes too.
	old := filepath.Join(p.AgentDir, "homebrew.mxcl.redis.plist")
	os.WriteFile(old, nil, 0o644)
	if err := p.Do(it, service.Disable); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(old); !os.IsNotExist(err) {
		t.Errorf("unregistered the old name: %v", err)
	}
}

func TestARootServiceChangesThroughLaunchctl(t *testing.T) {
	root := service.Item{Name: "php", State: service.StateRunning, Admin: true, Autostart: service.AutostartOn, Path: "/L/homebrew.mxcl.php.plist"}
	off := root
	off.State = service.StateStopped
	for _, c := range []struct {
		it   service.Item
		a    service.Action
		want []string
	}{
		{root, service.Stop, []string{"ADMIN", "launchctl", "bootout", "system/homebrew.mxcl.php"}},
		{root, service.Restart, []string{"ADMIN", "launchctl", "kickstart", "-k", "system/homebrew.mxcl.php"}},
		{off, service.Start, []string{"ADMIN", "launchctl", "bootstrap", "system", "/L/homebrew.mxcl.php.plist"}},
		{root, service.Disable, []string{"ADMIN", "/bin/sh", "-c", "launchctl bootout 'system/homebrew.mxcl.php'; rm -f '/L/homebrew.mxcl.php.plist'"}},
	} {
		calls := stub(t, "", nil)
		if err := provider(t).Do(c.it, c.a); err != nil {
			t.Fatal(err)
		}
		if len(*calls) != 1 || !slices.Equal((*calls)[0], c.want) {
			t.Errorf("%s: %q, want %q", c.a, *calls, c.want)
		}
	}
	if err := provider(t).Do(root, service.Enable); err == nil {
		t.Error("a root service has no way to be registered again")
	}
}

package brew

import (
	"errors"
	"os"
	"path/filepath"
	"slices"
	"testing"

	"macos-service-manager/internal/service"
	"macos-service-manager/internal/system"
)

const listJSON = `[
  {"name":"postgresql@16","status":"started","user":"jyeu","file":"/a.plist","exit_code":0},
  {"name":"redis","status":"none","user":null,"file":"/opt/homebrew/opt/redis/sh.brew.redis.plist","exit_code":null},
  {"name":"nginx","status":"error","user":"jyeu","file":"/n.plist","exit_code":1},
  {"name":"php","status":"scheduled","user":"root","file":"/p.plist","exit_code":null}
]`

func stub(t *testing.T, out string, fail map[string]error) *[][]string {
	t.Helper()
	old := system.Run
	t.Cleanup(func() { system.Run = old })
	var calls [][]string
	system.Run = func(name string, args ...string) (string, error) {
		call := append([]string{name}, args...)
		calls = append(calls, call)
		if err := fail[call[2]]; err != nil {
			return "", err
		}
		return out, nil
	}
	return &calls
}

func provider(t *testing.T, registered ...string) *Provider {
	t.Helper()
	dir := t.TempDir()
	for _, name := range registered {
		if err := os.WriteFile(filepath.Join(dir, name), nil, 0o644); err != nil {
			t.Fatal(err)
		}
	}
	return &Provider{Registered: []string{dir}}
}

func TestList(t *testing.T) {
	stub(t, listJSON, nil)
	// postgresql is registered the new way, php the old way; redis is not.
	items, err := provider(t, "sh.brew.postgresql@16.plist", "homebrew.mxcl.php.plist", "sh.brew.other.plist").List()
	if err != nil {
		t.Fatal(err)
	}
	type row struct {
		name  string
		state service.State
		auto  service.Autostart
		owner string
	}
	var got []row
	for _, it := range items {
		got = append(got, row{it.Name, it.State, it.Autostart, it.Owner})
	}
	want := []row{
		{"postgresql@16", service.StateRunning, service.AutostartOn, "jyeu"},
		{"redis", service.StateStopped, service.AutostartOff, ""},
		{"nginx", service.StateFailed, service.AutostartOff, "jyeu"},
		{"php", service.StateScheduled, service.AutostartOn, "root"},
	}
	if !slices.Equal(got, want) {
		t.Errorf("got  %+v\nwant %+v", got, want)
	}
	if items[0].ID != "brew:postgresql@16" || items[0].Source != service.Brew {
		t.Errorf("item = %+v", items[0])
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
	d, err := provider(t).Detail(service.Item{Name: "redis", Autostart: service.AutostartOn})
	if err != nil {
		t.Fatal(err)
	}
	rows := map[string]string{}
	for _, r := range d.Rows {
		rows[r.Key] = r.Value
	}
	if rows["pid"] != "42" || rows["starts at login"] != "yes" || rows["command"] != "redis-server" {
		t.Errorf("rows = %v", rows)
	}
	if _, ok := rows["working dir"]; ok {
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
	if got := p.Actions(running); !slices.Equal(got, []service.Action{service.Stop, service.Restart, service.Kill, service.Enable}) {
		t.Errorf("running: %v", got)
	}
	if got := p.Actions(stopped); !slices.Equal(got, []service.Action{service.Run, service.Start, service.Disable}) {
		t.Errorf("stopped: %v", got)
	}
	// Changing the autostart of a running service restarts it: ask first.
	if msg := p.Confirm(running, service.Enable); msg == "" {
		t.Error("enabling autostart of a running service must be confirmed")
	}
	runningOn := service.Item{Name: "redis", State: service.StateRunning, Autostart: service.AutostartOn}
	if msg := p.Confirm(runningOn, service.Disable); msg == "" {
		t.Error("disabling autostart of a running service must be confirmed")
	}
	// A stopped service has nothing to interrupt.
	if msg := p.Confirm(stopped, service.Disable); msg != "" {
		t.Errorf("a stopped service needs no confirmation: %q", msg)
	}
	if msg := p.Confirm(running, service.Stop); msg != "" {
		t.Errorf("stop needs no confirmation: %q", msg)
	}
}

func TestDo(t *testing.T) {
	running := service.Item{Name: "redis", State: service.StateRunning}
	stopped := service.Item{Name: "redis", State: service.StateStopped}
	for _, c := range []struct {
		name string
		it   service.Item
		a    service.Action
		want []string // the subcommands, in order
	}{
		{"run", stopped, service.Run, []string{"run"}},
		{"start", stopped, service.Start, []string{"start"}},
		{"stop", running, service.Stop, []string{"stop"}},
		{"restart", running, service.Restart, []string{"restart"}},
		{"kill", running, service.Kill, []string{"kill"}},
		// start refuses a running service, so it stops it first.
		{"enable running", running, service.Enable, []string{"stop", "start"}},
		{"enable stopped", stopped, service.Enable, []string{"start"}},
		// stop unregisters as it stops: run it again if it ran.
		{"disable running", running, service.Disable, []string{"stop", "run"}},
		{"disable stopped", stopped, service.Disable, []string{"stop"}},
	} {
		calls := stub(t, "", nil)
		if err := provider(t).Do(c.it, c.a); err != nil {
			t.Fatal(err)
		}
		var got []string
		for _, call := range *calls {
			if call[3] != "redis" || call[0] != "brew" || call[1] != "services" {
				t.Errorf("%s: call %v", c.name, call)
			}
			got = append(got, call[2])
		}
		if !slices.Equal(got, c.want) {
			t.Errorf("%s: ran %v, want %v", c.name, got, c.want)
		}
	}

	// A failure stops the sequence: a service that could not be stopped
	// is not started.
	calls := stub(t, "", map[string]error{"stop": errors.New("denied")})
	if err := provider(t).Do(running, service.Enable); err == nil || err.Error() != "denied" {
		t.Errorf("err = %v", err)
	}
	if len(*calls) != 1 {
		t.Errorf("calls after a failure: %v", *calls)
	}
}

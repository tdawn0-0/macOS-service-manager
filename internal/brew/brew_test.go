package brew

import (
	"errors"
	"slices"
	"testing"

	"macos-service-manager/internal/system"
)

func TestParseList(t *testing.T) {
	data := `[
	  {"name":"postgresql@16","status":"started","user":"jyeu","file":"/a.plist","exit_code":0},
	  {"name":"redis","status":"none","user":null,"file":"/b.plist","exit_code":null}
	]`
	services, err := ParseList([]byte(data))
	if err != nil {
		t.Fatal(err)
	}
	if len(services) != 2 || services[0].Name != "postgresql@16" || services[0].User == nil || *services[0].User != "jyeu" {
		t.Fatalf("unexpected services: %+v", services)
	}
	if services[1].User != nil || services[1].ExitCode != nil {
		t.Errorf("null fields should be nil: %+v", services[1])
	}
	if !services[0].Active() || services[1].Active() {
		t.Errorf("Active: started=%v none=%v", services[0].Active(), services[1].Active())
	}
	if _, err := ParseList([]byte("nope")); err == nil {
		t.Error("invalid JSON should fail")
	}
}

func TestActive(t *testing.T) {
	for status, want := range map[string]bool{
		"started": true, "scheduled": true, "error": true,
		"stopped": false, "none": false, "unknown": false, "other": false,
	} {
		if got := (Service{Status: status}).Active(); got != want {
			t.Errorf("Active(%q) = %v, want %v", status, got, want)
		}
	}
}

func TestParseInfoRows(t *testing.T) {
	data := `[{"name":"redis","service_name":"homebrew.mxcl.redis","running":true,"loaded":true,
	  "schedulable":false,"pid":42,"exit_code":null,"user":"jyeu","status":"started",
	  "file":"/r.plist","command":"/opt/homebrew/bin/redis-server","working_dir":null,
	  "root_dir":null,"log_path":"/var/log/redis.log","error_log_path":null,"interval":null,"cron":null}]`
	info, err := ParseInfo([]byte(data))
	if err != nil {
		t.Fatal(err)
	}
	var keys []string
	var logs []string
	for _, r := range info.Rows() {
		keys = append(keys, r.Key)
		if r.LogPath {
			logs = append(logs, r.Value)
		}
	}
	want := []string{"name", "service_name", "running", "loaded", "schedulable", "pid", "user", "status", "file", "command", "log_path"}
	if !slices.Equal(keys, want) {
		t.Errorf("keys = %v, want %v", keys, want)
	}
	if !slices.Equal(logs, []string{"/var/log/redis.log"}) {
		t.Errorf("log rows = %v", logs)
	}
	if _, err := ParseInfo([]byte("[]")); err == nil {
		t.Error("empty info should fail")
	}
}

func TestCLIRunsBrewServices(t *testing.T) {
	old := system.Run
	defer func() { system.Run = old }()
	var got [][]string
	system.Run = func(name string, args ...string) (string, error) {
		got = append(got, append([]string{name}, args...))
		return "[]", nil
	}
	if _, err := (CLI{}).List(); err != nil {
		t.Fatal(err)
	}
	if err := (CLI{}).Manage("redis", Restart); err != nil {
		t.Fatal(err)
	}
	want := [][]string{
		{"brew", "services", "list", "--json"},
		{"brew", "services", "restart", "redis"},
	}
	if !slices.EqualFunc(got, want, slices.Equal) {
		t.Errorf("commands = %v, want %v", got, want)
	}

	system.Run = func(string, ...string) (string, error) { return "", errors.New("boom") }
	if err := (CLI{}).Manage("redis", Stop); err == nil || err.Error() != "boom" {
		t.Errorf("error = %v", err)
	}
}

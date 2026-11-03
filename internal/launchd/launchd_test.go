package launchd

import (
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"testing"

	"howett.net/plist"

	"macos-service-manager/internal/system"
)

const xmlPlist = `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>com.example.b</string>
  <key>ProgramArguments</key><array><string>/bin/echo</string><string>hi</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>StandardOutPath</key><string>/tmp/b.log</string>
</dict></plist>`

func writeAgents(t *testing.T) string {
	t.Helper()
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "b.plist"), []byte(xmlPlist), 0o644); err != nil {
		t.Fatal(err)
	}
	// A binary property list, as many agents are.
	bin, err := plist.Marshal(map[string]any{"Label": "com.example.a", "Program": "/bin/true", "KeepAlive": true}, plist.BinaryFormat)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "a.plist"), bin, 0o644); err != nil {
		t.Fatal(err)
	}
	for name, content := range map[string]string{
		"nolabel.plist": `<?xml version="1.0"?><plist version="1.0"><dict><key>X</key><string>y</string></dict></plist>`,
		"broken.plist":  "not a plist",
		"readme.txt":    "ignored",
	} {
		if err := os.WriteFile(filepath.Join(dir, name), []byte(content), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	return dir
}

func TestParseJobs(t *testing.T) {
	jobs := ParseJobs("PID\tStatus\tLabel\n123\t0\tcom.example.a\n-\t0\tcom.example.b\n\n")
	if pid, ok := jobs["com.example.a"]; !ok || pid == nil || *pid != 123 {
		t.Errorf("a: %v %v", pid, ok)
	}
	if pid, ok := jobs["com.example.b"]; !ok || pid != nil {
		t.Errorf("b should be loaded without a pid: %v %v", pid, ok)
	}
	if _, ok := jobs["Label"]; ok {
		t.Error("the header is not a job")
	}
}

func TestScan(t *testing.T) {
	dir := writeAgents(t)
	pid := 7
	services := Scan([]Dir{{dir, DomainUser}}, Jobs{"com.example.a": &pid, "com.example.b": nil})
	if len(services) != 2 {
		t.Fatalf("want 2 services, got %+v", services)
	}
	a, b := services[0], services[1]
	if a.Label != "com.example.a" || !a.Loaded || !a.Running || *a.PID != 7 {
		t.Errorf("a = %+v", a)
	}
	if b.Label != "com.example.b" || !b.Loaded || b.Running || b.PID != nil {
		t.Errorf("b = %+v", b)
	}
	if got := Scan([]Dir{{dir, DomainUser}}, Jobs{}); got[0].Loaded {
		t.Error("a job that is not listed is not loaded")
	}
}

func TestInfo(t *testing.T) {
	dir := writeAgents(t)
	old := system.Run
	defer func() { system.Run = old }()
	system.Run = func(name string, args ...string) (string, error) {
		return "PID\tStatus\tLabel\n55\t0\tcom.example.b\n", nil
	}
	info, err := (Launchctl{}).Info(filepath.Join(dir, "b.plist"))
	if err != nil {
		t.Fatal(err)
	}
	if !info.Running || *info.PID != 55 || info.Domain != DomainUser {
		t.Errorf("info = %+v", info)
	}
	if info.KeepAlive != nil {
		t.Error("a KeepAlive dictionary has no single value")
	}
	rows := map[string]string{}
	var logs []string
	for _, r := range info.Rows() {
		rows[r.Key] = r.Value
		if r.LogPath {
			logs = append(logs, r.Key)
		}
	}
	if rows["program_arguments"] != "/bin/echo hi" || rows["run_at_load"] != "true" {
		t.Errorf("rows = %v", rows)
	}
	if _, ok := rows["keep_alive"]; ok {
		t.Error("keep_alive should be left out")
	}
	if !slices.Equal(logs, []string{"standard_out_path"}) {
		t.Errorf("log rows = %v", logs)
	}

	if _, err := (Launchctl{}).Info(filepath.Join(dir, "missing.plist")); err == nil || !strings.Contains(err.Error(), "not found") {
		t.Errorf("missing file: %v", err)
	}
	if _, err := (Launchctl{}).Info(filepath.Join(dir, "nolabel.plist")); err == nil {
		t.Error("a plist without a label should fail")
	}
}

func TestDomainOf(t *testing.T) {
	if DomainOf("/Library/LaunchAgents/a.plist") != DomainSystem {
		t.Error("/Library is the system's")
	}
	if DomainOf("/Users/x/Library/LaunchAgents/a.plist") != DomainUser {
		t.Error("a home directory is the user's")
	}
}

func TestManageArguments(t *testing.T) {
	old := system.Run
	defer func() { system.Run = old }()
	var got []string
	system.Run = func(name string, args ...string) (string, error) {
		got = append([]string{name}, args...)
		return "", nil
	}
	uid := os.Getuid()
	domain := "gui/" + strconv.Itoa(uid)
	for _, c := range []struct {
		cmd  Command
		want []string
	}{
		{Load, []string{"launchctl", "bootstrap", domain, "/p.plist"}},
		{Unload, []string{"launchctl", "bootout", domain + "/lbl"}},
		{Start, []string{"launchctl", "kickstart", "-k", domain + "/lbl"}},
		{Stop, []string{"launchctl", "kill", "SIGTERM", domain + "/lbl"}},
	} {
		if err := (Launchctl{}).Manage("lbl", "/p.plist", c.cmd); err != nil {
			t.Fatal(err)
		}
		if !slices.Equal(got, c.want) {
			t.Errorf("%s: %v, want %v", c.cmd, got, c.want)
		}
	}
	if err := (Launchctl{}).Manage("lbl", "/p.plist", "bogus"); err == nil {
		t.Error("unknown command should fail")
	}
}

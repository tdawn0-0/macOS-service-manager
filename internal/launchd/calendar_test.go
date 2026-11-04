package launchd

import (
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"macos-service-manager/internal/system"
)

func TestCalendarEntries(t *testing.T) {
	for _, test := range []struct {
		name string
		c    Calendar
		want int
		key  string
		vals []int
	}{
		{"none", Calendar{}, 0, "", nil},
		{"a number", Calendar{Minute: "5"}, 1, "Minute", []int{5}},
		{"a list", Calendar{Hour: "1, 3,5"}, 3, "Hour", []int{1, 3, 5}},
		{"a range", Calendar{Weekday: "1-5"}, 5, "Weekday", []int{1, 2, 3, 4, 5}},
		{"an interval from the start", Calendar{Minute: "*/15"}, 4, "Minute", []int{0, 15, 30, 45}},
		{"an interval from day 1", Calendar{Day: "*/10"}, 4, "Day", []int{1, 11, 21, 31}},
		{"duplicates once", Calendar{Hour: "1,1-2"}, 2, "Hour", []int{1, 2}},
		{"a product", Calendar{Minute: "0,30", Hour: "9-17"}, 18, "", nil},
		{"star leaves the field out", Calendar{Minute: "0", Hour: "*"}, 1, "Minute", []int{0}},
		{"every minute", Calendar{Minute: "*"}, 60, "Minute", nil},
		{"every minute by interval", Calendar{Hour: "*/1"}, 60, "Minute", nil},
		{"sunday as 7", Calendar{Weekday: "7"}, 1, "Weekday", []int{7}},
	} {
		t.Run(test.name, func(t *testing.T) {
			entries, err := test.c.Entries()
			if err != nil {
				t.Fatal(err)
			}
			if len(entries) != test.want {
				t.Fatalf("%d entries: %v", len(entries), entries)
			}
			if test.vals != nil {
				var got []int
				for _, e := range entries {
					got = append(got, e[test.key])
				}
				if !slices.Equal(got, test.vals) {
					t.Errorf("values = %v, want %v", got, test.vals)
				}
			}
		})
	}
}

func TestCalendarRejects(t *testing.T) {
	for text, c := range map[string]Calendar{
		"outside 0-59":    {Minute: "60"},
		"outside 0-23":    {Hour: "24"},
		"outside 1-31":    {Day: "0"},
		"outside 1-12":    {Month: "13"},
		"outside 0-7":     {Weekday: "8"},
		"goes backwards":  {Hour: "5-1"},
		"is not a number": {Minute: "abc"},
		"not a positive":  {Minute: "*/0"},
		"start times":     {Minute: "0-59", Hour: "0-23"},
	} {
		if _, err := c.Entries(); err == nil || !strings.Contains(err.Error(), text) {
			t.Errorf("%+v: err = %v, want %q", c, err, text)
		}
	}
}

// What the form writes it reads back the same, and a file it cannot say stays.
func TestCalendarRoundTripsThroughAFile(t *testing.T) {
	old := system.Run
	defer func() { system.Run = old }()
	system.Run = func(string, ...string) (string, error) { return "", nil }

	dir := t.TempDir()
	for name, c := range map[string]Calendar{
		"single":   {Minute: "30", Hour: "2"},
		"interval": {Minute: "*/15"},
		"product":  {Minute: "0,30", Hour: "9-17", Weekday: "1-5"},
		"sparse":   {Day: "1,15", Month: "1,7"},
	} {
		s := Spec{Label: "com.example." + name, Args: ShellArgs("date"), Calendar: c}
		path, err := Save(dir, s, "")
		if err != nil {
			t.Fatal(err)
		}
		got, err := LoadSpec(path)
		if err != nil {
			t.Fatal(err)
		}
		if got.CalendarCustom || got.Calendar != c {
			t.Errorf("%s: read %+v, custom %v, want %+v", name, got.Calendar, got.CalendarCustom, c)
		}
		if command, ok := ShellCommand(got.Args); !ok || command != "date" {
			t.Errorf("%s: args = %v", name, got.Args)
		}
	}

	// Two different times are not all combinations of their values.
	path := filepath.Join(dir, "custom.plist")
	custom := `<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>Label</key><string>com.example.custom</string>
<key>ProgramArguments</key><array><string>/bin/date</string></array>
<key>StartCalendarInterval</key><array>
<dict><key>Hour</key><integer>9</integer><key>Minute</key><integer>0</integer></dict>
<dict><key>Hour</key><integer>17</integer><key>Minute</key><integer>30</integer></dict>
</array>
</dict></plist>`
	if err := os.WriteFile(path, []byte(custom), 0o644); err != nil {
		t.Fatal(err)
	}
	spec, err := LoadSpec(path)
	if err != nil {
		t.Fatal(err)
	}
	if !spec.CalendarCustom {
		t.Fatalf("spec = %+v", spec)
	}
	if _, err := Save(dir, spec, path); err != nil {
		t.Fatal(err)
	}
	data, _ := os.ReadFile(path)
	if !strings.Contains(string(data), "<integer>17</integer>") || !strings.Contains(string(data), "<integer>30</integer>") {
		t.Errorf("an unknown calendar stays:\n%s", data)
	}
}

func TestServiceOptionsRoundTrip(t *testing.T) {
	old := system.Run
	defer func() { system.Run = old }()
	system.Run = func(string, ...string) (string, error) { return "", nil }

	dir := t.TempDir()
	s := Spec{Label: "com.example.svc", Args: ShellArgs("date"), RestartOnCrash: true, StartOnMount: true,
		QueueDirs: []string{"/tmp/a", "/tmp/b"}, UserName: "me", GroupName: "staff", RootDir: "/tmp/root"}
	path, err := Save(dir, s, "")
	if err != nil {
		t.Fatal(err)
	}
	data, _ := os.ReadFile(path)
	if !strings.Contains(string(data), "<key>SuccessfulExit</key>") {
		t.Errorf("restarting after a crash:\n%s", data)
	}
	got, err := LoadSpec(path)
	if err != nil {
		t.Fatal(err)
	}
	if !got.RestartOnCrash || got.KeepAlive || got.KeepAliveCustom || !got.RunAtLoad {
		t.Errorf("a restart after a crash, which implies running at load: %+v", got)
	}
	if !got.StartOnMount || !slices.Equal(got.QueueDirs, s.QueueDirs) || got.UserName != "me" || got.GroupName != "staff" || got.RootDir != "/tmp/root" {
		t.Errorf("got = %+v", got)
	}
	f, err := readFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if want := "When loaded, restarted after a crash, when a disk mounts, while a queue has files"; f.runs() != want {
		t.Errorf("runs = %q, want %q", f.runs(), want)
	}

	for text, bad := range map[string]Spec{
		"queue directory": {Label: "a", Args: []string{"/bin/date"}, QueueDirs: []string{"inbox"}},
		"root directory":  {Label: "a", Args: []string{"/bin/date"}, RootDir: "jail"},
		"a group needs":   {Label: "a", Args: []string{"/bin/date"}, GroupName: "staff"},
		"outside":         {Label: "a", Args: []string{"/bin/date"}, Calendar: Calendar{Hour: "99"}},
	} {
		if err := bad.Validate(); err == nil || !strings.Contains(err.Error(), text) {
			t.Errorf("%q: err = %v", text, err)
		}
	}
}

func stubRoot(t *testing.T) (runs *[]string, admin *[][]string) {
	t.Helper()
	oldRun, oldAdmin := system.Run, system.RunAdmin
	t.Cleanup(func() { system.Run, system.RunAdmin = oldRun, oldAdmin })
	runs, admin = new([]string), new([][]string)
	system.Run = func(name string, args ...string) (string, error) {
		*runs = append(*runs, name+" "+strings.Join(args, " "))
		return "", nil
	}
	system.RunAdmin = func(name string, args ...string) (string, error) {
		*admin = append(*admin, append([]string{name}, args...))
		return "", nil
	}
	return runs, admin
}

func TestADaemonInstallsAndLoadsUnderOneAuthentication(t *testing.T) {
	runs, admin := stubRoot(t)
	dir := t.TempDir()
	p := &Provider{Config{Dir: filepath.Join(dir, "my jobs"), Domain: "system", Elevated: true}}
	path, err := p.SaveAsRoot(Spec{Label: "com.example.d", Args: ShellArgs("date"), UserName: "me", GroupName: "staff"}, "", true)
	if err != nil {
		t.Fatal(err)
	}
	if want := filepath.Join(p.Dir, "com.example.d.plist"); path != want {
		t.Errorf("path = %q, want %q", path, want)
	}
	if len(*admin) != 1 {
		t.Fatalf("authentications = %v", *admin)
	}
	script := (*admin)[0][2]
	// As a shell reads it, with a space in the path.
	if out, err := exec.Command("/bin/sh", "-n", "-c", script).CombinedOutput(); err != nil {
		t.Errorf("the script does not parse: %v %s\n%s", err, out, script)
	}
	for _, want := range []string{
		"[ ! -e '" + path + "' ]",
		"/usr/bin/install -S -m 644 -o root -g wheel",
		"/bin/launchctl bootstrap 'system' '" + path + "'",
	} {
		if !strings.Contains(script, want) {
			t.Errorf("script lacks %q:\n%s", want, script)
		}
	}
	for _, want := range []string{"id -u me", "dscl . -read /Groups/staff PrimaryGroupID", "plutil -lint"} {
		if !slices.ContainsFunc(*runs, func(r string) bool { return strings.HasPrefix(r, want) }) {
			t.Errorf("no %q in %v", want, *runs)
		}
	}
}

func TestASystemAgentLoadsWithoutSecondAuthentication(t *testing.T) {
	runs, admin := stubRoot(t)
	p := &Provider{Config{Dir: t.TempDir(), Domain: "gui/501", Elevated: false}}
	path, err := p.SaveAsRoot(Spec{Label: "com.example.a", Args: ShellArgs("date")}, "", true)
	if err != nil {
		t.Fatal(err)
	}
	if len(*admin) != 1 || strings.Contains((*admin)[0][2], "bootstrap") {
		t.Fatalf("root only installs: %v", *admin)
	}
	if !slices.Contains(*runs, "launchctl bootstrap gui/501 "+path) {
		t.Errorf("the user loads it: %v", *runs)
	}
}

func TestSaveAsRootChecksBeforeAsking(t *testing.T) {
	_, admin := stubRoot(t)
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "com.example.taken.plist"), nil, 0o644); err != nil {
		t.Fatal(err)
	}
	p := &Provider{Config{Dir: dir, Domain: "system", Elevated: true}}
	if _, err := p.SaveAsRoot(Spec{Label: "com.example.taken", Args: ShellArgs("date")}, "", false); err == nil || !strings.Contains(err.Error(), "already exists") {
		t.Errorf("err = %v", err)
	}
	if _, err := p.SaveAsRoot(Spec{Label: "bad label", Args: ShellArgs("date")}, "", false); err == nil {
		t.Error("a bad label")
	}
	if _, err := p.SaveAsRoot(Spec{Label: "ok", Args: ShellArgs("date"), Calendar: Calendar{Hour: "99"}}, "", false); err == nil {
		t.Error("a bad calendar")
	}
	system.Run = func(name string, _ ...string) (string, error) {
		if name == "id" {
			return "", os.ErrNotExist
		}
		return "", nil
	}
	if _, err := p.SaveAsRoot(Spec{Label: "ok", Args: ShellArgs("date"), UserName: "nobody-here"}, "", false); err == nil || !strings.Contains(err.Error(), `no user "nobody-here"`) {
		t.Errorf("err = %v", err)
	}
	if len(*admin) != 0 {
		t.Errorf("asked for a password for what could not work: %v", *admin)
	}
}

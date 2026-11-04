package launchd

import (
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"testing"

	"howett.net/plist"

	"macos-service-manager/internal/service"
	"macos-service-manager/internal/system"
)

const vendorJob = `<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>Label</key><string>com.vendor.helper</string>
<key>Program</key><string>/opt/vendor/helper</string>
<key>StartInterval</key><integer>60</integer>
<key>KeepAlive</key><dict><key>Crashed</key><true/><key>NetworkState</key><true/><key>PathState</key><dict><key>/a</key><true/><key>/b</key><false/></dict></dict>
<key>MachServices</key><dict><key>com.vendor.a</key><true/><key>com.vendor.b</key><true/><key>com.vendor.c</key><true/></dict>
<key>Nice</key><integer>-5</integer>
</dict></plist>`

func TestAnEditOfNothingChangesNothing(t *testing.T) {
	binary, err := plist.Marshal(map[string]any{
		"Label":        "com.vendor.bin",
		"Program":      "/opt/vendor/bin",
		"RunAtLoad":    false,
		"StartOnMount": false,
		"Nice":         -5,
		"ExitTimeOut":  30,
		"Sockets":      map[string]any{"Listener": map[string]any{"SockPathName": "/tmp/s", "SockPathMode": 438}},
	}, plist.BinaryFormat)
	if err != nil {
		t.Fatal(err)
	}
	for name, data := range map[string][]byte{"xml": []byte(vendorJob), "binary": binary} {
		// Dictionaries come in any order: more than once.
		for range 20 {
			spec, err := specFromBytes(t, data)
			if err != nil {
				t.Fatal(err)
			}
			changes, err := spec.Changes(data)
			if err != nil {
				t.Fatal(err)
			}
			if len(changes) != 0 {
				t.Fatalf("%s: changes = %v", name, changes)
			}
		}
	}
}

func TestChangesNamesWhatChanged(t *testing.T) {
	data := []byte(vendorJob)
	spec, err := specFromBytes(t, data)
	if err != nil {
		t.Fatal(err)
	}
	spec.Args = []string{"/opt/vendor/helper", "--verbose"}
	spec.StartInterval = 0
	spec.RunAtLoad = true
	spec.UserName = "_vendor"
	changes, err := spec.Changes(data)
	if err != nil {
		t.Fatal(err)
	}
	want := []string{"ProgramArguments", "RunAtLoad", "StartInterval", "UserName"}
	if !slices.Equal(changes, want) {
		t.Errorf("changes = %v, want %v", changes, want)
	}
}

// launchd runs Program, with ProgramArguments for its argv.
func TestAFileWithAProgramRunsWhatTheFormSays(t *testing.T) {
	data := []byte(`<plist version="1.0"><dict>
<key>Label</key><string>com.vendor.O+Connect</string>
<key>Program</key><string>/opt/vendor/helper</string>
<key>ProgramArguments</key><array><string>helper</string><string>--old</string></array>
</dict></plist>`)
	spec, err := specFromBytes(t, data)
	if err != nil {
		t.Fatal(err)
	}
	if !slices.Equal(spec.Args, []string{"/opt/vendor/helper", "--old"}) {
		t.Fatalf("the form shows what runs: %v", spec.Args)
	}
	// A label no new job could have, which stays as it is.
	if err := spec.Validate(); err != nil {
		t.Errorf("an existing job: %v", err)
	}
	spec.Existing = false
	if err := spec.Validate(); err == nil {
		t.Error("a new job cannot have that label")
	}
	spec.Existing = true
	if changes, err := spec.Changes(data); err != nil || len(changes) != 0 {
		t.Errorf("changes = %v, %v", changes, err)
	}

	// New arguments keep what the process calls itself.
	spec.Args = []string{"/opt/vendor/helper", "--new"}
	out, err := spec.Marshal(data)
	if err != nil {
		t.Fatal(err)
	}
	m := decode(t, out)
	if args := m["ProgramArguments"].([]any); m["Program"] != "/opt/vendor/helper" || args[0] != "helper" || args[1] != "--new" {
		t.Errorf("Program %v, arguments %v", m["Program"], args)
	}

	// A new program runs: launchd would go on running the old one.
	spec.Args = []string{"/opt/other/tool", "--new"}
	out, err = spec.Marshal(data)
	if err != nil {
		t.Fatal(err)
	}
	m = decode(t, out)
	if args := m["ProgramArguments"].([]any); m["Program"] != "/opt/other/tool" || args[0] != "/opt/other/tool" {
		t.Errorf("Program %v, arguments %v", m["Program"], args)
	}
	got, err := specFromBytes(t, out)
	if err != nil || !slices.Equal(got.Args, spec.Args) {
		t.Errorf("read back %v, %v", got.Args, err)
	}
}

func TestTurningAnOptionOffKeepsItFalse(t *testing.T) {
	data := []byte(`<plist version="1.0"><dict>
<key>Label</key><string>a</string>
<key>ProgramArguments</key><array><string>/bin/date</string></array>
<key>RunAtLoad</key><true/>
<key>KeepAlive</key><true/>
<key>StartOnMount</key><true/>
</dict></plist>`)
	spec, err := specFromBytes(t, data)
	if err != nil {
		t.Fatal(err)
	}
	spec.RunAtLoad, spec.KeepAlive, spec.StartOnMount = false, false, false
	out, err := spec.Marshal(data)
	if err != nil {
		t.Fatal(err)
	}
	m := decode(t, out)
	for _, key := range []string{"RunAtLoad", "KeepAlive", "StartOnMount"} {
		if v, ok := m[key].(bool); !ok || v {
			t.Errorf("%s = %v", key, m[key])
		}
	}
	fresh, err := Spec{Label: "a", Args: []string{"/bin/date"}}.Marshal(nil)
	if err != nil {
		t.Fatal(err)
	}
	m = decode(t, fresh)
	for _, key := range []string{"RunAtLoad", "KeepAlive", "StartOnMount"} {
		if _, ok := m[key]; ok {
			t.Errorf("a new file says nothing of %s", key)
		}
	}
}

func TestEditingADaemonReplacesThenLoadsAgainUnderOneAuthentication(t *testing.T) {
	_, admin := stubRoot(t)
	dir := filepath.Join(t.TempDir(), "my jobs")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(dir, "com.vendor.helper.plist")
	if err := os.WriteFile(path, []byte(vendorJob), 0o644); err != nil {
		t.Fatal(err)
	}
	p := &Provider{Config{Source: service.Daemon, Dir: dir, Domain: "system", Elevated: true}}
	spec, err := LoadSpec(path)
	if err != nil {
		t.Fatal(err)
	}
	spec.UserName = "" // No lookup.
	if got, err := p.SaveAsRoot(spec, path, true); err != nil || got != path {
		t.Fatalf("path %q, err %v", got, err)
	}
	if len(*admin) != 1 {
		t.Fatalf("authentications = %v", *admin)
	}
	script := (*admin)[0][2]
	if out, err := exec.Command("/bin/sh", "-n", "-c", script).CombinedOutput(); err != nil {
		t.Errorf("the script does not parse: %v %s\n%s", err, out, script)
	}
	// The job stays as it is until the file is in place.
	last := -1
	for _, want := range []string{
		"[ -f '" + path + "' ]",
		"/usr/bin/install -S -m 644 -o root -g wheel",
		"/bin/launchctl print 'system/com.vendor.helper'",
		"/bin/launchctl bootout 'system/com.vendor.helper' || exit $?",
		"/bin/launchctl bootstrap 'system' '" + path + "'",
	} {
		at := strings.Index(script, want)
		if at <= last {
			t.Errorf("%q is not in its place:\n%s", want, script)
		}
		last = at
	}
}

func TestEditingASystemAgentLoadsAgainAsTheUser(t *testing.T) {
	runs, admin := stubRoot(t)
	dir := t.TempDir()
	path := filepath.Join(dir, "a.plist")
	if err := os.WriteFile(path, []byte(vendorJob), 0o644); err != nil {
		t.Fatal(err)
	}
	p := &Provider{Config{Source: service.SystemAgent, Dir: dir, Domain: "gui/501"}}
	spec, err := LoadSpec(path)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := p.SaveAsRoot(spec, path, true); err != nil {
		t.Fatal(err)
	}
	if len(*admin) != 1 || strings.Contains((*admin)[0][2], "launchctl") {
		t.Fatalf("root only installs: %v", *admin)
	}
	bootout := slices.Index(*runs, "launchctl bootout gui/501/com.vendor.helper")
	bootstrap := slices.Index(*runs, "launchctl bootstrap gui/501 "+path)
	if bootout < 0 || bootstrap < bootout {
		t.Errorf("runs = %v", *runs)
	}
}

func TestSavingOverWhatIsNotAFileAsksNothing(t *testing.T) {
	_, admin := stubRoot(t)
	dir := t.TempDir()
	real := filepath.Join(dir, "real.plist")
	if err := os.WriteFile(real, []byte(vendorJob), 0o644); err != nil {
		t.Fatal(err)
	}
	link := filepath.Join(dir, "link.plist")
	if err := os.Symlink(real, link); err != nil {
		t.Fatal(err)
	}
	p := &Provider{Config{Dir: dir, Domain: "system", Elevated: true}}
	s := Spec{Label: "com.vendor.helper", Args: []string{"/bin/date"}}
	if _, err := p.SaveAsRoot(s, link, true); err == nil || !strings.Contains(err.Error(), "not a regular file") {
		t.Errorf("err = %v", err)
	}
	if _, err := p.SaveAsRoot(s, filepath.Join(dir, "gone.plist"), true); err == nil {
		t.Error("a file that is gone")
	}
	if len(*admin) != 0 {
		t.Errorf("asked for a password for what could not work: %v", *admin)
	}
}

// A failure after the file is in place says so, and one before it does not.
func TestAFailureSaysWhetherTheFileWasSaved(t *testing.T) {
	stubRoot(t)
	dir := t.TempDir()
	path := filepath.Join(dir, "a.plist")
	if err := os.WriteFile(path, []byte(vendorJob), 0o644); err != nil {
		t.Fatal(err)
	}
	p := &Provider{Config{Dir: dir, Domain: "system", Elevated: true}}
	spec, err := LoadSpec(path)
	if err != nil {
		t.Fatal(err)
	}
	spec.RunAtLoad = true

	system.RunAdmin = func(string, ...string) (string, error) { return "", errors.New("authorization canceled") }
	if got, err := p.SaveAsRoot(spec, path, true); got != "" || err == nil || err.Error() != "authorization canceled" {
		t.Errorf("canceled: path %q, err %v", got, err)
	}

	// As root would: the file goes in place, and launchd refuses it.
	system.RunAdmin = func(name string, args ...string) (string, error) {
		script := args[1]
		_, rest, _ := strings.Cut(script, "wheel '")
		tmp, _, _ := strings.Cut(rest, "'")
		data, err := os.ReadFile(tmp)
		if err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, data, 0o644); err != nil {
			t.Fatal(err)
		}
		return "", errors.New("Bootstrap failed: 5: Input/output error")
	}
	got, err := p.SaveAsRoot(spec, path, true)
	if got != path || err == nil || !strings.Contains(err.Error(), "saved, but it did not load") {
		t.Errorf("not loaded: path %q, err %v", got, err)
	}
}

// runAsUser runs what root would, as far as the user can: on files of the
// user's, and with launchd's answer for a job that is not loaded.
func runAsUser(t *testing.T) {
	t.Helper()
	old := system.RunAdmin
	t.Cleanup(func() { system.RunAdmin = old })
	system.RunAdmin = func(name string, args ...string) (string, error) {
		out, err := exec.Command(name, args...).CombinedOutput()
		if err != nil {
			return "", errors.New(string(out) + err.Error())
		}
		return string(out), nil
	}
}

func TestRemovingARootJobPutsItInTheTrash(t *testing.T) {
	runAsUser(t)
	oldTrash := system.Trash
	t.Cleanup(func() { system.Trash = oldTrash })
	var trashed []string
	var kept []byte
	system.Trash = func(path string) error {
		trashed = append(trashed, path)
		kept, _ = os.ReadFile(path)
		return os.Remove(path)
	}

	dir := filepath.Join(t.TempDir(), "my jobs")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(dir, "com.vendor.it's.plist")
	if err := os.WriteFile(path, []byte(vendorJob), 0o644); err != nil {
		t.Fatal(err)
	}
	// launchd has no such job, so nothing is unloaded.
	p := &Provider{Config{Source: service.Daemon, Dir: dir, Domain: "gui/" + strconv.Itoa(os.Getuid()), Elevated: true}}
	it := service.Item{Name: "com.example.msm-test.no-such-job", Path: path}
	if err := p.Do(it, service.Remove); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Lstat(path); !os.IsNotExist(err) {
		t.Errorf("still there: %v", err)
	}
	if len(trashed) != 1 || filepath.Base(trashed[0]) != filepath.Base(path) || string(kept) != vendorJob {
		t.Errorf("trashed %v, kept %q", trashed, kept)
	}
	if _, err := os.Stat(filepath.Dir(trashed[0])); !os.IsNotExist(err) {
		t.Errorf("the temporary directory stays: %v", err)
	}
}

// runWithLaunchctl runs what root would, as the user, with a launchctl that says a
// job is loaded and exits with the status given for each of its commands.
func runWithLaunchctl(t *testing.T, status map[string]int) {
	t.Helper()
	fake := filepath.Join(t.TempDir(), "launchctl")
	script := "#!/bin/sh\ncase \"$1\" in\n"
	for command, code := range status {
		script += command + ") echo " + command + " failed >&2; exit " + strconv.Itoa(code) + ";;\n"
	}
	script += "esac\nexit 0\n"
	if err := os.WriteFile(fake, []byte(script), 0o755); err != nil {
		t.Fatal(err)
	}
	old := system.RunAdmin
	t.Cleanup(func() { system.RunAdmin = old })
	system.RunAdmin = func(name string, args ...string) (string, error) {
		line := strings.ReplaceAll(args[1], "/bin/launchctl", fake)
		line = strings.ReplaceAll(line, " -o root -g wheel", "")
		out, err := exec.Command(name, args[0], line).CombinedOutput()
		if err != nil {
			return "", errors.New(strings.TrimSpace(string(out)))
		}
		return string(out), nil
	}
}

// A job left running without a file could never be managed again.
func TestAJobLaunchdWillNotUnloadIsNotRemoved(t *testing.T) {
	t.Setenv("TMPDIR", t.TempDir())
	runWithLaunchctl(t, map[string]int{"bootout": 36})
	oldTrash := system.Trash
	t.Cleanup(func() { system.Trash = oldTrash })
	system.Trash = func(string) error { t.Error("nothing goes to the Trash"); return nil }

	path := filepath.Join(t.TempDir(), "a.plist")
	if err := os.WriteFile(path, []byte(vendorJob), 0o644); err != nil {
		t.Fatal(err)
	}
	p := &Provider{Config{Source: service.Daemon, Domain: "system", Elevated: true}}
	err := p.Do(service.Item{Name: "com.vendor.helper", Path: path}, service.Remove)
	if err == nil || !strings.Contains(err.Error(), "bootout failed") {
		t.Errorf("err = %v", err)
	}
	if got, err := os.ReadFile(path); err != nil || string(got) != vendorJob {
		t.Errorf("the file changed: %q, %v", got, err)
	}
	if left, _ := filepath.Glob(filepath.Join(os.TempDir(), "msm-removed-*")); len(left) != 0 {
		t.Errorf("left behind: %v", left)
	}
}

// As the real script runs, whichever step fails.
func TestAReplacedFileIsSavedWhetherOrNotTheJobLoadsAgain(t *testing.T) {
	stubRoot(t)
	for _, c := range []struct {
		name string
		fail map[string]int
	}{
		{"launchd will not unload it", map[string]int{"bootout": 36}},
		{"launchd will not load it", map[string]int{"bootstrap": 5}},
	} {
		t.Run(c.name, func(t *testing.T) {
			runWithLaunchctl(t, c.fail)
			path := filepath.Join(t.TempDir(), "a.plist")
			if err := os.WriteFile(path, []byte(vendorJob), 0o644); err != nil {
				t.Fatal(err)
			}
			spec, err := LoadSpec(path)
			if err != nil {
				t.Fatal(err)
			}
			spec.RunAtLoad = true
			p := &Provider{Config{Dir: filepath.Dir(path), Domain: "system", Elevated: true}}
			got, err := p.SaveAsRoot(spec, path, true)
			if got != path || err == nil || !strings.Contains(err.Error(), "saved, but it did not load") {
				t.Fatalf("path %q, err %v", got, err)
			}
			if saved, _ := LoadSpec(path); !saved.RunAtLoad {
				t.Error("the file was not replaced")
			}
		})
	}
}

func TestRemovingARootJobKeepsItIfAnythingFails(t *testing.T) {
	t.Setenv("TMPDIR", t.TempDir())
	_, admin := stubRoot(t)
	oldTrash := system.Trash
	t.Cleanup(func() { system.Trash = oldTrash })
	trashed := 0
	system.Trash = func(string) error { trashed++; return errors.New("no Trash") }

	path := filepath.Join(t.TempDir(), "a.plist")
	if err := os.WriteFile(path, []byte(vendorJob), 0o644); err != nil {
		t.Fatal(err)
	}
	p := &Provider{Config{Source: service.Daemon, Domain: "system", Elevated: true}}
	it := service.Item{Name: "com.vendor.helper", Path: path}

	// The script unloads first, and goes no further if launchd refuses.
	if err := p.Do(it, service.Remove); err == nil || !strings.Contains(err.Error(), "not in the Trash") {
		t.Errorf("err = %v", err)
	}
	if len(*admin) != 1 {
		t.Fatalf("authentications = %v", *admin)
	}
	script := (*admin)[0][2]
	if out, err := exec.Command("/bin/sh", "-n", "-c", script).CombinedOutput(); err != nil {
		t.Errorf("the script does not parse: %v %s\n%s", err, out, script)
	}
	if bootout, mv := strings.Index(script, "bootout 'system/com.vendor.helper' || exit $?"), strings.Index(script, "/bin/mv"); bootout < 0 || mv < bootout {
		t.Errorf("script:\n%s", script)
	}

	trashed = 0
	system.RunAdmin = func(string, ...string) (string, error) { return "", errors.New("authorization canceled") }
	if err := p.Do(it, service.Remove); err == nil || err.Error() != "authorization canceled" {
		t.Errorf("err = %v", err)
	}
	if trashed != 0 {
		t.Error("nothing to put in the Trash")
	}
	if _, err := os.Stat(path); err != nil {
		t.Errorf("canceling keeps the file: %v", err)
	}
}

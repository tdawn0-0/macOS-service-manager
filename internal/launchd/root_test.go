package launchd

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"testing"

	"howett.net/plist"

	"devbase/internal/privhelper"
	"devbase/internal/service"
	"devbase/internal/system"
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
	dir := filepath.Join(t.TempDir(), "my jobs")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	_, helperRuns := stubRoot(t, dir)
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
	want := [][]string{
		{"/bin/launchctl", "print", "system/com.vendor.helper"},
		{"/bin/launchctl", "bootout", "system/com.vendor.helper"},
		{"/bin/launchctl", "bootstrap", "system", path},
	}
	if !slices.EqualFunc(*helperRuns, want, slices.Equal) {
		t.Fatalf("helperRuns = %v, want %v", *helperRuns, want)
	}
}

func TestEditingASystemAgentLoadsAgainAsTheUser(t *testing.T) {
	dir := t.TempDir()
	runs, helperRuns := stubRoot(t, dir)
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
	if len(*helperRuns) != 0 {
		t.Fatalf("root only installs system agent without helper launchctl: %v", *helperRuns)
	}
	bootout := slices.Index(*runs, "launchctl bootout gui/501/com.vendor.helper")
	bootstrap := slices.Index(*runs, "launchctl bootstrap gui/501 "+path)
	if bootout < 0 || bootstrap < bootout {
		t.Errorf("runs = %v", *runs)
	}
}

func TestSavingOverWhatIsNotAFileAsksNothing(t *testing.T) {
	stubRoot(t) // no helper installed; checkManagedPath rejects before Require()
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
	if _, err := p.SaveAsRoot(s, filepath.Join(dir, "gone.plist"), true); err == nil || errors.Is(err, privhelper.ErrNotInstalled) {
		t.Errorf("expected path error before helper requirement, got %v", err)
	}
}

// A failure after the file is in place says so, and one before it does not.
func TestAFailureSaysWhetherTheFileWasSaved(t *testing.T) {
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

	oldRun := system.Run
	t.Cleanup(func() { system.Run = oldRun })
	system.Run = func(string, ...string) (string, error) { return "", nil }

	// Without helper, returns ErrNotInstalled and leaves path untouched.
	if got, err := p.SaveAsRoot(spec, path, true); got != "" || !errors.Is(err, privhelper.ErrNotInstalled) {
		t.Errorf("without helper: path %q, err %v", got, err)
	}

	// With helper where bootstrap fails, the file goes in place and reports saved-not-loaded.
	privhelper.InstallTestHelper(t, privhelper.ServerConfig{
		ManagedLaunchdDirs: []string{dir},
		Run: func(_ context.Context, _ string, args ...string) ([]byte, error) {
			if len(args) > 0 && args[0] == "bootstrap" {
				return []byte("Bootstrap failed: 5: Input/output error"), errors.New("exit 5")
			}
			return nil, nil
		},
	})
	got, err := p.SaveAsRoot(spec, path, true)
	if got != path || err == nil || !strings.Contains(err.Error(), "saved, but it did not load") {
		t.Errorf("not loaded: path %q, err %v", got, err)
	}
}

func TestRemovingARootJobPutsItInTheTrash(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "my jobs")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		t.Fatal(err)
	}
	privhelper.InstallTestHelper(t, privhelper.ServerConfig{
		ManagedLaunchdDirs: []string{dir},
		Run: func(_ context.Context, _ string, args ...string) ([]byte, error) {
			if len(args) > 0 && args[0] == "print" {
				return nil, errors.New("Could not find service")
			}
			return nil, nil
		},
	})
	oldTrash := system.Trash
	t.Cleanup(func() { system.Trash = oldTrash })
	var trashed []string
	var kept []byte
	system.Trash = func(path string) error {
		trashed = append(trashed, path)
		kept, _ = os.ReadFile(path)
		return os.Remove(path)
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

// installLaunchdFailureHelper installs a test helper whose launchctl fails for status codes > 0.
func installLaunchdFailureHelper(t *testing.T, dir string, status map[string]int) {
	t.Helper()
	oldRun := system.Run
	t.Cleanup(func() { system.Run = oldRun })
	system.Run = func(string, ...string) (string, error) { return "", nil }
	privhelper.InstallTestHelper(t, privhelper.ServerConfig{
		ManagedLaunchdDirs: []string{dir},
		Run: func(_ context.Context, _ string, args ...string) ([]byte, error) {
			if len(args) > 0 {
				if code := status[args[0]]; code != 0 {
					msg := fmt.Sprintf("%s failed", args[0])
					return []byte(msg), errors.New(msg)
				}
			}
			return nil, nil
		},
	})
}

// A job left running without a file could never be managed again.
func TestAJobLaunchdWillNotUnloadIsNotRemoved(t *testing.T) {
	t.Setenv("TMPDIR", t.TempDir())
	dir := t.TempDir()
	installLaunchdFailureHelper(t, dir, map[string]int{"bootout": 36})
	oldTrash := system.Trash
	t.Cleanup(func() { system.Trash = oldTrash })
	system.Trash = func(string) error { t.Error("nothing goes to the Trash"); return nil }

	path := filepath.Join(dir, "a.plist")
	if err := os.WriteFile(path, []byte(vendorJob), 0o644); err != nil {
		t.Fatal(err)
	}
	p := &Provider{Config{Source: service.Daemon, Dir: dir, Domain: "system", Elevated: true}}
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

// As the real helper runs, whichever step fails.
func TestAReplacedFileIsSavedWhetherOrNotTheJobLoadsAgain(t *testing.T) {
	for _, c := range []struct {
		name string
		fail map[string]int
	}{
		{"launchd will not unload it", map[string]int{"bootout": 36}},
		{"launchd will not load it", map[string]int{"bootstrap": 5}},
	} {
		t.Run(c.name, func(t *testing.T) {
			dir := t.TempDir()
			installLaunchdFailureHelper(t, dir, c.fail)
			path := filepath.Join(dir, "a.plist")
			if err := os.WriteFile(path, []byte(vendorJob), 0o644); err != nil {
				t.Fatal(err)
			}
			spec, err := LoadSpec(path)
			if err != nil {
				t.Fatal(err)
			}
			spec.RunAtLoad = true
			p := &Provider{Config{Dir: dir, Domain: "system", Elevated: true}}
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
	oldTrash := system.Trash
	t.Cleanup(func() { system.Trash = oldTrash })
	trashed := 0
	system.Trash = func(string) error { trashed++; return errors.New("no Trash") }

	dir := t.TempDir()
	path := filepath.Join(dir, "a.plist")
	if err := os.WriteFile(path, []byte(vendorJob), 0o644); err != nil {
		t.Fatal(err)
	}
	p := &Provider{Config{Source: service.Daemon, Dir: dir, Domain: "system", Elevated: true}}
	it := service.Item{Name: "com.vendor.helper", Path: path}

	// Without helper, returns ErrNotInstalled and does not touch file or Trash.
	if err := p.Do(it, service.Remove); !errors.Is(err, privhelper.ErrNotInstalled) {
		t.Errorf("err = %v, want ErrNotInstalled", err)
	}
	if trashed != 0 {
		t.Error("nothing to put in the Trash without helper")
	}
	if _, err := os.Stat(path); err != nil {
		t.Errorf("missing helper keeps the file: %v", err)
	}

	// With helper, if Trash fails after move, error reports temporary location.
	_, helperRuns := stubRoot(t, dir)
	if err := p.Do(it, service.Remove); err == nil || !strings.Contains(err.Error(), "not in the Trash") {
		t.Errorf("err = %v", err)
	}
	wantBootout := []string{"/bin/launchctl", "bootout", "system/com.vendor.helper"}
	if !slices.ContainsFunc(*helperRuns, func(c []string) bool { return slices.Equal(c, wantBootout) }) {
		t.Errorf("helperRuns missing bootout: %v", *helperRuns)
	}
}

// Legacy editing must preserve a throttle it does not expose, while managed
// development services can explicitly choose their own restart throttle.
func TestOptionalThrottlePreservesExistingJobPolicy(t *testing.T) {
	data := []byte(`<plist version="1.0"><dict><key>Label</key><string>a</string><key>ProgramArguments</key><array><string>/bin/date</string></array><key>ThrottleInterval</key><integer>45</integer></dict></plist>`)
	spec, err := specFromBytes(t, data)
	if err != nil {
		t.Fatal(err)
	}
	out, err := spec.Marshal(data)
	if err != nil || decode(t, out)["ThrottleInterval"] != uint64(45) {
		t.Fatal("legacy edit changed restart throttle", err, decode(t, out))
	}
	throttle := 3
	spec.ThrottleInterval = &throttle
	out, err = spec.Marshal(data)
	if err != nil || decode(t, out)["ThrottleInterval"] != uint64(3) {
		t.Fatal("explicit throttle was not applied", err, decode(t, out))
	}
}

func TestAuthorizedJobPayload(t *testing.T) {
	for _, edit := range []bool{false, true} {
		for _, load := range []bool{false, true} {
			t.Run(fmt.Sprintf("edit-%t-load-%t", edit, load), func(t *testing.T) {
				dir := t.TempDir()
				_, helperRuns := stubRoot(t, dir)
				existing := ""
				spec := Spec{Label: "com.example.fixed", Args: []string{"/bin/echo", "quoted ' Unicode 字段"}}
				targetPath := filepath.Join(dir, "com.example.fixed.plist")
				if edit {
					existing = targetPath
					b, e := spec.Marshal(nil)
					if e != nil {
						t.Fatal(e)
					}
					if e = os.WriteFile(existing, b, 0644); e != nil {
						t.Fatal(e)
					}
				}
				p := &Provider{Config{Dir: dir, Domain: "system", Elevated: true}}
				if _, e := p.SaveAsRoot(spec, existing, load); e != nil {
					t.Fatal(e)
				}
				b, e := os.ReadFile(targetPath)
				if e != nil {
					t.Fatal(e)
				}
				var decoded map[string]any
				if _, e := plist.Unmarshal(b, &decoded); e != nil {
					t.Fatal(e)
				}
				if decoded["Label"] != spec.Label {
					t.Fatal("approved label changed")
				}
				args := decoded["ProgramArguments"].([]any)
				if args[1] != spec.Args[1] {
					t.Fatal("approved arguments changed")
				}
				if load {
					wantBoot := []string{"/bin/launchctl", "bootstrap", "system", targetPath}
					if !slices.ContainsFunc(*helperRuns, func(c []string) bool { return slices.Equal(c, wantBoot) }) {
						t.Fatalf("expected bootstrap in %v", *helperRuns)
					}
				}
			})
		}
	}
}

func TestAuthorizedJobPayloadSizeLimit(t *testing.T) {
	dir := t.TempDir()
	_, helperRuns := stubRoot(t, dir)
	p := &Provider{Config{Dir: dir, Domain: "system", Elevated: true}}
	_, e := p.SaveAsRoot(Spec{Label: "com.example.large", Args: []string{"/bin/echo", strings.Repeat("x", 65<<10)}}, "", true)
	if e == nil || !strings.Contains(e.Error(), "size limit") || len(*helperRuns) != 0 {
		t.Fatal("oversized configuration reached helper", e)
	}
}

func TestRootOperationsRejectPathsOutsideProviderDirectory(t *testing.T) {
	dir := t.TempDir()
	_, helperRuns := stubRoot(t, dir)
	p := &Provider{Config{Source: service.Daemon, Dir: dir, Domain: "system", Elevated: true}}

	otherDir := t.TempDir()
	outsidePath := filepath.Join(otherDir, "outside.plist")
	if err := os.WriteFile(outsidePath, []byte(vendorJob), 0o644); err != nil {
		t.Fatal(err)
	}

	// 1. SaveAsRoot with existing pointing to a regular .plist in a different directory.
	spec, err := LoadSpec(outsidePath)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := p.SaveAsRoot(spec, outsidePath, false); err == nil || len(*helperRuns) != 0 {
		t.Fatalf("expected error and 0 helper calls for outside path, err: %v, calls: %d", err, len(*helperRuns))
	}

	// 2. SaveAsRoot with existing pointing to a symlink inside p.Dir.
	targetPath := filepath.Join(dir, "target.plist")
	if err := os.WriteFile(targetPath, []byte(vendorJob), 0o644); err != nil {
		t.Fatal(err)
	}
	symlinkPath := filepath.Join(dir, "link.plist")
	if err := os.Symlink(targetPath, symlinkPath); err != nil {
		t.Fatal(err)
	}
	spec, err = LoadSpec(symlinkPath)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := p.SaveAsRoot(spec, symlinkPath, false); err == nil || len(*helperRuns) != 0 {
		t.Fatalf("expected error and 0 helper calls for symlink path, err: %v, calls: %d", err, len(*helperRuns))
	}

	// 3. p.Do with service.Remove pointing outside p.Dir.
	it := service.Item{Name: "com.vendor.helper", Path: outsidePath}
	if err := p.Do(it, service.Remove); err == nil || len(*helperRuns) != 0 {
		t.Fatalf("expected error and 0 helper calls for remove outside path, err: %v, calls: %d", err, len(*helperRuns))
	}
}

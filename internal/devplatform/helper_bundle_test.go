package devplatform

import (
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"
)

func signedHelperFixture(t *testing.T) (*Manager, string) {
	t.Helper()
	bundle := filepath.Join(t.TempDir(), "Signed fixture.app")
	macos := filepath.Join(bundle, "Contents", "MacOS")
	if e := os.MkdirAll(macos, 0700); e != nil {
		t.Fatal(e)
	}
	source, e := os.Executable()
	if e != nil {
		t.Fatal(e)
	}
	b, e := os.ReadFile(source)
	if e != nil {
		t.Fatal(e)
	}
	program := filepath.Join(macos, "fixture")
	if e = os.WriteFile(program, b, 0700); e != nil {
		t.Fatal(e)
	}
	info := `<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>xyz.jyeu.msm.signed-helper-test</string><key>CFBundleExecutable</key><string>fixture</string><key>CFBundlePackageType</key><string>APPL</string></dict></plist>`
	if e = os.WriteFile(filepath.Join(bundle, "Contents", "Info.plist"), []byte(info), 0600); e != nil {
		t.Fatal(e)
	}
	if out, e := exec.Command("/usr/bin/codesign", "--force", "--sign", "-", bundle).CombinedOutput(); e != nil {
		t.Fatal(e, string(out))
	}
	m := testManager(t)
	m.Program = program
	return m, bundle
}

func TestSignedBundleHelperExecutesAfterOriginalAppRemovalAndRejectsTampering(t *testing.T) {
	m, bundle := signedHelperFixture(t)
	p := Project{SchemaVersion: Schema, ID: "signed-worker", Name: "Signed worker", Directory: t.TempDir(), Services: []Service{{ID: "web", Command: []string{"/usr/bin/true"}}}}
	if e := m.SaveProject(context.Background(), p); e != nil {
		t.Fatal(e)
	}
	if out, e := m.Exec(context.Background(), p.ID, []string{"/usr/bin/printf", "%s", "signed worker ran"}); e != nil || out != "signed worker ran" {
		t.Fatal("signed bundle worker did not execute", out, e)
	}
	helper, e := m.helperExecutable("project-launcher")
	if e != nil || !strings.Contains(helper, ".app/Contents/MacOS/") {
		t.Fatal(helper, e)
	}
	if e = os.RemoveAll(bundle); e != nil {
		t.Fatal(e)
	}
	// Model an already registered stable launcher whose source App was moved.
	m.Program = helper
	if out, e := m.Exec(context.Background(), p.ID, []string{"/usr/bin/printf", "%s", "stable after move"}); e != nil || out != "stable after move" {
		t.Fatal("stable signed worker depended on original App", out, e)
	}
	info := filepath.Join(filepath.Dir(filepath.Dir(helper)), "Info.plist")
	if e = os.WriteFile(info, []byte("changed"), 0600); e != nil {
		t.Fatal(e)
	}
	if _, e = m.helperExecutable("command-worker"); e == nil {
		t.Fatal("accepted modified signed helper bundle")
	}
}

func TestSignedBundleHelpersPublishOneCompleteSnapshotConcurrently(t *testing.T) {
	m, _ := signedHelperFixture(t)
	var wg sync.WaitGroup
	paths := make(chan string, 3)
	errs := make(chan error, 3)
	for _, role := range []string{"proxy", "database-worker", "command-worker"} {
		wg.Add(1)
		go func(role string) {
			defer wg.Done()
			path, e := m.helperExecutable(role)
			paths <- path
			errs <- e
		}(role)
	}
	wg.Wait()
	close(paths)
	close(errs)
	for e := range errs {
		if e != nil {
			t.Fatal(e)
		}
	}
	first := ""
	for path := range paths {
		if first != "" && first != path {
			t.Fatal("helper roles did not share their signed snapshot", first, path)
		}
		first = path
	}
	entries, e := os.ReadDir(filepath.Join(m.Root, "helpers", "bundles"))
	if e != nil || len(entries) != 1 || !strings.HasSuffix(entries[0].Name(), ".app") {
		t.Fatal("partial or duplicate snapshot retained", entries, e)
	}
}

package devplatform

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

var testNodeReleases = []NodeRelease{
	{Version: "24.1.0"},
	{Version: "22.14.0", LTS: "Jod"},
	{Version: "22.3.1", LTS: "Jod"},
	{Version: "20.19.0", LTS: "Iron"},
	{Version: "20.1.0", LTS: "Iron"},
	{Version: "18.20.8", LTS: "Hydrogen"},
	{Version: "0.12.18"},
}

func TestPickNodeUnderstandsWhatNvmAndFnmAccept(t *testing.T) {
	for spec, want := range map[string]string{
		"latest":          "24.1.0",
		"node":            "24.1.0",
		"current":         "24.1.0",
		"lts":             "22.14.0",
		"lts/*":           "22.14.0",
		"lts/iron":        "20.19.0",
		"LTS/Hydrogen":    "18.20.8",
		"22":              "22.14.0",
		"v22":             "22.14.0",
		"22.3":            "22.3.1",
		"22.3.1":          "22.3.1",
		"20.x":            "20.19.0",
		"^20.1.0":         "20.19.0",
		"~20.1":           "20.1.0",
		">=18 <21":        "20.19.0",
		">= 18 < 21":      "20.19.0",
		"18 || 22.3":      "22.3.1",
		"18.0.0 - 20.5.0": "20.1.0",
		">22":             "24.1.0",
		"<=18":            "18.20.8",
		"*":               "24.1.0",
		"^0.12":           "0.12.18",
	} {
		got, ok, err := PickNode(spec, testNodeReleases)
		if err != nil || !ok || got.Version != want {
			t.Errorf("%q = %q, %v, %v; want %s", spec, got.Version, ok, err, want)
		}
	}
	for _, spec := range []string{"23", "19.x", "lts/gallium", ">=25", "^21.0.0"} {
		if got, ok, err := PickNode(spec, testNodeReleases); err != nil || ok {
			t.Errorf("%q matched %q, %v", spec, got.Version, err)
		}
	}
	for _, spec := range []string{"", "banana", "1.2.3.4", "^x.y"} {
		if _, _, err := PickNode(spec, testNodeReleases); err == nil {
			t.Errorf("%q was accepted", spec)
		}
	}
}

func nodeIndexServer(t *testing.T, calls *int) {
	t.Helper()
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		*calls++
		files := []string{"osx-arm64-tar", "osx-x64-tar"}
		rows := []map[string]any{
			{"version": "v24.1.0", "date": "2026-01-01", "files": files, "npm": "11.0.0", "lts": false},
			{"version": "v22.14.0", "date": "2025-02-01", "files": files, "npm": "10.9.2", "lts": "Jod", "security": true},
			{"version": "v20.19.0", "date": "2025-03-01", "files": files, "lts": "Iron"},
			{"version": "v10.0.0", "date": "2018-01-01", "files": []string{"osx-x64-tar"}, "lts": false},
		}
		json.NewEncoder(w).Encode(rows)
	}))
	t.Cleanup(server.Close)
	nodeDistOverride = server.URL + "/"
	t.Cleanup(func() { nodeDistOverride = "" })
}

func TestNodeReleasesComeFromNodejsOrgKeepAppleSiliconAndCache(t *testing.T) {
	calls := 0
	nodeIndexServer(t, &calls)
	m := New(t.TempDir())
	releases, stale, err := m.NodeReleases(context.Background(), false)
	if err != nil || stale || len(releases) != 3 {
		t.Fatalf("%v %v %+v", err, stale, releases)
	}
	if releases[0].Version != "24.1.0" || releases[1].LTS != "Jod" || !releases[1].Security || releases[0].LTS != "" {
		t.Fatalf("%+v", releases)
	}
	if _, _, err = m.NodeReleases(context.Background(), false); err != nil || calls != 1 {
		t.Fatalf("a fresh cache was not used: %d calls, %v", calls, err)
	}
	if _, _, err = m.NodeReleases(context.Background(), true); err != nil || calls != 2 {
		t.Fatalf("refresh did not fetch: %d calls, %v", calls, err)
	}
	// Offline, the last list is still there, and says so.
	nodeDistOverride = "http://127.0.0.1:1/"
	releases, stale, err = m.NodeReleases(context.Background(), true)
	if err != nil || !stale || len(releases) != 3 {
		t.Fatalf("offline: %v %v %d", err, stale, len(releases))
	}
	if _, _, err = New(t.TempDir()).NodeReleases(context.Background(), false); err == nil {
		t.Fatal("no cache and no network is an error")
	}
}

func TestResolveNodeSpecRefreshesAStaleIndexForNewerReleases(t *testing.T) {
	calls := 0
	nodeIndexServer(t, &calls)
	m := New(t.TempDir())
	old := []NodeRelease{{Version: "20.19.0", LTS: "Iron"}}
	if err := writeJSON(m.nodeIndexPath(), nodeIndexCache{FetchedAt: time.Now(), Releases: old}); err != nil {
		t.Fatal(err)
	}
	r, err := m.ResolveNodeSpec(context.Background(), "22")
	if err != nil || r.Version != "22.14.0" || calls != 1 {
		t.Fatalf("%+v %v %d", r, err, calls)
	}
	if _, err = m.ResolveNodeSpec(context.Background(), "23"); err == nil || !strings.Contains(err.Error(), "no Node.js release") {
		t.Fatal(err)
	}
}

func installedNodeFixture(t *testing.T, m *Manager, versions ...string) {
	t.Helper()
	s := emptyState()
	for _, v := range versions {
		p := Package{Name: "node", Version: v, Arch: "arm64", Format: "tar.gz", Binaries: map[string]string{"node": "bin/node"}}
		s.Packages[p.ID()] = p
		if err := os.MkdirAll(filepath.Join(m.packageDir(p), "bin"), 0700); err != nil {
			t.Fatal(err)
		}
	}
	if err := m.withState(context.Background(), func(cur *State) error { *cur = s; return nil }); err != nil {
		t.Fatal(err)
	}
}

func TestSelectNodeFollowsOverrideFilesThenDefault(t *testing.T) {
	m := New(t.TempDir())
	installedNodeFixture(t, m, "22.14.0", "20.19.0", "18.20.8")
	if _, err := m.SelectNode(t.TempDir(), ""); err != ErrNoNodeDefault {
		t.Fatalf("no default: %v", err)
	}
	if err := m.SetNodeDefault(context.Background(), "node@22.14.0-arm64"); err != nil {
		t.Fatal(err)
	}
	if target, err := os.Readlink(m.nodeDefaultLink()); err != nil || target != filepath.Join(m.Root, "packages", "node@22.14.0-arm64") {
		t.Fatalf("link %q %v", target, err)
	}
	root := t.TempDir()
	project := filepath.Join(root, "app", "src", "deep")
	if err := os.MkdirAll(project, 0700); err != nil {
		t.Fatal(err)
	}
	check := func(dir, override, wantVersion, wantSource string) {
		t.Helper()
		sel, err := m.SelectNode(dir, override)
		if err != nil || sel.Version != wantVersion || !strings.HasSuffix(sel.Source, wantSource) {
			t.Fatalf("%s %q: %+v %v; want %s from %s", dir, override, sel, err, wantVersion, wantSource)
		}
	}
	check(project, "", "22.14.0", "default")
	os.WriteFile(filepath.Join(root, "app", "package.json"), []byte(`{"engines":{"node":">=18 <19"}}`), 0600)
	check(project, "", "18.20.8", "package.json")
	os.WriteFile(filepath.Join(root, "app", ".nvmrc"), []byte("# the lts\nv20\n"), 0600)
	check(project, "", "20.19.0", ".nvmrc")
	os.WriteFile(filepath.Join(root, "app", "src", ".node-version"), []byte("18.20.8\n"), 0600)
	check(project, "", "18.20.8", ".node-version")
	check(project, "20", "20.19.0", "the requested version")

	os.WriteFile(filepath.Join(root, "app", "src", ".node-version"), []byte("system"), 0600)
	sel, err := m.SelectNode(project, "")
	if err != nil || !sel.System || sel.Path() != "" {
		t.Fatalf("system: %+v %v", sel, err)
	}

	os.WriteFile(filepath.Join(root, "app", "src", ".node-version"), []byte("16\n"), 0600)
	var missing *NodeMissingError
	if _, err = m.SelectNode(project, ""); err == nil || !strings.Contains(err.Error(), "Software, Node.js") {
		t.Fatalf("missing: %v", err)
	} else if e, ok := err.(*NodeMissingError); ok {
		missing = e
	}
	if missing == nil || missing.Spec != "16" {
		t.Fatalf("%v", err)
	}
	if _, err = m.SelectNode(project, "banana"); err == nil {
		t.Fatal("a bad override was accepted")
	}

	sel, err = m.SelectNode(t.TempDir(), "")
	if err != nil || sel.Path() != filepath.Join(m.nodeGlobalPrefix("22.14.0"), "bin")+":"+filepath.Join(m.Root, "packages", "node@22.14.0-arm64", "bin") {
		t.Fatalf("path %q %v", sel.Path(), err)
	}
	if sel.Prefix == filepath.Join(m.Root, "packages", "node@22.14.0-arm64") {
		t.Fatal("global packages go into the verified installation")
	}
}

func TestUninstallingNodeClearsDefaultAndGlobalPackages(t *testing.T) {
	m := New(t.TempDir())
	installedNodeFixture(t, m, "22.14.0", "20.19.0")
	if err := m.SetNodeDefault(context.Background(), "node@22.14.0-arm64"); err != nil {
		t.Fatal(err)
	}
	global := filepath.Join(m.nodeGlobalPrefix("22.14.0"), "lib", "node_modules", "typescript")
	if err := os.MkdirAll(global, 0700); err != nil {
		t.Fatal(err)
	}
	if err := m.Uninstall(context.Background(), "node@22.14.0-arm64"); err != nil {
		t.Fatal(err)
	}
	s, _ := m.Read()
	if s.NodeDefault != "" {
		t.Fatalf("default %q", s.NodeDefault)
	}
	if _, err := os.Lstat(m.nodeDefaultLink()); !os.IsNotExist(err) {
		t.Fatalf("the default link stays: %v", err)
	}
	if _, err := os.Stat(global); !os.IsNotExist(err) {
		t.Fatal("global packages of a removed version stay")
	}
	if _, err := os.Stat(filepath.Join(m.Root, "packages", "node@20.19.0-arm64")); err != nil {
		t.Fatal("another version was touched")
	}
	if err := m.Uninstall(context.Background(), "node@22.14.0-arm64"); err == nil {
		t.Fatal("uninstalled twice")
	}
}

func TestNodeShellBlockIsAddedOnceAndRemovedCleanly(t *testing.T) {
	m := New(t.TempDir())
	m.Home = t.TempDir()
	m.Program = filepath.Join(t.TempDir(), "app")
	if err := os.WriteFile(m.Program, []byte("#!/bin/sh\n"), 0700); err != nil {
		t.Fatal(err)
	}
	rc := filepath.Join(m.Home, ".zshrc")
	original := "export EDITOR=vim\nalias ll='ls -l'"
	os.WriteFile(rc, []byte(original), 0640)
	if m.NodeShellEnabled() {
		t.Fatal("enabled before anything was written")
	}
	for i := 0; i < 2; i++ {
		if _, err := m.EnableNodeShell(context.Background()); err != nil {
			t.Fatal(err)
		}
	}
	b, _ := os.ReadFile(rc)
	if strings.Count(string(b), nodeBlockBegin) != 1 || !strings.HasPrefix(string(b), original+"\n") || !m.NodeShellEnabled() {
		t.Fatalf("%q", b)
	}
	if st, _ := os.Stat(rc); st.Mode().Perm() != 0640 {
		t.Fatalf("mode %v", st.Mode())
	}
	init, err := os.ReadFile(m.nodeInitPath())
	if err != nil || strings.Contains(string(init), "msm-node") || strings.Contains(string(init), "MSM_NODE_VERSION") || !strings.Contains(string(init), m.nodeWrapperPath()) {
		t.Fatalf("%v %q", err, init)
	}
	for _, shell := range []string{"/bin/zsh", "/bin/bash"} {
		if _, err := os.Stat(shell); err == nil {
			if out, err := exec.Command(shell, "-n", m.nodeInitPath()).CombinedOutput(); err != nil {
				t.Fatalf("%s rejects the setup script: %v %s", shell, err, out)
			}
		}
	}
	wrapper, _ := os.ReadFile(m.nodeWrapperPath())
	if !strings.Contains(string(wrapper), "--dev") {
		t.Fatalf("%q", wrapper)
	}
	if _, err = m.DisableNodeShell(context.Background()); err != nil {
		t.Fatal(err)
	}
	b, _ = os.ReadFile(rc)
	if strings.TrimRight(string(b), "\n") != original || m.NodeShellEnabled() {
		t.Fatalf("%q", b)
	}
	// With no .zshrc at all, enabling creates one and disabling leaves nothing of ours.
	os.Remove(rc)
	if _, err = m.EnableNodeShell(context.Background()); err != nil || !m.NodeShellEnabled() {
		t.Fatal(err)
	}
	if _, err = m.DisableNodeShell(context.Background()); err != nil || m.NodeShellEnabled() {
		t.Fatal(err)
	}
}

func TestNodeMirrorMustBeHTTPS(t *testing.T) {
	for _, bad := range []string{"http://mirror.example/node", "ftp://x", "https://user:pw@mirror.example/"} {
		t.Setenv("MSM_NODE_DIST_MIRROR", bad)
		if _, err := nodeDist(); err == nil {
			t.Errorf("%q was accepted", bad)
		}
		if _, err := fetchNodeReleases(context.Background()); err == nil {
			t.Errorf("%q was fetched", bad)
		}
	}
	t.Setenv("MSM_NODE_DIST_MIRROR", "https://mirror.example/node")
	if dist, err := nodeDist(); err != nil || dist != "https://mirror.example/node/" {
		t.Fatalf("%q %v", dist, err)
	}
	t.Setenv("MSM_NODE_DIST_MIRROR", "")
	if dist, _ := nodeDist(); dist != "https://nodejs.org/dist/" {
		t.Fatalf("default %q", dist)
	}
}

func TestTheFirstNodeInstalledBecomesTheDefault(t *testing.T) {
	m := New(t.TempDir())
	first := Package{Name: "node", Version: "22.14.0", Arch: "arm64", Format: "tar.gz"}
	second := Package{Name: "node", Version: "20.19.0", Arch: "arm64", Format: "tar.gz"}
	s := emptyState()
	s.Packages[first.ID()], s.Packages[second.ID()] = first, second
	if err := m.withState(context.Background(), func(cur *State) error { *cur = s; return nil }); err != nil {
		t.Fatal(err)
	}
	for _, p := range []Package{first, second} {
		if err := m.defaultFirstNode(context.Background(), p); err != nil {
			t.Fatal(err)
		}
	}
	got, _ := m.Read()
	if got.NodeDefault != first.ID() {
		t.Fatalf("default %q", got.NodeDefault)
	}
	other := Package{Name: "redis", Version: "7.4.7", Arch: "arm64"}
	if err := m.defaultFirstNode(context.Background(), other); err != nil {
		t.Fatal(err)
	}
}

func shellLinkFixture(t *testing.T) (*Manager, string, string) {
	t.Helper()
	m := New(t.TempDir())
	m.Home = t.TempDir()
	m.Program = filepath.Join(t.TempDir(), "fixture-app")
	if e := os.WriteFile(m.Program, []byte("#!/bin/sh\n"), 0700); e != nil {
		t.Fatal(e)
	}
	return m, filepath.Join(m.Home, ".zshrc"), filepath.Join(m.Home, "managed-rc")
}
func TestNodeShellSymlinkLifecycle(t *testing.T) {
	for _, kind := range []string{"relative", "absolute", "chain"} {
		t.Run(kind, func(t *testing.T) {
			m, rc, target := shellLinkFixture(t)
			original := []byte("export EDITOR=vim\n")
			if e := os.WriteFile(target, original, 0640); e != nil {
				t.Fatal(e)
			}
			link := target
			if kind == "relative" {
				link = filepath.Base(target)
			}
			if kind == "chain" {
				link = filepath.Join(m.Home, "indirect")
				if e := os.Symlink(target, link); e != nil {
					t.Fatal(e)
				}
			}
			if e := os.Symlink(link, rc); e != nil {
				t.Fatal(e)
			}
			for _, enable := range []bool{true, true, false, false} {
				var e error
				if enable {
					_, e = m.EnableNodeShell(context.Background())
				} else {
					_, e = m.DisableNodeShell(context.Background())
				}
				if e != nil {
					t.Fatal(e)
				}
				got, e := os.Readlink(rc)
				if e != nil || got != link {
					t.Fatal("symlink replaced", e)
				}
				b, e := os.ReadFile(target)
				if e != nil {
					t.Fatal(e)
				}
				st, e := os.Stat(target)
				if e != nil || st.Mode().Perm() != 0640 {
					t.Fatal("permissions changed", e)
				}
				if m.NodeShellEnabled() != enable {
					t.Fatal("enabled state differs")
				}
				if enable {
					if strings.Count(string(b), nodeBlockBegin) != 1 || !strings.HasPrefix(string(b), string(original)) {
						t.Fatal("bad integration block")
					}
				} else if string(b) != string(original) {
					t.Fatal("failed to remove only block")
				}
			}
		})
	}
}
func TestNodeShellSymlinkRejectsUnsafeTarget(t *testing.T) {
	for _, kind := range []string{"dangling", "cycle", "directory"} {
		for _, enable := range []bool{true, false} {
			t.Run(fmt.Sprintf("%s-enable-%t", kind, enable), func(t *testing.T) {
				m, rc, target := shellLinkFixture(t)
				if kind == "cycle" {
					target = rc
				}
				if kind == "directory" {
					if e := os.Mkdir(target, 0700); e != nil {
						t.Fatal(e)
					}
				}
				if e := os.Symlink(target, rc); e != nil {
					t.Fatal(e)
				}
				var e error
				if enable {
					_, e = m.EnableNodeShell(context.Background())
				} else {
					_, e = m.DisableNodeShell(context.Background())
				}
				if e == nil {
					t.Fatal("unsafe target accepted")
				}
				if got, e := os.Readlink(rc); e != nil || got != target {
					t.Fatal("link changed", e)
				}
			})
		}
	}
}
func TestNodeShellRejectsChangedConfiguration(t *testing.T) {
	for _, change := range []string{"content", "target", "link"} {
		for _, enable := range []bool{true, false} {
			t.Run(fmt.Sprintf("%s-enable-%t", change, enable), func(t *testing.T) {
				m, rc, target := shellLinkFixture(t)
				initial := []byte("initial\n")
				if !enable {
					initial = append(initial, []byte(m.nodeBlock())...)
				}
				if e := os.WriteFile(target, initial, 0640); e != nil {
					t.Fatal(e)
				}
				if e := os.Symlink(target, rc); e != nil {
					t.Fatal(e)
				}
				c, e := readNodeShellConfiguration(rc)
				if e != nil {
					t.Fatal(e)
				}
				switch change {
				case "content":
					e = os.WriteFile(target, []byte("changed\n"), 0640)
				case "target":
					e = os.Rename(target, target+".old")
					if e == nil {
						e = os.WriteFile(target, initial, 0640)
					}
				case "link":
					e = os.Remove(rc)
					if e == nil {
						e = os.Symlink(filepath.Base(target), rc)
					}
				}
				if e != nil {
					t.Fatal(e)
				}
				before, e := os.ReadFile(target)
				if e != nil {
					t.Fatal(e)
				}
				link, e := os.Readlink(rc)
				if e != nil {
					t.Fatal(e)
				}
				if e = c.commit([]byte("must not write")); e == nil {
					t.Fatal("overwrote changed configuration")
				}
				after, e := os.ReadFile(target)
				if e != nil || string(before) != string(after) {
					t.Fatal("changed target overwritten", e)
				}
				if got, e := os.Readlink(rc); e != nil || got != link {
					t.Fatal("changed link overwritten", e)
				}
			})
		}
	}
}

func TestNodeTerminalConfiguration(t *testing.T) {
	m := New(t.TempDir())
	m.Home = t.TempDir()
	m.Program = filepath.Join(t.TempDir(), "app")
	if err := os.WriteFile(m.Program, []byte("#!/bin/sh\n"), 0700); err != nil {
		t.Fatal(err)
	}
	rc := filepath.Join(m.Home, ".zshrc")
	if err := os.WriteFile(rc, []byte("export EDITOR=vim\n"), 0640); err != nil {
		t.Fatal(err)
	}
	if m.NodeShellEnabled() {
		t.Fatal("shell enabled before request")
	}
	for range 2 {
		if _, err := m.EnableNodeShell(context.Background()); err != nil {
			t.Fatal(err)
		}
	}
	data, err := os.ReadFile(rc)
	if err != nil || strings.Count(string(data), nodeBlockBegin) != 1 {
		t.Fatalf("non-idempotent shell block: %q %v", data, err)
	}
	if out, err := exec.Command("zsh", "-n", rc).CombinedOutput(); err != nil {
		t.Fatalf("invalid zshrc: %v %s", err, out)
	}
	if _, err := m.DisableNodeShell(context.Background()); err != nil {
		t.Fatal(err)
	}
	data, err = os.ReadFile(rc)
	if err != nil || string(data) != "export EDITOR=vim\n" {
		t.Fatalf("original rc changed: %q %v", data, err)
	}
	if m.NodeShellEnabled() {
		t.Fatal("shell remains enabled")
	}
}

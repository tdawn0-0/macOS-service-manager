package main

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"devbase/internal/devplatform"
)
// nodeFixtureReleases returns the fixed Node.js release list used by tests.
func nodeFixtureReleases() []devplatform.NodeRelease {
	return []devplatform.NodeRelease{
		{Version: "24.1.0"},
		{Version: "22.14.0", LTS: "Jod"},
		{Version: "20.19.0", LTS: "Iron"},
		{Version: "18.20.8", LTS: "Hydrogen"},
	}
}

func stubNodeReleases(h *devUIHarness) {
	base := h.a.dev.managerFactory
	h.a.dev.managerFactory = func() *devplatform.Manager {
		m := base()
		m.FetchNodeReleases = func(context.Context) ([]devplatform.NodeRelease, error) {
			return nodeFixtureReleases(), nil
		}
		return m
	}
}

// seedNodeWorkspace writes installed Node.js versions and a fresh copy of
// nodejs.org's release list, so that nothing here needs the network.
func seedNodeWorkspace(t *testing.T, h *devUIHarness, versions ...string) {
	t.Helper()
	state := devplatform.State{SchemaVersion: devplatform.Schema, Packages: map[string]devplatform.Package{}, Instances: map[string]devplatform.Instance{}, ProxyRules: []devplatform.ProxyRule{}, ProxyListenerPort: 443}
	for _, v := range versions {
		p := devplatform.Package{Name: "node", Version: v, Arch: "arm64", Format: "tar.gz", URL: "https://nodejs.org/dist/v" + v + "/node-v" + v + "-darwin-arm64.tar.gz", SHA256: strings.Repeat("ab", 32), Strip: 1, Binaries: map[string]string{"node": "bin/node"}}
		state.Packages[p.ID()] = p
	}
	write := func(name string, v any) {
		b, e := json.Marshal(v)
		if e != nil {
			t.Fatal(e)
		}
		path := filepath.Join(h.root, name)
		if e = os.MkdirAll(filepath.Dir(path), 0700); e != nil {
			t.Fatal(e)
		}
		if e = os.WriteFile(path, b, 0600); e != nil {
			t.Fatal(e)
		}
	}
	write("state.json", state)
	write("cache/node-index.json", map[string]any{"fetchedAt": time.Now().UTC(), "releases": nodeFixtureReleases()})
}
// drain runs what background work posted, until cond holds.
func (h *devUIHarness) drain(cond func() bool) {
	h.t.Helper()
	deadline := time.After(5 * time.Second)
	for !cond() {
		select {
		case f := <-h.posted:
			f()
			h.ui.Frame()
		case <-deadline:
			h.t.Fatal("background work did not finish", h.ui.Texts())
		}
	}
}

func TestUINodeVersionsListDefaultAliasesAndTerminal(t *testing.T) {
	for _, size := range [][2]int{{1120, 720}, {900, 520}} {
		h := newDevUIHarnessSize(t, size[0], size[1], false)
		stubNodeReleases(h)
		seedNodeWorkspace(t, h, "20.19.0", "22.14.0")
		h.click("Refresh") // reads the state written above
		h.a.dev.nodeReleasesTried, h.a.dev.nodeReleasesRefresh = false, false
		h.click("Software")
		h.drain(func() bool { return len(h.a.dev.nodeReleases) == 4 })

		// The newest release of each major, with the long-term support lines named.
		if h.a.dev.version == "" || h.a.dev.nodeReleasesStale || h.a.dev.nodeReleasesError != "" {
			t.Fatalf("version %q stale %v %q", h.a.dev.version, h.a.dev.nodeReleasesStale, h.a.dev.nodeReleasesError)
		}
		if got := h.a.defaultVersion("node"); got != "22.14.0" {
			t.Fatalf("the first version offered is the newest LTS, not %q", got)
		}

		h.click("Set default")
		if h.a.dev.state.NodeDefault != "node@20.19.0-arm64" {
			t.Fatalf("default %q", h.a.dev.state.NodeDefault)
		}
		if !h.ui.HasText("Default") {
			t.Fatal("the default is not marked", h.ui.Texts())
		}
		h.click("Set default")
		if h.a.dev.state.NodeDefault != "node@22.14.0-arm64" {
			t.Fatalf("default %q", h.a.dev.state.NodeDefault)
		}

		// A name resolves to the exact release before anything is installed.
		h.fill("Exact version", "lts/iron")
		h.click("Review package")
		if h.a.dev.version != "20.19.0" || h.a.dev.packagePreview == nil || h.a.dev.packagePreview.Package.Version != "20.19.0" {
			t.Fatalf("lts/iron became %q", h.a.dev.version)
		}
		h.fill("Exact version", "22")
		h.click("Review package")
		if h.a.dev.version != "22.14.0" {
			t.Fatalf("22 became %q", h.a.dev.version)
		}

		// The terminal setup covers every zsh startup mode, and removes all blocks.
		home := h.a.dev.managerFactory().Home
		h.click("Enable in terminal")
		for _, name := range []string{".zshenv", ".zprofile", ".zshrc"} {
			b, e := os.ReadFile(filepath.Join(home, name))
			if e != nil || !strings.Contains(string(b), "Devbase Node.js") {
				t.Fatalf("%s: %v %q", name, e, b)
			}
		}
		if !h.ui.HasText("Enabled for detected shells") {
			t.Fatal("enabled is not shown", h.ui.Texts())
		}
		h.click("Copy Bash command")
		if len(h.copied) == 0 || h.copied[len(h.copied)-1] != h.a.dev.managerFactory().BashCommand() {
			t.Fatal("Bash command was not copied")
		}
		h.click("Disable in terminal")
		for _, name := range []string{".zshenv", ".zprofile", ".zshrc"} {
			if b, _ := os.ReadFile(filepath.Join(home, name)); strings.Contains(string(b), "Devbase Node.js") {
				t.Fatalf("%s: %q", name, b)
			}
		}
		if _, ok := h.ui.Find("Enable in terminal"); !ok {
			t.Fatal("the setup cannot be enabled again", h.ui.Texts())
		}
	}
}

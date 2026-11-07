package main

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"macos-service-manager/internal/devplatform"
)

// seedNodeWorkspace writes installed Node.js versions and a fresh copy of
// nodejs.org's release list, so that nothing here needs the network.
func seedNodeWorkspace(t *testing.T, h *devUIHarness, versions ...string) {
	t.Helper()
	state := devplatform.State{SchemaVersion: devplatform.Schema, Packages: map[string]devplatform.Package{}, Projects: map[string]devplatform.Project{}, Instances: map[string]devplatform.Instance{}, Sites: map[string]devplatform.Site{}}
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
	write("cache/node-index.json", map[string]any{"fetchedAt": time.Now().UTC(), "releases": []devplatform.NodeRelease{
		{Version: "24.1.0"}, {Version: "22.14.0", LTS: "Jod"}, {Version: "20.19.0", LTS: "Iron"}, {Version: "18.20.8", LTS: "Hydrogen"},
	}})
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

		h.click("Set default 22.14.0")
		if h.a.dev.state.NodeDefault != "node@22.14.0-arm64" {
			t.Fatalf("default %q", h.a.dev.state.NodeDefault)
		}
		if !h.ui.HasText("Default") {
			t.Fatal("the default is not marked", h.ui.Texts())
		}
		if _, ok := h.ui.Find("Set default 22.14.0"); ok {
			t.Fatal("the default offers to become the default")
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

		// The terminal setup adds one block to .zshrc, and takes it away.
		rc := filepath.Join(h.a.dev.managerFactory().Home, ".zshrc")
		h.click("Enable in terminal")
		b, e := os.ReadFile(rc)
		if e != nil || !strings.Contains(string(b), "Service Manager Node.js") {
			t.Fatalf("%v %q", e, b)
		}
		if !h.ui.HasText("Enabled in ~/.zshrc") {
			t.Fatal("enabled is not shown", h.ui.Texts())
		}
		h.click("Disable in terminal")
		if b, _ = os.ReadFile(rc); strings.Contains(string(b), "Service Manager Node.js") {
			t.Fatalf("%q", b)
		}
		if _, ok := h.ui.Find("Enable in terminal"); !ok {
			t.Fatal("the setup cannot be enabled again", h.ui.Texts())
		}
	}
}

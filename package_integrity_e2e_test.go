package main

import (
	"bytes"
	"context"
	"macos-service-manager/internal/devplatform"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func TestUICheckRepairRealNodeAndRefuseRunningUsers(t *testing.T) {
	if os.Getenv("MSM_UI_E2E_PACKAGE_CACHE") == "" {
		t.Skip("set MSM_UI_E2E_PACKAGE_CACHE to verified Node archives")
	}
	h := newDevUIHarnessSize(t, 900, 520, true)
	pkg, err := devplatform.Resolve(context.Background(), "node", "20.19.0")
	if err != nil {
		t.Fatal(err)
	}
	seedRuntimeCache(t, h.root, pkg)
	h.click("Software")
	h.fill("Exact version", "20.19.0")
	h.click("Review package")
	h.click("Download and install")
	id := pkg.ID()
	if _, ok := h.a.dev.state.Packages[id]; !ok {
		t.Fatal("runtime install failed", h.a.dev.output)
	}
	h.click("Check files node 20.19.0")
	if r := h.a.dev.packageChecks[id]; r.Status != "Verified" || r.Files < 100 {
		t.Fatal("UI did not check the installed tree", r, h.a.dev.output)
	}
	packageDir := filepath.Join(h.root, "packages", id)
	dataFile := filepath.Join(packageDir, "lib/node_modules/npm/package.json")
	original, err := os.ReadFile(dataFile)
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(dataFile, append(append([]byte{}, original...), '\n'), 0644); err != nil {
		t.Fatal(err)
	}
	h.click("Check files node 20.19.0")
	if r := h.a.dev.packageChecks[id]; r.Status != "Needs repair" || !strings.Contains(r.Text(), "lib/node_modules/npm/package.json") {
		t.Fatal("UI hid non-binary package damage", r)
	}
	sentinel := filepath.Join(h.root, "instances", "retained-data", "sentinel")
	if err = os.MkdirAll(filepath.Dir(sentinel), 0700); err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(sentinel, []byte("database-data"), 0600); err != nil {
		t.Fatal(err)
	}
	h.click("Repair node 20.19.0")
	if r := h.a.dev.packageChecks[id]; r.Status != "Verified" {
		t.Fatal("UI repair failed", r, h.a.dev.output)
	}
	if b, err := os.ReadFile(dataFile); err != nil || !bytes.Equal(b, original) {
		t.Fatal("repair did not restore original package bytes", err)
	}
	var revealed string
	h.a.reveal = func(path string) { revealed = path }
	h.click("Reveal package folder node 20.19.0")
	if revealed != packageDir {
		t.Fatal("revealed a different workspace package", revealed)
	}
	if out, err := exec.Command(filepath.Join(packageDir, "bin/node"), "--version").Output(); err != nil || strings.TrimSpace(string(out)) != "v20.19.0" {
		t.Fatal("repair changed runtime version", string(out), err)
	}
	if b, err := os.ReadFile(sentinel); err != nil || string(b) != "database-data" {
		t.Fatal("repair touched database files", err)
	}
}

package main

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"strings"
	"testing"

	"devbase/internal/devplatform"
)

// TestNativeResourceProduct is required by the disposable-runner release gate.
// Its children exercise real installed binaries and app-owned jobs; the gate
// rejects any SKIP, including a missing verified package cache.
func TestNativeResourceProduct(t *testing.T) {
	if os.Getenv("MSM_UI_E2E_PACKAGE_CACHE") == "" {
		t.Skip("verified package cache is required for native resource acceptance")
	}
	t.Run("node-terminal", func(t *testing.T) {
		h := newDevUIHarness(t)
		pkg, err := devplatform.Resolve(context.Background(), "node", "22.14.0")
		if err != nil {
			t.Fatal(err)
		}
		seedRuntimeCache(t, h.root, pkg)
		h.click("Software")
		h.fill("Exact version", "22.14.0")
		h.click("Review package")
		h.click("Download and install")
		if _, ok := h.a.dev.state.Packages[pkg.ID()]; !ok {
			t.Fatal("managed Node installation missing")
		}
		m := h.a.dev.managerFactory()
		if err := m.SetNodeDefault(context.Background(), pkg.ID()); err != nil {
			t.Fatal(err)
		}
		if _, err := m.EnableNodeShell(context.Background()); err != nil {
			t.Fatal(err)
		}
		defer m.DisableNodeShell(context.Background())
		cmd := exec.Command("zsh", "-ic", "node --version")
		cmd.Dir = t.TempDir()
		cmd.Env = append(os.Environ(), "HOME="+m.Home, "ZDOTDIR="+m.Home)
		out, err := cmd.CombinedOutput()
		if err != nil || strings.TrimSpace(string(out)) != "v22.14.0" {
			t.Fatalf("fresh terminal did not use managed Node: %v %q", err, out)
		}
	})
	t.Run("external-upstream-https", func(t *testing.T) {
		TestUIStandaloneRuntimeAndHTTPS(t)
	})
	t.Run("resource-quit", func(t *testing.T) {
		TestUIQuitKeepsServicesOrStopsOwnedScopeAndRetriesFailure(t)
		h := newDevUIHarness(t)
		pkg, err := devplatform.Resolve(context.Background(), "redis", "7.4.7")
		if err != nil {
			t.Fatal(err)
		}
		seedRuntimeCache(t, h.root, pkg)
		h.click("Software")
		h.click("Software redis")
		h.click("Review package")
		h.click("Download and install")
		h.click("Databases")
		h.click("New database instance")
		h.fill("Instance name", "quit-native")
		h.choose("Database engine", "redis")
		h.fill("Local port", fmt.Sprint(freeUIPort(t)))
		h.click("Create instance")
		m := devplatform.New(h.root)
		defer m.InstanceAction(context.Background(), "quit-native", "remove")
		h.click("Start")
		plan, err := m.PreviewShutdown(context.Background())
		if err != nil || len(plan.Instances) != 1 || plan.Instances[0].ID != "quit-native" {
			t.Fatalf("quit scope missed managed database: %v %+v", err, plan.Instances)
		}
		if err := m.Shutdown(context.Background(), plan, devplatform.ShutdownOptions{}); err != nil {
			t.Fatal(err)
		}
		state, err := m.Read()
		if err != nil || state.Instances["quit-native"].ID != "quit-native" {
			t.Fatal("quit lost database configuration", err)
		}
	})
	t.Run("connection-profiles", func(t *testing.T) {
		h := newDevUIHarness(t)
		pkg, err := devplatform.Resolve(context.Background(), "redis", "7.4.7")
		if err != nil {
			t.Fatal(err)
		}
		seedRuntimeCache(t, h.root, pkg)
		h.click("Software")
		h.click("Software redis")
		h.click("Review package")
		h.click("Download and install")
		if _, ok := h.a.dev.state.Packages[pkg.ID()]; !ok {
			t.Fatal("managed Redis installation missing")
		}
		h.click("Databases")
		h.click("New database instance")
		h.fill("Instance name", "profile-native")
		h.choose("Database engine", "redis")
		port := freeUIPort(t)
		h.fill("Local port", fmt.Sprint(port))
		h.click("Create instance")
		m := devplatform.New(h.root)
		defer m.InstanceAction(context.Background(), "profile-native", "remove")
		if _, ok := h.a.dev.state.Instances["profile-native"]; !ok {
			t.Fatal("Redis instance missing")
		}
		h.click("Start")
		defer m.InstanceAction(context.Background(), "profile-native", "stop")
		p, err := m.SaveConnectionProfile(context.Background(), devplatform.ConnectionProfile{InstanceID: "profile-native", Name: "Business database", RedisDB: 3})
		if err != nil {
			t.Fatal(err)
		}
		info, err := m.ProfileConnectionInfo("profile-native", p.ID)
		if err != nil || !strings.Contains(info.URI, fmt.Sprintf("127.0.0.1:%d/3", port)) || strings.Contains(info.URI, "password") {
			t.Fatalf("profile URI did not use current instance: %v %q", err, info.URI)
		}
	})
}

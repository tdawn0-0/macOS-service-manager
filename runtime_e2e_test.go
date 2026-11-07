package main

import (
	"context"
	"crypto/tls"
	"crypto/x509"
	"fmt"
	"io"
	"macos-service-manager/internal/devplatform"
	"net"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func seedRuntimeCache(t *testing.T, root string, pkg devplatform.Package) {
	t.Helper()
	for _, dir := range filepath.SplitList(os.Getenv("MSM_UI_E2E_PACKAGE_CACHE")) {
		source, e := os.Open(filepath.Join(dir, pkg.SHA256))
		if e != nil {
			continue
		}
		defer source.Close()
		if e = os.MkdirAll(filepath.Join(root, "downloads"), 0700); e != nil {
			t.Fatal(e)
		}
		dest, e := os.OpenFile(filepath.Join(root, "downloads", pkg.SHA256), os.O_CREATE|os.O_TRUNC|os.O_WRONLY, 0600)
		if e != nil {
			t.Fatal(e)
		}
		_, e = io.Copy(dest, source)
		closeError := dest.Close()
		if e != nil || closeError != nil {
			t.Fatal(e, closeError)
		}
		return
	}
	t.Fatal("verified archive is missing from the configured package cache", pkg.ID())
}

func TestUIRuntimeDeclarationsReviewInstallAndHTTPS(t *testing.T) {
	if os.Getenv("MSM_UI_E2E_PACKAGE_CACHE") == "" {
		t.Skip("set MSM_UI_E2E_PACKAGE_CACHE to verified Node.js archives")
	}
	h := newDevUIHarness(t)
	for _, version := range []string{"22.14.0", "20.19.0"} {
		pkg, e := devplatform.Resolve(context.Background(), "node", version)
		if e != nil {
			t.Fatal(e)
		}
		seedRuntimeCache(t, h.root, pkg)
	}
	dir := t.TempDir()
	port, httpsPort := freeUIPort(t), freeUIPort(t)
	for name, value := range map[string]string{
		".nvmrc":        "v22.14.0\n",
		".node-version": "20.19.0\n",
		"package.json":  `{"scripts":{"dev":"node server.js"}}`,
		"server.js":     fmt.Sprintf(`require("fs").writeFileSync("started","yes"); require("http").createServer((q,s)=>s.end("managed-runtime "+process.version)).listen(%d,"127.0.0.1");`, port),
	} {
		if e := os.WriteFile(filepath.Join(dir, name), []byte(value), 0600); e != nil {
			t.Fatal(e)
		}
	}
	h.fill("Project folder", dir)
	h.click("Import project")
	id := h.a.dev.id
	manager := devplatform.New(h.root)
	defer func() {
		_ = manager.RemoveProject(context.Background(), id)
		_ = manager.StopProxy(context.Background())
	}()
	if !strings.Contains(strings.Join(h.ui.Texts(), " "), "declarations disagree") {
		t.Fatal("UI hid conflicting exact runtime declarations")
	}
	h.fill("Project name", "Versioned app")
	h.click("Choose Node.js 22.14.0 from .nvmrc")
	h.fill("Exact version", "v22.14.0")
	if h.a.dev.version != "v22.14.0" {
		t.Fatal("version input forced the preset list", h.a.dev.version)
	}
	h.click("Review package")
	if !h.a.dev.previewMatches() || !h.a.dev.packagePreview.Cached || h.a.dev.packagePreview.ArchiveBytes <= 0 || len(h.a.dev.state.Packages) != 0 {
		t.Fatal("package review did not show a verified archive or installed prematurely", h.a.dev.output)
	}
	h.fill("Exact version", "20.19.0")
	previous := h.a.dev.output
	h.click("Download and install")
	if h.a.dev.output != previous || len(h.a.dev.state.Packages) != 0 {
		t.Fatal("stale package review permitted installing a different version")
	}
	h.click("Review package")
	h.click("Copy package source")
	h.click("Copy package checksum")
	if len(h.copied) != 2 || !strings.Contains(h.copied[0], "v20.19.0") || len(h.copied[1]) != 64 {
		t.Fatal("review details did not match the selected version")
	}
	h.click("Download and install")
	wanted := "node@20.19.0-" + h.a.dev.packagePreview.Package.Arch
	if _, ok := h.a.dev.state.Packages[wanted]; !ok || h.a.dev.servicePackage != wanted {
		t.Fatal("installed runtime was not linked to the selected project", h.a.dev.output)
	}
	h.click("Back to project")
	h.click("Services")
	h.fill("Readiness check", fmt.Sprintf("tcp://127.0.0.1:%d", port))
	h.click("Save project")
	if h.a.dev.state.Projects[id].Services[0].Package != wanted {
		t.Fatal("runtime selection was not saved", h.a.dev.output)
	}
	command := exec.Command(h.a.dev.managerFactory().Program, "--dev", "exec", id, "node", "--version")
	command.Env = append(os.Environ(), "MSM_DEV_HOME="+h.root)
	if out, err := command.CombinedOutput(); err != nil || string(out) != "v20.19.0\n" {
		t.Fatal("CLI exec changed runtime or polluted command output", string(out), err)
	}
	if _, e := os.Stat(filepath.Join(dir, "started")); !os.IsNotExist(e) {
		t.Fatal("import, preview or runtime install ran a project script")
	}
	h.click("HTTPS")
	h.fill("Site name", "versioned")
	h.fill("Local domain", "versioned.test")
	h.fill("Upstream address", fmt.Sprintf("http://127.0.0.1:%d", port))
	h.fill("HTTPS port", fmt.Sprint(httpsPort))
	h.choose("Project", "Versioned app")
	h.click("Save HTTPS site")
	h.click("Projects")
	h.click("Start project")
	if strings.Contains(h.a.dev.output, "Error:") {
		t.Fatal(h.a.dev.output)
	}
	ca, e := os.ReadFile(filepath.Join(h.root, "proxy", "ca.pem"))
	if e != nil {
		t.Fatal(e)
	}
	roots := x509.NewCertPool()
	roots.AppendCertsFromPEM(ca)
	transport := &http.Transport{TLSClientConfig: &tls.Config{RootCAs: roots, ServerName: "versioned.test"}}
	defer transport.CloseIdleConnections()
	client := &http.Client{Transport: transport, Timeout: 5 * time.Second}
	req, _ := http.NewRequest("GET", fmt.Sprintf("https://127.0.0.1:%d/", httpsPort), nil)
	req.Host = "versioned.test"
	response, e := client.Do(req)
	if e != nil {
		t.Fatal(e)
	}
	body, e := io.ReadAll(response.Body)
	response.Body.Close()
	if e != nil || string(body) != "managed-runtime v20.19.0" {
		t.Fatal("HTTPS used a runtime other than the selected version", string(body), e)
	}
	h.click("Stop project")
	if conn, e := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", port), time.Second); e == nil {
		conn.Close()
		t.Fatal("stopping npm left its Node.js child listening")
	}
	for name, value := range map[string]string{".nvmrc": "v22.14.0\n", ".node-version": "20.19.0\n"} {
		b, e := os.ReadFile(filepath.Join(dir, name))
		if e != nil || string(b) != value {
			t.Fatal("runtime choice overwrote a repository version declaration", e)
		}
	}
}

func TestUIUnlistedExactNodeVersionReview(t *testing.T) {
	if os.Getenv("MSM_METADATA_NETWORK_TEST") != "1" {
		t.Skip("set MSM_METADATA_NETWORK_TEST=1 to verify live upstream version metadata")
	}
	h := newDevUIHarness(t)
	h.click("Software")
	h.fill("Exact version", "22.13.1")
	h.click("Review package")
	if !h.a.dev.previewMatches() || h.a.dev.packagePreview.Package.Version != "22.13.1" || h.a.dev.packagePreview.ArchiveBytes <= 0 || len(h.a.dev.packagePreview.Package.SHA256) != 64 || !strings.Contains(h.a.dev.packagePreview.Package.URL, "/v22.13.1/") || len(h.a.dev.state.Packages) != 0 {
		t.Fatal("unlisted exact version was replaced or its metadata is incomplete", h.a.dev.output)
	}
	h.fill("Exact version", "2300.0.0")
	h.click("Review package")
	if !strings.Contains(h.a.dev.output, "Error:") || h.a.dev.previewMatches() || len(h.a.dev.state.Packages) != 0 {
		t.Fatal("unavailable exact version silently reused another package", h.a.dev.output)
	}
}

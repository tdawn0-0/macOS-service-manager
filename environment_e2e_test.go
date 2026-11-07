package main

import (
	"context"
	"crypto/tls"
	"crypto/x509"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"macos-service-manager/internal/devplatform"
)

// All configuration changes use real UI input. The filesystem copy represents
// checking out the shared repository; no declaration is edited by the test.
func TestUISharedEnvironmentLocalSetupAndConcurrentHTTPS(t *testing.T) {
	for _, dark := range []bool{false, true} {
		t.Run(fmt.Sprintf("minimum-dark-%t", dark), func(t *testing.T) {
			h := newDevUIHarnessSize(t, 900, 520, dark)
			manager := h.a.dev.managerFactory()
			var ids []string
			defer func() {
				for _, id := range ids {
					_ = manager.Down(context.Background(), id)
				}
				_ = manager.StopProxy(context.Background())
			}()
			python, err := exec.LookPath("python3")
			if err != nil {
				t.Fatal(err)
			}
			dir := t.TempDir()
			if err = os.WriteFile(filepath.Join(dir, "index.html"), []byte("original-working-copy"), 0600); err != nil {
				t.Fatal(err)
			}
			h.fill("Project folder", dir)
			h.click("Import project")
			ids = append(ids, h.a.dev.id)
			h.fill("Project name", "Shared HTTPS project")
			h.click("Services")
			h.click("Add service")
			h.fill("Service name", "web")
			port := freeUIPort(t)
			h.fill("Program and arguments", fmt.Sprintf("%s\n-m\nhttp.server\n%d\n--bind\n127.0.0.1\n--directory=%s", python, port, dir))
			h.fill("Readiness check", fmt.Sprintf("http://127.0.0.1:%d", port))
			h.click("Environment")
			h.fill("Environment variables", "SECRET=source-private-value")
			h.click("Save project")
			h.click("HTTPS")
			h.fill("Site name", "shared")
			h.fill("Local domain", "shared-ui.test")
			h.fill("Upstream address", fmt.Sprintf("http://127.0.0.1:%d", port))
			httpsPort := freeUIPort(t)
			h.fill("HTTPS port", fmt.Sprint(httpsPort))
			h.choose("Project", "Shared HTTPS project")
			h.click("Save HTTPS site")
			h.click("Projects")
			h.click("Start project")
			if strings.Contains(h.a.dev.output, "Error:") {
				t.Fatal(h.a.dev.output)
			}
			h.click("Overview")
			h.click("Export environment")
			if strings.Contains(h.a.dev.output, "Error:") {
				t.Fatal(h.a.dev.output)
			}
			cloneDir := t.TempDir()
			for _, name := range []string{"devmanager.json", "devmanager.lock.json"} {
				b, err := os.ReadFile(filepath.Join(dir, name))
				if err != nil {
					t.Fatal(err)
				}
				if strings.Contains(string(b), python) || strings.Contains(string(b), "source-private-value") || strings.Contains(string(b), fmt.Sprint(port)) {
					t.Fatal("shared environment contains local values", name)
				}
				if err = os.WriteFile(filepath.Join(cloneDir, name), b, 0600); err != nil {
					t.Fatal(err)
				}
			}
			if err = os.WriteFile(filepath.Join(cloneDir, "index.html"), []byte("second-working-copy"), 0600); err != nil {
				t.Fatal(err)
			}
			h.click("Import a project")
			h.fill("Project folder", cloneDir)
			h.click("Import project")
			cloneID := h.a.dev.id
			ids = append(ids, cloneID)
			p := h.a.dev.state.Projects[cloneID]
			if len(p.MissingLocalSettings()) != 3 {
				t.Fatal("local setup not surfaced", p.MissingLocalSettings(), h.a.dev.output)
			}
			h.click("Start project")
			if h.a.dev.statuses["project:"+cloneID] == "Healthy" {
				t.Fatal("incomplete import was started")
			}
			h.click("Complete local setup")
			if h.a.dev.projectTab != 1 {
				t.Fatal("setup action did not open service fields")
			}
			h.fill("Local argument 1 · Executable path on this Mac", python)
			h.fill("Local argument 7 · Local path for argument 7", cloneDir)
			h.click("Environment")
			h.fill("Environment variables", "SECRET=clone-private-value")
			h.click("Save project")
			p = h.a.dev.state.Projects[cloneID]
			if len(p.MissingLocalSettings()) != 0 || p.Services[0].Command[0] != python {
				t.Fatal("local fields did not persist", p.MissingLocalSettings(), h.a.dev.output)
			}
			h.click("Start project")
			if strings.Contains(h.a.dev.output, "Error:") {
				t.Fatal(h.a.dev.output)
			}
			ca, err := os.ReadFile(filepath.Join(h.root, "proxy", "ca.pem"))
			if err != nil {
				t.Fatal(err)
			}
			pool := x509.NewCertPool()
			pool.AppendCertsFromPEM(ca)
			for n, id := range ids {
				p := h.a.dev.state.Projects[id]
				if len(p.Sites) != 1 {
					t.Fatalf("project lost site: %+v; %s", p, h.a.dev.output)
				}
				site := p.Sites[0]
				transport := &http.Transport{TLSClientConfig: &tls.Config{RootCAs: pool, ServerName: site.Domain}}
				client := &http.Client{Transport: transport, Timeout: 3 * time.Second}
				req, _ := http.NewRequest("GET", fmt.Sprintf("https://127.0.0.1:%d/", site.Port), nil)
				req.Host = site.Domain
				res, err := client.Do(req)
				if err != nil {
					t.Fatal(err)
				}
				b, _ := io.ReadAll(res.Body)
				res.Body.Close()
				transport.CloseIdleConnections()
				want := []string{"original-working-copy", "second-working-copy"}[n]
				if res.StatusCode != 200 || string(b) != want {
					t.Fatal("working copies routed incorrectly", site, string(b))
				}
			}
			h.click("Overview")
			h.click("Export environment")
			b, err := os.ReadFile(filepath.Join(cloneDir, "devmanager.json"))
			if err != nil {
				t.Fatal(err)
			}
			if strings.Contains(string(b), python) || strings.Contains(string(b), "clone-private-value") {
				t.Fatal("re-export leaked local override")
			}
			// A new manager models reopening the app. Private settings must survive.
			state, err := devplatform.New(h.root).Read()
			if err != nil {
				t.Fatal(err)
			}
			if state.Projects[cloneID].Services[0].LocalArguments[0].Value != python {
				t.Fatal("private override lost on reload")
			}
			h.click("Stop project")
		})
	}
}

func TestUILocalSecretArgumentSaveAndExport(t *testing.T) {
	h := newDevUIHarnessSize(t, 900, 520, true)
	m := h.a.dev.managerFactory()
	p := devplatform.Project{SchemaVersion: 1, ID: "private-args", Name: "Private arguments", Directory: t.TempDir(), Services: []devplatform.Service{{ID: "web", Command: []string{"/usr/bin/true", devplatform.LocalArgumentPlaceholder(1)}, LocalArguments: []devplatform.LocalArgument{{Index: 1, Kind: "secret", Label: "API token", Prefix: "--token="}}}}}
	if err := m.SaveProject(context.Background(), p); err != nil {
		t.Fatal(err)
	}
	h.click("Refresh")
	h.click("Project Private arguments")
	h.click("Complete local setup")
	h.fill("Local argument 2 · API token", "private-ui-token")
	h.click("Save project")
	saved := h.a.dev.state.Projects[p.ID].Services[0]
	if saved.Command[1] != "--token=private-ui-token" || saved.LocalArguments[0].Value != "private-ui-token" {
		t.Fatal("private argument or prefix not saved", h.a.dev.output)
	}
	if strings.Contains(h.a.dev.arguments, "private-ui-token") {
		t.Fatal("secret duplicated into command editor")
	}
	h.click("Overview")
	h.click("Export environment")
	b, err := os.ReadFile(filepath.Join(p.Directory, "devmanager.json"))
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(b), "private-ui-token") {
		t.Fatal("UI export disclosed local secret")
	}
}

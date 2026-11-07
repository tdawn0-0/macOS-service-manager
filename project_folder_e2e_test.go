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

	"github.com/egoist/mygo/ui"
)

func reopenProjectUI(t *testing.T, previous *devUIHarness, dark bool) *devUIHarness {
	t.Helper()
	h := &devUIHarness{t: t, root: previous.root, posted: make(chan func(), 256), width: 900, height: 520}
	a := newAppWith(nil, nil, t.TempDir())
	h.a = a
	a.nav = "development"
	a.post = func(f func()) { h.posted <- f }
	a.spawn = func(f func()) { go f() }
	a.dev.managerFactory = previous.a.dev.managerFactory
	h.ui = ui.NewTester(a.view, h.width, h.height)
	h.ui.SetDark(dark)
	h.ui.Frame()
	h.waitIdle()
	t.Cleanup(func() {
		if a.dev.cancel != nil {
			a.dev.cancel()
		}
		h.waitIdle()
	})
	return h
}

func TestUIProjectFolderRelocationAndReopenKeepsHTTPSIdentity(t *testing.T) {
	for _, dark := range []bool{false, true} {
		t.Run(fmt.Sprintf("minimum-dark-%t", dark), func(t *testing.T) {
			h := newDevUIHarnessSize(t, 900, 520, dark)
			dir := t.TempDir()
			for _, name := range []string{"server", "public"} {
				if err := os.Mkdir(filepath.Join(dir, name), 0700); err != nil {
					t.Fatal(err)
				}
			}
			program := `from http.server import BaseHTTPRequestHandler, HTTPServer
import os, argparse
from pathlib import Path
p = argparse.ArgumentParser()
p.add_argument('--directory', required=True)
args = p.parse_args()
class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        body = (Path(args.directory, 'index.html').read_text() + '|' + os.environ['MODE'] + '|' + os.getcwd()).encode()
        self.send_response(200)
        self.end_headers()
        self.wfile.write(body)
HTTPServer(('127.0.0.1', int(os.environ['PORT'])), Handler).serve_forever()
`
			if err := os.WriteFile(filepath.Join(dir, "server", "serve.py"), []byte(program), 0600); err != nil {
				t.Fatal(err)
			}
			port := freeUIPort(t)
			if err := os.WriteFile(filepath.Join(dir, ".env.local"), []byte(fmt.Sprintf("PORT=%d\nMODE=before\n", port)), 0600); err != nil {
				t.Fatal(err)
			}
			if err := os.WriteFile(filepath.Join(dir, "public", "index.html"), []byte("original-folder"), 0600); err != nil {
				t.Fatal(err)
			}
			python, err := exec.LookPath("python3")
			if err != nil {
				t.Fatal(err)
			}
			h.fill("Project folder", dir)
			h.click("Import project")
			id := h.a.dev.id
			original := h.a.dev.draft.Directory
			m := h.a.dev.managerFactory()
			defer func() { _ = m.Down(context.Background(), id); _ = m.StopProxy(context.Background()) }()
			h.fill("Project name", "Relocate UI")
			h.click("Services")
			h.click("Add service")
			h.fill("Service name", "web")
			h.fill("Program and arguments", fmt.Sprintf("%s\nserve.py\n--directory=%s", python, filepath.Join(original, "public")))
			h.fill("Service subfolder", "server")
			h.fill("Readiness check", fmt.Sprintf("http://127.0.0.1:%d/", port))
			h.click("Environment")
			h.fill("Environment file (optional)", filepath.Join(original, ".env.local"))
			h.click("Save project")
			h.click("Add project HTTPS site")
			if h.a.dev.siteProject != id || h.a.dev.siteDomain != "relocate-ui.test" || h.a.dev.siteUpstream != fmt.Sprintf("http://127.0.0.1:%d", port) {
				t.Fatal("project site defaults are wrong", h.a.dev.siteDomain, h.a.dev.siteUpstream)
			}
			httpsPort := freeUIPort(t)
			h.fill("HTTPS port", fmt.Sprint(httpsPort))
			h.click("Save HTTPS site")
			h.click("Projects")
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
			transport := &http.Transport{TLSClientConfig: &tls.Config{RootCAs: pool, ServerName: "relocate-ui.test"}}
			defer transport.CloseIdleConnections()
			client := &http.Client{Transport: transport, Timeout: 3 * time.Second}
			check := func(want string) {
				t.Helper()
				req, _ := http.NewRequest("GET", fmt.Sprintf("https://127.0.0.1:%d", httpsPort), nil)
				req.Host = "relocate-ui.test"
				response, err := client.Do(req)
				if err != nil {
					t.Fatal(err)
				}
				body, _ := io.ReadAll(response.Body)
				response.Body.Close()
				if response.StatusCode != 200 || !strings.HasPrefix(string(body), want) {
					t.Fatal("HTTPS did not use expected source/environment", string(body))
				}
			}
			check("original-folder|before|")
			h.click("Overview")
			h.fill("Working folder", t.TempDir())
			h.click("Save project")
			if h.a.dev.folderPreview != nil || !strings.Contains(h.a.dev.output, "stop the project") {
				t.Fatal("running project folder was changed", h.a.dev.output)
			}
			h.fill("Working folder", original)
			h.click("Stop project")
			moved := filepath.Join(t.TempDir(), "Moved project 开发")
			if err = os.Rename(original, moved); err != nil {
				t.Fatal(err)
			}
			if err = os.WriteFile(filepath.Join(moved, ".env.local"), []byte(fmt.Sprintf("PORT=%d\nMODE=after\n", port)), 0600); err != nil {
				t.Fatal(err)
			}
			if err = os.WriteFile(filepath.Join(moved, "public", "index.html"), []byte("relocated-folder"), 0600); err != nil {
				t.Fatal(err)
			}
			h.click("Refresh")
			if h.a.dev.folderErrors[id] == "" {
				t.Fatal("missing folder is not visible in UI")
			}
			h.click("Locate the project folder")
			h.fill("Working folder", moved)
			h.click("Save project")
			if h.a.dev.folderPreview == nil || len(h.a.dev.folderPreview.Changes) != 2 {
				t.Fatal("relocation did not preview both paths", h.a.dev.output)
			}
			h.click("Cancel folder change")
			if h.a.dev.folderPreview != nil || h.a.dev.draft.Directory != original {
				t.Fatal("cancel did not restore the folder field")
			}
			h.fill("Working folder", moved)
			h.click("Save project")
			h.fill("Project name", "Relocated UI project")
			h.click("Apply folder change")
			if h.a.dev.state.Projects[id].Directory != original {
				t.Fatal("outdated UI preview applied")
			}
			h.click("Save project")
			h.click("Apply folder change")
			p := h.a.dev.state.Projects[id]
			if p.Directory == original || p.Services[0].EnvFile != filepath.Join(p.Directory, ".env.local") || p.Sites[0].Domain != "relocate-ui.test" || p.Sites[0].Port != httpsPort {
				t.Fatal("folder update lost project configuration", h.a.dev.output)
			}
			// A fresh app/view/manager loads persisted state without the old draft.
			h = reopenProjectUI(t, h, dark)
			h.click("Project Relocated UI project")
			if h.a.dev.id != id || h.a.dev.draft.Directory != p.Directory {
				t.Fatal("UI reopen lost stable project identity")
			}
			h.click("Start project")
			if strings.Contains(h.a.dev.output, "Error:") {
				t.Fatal(h.a.dev.output)
			}
			check("relocated-folder|after|" + filepath.Join(p.Directory, "server"))
			h.click("Restart project")
			check("relocated-folder|after|")
			h.click("Stop project")
		})
	}
}

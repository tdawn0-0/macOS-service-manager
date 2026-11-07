package devplatform

import (
	"context"
	"fmt"
	"io"
	"macos-service-manager/internal/launchd"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestLoginLauncherExecutesReadinessAndRelocatedSavedEnvironment(t *testing.T) {
	m := testManager(t)
	m.LoginDir = t.TempDir()
	m.Program = filepath.Join(t.TempDir(), "devmgr")
	if output, e := exec.Command("go", "build", "-o", m.Program, "../../cmd/devmgr").CombinedOutput(); e != nil {
		t.Fatal(e, string(output))
	}
	ctx, cancel := context.WithTimeout(context.Background(), 40*time.Second)
	defer cancel()
	if e := m.withState(ctx, func(s *State) error { s.LogRotationDisabled = true; return nil }); e != nil {
		t.Fatal(e)
	}
	source := filepath.Join(t.TempDir(), "source")
	if e := os.Mkdir(source, 0700); e != nil {
		t.Fatal(e)
	}
	script := `import http.server,os,pathlib,sys,urllib.request
mode=sys.argv[1]
if mode=='api':
 urllib.request.urlopen('http://127.0.0.1:'+os.environ['BASE_PORT'],timeout=2).close()
 pathlib.Path('api-entered').write_text('after base readiness')
class Handler(http.server.BaseHTTPRequestHandler):
 def do_GET(self):
  ready=mode=='api' or pathlib.Path('ready').exists()
  self.send_response(200 if ready else 503);self.end_headers()
  self.wfile.write((os.getcwd()+'|'+os.environ['MODE']).encode())
http.server.HTTPServer(('127.0.0.1',int(sys.argv[2])),Handler).serve_forever()
`
	if e := os.WriteFile(filepath.Join(source, "server.py"), []byte(script), 0600); e != nil {
		t.Fatal(e)
	}
	if e := os.WriteFile(filepath.Join(source, ".env"), []byte("MODE=literal-$(no-command)\n"), 0600); e != nil {
		t.Fatal(e)
	}
	basePort, apiPort := integrationPort(t), integrationPort(t)
	p := Project{SchemaVersion: Schema, ID: "actual-login", Name: "Actual login launcher", Directory: source, Autostart: true, Services: []Service{
		{ID: "api", Command: []string{"/usr/bin/python3", filepath.Join(source, "server.py"), "api", fmt.Sprint(apiPort)}, DependsOn: []string{"base"}, EnvFile: filepath.Join(source, ".env"), Environment: map[string]string{"BASE_PORT": fmt.Sprint(basePort)}, Health: fmt.Sprintf("http://127.0.0.1:%d/", apiPort), ReadinessTimeoutSeconds: 15},
		{ID: "base", Command: []string{"/usr/bin/python3", filepath.Join(source, "server.py"), "base", fmt.Sprint(basePort)}, EnvFile: filepath.Join(source, ".env"), Health: fmt.Sprintf("http://127.0.0.1:%d/", basePort), ReadinessTimeoutSeconds: 15},
	}}
	t.Cleanup(func() { _ = m.stopJob(context.Background(), "login-"+p.ID); _ = m.Down(context.Background(), p.ID) })
	if e := m.SaveProject(ctx, p); e != nil {
		t.Fatal(e)
	}
	movedProgram := m.Program + "-moved"
	if e := os.Rename(m.Program, movedProgram); e != nil {
		t.Fatal(e)
	}
	bootstrap := func() {
		t.Helper()
		if _, e := m.Run(ctx, "launchctl", "bootstrap", domain(), m.loginPath(p.ID)); e != nil {
			t.Fatal(e)
		}
	}
	bootstrap()
	deadline := time.Now().Add(8 * time.Second)
	for time.Now().Before(deadline) {
		job, _ := m.readJobState(ctx, "p-"+p.ID+"-base")
		if job.Running {
			break
		}
		time.Sleep(20 * time.Millisecond)
	}
	job, e := m.readJobState(ctx, "p-"+p.ID+"-base")
	if e != nil || !job.Running {
		t.Fatal("stable helper did not start the base service after original app moved", job, e)
	}
	if _, e = os.Stat(filepath.Join(source, "api-entered")); !os.IsNotExist(e) {
		t.Fatal("dependent service ran before readiness", e)
	}
	if e = os.WriteFile(filepath.Join(source, "ready"), nil, 0600); e != nil {
		t.Fatal(e)
	}
	waitProjectResponse(t, Project{ID: p.ID, Services: []Service{p.Services[0]}}, 200)
	waitLogin := func() {
		t.Helper()
		deadline := time.Now().Add(5 * time.Second)
		for time.Now().Before(deadline) {
			job, e := m.readJobState(ctx, "login-"+p.ID)
			if e == nil && !job.Running && job.Runs > 0 && job.LastExitCode != nil {
				if *job.LastExitCode != 0 {
					t.Fatal("login launcher failed", job)
				}
				return
			}
			time.Sleep(20 * time.Millisecond)
		}
		t.Fatal("finite login launcher did not finish")
	}
	waitLogin()
	if e = m.Down(ctx, p.ID); e != nil {
		t.Fatal(e)
	}
	if e = m.stopJob(ctx, "login-"+p.ID); e != nil {
		t.Fatal(e)
	}
	destination := source + "-moved"
	if e = os.Rename(source, destination); e != nil {
		t.Fatal(e)
	}
	destination, e = canonicalProjectDirectory(destination)
	if e != nil {
		t.Fatal(e)
	}
	m.Program = movedProgram
	draft := p.Clone()
	draft.Directory = destination
	preview, e := m.PreviewProjectFolder(ctx, draft)
	if e != nil {
		t.Fatal(e)
	}
	if e = m.ApplyProjectFolder(ctx, draft, preview.Fingerprint); e != nil {
		t.Fatal(e)
	}
	spec, e := launchd.LoadSpec(m.loginPath(p.ID))
	if e != nil || spec.WorkingDir != destination {
		t.Fatal("saved login launcher did not follow source relocation", spec, e)
	}
	bootstrap()
	waitProjectResponse(t, Project{ID: p.ID, Services: []Service{p.Services[0]}}, 200)
	waitLogin()
	client := http.Client{Timeout: time.Second}
	defer client.CloseIdleConnections()
	response, e := client.Get(p.Services[0].Health)
	if e != nil {
		t.Fatal(e)
	}
	body, e := io.ReadAll(response.Body)
	response.Body.Close()
	if e != nil || !strings.Contains(string(body), destination+"|literal-$(no-command)") {
		t.Fatal("login did not use relocated saved cwd/environment", string(body), e)
	}
	state, e := m.Read()
	if e != nil || state.Projects[p.ID].ID != p.ID || state.Projects[p.ID].Directory != destination {
		t.Fatal(state, e)
	}
	p = state.Projects[p.ID]
	p.Autostart = false
	if e = m.Down(ctx, p.ID); e != nil {
		t.Fatal(e)
	}
	if e = m.SaveProject(ctx, p); e != nil {
		t.Fatal(e)
	}
	if _, e = os.Stat(m.loginPath(p.ID)); !os.IsNotExist(e) {
		t.Fatal("disabling autostart left its plist", e)
	}
	if e = m.verifyShutdownTarget(ctx, domain()+"/"+m.label("login-"+p.ID)); e != nil {
		t.Fatal(e)
	}
}

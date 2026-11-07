package devplatform

import (
	"context"
	"crypto/tls"
	"crypto/x509"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func projectLifecycleFixture(t *testing.T) (*Manager, Package, Project, Project) {
	t.Helper()
	script := `#!/usr/bin/python3
import http.server,pathlib,sys
ready=pathlib.Path(sys.argv[2])
class Handler(http.server.BaseHTTPRequestHandler):
 def do_GET(self):
  self.send_response(200 if ready.exists() else 503);self.end_headers();self.wfile.write(b'project fixture')
http.server.HTTPServer(('127.0.0.1',int(sys.argv[1])),Handler).serve_forever()
`
	m, pkg := initializationFixture(t, script)
	create := func(id string) Project {
		dir := t.TempDir()
		port := integrationPort(t)
		p := Project{SchemaVersion: Schema, ID: id, Name: id, Directory: dir, Services: []Service{{ID: "web", Package: pkg.ID(), Command: []string{"redis-server", fmt.Sprint(port), filepath.Join(dir, "ready")}, Health: fmt.Sprintf("http://127.0.0.1:%d/", port), ReadinessTimeoutSeconds: 15}}}
		if e := m.SaveProject(context.Background(), p); e != nil {
			t.Fatal(e)
		}
		return p
	}
	a, b := create("alpha"), create("beta")
	t.Cleanup(func() {
		for _, p := range []Project{a, b} {
			_ = m.Down(context.Background(), p.ID)
		}
	})
	return m, pkg, a, b
}
func waitProjectResponse(t *testing.T, p Project, want int) {
	t.Helper()
	client := &http.Client{Timeout: time.Second}
	defer client.CloseIdleConnections()
	deadline := time.Now().Add(6 * time.Second)
	for time.Now().Before(deadline) {
		r, e := client.Get(p.Services[0].Health)
		if e == nil {
			r.Body.Close()
			if r.StatusCode == want {
				return
			}
		}
		time.Sleep(20 * time.Millisecond)
	}
	t.Fatal("project did not reach expected HTTP readiness", p.ID, want)
}
func TestProjectLifecyclesOverlapAndCancelOnlyTheirResources(t *testing.T) {
	m, pkg, a, b := projectLifecycleFixture(t)
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	firstCtx, cancelFirst := context.WithCancel(ctx)
	defer cancelFirst()
	first, second := make(chan error, 1), make(chan error, 1)
	go func() { first <- m.Up(firstCtx, a.ID) }()
	other := New(m.Root)
	go func() { second <- other.Up(ctx, b.ID) }()
	defer func() {
		cancel()
		cancelFirst()
		for _, ch := range []chan error{first, second} {
			select {
			case <-ch:
			case <-time.After(5 * time.Second):
			}
		}
	}()
	waitProjectResponse(t, a, 503)
	waitProjectResponse(t, b, 503)
	short, stop := context.WithTimeout(ctx, 3*time.Second)
	defer stop()
	independent := Project{SchemaVersion: Schema, ID: "independent", Name: "Saved during starts", Directory: t.TempDir()}
	if e := m.SaveProject(short, independent); e != nil {
		t.Fatal("project readiness held the workspace lock", e)
	}
	assertBusy := func(name string, e error) {
		t.Helper()
		var busy *ResourceBusyError
		if !errors.As(e, &busy) || busy.Kind != "project" || busy.ID != a.ID {
			t.Fatal(name, e)
		}
	}
	changed := a.Clone()
	changed.Name = "Changed during start"
	assertBusy("save project", m.SaveProject(short, changed))
	assertBusy("second start", m.Up(short, a.ID))
	assertBusy("stop", m.Down(short, a.ID))
	assertBusy("restart", m.RestartProject(short, a.ID))
	assertBusy("remove", m.RemoveProject(short, a.ID))
	assertBusy("prepare", m.PrepareEnvironment(short, a.ID))
	changed.Directory = t.TempDir()
	assertBusy("folder change", m.ApplyProjectFolder(short, changed, "unused"))
	assertBusy("site change", m.EditSite(short, "", "", a.ID, Site{ID: "new-site", Domain: "new-project.test", Upstream: a.Services[0].Health, Port: integrationPort(t)}))
	plan, e := m.PreviewShutdown(short)
	if e != nil {
		t.Fatal(e)
	}
	if e = m.Shutdown(short, plan, ShutdownOptions{}); e == nil {
		t.Fatal("shutdown ignored active project operations")
	}
	var packageBusy *PackageBusyError
	if e = m.RepairPackage(short, pkg.ID()); !errors.As(e, &packageBusy) {
		t.Fatal("running preparation lost its package lease", e)
	}
	cancelFirst()
	e = <-first
	first <- e
	if e == nil {
		t.Fatal("cancelled startup was reported successful")
	}
	if e = m.verifyShutdownTarget(ctx, domain()+"/"+m.label("p-"+a.ID+"-web")); e != nil {
		t.Fatal(e)
	}
	waitProjectResponse(t, b, 503)
	if e = os.WriteFile(filepath.Join(b.Directory, "ready"), nil, 0600); e != nil {
		t.Fatal(e)
	}
	e = <-second
	second <- e
	if e != nil {
		t.Fatal(e)
	}
	waitProjectResponse(t, b, 200)
	s, e := m.Read()
	if e != nil || s.Projects[independent.ID].Name != independent.Name {
		t.Fatal("startup replaced newer settings", e)
	}
}
func TestProjectLifecycleStopWaitAllowsIndependentConfiguration(t *testing.T) {
	m, _, a, _ := projectLifecycleFixture(t)
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	if e := os.WriteFile(filepath.Join(a.Directory, "ready"), nil, 0600); e != nil {
		t.Fatal(e)
	}
	if e := m.Up(ctx, a.ID); e != nil {
		t.Fatal(e)
	}
	worker := New(m.Root)
	run := worker.Run
	entered, release := make(chan struct{}), make(chan struct{})
	worker.Run = func(ctx context.Context, name string, args ...string) (string, error) {
		if name == "launchctl" && len(args) > 1 && args[0] == "bootout" && strings.HasSuffix(args[1], m.label("p-"+a.ID+"-web")) {
			close(entered)
			select {
			case <-release:
			case <-ctx.Done():
				return "", ctx.Err()
			}
		}
		return run(ctx, name, args...)
	}
	done := make(chan error, 1)
	go func() { done <- worker.Down(ctx, a.ID) }()
	defer func() {
		cancel()
		select {
		case <-done:
		case <-time.After(5 * time.Second):
		}
	}()
	select {
	case <-entered:
	case <-time.After(5 * time.Second):
		t.Fatal("stop did not reach its owned service")
	}
	short, stop := context.WithTimeout(ctx, 2*time.Second)
	defer stop()
	if e := m.SaveProject(short, Project{SchemaVersion: Schema, ID: "during-stop", Name: "Saved", Directory: t.TempDir()}); e != nil {
		t.Fatal("stop held the workspace lock", e)
	}
	waitProjectResponse(t, a, 200)
	close(release)
	e := <-done
	done <- e
	if e != nil {
		t.Fatal(e)
	}
	if e = m.verifyShutdownTarget(ctx, domain()+"/"+m.label("p-"+a.ID+"-web")); e != nil {
		t.Fatal(e)
	}
}
func TestProjectStartupPublishesLatestIndependentHTTPSConfiguration(t *testing.T) {
	m, _, a, _ := projectLifecycleFixture(t)
	m.Program = filepath.Join(t.TempDir(), "devmgr")
	if out, e := exec.Command("go", "build", "-o", m.Program, "../../cmd/devmgr").CombinedOutput(); e != nil {
		t.Fatal(e, string(out))
	}
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { io.WriteString(w, "latest independent route") }))
	defer upstream.Close()
	t.Cleanup(func() { _ = m.StopProxy(context.Background()) })
	done := make(chan error, 1)
	go func() { done <- m.Up(ctx, a.ID) }()
	defer func() {
		cancel()
		select {
		case <-done:
		case <-time.After(5 * time.Second):
		}
	}()
	waitProjectResponse(t, a, 503)
	site := Site{ID: "independent", Domain: "latest-during-start.test", Upstream: upstream.URL, Port: integrationPort(t)}
	short, stop := context.WithTimeout(ctx, 2*time.Second)
	defer stop()
	if e := m.SaveSite(short, site); e != nil {
		t.Fatal("startup blocked independent routing", e)
	}
	if e := os.WriteFile(filepath.Join(a.Directory, "ready"), nil, 0600); e != nil {
		t.Fatal(e)
	}
	e := <-done
	done <- e
	if e != nil {
		t.Fatal(e)
	}
	cert, e := os.ReadFile(filepath.Join(m.Root, "proxy", "ca.pem"))
	if e != nil {
		t.Fatal(e)
	}
	pool := x509.NewCertPool()
	pool.AppendCertsFromPEM(cert)
	transport := &http.Transport{TLSClientConfig: &tls.Config{RootCAs: pool, ServerName: site.Domain}}
	defer transport.CloseIdleConnections()
	client := &http.Client{Transport: transport, Timeout: 3 * time.Second}
	req, _ := http.NewRequest("GET", fmt.Sprintf("https://127.0.0.1:%d/", site.Port), nil)
	req.Host = site.Domain
	response, e := client.Do(req)
	if e != nil {
		t.Fatal(e)
	}
	body, _ := io.ReadAll(response.Body)
	response.Body.Close()
	if response.StatusCode != 200 || string(body) != "latest independent route" {
		t.Fatal("startup published stale routing", response.StatusCode, string(body))
	}
}

func TestProjectLifecycleUnknownObservationDoesNotMutateJobs(t *testing.T) {
	m, _, a, _ := projectLifecycleFixture(t)
	m.Run = func(_ context.Context, name string, args ...string) (string, error) {
		if name != "launchctl" || len(args) < 1 || args[0] != "print" {
			t.Fatal("changed an unobservable project job", name, args)
		}
		return "", errors.New("observation denied")
	}
	for name, action := range map[string]func(context.Context, string) error{"up": m.Up, "down": m.Down, "restart": m.RestartProject, "remove": m.RemoveProject} {
		if e := action(context.Background(), a.ID); e == nil {
			t.Fatal("unknown project observation was treated as stopped", name)
		}
	}
	s, e := m.Read()
	if e != nil || s.Projects[a.ID].ID != a.ID {
		t.Fatal("unknown project was removed", e)
	}
	files, e := os.ReadDir(filepath.Join(m.Root, "jobs"))
	if e != nil || len(files) != 0 {
		t.Fatal("unknown observation wrote job configuration", files, e)
	}
}

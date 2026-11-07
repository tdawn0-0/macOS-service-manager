package devplatform

import (
	"bytes"
	"context"
	"crypto/tls"
	"crypto/x509"
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

func TestProjectRemovalStateFailureRestoresLoginAndLiveProxyRoutes(t *testing.T) {
	m := testManager(t)
	m.LoginDir = t.TempDir()
	m.Program = filepath.Join(t.TempDir(), "devmgr")
	if b, e := exec.Command("go", "build", "-o", m.Program, "../../cmd/devmgr").CombinedOutput(); e != nil {
		t.Fatal(e, string(b))
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	a := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { fmt.Fprint(w, "project route") }))
	defer a.Close()
	b := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { fmt.Fprint(w, "independent route") }))
	defer b.Close()
	port := integrationPort(t)
	p := Project{SchemaVersion: Schema, ID: "rollback-project", Name: "Rollback project", Directory: t.TempDir(), Autostart: true, Sites: []Site{{ID: "owned", Domain: "owned.test", Upstream: a.URL, Port: port}}}
	if e := m.SaveProject(ctx, p); e != nil {
		t.Fatal(e)
	}
	if e := m.SaveSite(ctx, Site{ID: "independent", Domain: "other.test", Upstream: b.URL, Port: port}); e != nil {
		t.Fatal(e)
	}
	if e := m.Up(ctx, p.ID); e != nil {
		t.Fatal(e)
	}
	t.Cleanup(func() { _ = m.Down(context.Background(), p.ID); _ = m.StopProxy(context.Background()) })
	beforeLogin, e := os.ReadFile(m.loginPath(p.ID))
	if e != nil {
		t.Fatal(e)
	}
	beforeJob, e := m.readJobState(ctx, "proxy")
	if e != nil || !beforeJob.Running {
		t.Fatal(beforeJob, e)
	}
	ca, e := os.ReadFile(filepath.Join(m.Root, "proxy/ca.pem"))
	if e != nil {
		t.Fatal(e)
	}
	pool := x509.NewCertPool()
	pool.AppendCertsFromPEM(ca)
	check := func(domain, want string) {
		t.Helper()
		transport := &http.Transport{TLSClientConfig: &tls.Config{RootCAs: pool, ServerName: domain}}
		defer transport.CloseIdleConnections()
		client := http.Client{Transport: transport, Timeout: 2 * time.Second}
		req, _ := http.NewRequest("GET", fmt.Sprintf("https://127.0.0.1:%d/", port), nil)
		req.Host = domain
		deadline := time.Now().Add(3 * time.Second)
		for time.Now().Before(deadline) {
			response, e := client.Do(req)
			if e == nil {
				body, e := io.ReadAll(response.Body)
				response.Body.Close()
				if e == nil && string(body) == want {
					return
				}
			}
			time.Sleep(20 * time.Millisecond)
		}
		t.Fatal("expected HTTPS route was not restored", domain)
	}
	check("owned.test", "project route")
	check("other.test", "independent route")
	statePath := filepath.Join(m.Root, "state.json")
	backup := statePath + ".saved"
	realRun := m.Run
	injected := false
	m.Run = func(ctx context.Context, name string, args ...string) (string, error) {
		if !injected && len(args) > 1 && args[0] == "print" && strings.HasSuffix(args[1], "."+"proxy") {
			if _, e := os.Stat(m.loginPath(p.ID)); os.IsNotExist(e) {
				if e = os.Rename(statePath, backup); e != nil {
					t.Fatal(e)
				}
				if e = os.Mkdir(statePath, 0700); e != nil {
					t.Fatal(e)
				}
				injected = true
			}
		}
		return realRun(ctx, name, args...)
	}
	if e = m.RemoveProject(ctx, p.ID); e == nil || !injected {
		t.Fatal("state failure not reported", e, injected)
	}
	if e = os.Remove(statePath); e != nil {
		t.Fatal(e)
	}
	if e = os.Rename(backup, statePath); e != nil {
		t.Fatal(e)
	}
	state, e := m.Read()
	if e != nil || state.Projects[p.ID].ID != p.ID {
		t.Fatal("failed removal deleted record", state, e)
	}
	after, e := os.ReadFile(m.loginPath(p.ID))
	if e != nil || !bytes.Equal(after, beforeLogin) {
		t.Fatal("failed removal changed login preference", e)
	}
	check("owned.test", "project route")
	check("other.test", "independent route")
	afterJob, e := m.readJobState(ctx, "proxy")
	if e != nil || afterJob.PID != beforeJob.PID {
		t.Fatal("rollback restarted shared proxy unnecessarily", afterJob, e)
	}
}

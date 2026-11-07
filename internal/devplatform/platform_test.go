package devplatform

import (
	"archive/tar"
	"compress/gzip"
	"context"
	"crypto/tls"
	"crypto/x509"
	"encoding/pem"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"
)

func testManager(t *testing.T) *Manager {
	t.Helper()
	m := New(t.TempDir())
	if e := m.init(); e != nil {
		t.Fatal(e)
	}
	return m
}
func TestHostsPreserveAndConflict(t *testing.T) {
	site := Site{ID: "web", Domain: "web.test", Upstream: "http://127.0.0.1:3000", Port: 443}
	old := "127.0.0.1 localhost\n\n# user comment\n"
	got, e := HostsContent(old, []Site{site})
	if e != nil {
		t.Fatal(e)
	}
	if !strings.Contains(got, "# user comment") || !strings.Contains(got, "127.0.0.1 web.test") {
		t.Fatal(got)
	}
	again, e := HostsContent(got, []Site{site})
	if e != nil || again != got {
		t.Fatalf("not idempotent: %q %v", again, e)
	}
	if _, e = HostsContent(old+"127.0.0.2 web.test\n", []Site{site}); e == nil {
		t.Fatal("overwrote external domain")
	}
	if _, e = HostsContent(hostsBegin, []Site{site}); e == nil {
		t.Fatal("accepted broken block")
	}
}
func TestDependencyCycleAndMissing(t *testing.T) {
	for _, services := range [][]Service{{{ID: "a", DependsOn: []string{"b"}}, {ID: "b", DependsOn: []string{"a"}}}, {{ID: "a", DependsOn: []string{"missing"}}}} {
		if _, e := serviceOrder(services); e == nil {
			t.Fatal("accepted invalid dependencies")
		}
	}
	out, e := serviceOrder([]Service{{ID: "web", DependsOn: []string{"worker"}}, {ID: "worker"}})
	if e != nil || out[0].ID != "worker" {
		t.Fatal(out, e)
	}
}
func TestStateConcurrentAndSchema(t *testing.T) {
	m := testManager(t)
	var wg sync.WaitGroup
	for n := 0; n < 12; n++ {
		wg.Add(1)
		go func(n int) {
			defer wg.Done()
			p := Project{SchemaVersion: 1, ID: fmt.Sprint("p", n), Name: "Project", Directory: t.TempDir()}
			if e := m.SaveProject(context.Background(), p); e != nil {
				t.Error(e)
			}
		}(n)
	}
	wg.Wait()
	s, e := m.Read()
	if e != nil || len(s.Projects) != 12 {
		t.Fatal(len(s.Projects), e)
	}
	if e = atomicWrite(filepath.Join(m.Root, "state.json"), []byte(`{"schemaVersion":99}`), 0600); e != nil {
		t.Fatal(e)
	}
	if _, e = m.Read(); e == nil {
		t.Fatal("accepted unknown schema")
	}
}
func TestArchiveRejectsTraversalAndSymlinkWrites(t *testing.T) {
	for _, bad := range []string{"../outside", "/tmp/outside"} {
		t.Run(bad, func(t *testing.T) {
			path := filepath.Join(t.TempDir(), "bad.tar.gz")
			f, _ := os.Create(path)
			gz := gzip.NewWriter(f)
			tw := tar.NewWriter(gz)
			_ = tw.WriteHeader(&tar.Header{Name: bad, Mode: 0755, Size: 1})
			_, _ = tw.Write([]byte("x"))
			tw.Close()
			gz.Close()
			f.Close()
			if e := extract(path, t.TempDir(), "tar.gz", 0); e == nil {
				t.Fatal("accepted traversal")
			}
		})
	}
}
func TestProjectImportDoesNotExecute(t *testing.T) {
	m := testManager(t)
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, "package.json"), []byte(`{"scripts":{"dev":"touch SHOULD_NOT_EXIST"}}`), 0600)
	p, e := m.Import(context.Background(), dir)
	if e != nil {
		t.Fatal(e)
	}
	if len(p.Services) != 1 || p.Services[0].Command[2] != "dev" {
		t.Fatal(p)
	}
	if _, e = os.Stat(filepath.Join(dir, "SHOULD_NOT_EXIST")); !os.IsNotExist(e) {
		t.Fatal("import executed a script")
	}
}
func TestHTTPSProxyWithTrustedCAAndReload(t *testing.T) {
	m := testManager(t)
	up := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { fmt.Fprint(w, r.URL.RequestURI()) }))
	defer up.Close()
	l, e := net.Listen("tcp4", "127.0.0.1:0")
	if e != nil {
		t.Fatal(e)
	}
	port := l.Addr().(*net.TCPAddr).Port
	l.Close()
	site := Site{ID: "web", Domain: "web.test", Upstream: up.URL, Port: port}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	if _, _, e = m.ca(); e != nil {
		t.Fatal(e)
	}
	if e = writeJSON(filepath.Join(m.Root, "proxy", "sites.json"), []Site{site}); e != nil {
		t.Fatal(e)
	}
	done := make(chan error, 1)
	go func() { done <- m.ServeProxy(ctx) }()
	roots := x509.NewCertPool()
	b, _ := os.ReadFile(filepath.Join(m.Root, "proxy", "ca.pem"))
	if !roots.AppendCertsFromPEM(b) {
		t.Fatal("CA missing")
	}
	transport := &http.Transport{TLSClientConfig: &tls.Config{RootCAs: roots, ServerName: "web.test"}, DialContext: func(ctx context.Context, network, addr string) (net.Conn, error) {
		return (&net.Dialer{}).DialContext(ctx, "tcp", fmt.Sprintf("127.0.0.1:%d", port))
	}}
	defer transport.CloseIdleConnections()
	client := &http.Client{Transport: transport, Timeout: time.Second}
	var res *http.Response
	for n := 0; n < 40; n++ {
		res, e = client.Get(fmt.Sprintf("https://web.test:%d/path?q=1", port))
		if e == nil {
			break
		}
		time.Sleep(25 * time.Millisecond)
	}
	if e != nil {
		t.Fatal(e)
	}
	body, _ := io.ReadAll(res.Body)
	res.Body.Close()
	if string(body) != "/path?q=1" {
		t.Fatal(string(body))
	}
	cert := res.TLS.PeerCertificates[0]
	if e = cert.VerifyHostname("web.test"); e != nil {
		t.Fatal(e)
	}
	block, _ := pem.Decode(b)
	ca, _ := x509.ParseCertificate(block.Bytes)
	if e = cert.CheckSignatureFrom(ca); e != nil {
		t.Fatal(e)
	}
	site.Upstream = "http://127.0.0.1:1"
	writeJSON(filepath.Join(m.Root, "proxy", "sites.json"), []Site{site})
	res, e = client.Get(fmt.Sprintf("https://web.test:%d/", port))
	if e != nil {
		t.Fatal(e)
	}
	res.Body.Close()
	if res.StatusCode != 502 {
		t.Fatal(res.Status)
	}
	cancel()
	select {
	case e := <-done:
		if e != nil {
			t.Fatal(e)
		}
	case <-time.After(2 * time.Second):
		t.Fatal("proxy did not stop")
	}
}
func TestPackageReferenceProtectsData(t *testing.T) {
	m := testManager(t)
	p := Package{Name: "node", Version: "1.0", Arch: "arm64"}
	if e := m.withState(context.Background(), func(s *State) error {
		s.Packages[p.ID()] = p
		s.Projects["web"] = Project{Services: []Service{{Package: p.ID()}}}
		return nil
	}); e != nil {
		t.Fatal(e)
	}
	if e := m.Uninstall(context.Background(), p.ID()); e == nil {
		t.Fatal("removed referenced package")
	}
}

func TestExportDoesNotEraseLocalSecrets(t *testing.T) {
	m := testManager(t)
	p := Project{SchemaVersion: 1, ID: "secret-project", Name: "Secret", Directory: t.TempDir(), Services: []Service{{ID: "worker", Command: []string{"/usr/bin/true"}, Environment: map[string]string{"PASSWORD": "local-only"}}}}
	if e := m.SaveProject(context.Background(), p); e != nil {
		t.Fatal(e)
	}
	if e := m.ExportProject(context.Background(), p.ID); e != nil {
		t.Fatal(e)
	}
	state, e := m.Read()
	if e != nil {
		t.Fatal(e)
	}
	if state.Projects[p.ID].Services[0].Environment["PASSWORD"] != "local-only" {
		t.Fatal("export erased local secret")
	}
	b, e := os.ReadFile(filepath.Join(p.Directory, "devmanager.json"))
	if e != nil {
		t.Fatal(e)
	}
	if strings.Contains(string(b), "local-only") {
		t.Fatal("export leaked secret")
	}
}

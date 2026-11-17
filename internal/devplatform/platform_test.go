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
func TestStateConcurrentAndSchema(t *testing.T) {
	m := testManager(t)
	var wg sync.WaitGroup
	for n := 0; n < 12; n++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			if e := m.withState(context.Background(), func(s *State) error { s.ProxyRulesRevision++; return nil }); e != nil {
				t.Error(e)
			}
		}()
	}
	wg.Wait()
	state, e := m.Read()
	if e != nil || state.ProxyRulesRevision != 12 {
		t.Fatal(state.ProxyRulesRevision, e)
	}
	if e = atomicWrite(filepath.Join(m.Root, "state.json"), []byte(`{"schemaVersion":1}`), 0600); e != nil {
		t.Fatal(e)
	}
	if _, e = m.Read(); e == nil {
		t.Fatal("accepted old schema")
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
	route := proxyRoute{Domain: "web.devbase.localhost", LocalPort: up.Listener.Addr().(*net.TCPAddr).Port, Port: port}
	localPort := up.Listener.Addr().(*net.TCPAddr).Port
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	if _, _, e = m.ca(); e != nil {
		t.Fatal(e)
	}
	if e = writeJSON(filepath.Join(m.Root, "proxy", "rules.json"), proxyPublishedConfig{SchemaVersion: 1, ListenerPort: port, Rules: []ProxyRule{{Domain: "web.devbase.localhost", LocalPort: localPort}}}); e != nil {
		t.Fatal(e)
	}
	done := make(chan error, 1)
	go func() { done <- m.ServeProxy(ctx) }()
	roots := x509.NewCertPool()
	b, _ := os.ReadFile(filepath.Join(m.Root, "proxy", "ca.pem"))
	if !roots.AppendCertsFromPEM(b) {
		t.Fatal("CA missing")
	}
	transport := &http.Transport{TLSClientConfig: &tls.Config{RootCAs: roots, ServerName: "web.devbase.localhost"}, DialContext: func(ctx context.Context, network, addr string) (net.Conn, error) {
		return (&net.Dialer{}).DialContext(ctx, "tcp", fmt.Sprintf("127.0.0.1:%d", port))
	}}
	defer transport.CloseIdleConnections()
	client := &http.Client{Transport: transport, Timeout: time.Second}
	var res *http.Response
	for n := 0; n < 40; n++ {
		res, e = client.Get(fmt.Sprintf("https://web.devbase.localhost:%d/path?q=1", port))
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
	if e = cert.VerifyHostname("web.devbase.localhost"); e != nil {
		t.Fatal(e)
	}
	block, _ := pem.Decode(b)
	ca, _ := x509.ParseCertificate(block.Bytes)
	if e = cert.CheckSignatureFrom(ca); e != nil {
		t.Fatal(e)
	}
	route.LocalPort = 1
	localPort = 1
	writeJSON(filepath.Join(m.Root, "proxy", "rules.json"), proxyPublishedConfig{SchemaVersion: 1, ListenerPort: port, Rules: []ProxyRule{{Domain: "web.devbase.localhost", LocalPort: localPort}}})
	res, e = client.Get(fmt.Sprintf("https://web.devbase.localhost:%d/", port))
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
	p := Package{Name: "redis", Version: "1.0", Arch: "arm64"}
	if e := m.withState(context.Background(), func(s *State) error {
		s.Packages[p.ID()] = p
		s.Instances["db"] = Instance{Kind: "redis", Package: p.ID(), Port: 56379}
		return nil
	}); e != nil {
		t.Fatal(e)
	}
	if e := m.Uninstall(context.Background(), p.ID()); e == nil {
		t.Fatal("removed referenced package")
	}
}

func extract(src, root, format string, strip int) error {
	return extractContext(context.Background(), src, root, format, strip)
}

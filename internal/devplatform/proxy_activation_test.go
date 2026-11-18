package devplatform

import (
	"context"
	"crypto/tls"
	"crypto/x509"
	"net"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strconv"
	"sync/atomic"
	"testing"
	"time"
)

func TestProxyActivationStates(t *testing.T) {
	m := testManager(t)
	if state, err := m.ProxyPublicationState(); err != nil || state != "Unknown" {
		t.Fatal(state, err)
	}
	if err := m.ReplaceProxyRules(context.Background(), 0, 8443, []ProxyRule{{Domain: "state.devbase.localhost", LocalPort: 3000}}); err != nil {
		t.Fatal(err)
	}
	s, _ := m.Read()
	if err := writeJSON(filepath.Join(m.Root, "proxy", "rules.json"), proxyConfigFromState(&s)); err != nil {
		t.Fatal(err)
	}
	if state, err := m.ProxyPublicationState(); err != nil || state != "Published" {
		t.Fatal(state, err)
	}
	if err := m.ReplaceProxyRules(context.Background(), 1, 8443, []ProxyRule{}); err != nil {
		t.Fatal(err)
	}
	if state, err := m.ProxyPublicationState(); err != nil || state != "Pending" {
		t.Fatal(state, err)
	}
	now := time.Now()
	r := ProxyHTTPResult{Revision: 3, Fingerprint: "a", CheckedAt: now, StatusCode: 404}
	if !r.Current(now, 3, "a") || r.Current(now.Add(31*time.Second), 3, "a") || r.Current(now, 4, "a") || r.Current(now, 3, "b") {
		t.Fatal("stale observation reused")
	}
	for _, code := range []int{200, 204, 301, 401, 404, 500, 502} {
		r.StatusCode = code
		if !r.Current(now, 3, "a") {
			t.Fatal(code)
		}
	}
}

func TestProxyRuleHTTPCheck(t *testing.T) {
	m := testManager(t)
	cert, err := m.leaf("check.devbase.localhost")
	if err != nil {
		t.Fatal(err)
	}
	ca, _, err := m.ca()
	if err != nil {
		t.Fatal(err)
	}
	roots := x509.NewCertPool()
	roots.AddCert(ca)
	var redirected atomic.Int32
	receiver := httptest.NewServer(http.HandlerFunc(func(http.ResponseWriter, *http.Request) { redirected.Add(1) }))
	defer receiver.Close()
	code := 404
	server := httptest.NewUnstartedServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != "GET" || r.URL.Path != "/" || r.Header.Get("Cookie") != "" || r.Header.Get("Authorization") != "" {
			t.Error("unexpected request", r.Method, r.URL.Path)
		}
		if code == 301 {
			w.Header().Set("Location", receiver.URL)
		}
		w.WriteHeader(code)
		_, _ = w.Write([]byte("secret-response-body"))
	}))
	server.TLS = &tls.Config{Certificates: []tls.Certificate{cert}}
	server.StartTLS()
	defer server.Close()
	_, portText, _ := net.SplitHostPort(server.Listener.Addr().String())
	port, _ := strconv.Atoi(portText)
	if err := m.ReplaceProxyRules(context.Background(), 0, port, []ProxyRule{{Domain: "check.devbase.localhost", LocalPort: 3000}}); err != nil {
		t.Fatal(err)
	}
	fp, revision, err := m.ProxyRuleFingerprint("check.devbase.localhost")
	if err != nil {
		t.Fatal(err)
	}
	for _, status := range []int{200, 204, 301, 401, 404, 500, 502} {
		code = status
		r, e := m.checkProxyRuleHTTP(context.Background(), "check.devbase.localhost", revision, fp, roots)
		if e != nil || r.StatusCode != status || r.Failure != "" {
			t.Fatal(r, e)
		}
	}
	if redirected.Load() != 0 {
		t.Fatal("followed redirect")
	}
	r, e := m.checkProxyRuleHTTP(context.Background(), "check.devbase.localhost", revision, fp, x509.NewCertPool())
	if e != nil || r.Failure != "TLS" {
		t.Fatal(r, e)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	r, e = m.checkProxyRuleHTTP(ctx, "check.devbase.localhost", revision, fp, roots)
	if e != nil || r.Failure == "" {
		t.Fatal(r, e)
	}
	if _, e = m.checkProxyRuleHTTP(context.Background(), "check.devbase.localhost", revision+1, fp, roots); e == nil {
		t.Fatal("stale revision accepted")
	}
	s, _ := m.Read()
	if s.ProxyRulesRevision != revision || len(s.ProxyRules) != 1 {
		t.Fatal("check modified state")
	}
}

// The privileged socket path publishes routing only. Tunnel settings in the
// saved rules must not make an applied proxy look pending.
func TestProxyPublicationIgnoresTunnelSettings(t *testing.T) {
	m := testManager(t)
	rules := []ProxyRule{{Domain: "tunnel.devbase.localhost", LocalPort: 8000, Tunnel: true, TunnelURL: "https://devbase-tunnel.example.workers.dev"}}
	if err := m.ReplaceProxyRules(context.Background(), 0, 8443, rules); err != nil {
		t.Fatal(err)
	}
	published := proxyPublishedConfig{SchemaVersion: 1, ListenerPort: 8443, Rules: []ProxyRule{{Domain: "tunnel.devbase.localhost", LocalPort: 8000}}}
	if err := writeJSON(filepath.Join(m.Root, "proxy", "rules.json"), published); err != nil {
		t.Fatal(err)
	}
	if state, err := m.ProxyPublicationState(); err != nil || state != "Published" {
		t.Fatal("applied proxy reads as", state, err)
	}
	published.Rules[0].LocalPort = 8001
	if err := writeJSON(filepath.Join(m.Root, "proxy", "rules.json"), published); err != nil {
		t.Fatal(err)
	}
	if state, err := m.ProxyPublicationState(); err != nil || state != "Pending" {
		t.Fatal("a different port must stay pending, got", state, err)
	}
}

func TestCheckTunnelRuleHTTP(t *testing.T) {
	m := testManager(t)
	var seenAuth atomic.Value
	srv := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		seenAuth.Store(r.Header.Get("Authorization"))
		w.WriteHeader(http.StatusOK)
	}))
	defer srv.Close()
	m.tunnelTLSConfig = srv.Client().Transport.(*http.Transport).TLSClientConfig

	if err := m.ReplaceProxyRules(context.Background(), 0, 443, []ProxyRule{
		{Domain: "edge.devbase.localhost", LocalPort: 3000, Tunnel: true, TunnelAuth: true, TunnelURL: srv.URL},
	}); err != nil {
		t.Fatal(err)
	}
	token, err := m.EnsureTunnelAccessToken("edge.devbase.localhost")
	if err != nil {
		t.Fatal(err)
	}
	fp, rev, err := m.TunnelRuleFingerprint("edge.devbase.localhost")
	if err != nil {
		t.Fatal(err)
	}
	res, err := m.CheckTunnelRuleHTTP(context.Background(), "edge.devbase.localhost", rev, fp)
	if err != nil || res.Failure != "" || res.StatusCode != 200 {
		t.Fatalf("unexpected tunnel HTTP check result: %+v err=%v", res, err)
	}
	if gotAuth, _ := seenAuth.Load().(string); gotAuth != "Bearer "+token {
		t.Fatalf("expected Bearer token header, got %q", gotAuth)
	}
	if _, err := m.CheckTunnelRuleHTTP(context.Background(), "edge.devbase.localhost", rev+1, fp); err == nil {
		t.Fatal("expected stale revision error")
	}

	if err := m.ReplaceProxyRules(context.Background(), rev, 443, []ProxyRule{
		{Domain: "pending.devbase.localhost", LocalPort: 3000, Tunnel: true},
	}); err != nil {
		t.Fatal(err)
	}
	fp2, rev2, err := m.TunnelRuleFingerprint("pending.devbase.localhost")
	if err != nil {
		t.Fatal(err)
	}
	res2, err := m.CheckTunnelRuleHTTP(context.Background(), "pending.devbase.localhost", rev2, fp2)
	if err != nil || res2.Failure != "Not deployed" {
		t.Fatalf("expected Not deployed failure, got %+v err=%v", res2, err)
	}
}

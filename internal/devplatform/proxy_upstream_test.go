package devplatform

import (
	"context"
	"net"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"testing"
)

func TestNormalizeUpstreamHost(t *testing.T) {
	for in, want := range map[string]string{
		"":                     "",
		"  127.0.0.1 ":         "",
		"localhost":            "",
		"LOCALHOST":            "",
		"192.168.1.20":         "192.168.1.20",
		"[::1]":                "::1",
		"fd00::5":              "fd00::5",
		"Host.Docker.Internal": "host.docker.internal",
		"devbox.local.":        "devbox.local",
		"10.0.0.7":             "10.0.0.7",
	} {
		got, err := normalizeUpstreamHost(in)
		if err != nil || got != want {
			t.Errorf("%q: got %q, %v; want %q", in, got, err, want)
		}
	}
	for _, bad := range []string{"0.0.0.0", "::", "224.0.0.1", "http://a.test", "a.test:8080", "a/b", "user@host", "bad host", "-a.test", "a..test", strings.Repeat("a", 64) + ".test", "fe80::1%en0"} {
		if got, err := normalizeUpstreamHost(bad); err == nil {
			t.Errorf("%q accepted as %q", bad, got)
		}
	}
}

func TestProxyRulesWithRemoteUpstream(t *testing.T) {
	rules, err := normalizeProxyRules(443, []ProxyRule{
		{Domain: "lan.test", LocalPort: 443, UpstreamHost: "192.168.1.20"}, // same number as the HTTPS port is fine off this Mac
		{Domain: "mac.test", LocalPort: 3000, UpstreamHost: "localhost"},
	})
	if err != nil || rules[0].UpstreamHost != "192.168.1.20" || rules[1].UpstreamHost != "" {
		t.Fatal(rules, err)
	}
	if _, err = normalizeProxyRules(443, []ProxyRule{{Domain: "mac.test", LocalPort: 443}}); err == nil {
		t.Fatal("this Mac's own HTTPS port must stay refused")
	}
	if _, err = normalizeProxyRules(443, []ProxyRule{{Domain: "x.test", LocalPort: 80, UpstreamHost: "a b"}}); err == nil {
		t.Fatal("invalid upstream accepted")
	}
	if got := (proxyRoute{LocalPort: 8080, UpstreamHost: "fd00::5"}).upstream(); got != "http://[fd00::5]:8080" {
		t.Fatal(got)
	}
	if got := (proxyRoute{LocalPort: 8080}).upstream(); got != "http://127.0.0.1:8080" {
		t.Fatal(got)
	}
}

func TestProxyUpstreamHostIsPublishedAndRouted(t *testing.T) {
	m := testManager(t)
	rules := []ProxyRule{{Domain: "lan.test", LocalPort: 8000, UpstreamHost: "192.168.1.20", Tunnel: true}}
	if err := m.ReplaceProxyRules(context.Background(), 0, 8443, rules); err != nil {
		t.Fatal(err)
	}
	s, _ := m.Read()
	if err := writeJSON(filepath.Join(m.Root, "proxy", "rules.json"), proxyConfigFromRoutes(proxyRoutesFromState(&s))); err != nil {
		t.Fatal(err)
	}
	published, err := m.publishedProxyRoutes()
	if err != nil || len(published) != 1 || published[0].UpstreamHost != "192.168.1.20" {
		t.Fatal("snapshot lost the upstream host", published, err)
	}
	if state, err := m.ProxyPublicationState(); err != nil || state != "Published" {
		t.Fatal(state, err)
	}
	// Changing only the upstream host is a routing change.
	if err = m.ReplaceProxyRules(context.Background(), 1, 8443, []ProxyRule{{Domain: "lan.test", LocalPort: 8000, UpstreamHost: "192.168.1.21", Tunnel: true}}); err != nil {
		t.Fatal(err)
	}
	if state, _ := m.ProxyPublicationState(); state != "Pending" {
		t.Fatal("a new upstream host must be pending, got", state)
	}
	old := newLiveProxyRoutes(m, []proxyRoute{{Domain: "lan.test", LocalPort: 8000, UpstreamHost: "192.168.1.20", Port: 8443}}).current
	next := (&liveProxyRoutes{manager: m}).build([]proxyRoute{{Domain: "lan.test", LocalPort: 8000, UpstreamHost: "192.168.1.21", Port: 8443}}, old)
	if next.routes["lan.test:8443"] == old.routes["lan.test:8443"] {
		t.Fatal("a route kept its old transport after the upstream host changed")
	}
}

func TestUpstreamDialerReachesNonDefaultHost(t *testing.T) {
	l, err := net.Listen("tcp6", "[::1]:0")
	if err != nil {
		t.Skip("IPv6 loopback unavailable")
	}
	defer l.Close()
	srv := httptest.NewUnstartedServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.Write([]byte("remote ok")) }))
	srv.Listener = l
	srv.Start()
	defer srv.Close()
	site := proxyRoute{Domain: "v6.test", UpstreamHost: "::1", LocalPort: l.Addr().(*net.TCPAddr).Port, Port: 8443}
	route := newReverseRoute(site)
	rec := httptest.NewRecorder()
	req := httptest.NewRequest("GET", "https://v6.test/", nil)
	route.proxy.ServeHTTP(rec, req)
	if rec.Code != 200 || rec.Body.String() != "remote ok" {
		t.Fatal(rec.Code, rec.Body.String())
	}
}

func TestTunnelRouteUpstreamHost(t *testing.T) {
	if got := (TunnelRoute{LocalPort: 3000}).localHostHeader(); got != "localhost:3000" {
		t.Fatal(got)
	}
	if got := (TunnelRoute{LocalPort: 3000, UpstreamHost: "192.168.1.20"}).localHostHeader(); got != "192.168.1.20:3000" {
		t.Fatal(got)
	}
}

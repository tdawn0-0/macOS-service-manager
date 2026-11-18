package devplatform

import (
	"context"
	"crypto/tls"
	"crypto/x509"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestProxyRulesForwardHeadersAndKeepLastGoodRouting(t *testing.T) {
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_ = json.NewEncoder(w).Encode(map[string]string{"host": r.Host, "path": r.URL.RequestURI(), "for": r.Header.Get("X-Forwarded-For"), "proto": r.Header.Get("X-Forwarded-Proto"), "forwardedHost": r.Header.Get("X-Forwarded-Host"), "forwarded": r.Header.Get("Forwarded")})
	}))
	defer upstream.Close()
	m := testManager(t)
	listener := integrationPort(t)
	local := upstream.Listener.Addr().(*net.TCPAddr).Port
	config := proxyPublishedConfig{SchemaVersion: 1, ListenerPort: listener, Rules: []ProxyRule{{Domain: "settings.devbase.localhost", LocalPort: local}}}
	path := filepath.Join(m.Root, "proxy", "rules.json")
	if e := writeJSON(path, config); e != nil {
		t.Fatal(e)
	}
	ca, _, e := m.ca()
	if e != nil {
		t.Fatal(e)
	}
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() { done <- m.ServeProxy(ctx) }()
	defer func() {
		cancel()
		select {
		case e := <-done:
			if e != nil {
				t.Error(e)
			}
		case <-time.After(3 * time.Second):
			t.Error("proxy did not stop")
		}
	}()
	if e = m.verifyProxyTLS(ctx, proxyRoute{Domain: "settings.devbase.localhost", Port: listener}); e != nil {
		t.Fatal(e)
	}
	roots := x509.NewCertPool()
	roots.AddCert(ca)
	transport := &http.Transport{TLSClientConfig: &tls.Config{RootCAs: roots}, DialContext: func(ctx context.Context, network, address string) (net.Conn, error) {
		return (&net.Dialer{}).DialContext(ctx, "tcp", fmt.Sprintf("127.0.0.1:%d", listener))
	}}
	defer transport.CloseIdleConnections()
	client := &http.Client{Transport: transport, Timeout: 4 * time.Second}
	request := func() (*http.Response, map[string]string) {
		req, _ := http.NewRequest("GET", fmt.Sprintf("https://settings.devbase.localhost:%d/path%%2Fsegment?x=1&x=2", listener), nil)
		req.Header.Set("X-Forwarded-For", "198.51.100.99")
		req.Header.Set("Forwarded", "host=spoofed.invalid")
		res, e := client.Do(req)
		if e != nil {
			t.Fatal(e)
		}
		defer res.Body.Close()
		row := map[string]string{}
		if res.StatusCode == 200 {
			if e = json.NewDecoder(res.Body).Decode(&row); e != nil {
				t.Fatal(e)
			}
		}
		return res, row
	}
	res, row := request()
	if res.StatusCode != 200 || row["host"] != fmt.Sprintf("settings.devbase.localhost:%d", listener) || row["path"] != "/path%2Fsegment?x=1&x=2" || row["for"] != "127.0.0.1" || row["proto"] != "https" || row["forwarded"] != "" {
		t.Fatal(res.StatusCode, row)
	}
	if e = os.WriteFile(path, []byte(`{"schemaVersion":1,"listenerPort":`+fmt.Sprint(listener)+`,"rules":[{"domain":"settings.devbase.localhost","localPort":`+fmt.Sprint(local)+`,"upstream":"http://evil.devbase.localhost"}]}`), 0600); e != nil {
		t.Fatal(e)
	}
	transport.CloseIdleConnections()
	res, row = request()
	if res.StatusCode != 200 || row["host"] == "" {
		t.Fatal("invalid reload displaced last good routes", res.StatusCode, row)
	}
}

func TestProxyHeaderTimeoutAndLocalPortRules(t *testing.T) {
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		select {
		case <-time.After(1300 * time.Millisecond):
			io.WriteString(w, "slow but ready")
		case <-r.Context().Done():
		}
	}))
	defer upstream.Close()
	local := upstream.Listener.Addr().(*net.TCPAddr).Port
	route := newReverseRoute(proxyRoute{Domain: "timeout.devbase.localhost", LocalPort: local, Port: 8443})
	defer route.transport.CloseIdleConnections()
	route.transport.ResponseHeaderTimeout = time.Second
	server := httptest.NewServer(route.proxy)
	defer server.Close()
	start := time.Now()
	res, e := http.Get(server.URL)
	if e != nil {
		t.Fatal(e)
	}
	body, _ := io.ReadAll(res.Body)
	res.Body.Close()
	if res.StatusCode != 504 || !strings.Contains(string(body), "timed out") || time.Since(start) > 2*time.Second {
		t.Fatal(res.StatusCode, string(body), time.Since(start))
	}
	m := testManager(t)
	if e = m.ReplaceProxyRules(context.Background(), 0, 8443, []ProxyRule{{Domain: "timeout.devbase.localhost", LocalPort: local}}); e != nil {
		t.Fatal(e)
	}
	if _, e := normalizeProxyRules(8443, []ProxyRule{{Domain: "evil.devbase.localhost", LocalPort: 8443}}); e == nil {
		t.Fatal("accepted listener loop")
	}
	if e = os.WriteFile(filepath.Join(m.Root, "proxy", "rules.json"), []byte(`{"schemaVersion":1,"listenerPort":8443,"rules":[{"domain":"evil.devbase.localhost","localPort":3000,"upstream":"https://localhost:3000"}]}`), 0600); e != nil {
		t.Fatal(e)
	}
	if _, e = readPublishedProxyConfig(m.Root); e == nil {
		t.Fatal("accepted arbitrary HTTPS upstream")
	}
}

package devplatform

import (
	"context"
	"crypto/tls"
	"crypto/x509"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func serveSettingsFixture(t *testing.T, sites []Site) (*Manager, *http.Client) {
	t.Helper()
	m := testManager(t)
	if e := writeJSON(filepath.Join(m.Root, "proxy", "sites.json"), sites); e != nil {
		t.Fatal(e)
	}
	ca, _, e := m.ca()
	if e != nil {
		t.Fatal(e)
	}
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() { done <- m.ServeProxy(ctx) }()
	t.Cleanup(func() {
		cancel()
		select {
		case e := <-done:
			if e != nil {
				t.Error(e)
			}
		case <-time.After(3 * time.Second):
			t.Error("proxy did not stop")
		}
	})
	if e = m.verifySiteTLS(ctx, sites[0]); e != nil {
		t.Fatal(e)
	}
	roots := x509.NewCertPool()
	roots.AddCert(ca)
	transport := &http.Transport{TLSClientConfig: &tls.Config{RootCAs: roots}, DialContext: func(ctx context.Context, network, address string) (net.Conn, error) {
		return (&net.Dialer{}).DialContext(ctx, "tcp", fmt.Sprintf("127.0.0.1:%d", sites[0].Port))
	}}
	t.Cleanup(transport.CloseIdleConnections)
	return m, &http.Client{Transport: transport, Timeout: 4 * time.Second}
}

func settingsRequest(t *testing.T, client *http.Client, site Site, path string) (*http.Response, string) {
	t.Helper()
	req, e := http.NewRequest("GET", site.URL()+path, nil)
	if e != nil {
		t.Fatal(e)
	}
	req.Header.Set("X-Forwarded-For", "198.51.100.99")
	req.Header.Set("X-Forwarded-Host", "spoofed.invalid")
	req.Header.Set("X-Forwarded-Proto", "http")
	req.Header.Set("Forwarded", "host=spoofed.invalid;proto=http")
	res, e := client.Do(req)
	if e != nil {
		t.Fatal(e)
	}
	body, e := io.ReadAll(res.Body)
	res.Body.Close()
	if e != nil {
		t.Fatal(e)
	}
	return res, string(body)
}

func TestProxyHostPolicyForwardedHeadersAndLastGoodRouting(t *testing.T) {
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_ = json.NewEncoder(w).Encode(map[string]string{"host": r.Host, "path": r.URL.RequestURI(), "for": r.Header.Get("X-Forwarded-For"), "proto": r.Header.Get("X-Forwarded-Proto"), "forwardedHost": r.Header.Get("X-Forwarded-Host"), "forwarded": r.Header.Get("Forwarded")})
	}))
	defer upstream.Close()
	port := integrationPort(t)
	site := Site{ID: "settings", Domain: "settings.test", Upstream: upstream.URL, Port: port}
	other := Site{ID: "other", Domain: "other.test", Upstream: upstream.URL, Port: port}
	m, client := serveSettingsFixture(t, []Site{site, other})
	check := func(expectedHost string) {
		t.Helper()
		res, body := settingsRequest(t, client, site, "/path%2Fsegment?x=1&x=2")
		var row map[string]string
		if e := json.Unmarshal([]byte(body), &row); e != nil {
			t.Fatal(body, e)
		}
		if res.StatusCode != 200 || row["host"] != expectedHost || row["path"] != "/path%2Fsegment?x=1&x=2" || row["for"] != "127.0.0.1" || row["proto"] != "https" || row["forwardedHost"] != fmt.Sprintf("settings.test:%d", port) || row["forwarded"] != "" {
			t.Fatal("incorrect upstream request", row, res.StatusCode)
		}
	}
	check(fmt.Sprintf("settings.test:%d", port))
	site.Proxy = &ProxyOptions{ConnectTimeoutSeconds: 2, ResponseHeaderTimeoutSeconds: 1, HostPolicy: "upstream"}
	path := filepath.Join(m.Root, "proxy", "sites.json")
	if e := writeJSON(path, []Site{site, other}); e != nil {
		t.Fatal(e)
	}
	target, _ := url.Parse(upstream.URL)
	check(target.Host)
	if e := os.WriteFile(path, []byte("{incomplete configuration"), 0600); e != nil {
		t.Fatal(e)
	}
	client.Transport.(*http.Transport).CloseIdleConnections()
	check(target.Host) // Includes a new TLS handshake while the file is invalid.
	res, _ := settingsRequest(t, client, other, "/")
	if res.StatusCode != 200 {
		t.Fatal("invalid reload interrupted another site")
	}
	site.Upstream = "http://127.0.0.1:1"
	if e := writeJSON(path, []Site{site, other}); e != nil {
		t.Fatal(e)
	}
	res, body := settingsRequest(t, client, site, "/")
	if res.StatusCode != 502 || !strings.Contains(body, "Local service is unavailable") || !strings.HasPrefix(res.Header.Get("Content-Type"), "text/html") {
		t.Fatal(res.StatusCode, body)
	}
	res, _ = settingsRequest(t, client, other, "/")
	if res.StatusCode != 200 {
		t.Fatal("failed upstream interrupted another site")
	}
}

func TestProxyHeaderTimeoutAndVerifiedHTTPSUpstream(t *testing.T) {
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		select {
		case <-time.After(1300 * time.Millisecond):
			io.WriteString(w, "slow but ready")
		case <-r.Context().Done():
		}
	}))
	defer upstream.Close()
	site := Site{ID: "timeout", Domain: "timeout.test", Upstream: upstream.URL, Port: integrationPort(t), Proxy: &ProxyOptions{ConnectTimeoutSeconds: 1, ResponseHeaderTimeoutSeconds: 1}}
	m, client := serveSettingsFixture(t, []Site{site})
	start := time.Now()
	res, body := settingsRequest(t, client, site, "/")
	if res.StatusCode != 504 || !strings.Contains(body, "timed out") || time.Since(start) > 2*time.Second {
		t.Fatal(res.StatusCode, body, time.Since(start))
	}
	site.Proxy.ResponseHeaderTimeoutSeconds = 0
	if e := writeJSON(filepath.Join(m.Root, "proxy", "sites.json"), []Site{site}); e != nil {
		t.Fatal(e)
	}
	res, body = settingsRequest(t, client, site, "/")
	if res.StatusCode != 200 || body != "slow but ready" {
		t.Fatal("zero did not disable the header timeout", res.StatusCode, body)
	}
	untrusted := httptest.NewUnstartedServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { io.WriteString(w, "must not be accepted") }))
	untrusted.Config.ErrorLog = log.New(io.Discard, "", 0)
	untrusted.StartTLS()
	defer untrusted.Close()
	site.Upstream = untrusted.URL
	if e := writeJSON(filepath.Join(m.Root, "proxy", "sites.json"), []Site{site}); e != nil {
		t.Fatal(e)
	}
	res, body = settingsRequest(t, client, site, "/")
	if res.StatusCode != 502 || !strings.Contains(body, "Upstream certificate is not trusted") || strings.Contains(body, "must not be accepted") {
		t.Fatal("HTTPS upstream verification was bypassed", res.StatusCode, body)
	}
}

package devplatform

import (
	"bufio"
	"bytes"
	"context"
	"crypto/sha1"
	"crypto/sha256"
	"crypto/tls"
	"crypto/x509"
	"encoding/base64"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"
)

func TestHTTPSProxyWebSocketStreamingAndUpload(t *testing.T) {
	release := make(chan struct{})
	var releaseOnce sync.Once
	finishStream := func() { releaseOnce.Do(func() { close(release) }) }
	defer finishStream()
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/events":
			w.Header().Set("Content-Type", "text/event-stream")
			io.WriteString(w, "data: first\n\n")
			w.(http.Flusher).Flush()
			select {
			case <-release:
			case <-r.Context().Done():
			}
			io.WriteString(w, "data: second\n\n")
		case "/upload":
			h := sha256.New()
			io.Copy(h, r.Body)
			fmt.Fprintf(w, "%x", h.Sum(nil))
		case "/ws":
			conn, rw, e := w.(http.Hijacker).Hijack()
			if e != nil {
				return
			}
			defer conn.Close()
			conn.SetDeadline(time.Now().Add(3 * time.Second))
			key := sha1.Sum([]byte(r.Header.Get("Sec-WebSocket-Key") + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"))
			fmt.Fprintf(rw, "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: %s\r\n\r\n", base64.StdEncoding.EncodeToString(key[:]))
			rw.Flush()
			frame := make([]byte, 11)
			if _, e = io.ReadFull(rw, frame); e != nil {
				return
			}
			for n := 0; n < 5; n++ {
				frame[6+n] ^= frame[2+n%4]
			}
			conn.Write(append([]byte{0x81, 5}, frame[6:]...))
		default:
			http.NotFound(w, r)
		}
	}))
	defer upstream.Close()
	m := testManager(t)
	port := integrationPort(t)
	site := proxyRoute{Domain: "protocols.test", LocalPort: upstream.Listener.Addr().(*net.TCPAddr).Port, Port: port}
	localPort := upstream.Listener.Addr().(*net.TCPAddr).Port
	if e := writeJSON(filepath.Join(m.Root, "proxy", "rules.json"), proxyPublishedConfig{SchemaVersion: 1, ListenerPort: port, Rules: []ProxyRule{{Domain: site.Domain, LocalPort: localPort}}}); e != nil {
		t.Fatal(e)
	}
	ca, _, e := m.ca()
	if e != nil {
		t.Fatal(e)
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	done := make(chan error, 1)
	go func() { done <- m.ServeProxy(ctx) }()
	if e = m.verifyProxyTLS(ctx, site); e != nil {
		t.Fatal(e)
	}
	roots := x509.NewCertPool()
	roots.AddCert(ca)
	tlsConfig := &tls.Config{RootCAs: roots, ServerName: site.Domain, MinVersion: tls.VersionTLS12}
	transport := &http.Transport{TLSClientConfig: tlsConfig}
	defer transport.CloseIdleConnections()
	client := &http.Client{Transport: transport, Timeout: 3 * time.Second}
	base := fmt.Sprintf("https://127.0.0.1:%d", port)
	request := func(t *testing.T, method, path string, body io.Reader) *http.Response {
		req, e := http.NewRequest(method, base+path, body)
		if e != nil {
			t.Fatal(e)
		}
		req.Host = site.Domain
		res, e := client.Do(req)
		if e != nil {
			t.Fatal(e)
		}
		return res
	}
	t.Run("SSE flush", func(t *testing.T) {
		res := request(t, "GET", "/events", nil)
		defer res.Body.Close()
		reader := bufio.NewReader(res.Body)
		line, e := reader.ReadString('\n')
		if e != nil || line != "data: first\n" {
			t.Fatal(line, e)
		}
		time.Sleep(1200 * time.Millisecond) // A header timeout must not cap an established stream.
		finishStream()
		body, e := io.ReadAll(reader)
		if e != nil || !bytes.Contains(body, []byte("data: second")) {
			t.Fatal(string(body), e)
		}
	})
	t.Run("large upload", func(t *testing.T) {
		payload := bytes.Repeat([]byte("development-upload"), 128*1024)
		want := sha256.Sum256(payload)
		res := request(t, "POST", "/upload", bytes.NewReader(payload))
		defer res.Body.Close()
		body, e := io.ReadAll(res.Body)
		if e != nil || string(body) != fmt.Sprintf("%x", want) {
			t.Fatal(string(body), e)
		}
	})
	t.Run("WebSocket echo", func(t *testing.T) {
		conn, e := tls.DialWithDialer(&net.Dialer{Timeout: time.Second}, "tcp", fmt.Sprintf("127.0.0.1:%d", port), tlsConfig)
		if e != nil {
			t.Fatal(e)
		}
		defer conn.Close()
		conn.SetDeadline(time.Now().Add(3 * time.Second))
		fmt.Fprintf(conn, "GET /ws HTTP/1.1\r\nHost: %s\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n", site.Domain)
		reader := bufio.NewReader(conn)
		res, e := http.ReadResponse(reader, nil)
		if e != nil {
			t.Fatal(e)
		}
		if res.StatusCode != 101 || res.Header.Get("Sec-WebSocket-Accept") != "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=" {
			t.Fatal(res.Status, res.Header)
		}
		frame := []byte{0x81, 0x85, 1, 2, 3, 4}
		for n, ch := range []byte("hello") {
			frame = append(frame, ch^frame[2+n%4])
		}
		if _, e = conn.Write(frame); e != nil {
			t.Fatal(e)
		}
		reply := make([]byte, 7)
		if _, e = io.ReadFull(reader, reply); e != nil || !bytes.Equal(reply, append([]byte{0x81, 5}, []byte("hello")...)) {
			t.Fatal(reply, e)
		}
	})
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

func TestHTTPSProxyRejectsMismatchedHostAndSNI(t *testing.T) {
	var alphaServed, betaServed bool
	upstreamAlpha := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		alphaServed = true
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte("alpha"))
	}))
	defer upstreamAlpha.Close()
	upstreamBeta := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		betaServed = true
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte("beta"))
	}))
	defer upstreamBeta.Close()

	m := testManager(t)
	port := integrationPort(t)
	alphaPort := upstreamAlpha.Listener.Addr().(*net.TCPAddr).Port
	betaPort := upstreamBeta.Listener.Addr().(*net.TCPAddr).Port
	config := proxyPublishedConfig{
		SchemaVersion: 1,
		ListenerPort:  port,
		Rules: []ProxyRule{
			{Domain: "alpha.test", LocalPort: alphaPort},
			{Domain: "beta.test", LocalPort: betaPort},
		},
	}
	if e := writeJSON(filepath.Join(m.Root, "proxy", "rules.json"), config); e != nil {
		t.Fatal(e)
	}
	ca, _, e := m.ca()
	if e != nil {
		t.Fatal(e)
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	done := make(chan error, 1)
	go func() { done <- m.ServeProxy(ctx) }()
	if e = m.verifyProxyTLS(ctx, proxyRoute{Domain: "alpha.test", LocalPort: alphaPort, Port: port}); e != nil {
		t.Fatal(e)
	}
	if e = m.verifyProxyTLS(ctx, proxyRoute{Domain: "beta.test", LocalPort: betaPort, Port: port}); e != nil {
		t.Fatal(e)
	}

	roots := x509.NewCertPool()
	roots.AddCert(ca)
	// Client negotiates TLS with ServerName: "alpha.test"
	tlsConfig := &tls.Config{RootCAs: roots, ServerName: "alpha.test", MinVersion: tls.VersionTLS12}
	transport := &http.Transport{
		TLSClientConfig: tlsConfig,
		DialContext: func(ctx context.Context, network, address string) (net.Conn, error) {
			return (&net.Dialer{}).DialContext(ctx, "tcp", fmt.Sprintf("127.0.0.1:%d", port))
		},
	}
	defer transport.CloseIdleConnections()
	client := &http.Client{Transport: transport, Timeout: 3 * time.Second}

	// 1. Mismatch: TLS ServerName is "alpha.test", but request Host is "beta.test"
	reqMismatch, err := http.NewRequest("GET", fmt.Sprintf("https://127.0.0.1:%d/", port), nil)
	if err != nil {
		t.Fatal(err)
	}
	reqMismatch.Host = "beta.test"
	resMismatch, err := client.Do(reqMismatch)
	if err != nil {
		t.Fatal(err)
	}
	defer resMismatch.Body.Close()
	if resMismatch.StatusCode != http.StatusMisdirectedRequest {
		t.Fatalf("expected status 421 Misdirected Request, got %d", resMismatch.StatusCode)
	}
	if betaServed {
		t.Fatal("beta upstream handler was invoked on mismatched SNI/Host request")
	}
	if alphaServed {
		t.Fatal("alpha upstream handler was invoked on mismatched SNI/Host request")
	}

	// 2. Case variation matching SNI: req.Host = "ALPHA.test"
	reqMatch, err := http.NewRequest("GET", fmt.Sprintf("https://127.0.0.1:%d/", port), nil)
	if err != nil {
		t.Fatal(err)
	}
	reqMatch.Host = "ALPHA.test"
	resMatch, err := client.Do(reqMatch)
	if err != nil {
		t.Fatal(err)
	}
	defer resMatch.Body.Close()
	if resMatch.StatusCode != http.StatusOK {
		t.Fatalf("expected status 200 OK for case-insensitive match, got %d", resMatch.StatusCode)
	}
	if !alphaServed {
		t.Fatal("alpha upstream handler was not invoked for matching host")
	}
}

func listenIPv6OrSkip(t *testing.T) net.Listener {
	t.Helper()
	l, err := net.Listen("tcp6", "[::1]:0")
	if err != nil {
		t.Skip("IPv6 loopback unavailable")
	}
	return l
}

func acceptOnce(l net.Listener) {
	go func() {
		if c, err := l.Accept(); err == nil {
			c.Close()
		}
	}()
}

func TestLocalUpstreamDialFallsBackToIPv6Loopback(t *testing.T) {
	l := listenIPv6OrSkip(t)
	defer l.Close()
	acceptOnce(l)
	_, port, _ := net.SplitHostPort(l.Addr().String())
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	conn, err := dialLocalUpstream(ctx, port)
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	ip := conn.RemoteAddr().(*net.TCPAddr).IP
	if ip.To4() != nil || !ip.IsLoopback() {
		t.Fatalf("remote %v is not IPv6 loopback", ip)
	}
}

func TestLocalUpstreamDialPrefersIPv4(t *testing.T) {
	l, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer l.Close()
	acceptOnce(l)
	_, port, _ := net.SplitHostPort(l.Addr().String())
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	conn, err := dialLocalUpstream(ctx, port)
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	if conn.RemoteAddr().(*net.TCPAddr).IP.To4() == nil {
		t.Fatalf("remote %v is not IPv4", conn.RemoteAddr())
	}
}

func TestLocalUpstreamDialReportsIPv4ErrorWhenNothingListens(t *testing.T) {
	l, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	_, port, _ := net.SplitHostPort(l.Addr().String())
	l.Close()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	conn, err := dialLocalUpstream(ctx, port)
	if err == nil {
		conn.Close()
		t.Fatal("expected an error")
	}
	if !strings.Contains(err.Error(), "127.0.0.1") {
		t.Fatalf("error %q does not mention 127.0.0.1", err)
	}
}

func TestHTTPSProxyForwardsToIPv6OnlyUpstream(t *testing.T) {
	l6 := listenIPv6OrSkip(t)
	upstream := httptest.NewUnstartedServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		io.WriteString(w, "ipv6 upstream")
	}))
	upstream.Listener.Close()
	upstream.Listener = l6
	upstream.Start()
	defer upstream.Close()
	m := testManager(t)
	port := integrationPort(t)
	localPort := upstream.Listener.Addr().(*net.TCPAddr).Port
	site := proxyRoute{Domain: "ipv6.test", LocalPort: localPort, Port: port}
	if e := writeJSON(filepath.Join(m.Root, "proxy", "rules.json"), proxyPublishedConfig{SchemaVersion: 1, ListenerPort: port, Rules: []ProxyRule{{Domain: site.Domain, LocalPort: localPort}}}); e != nil {
		t.Fatal(e)
	}
	ca, _, e := m.ca()
	if e != nil {
		t.Fatal(e)
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	done := make(chan error, 1)
	go func() { done <- m.ServeProxy(ctx) }()
	if e = m.verifyProxyTLS(ctx, site); e != nil {
		t.Fatal(e)
	}
	roots := x509.NewCertPool()
	roots.AddCert(ca)
	transport := &http.Transport{TLSClientConfig: &tls.Config{RootCAs: roots, ServerName: site.Domain, MinVersion: tls.VersionTLS12}}
	defer transport.CloseIdleConnections()
	client := &http.Client{Transport: transport, Timeout: 5 * time.Second}
	req, e := http.NewRequest("GET", fmt.Sprintf("https://127.0.0.1:%d/", port), nil)
	if e != nil {
		t.Fatal(e)
	}
	req.Host = site.Domain
	res, e := client.Do(req)
	if e != nil {
		t.Fatal(e)
	}
	defer res.Body.Close()
	body, _ := io.ReadAll(res.Body)
	if res.StatusCode != 200 || string(body) != "ipv6 upstream" {
		t.Fatal(res.StatusCode, string(body))
	}
}

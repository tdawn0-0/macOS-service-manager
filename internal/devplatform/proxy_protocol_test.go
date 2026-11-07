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
	site := Site{ID: "protocols", Domain: "protocols.test", Upstream: upstream.URL, Port: port, Proxy: &ProxyOptions{ConnectTimeoutSeconds: 2, ResponseHeaderTimeoutSeconds: 1}}
	if e := writeJSON(filepath.Join(m.Root, "proxy", "sites.json"), []Site{site}); e != nil {
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
	if e = m.verifySiteTLS(ctx, site); e != nil {
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

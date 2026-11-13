package devplatform

import (
	"bufio"
	"bytes"
	"context"
	"crypto/rand"
	"crypto/tls"
	"crypto/x509"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"mime"
	"mime/multipart"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"
)

func TestTunnelConfigValidationAndSecretIsolation(t *testing.T) {
	m := testManager(t)
	ctx := context.Background()

	secret, err := m.ensureTunnelSecret()
	if err != nil || len(secret) != 64 {
		t.Fatalf("unexpected secret: len=%d err=%v", len(secret), err)
	}
	info, err := os.Stat(m.tunnelKeyPath())
	if err != nil || info.Mode().Perm() != 0600 {
		t.Fatalf("tunnel key permissions wrong: %v %v", info.Mode().Perm(), err)
	}

	apiToken := "cf-secret-api-token-0123456789abcdef"
	if err := m.SaveCloudflareSettings(ctx, apiToken, "acc_test_12345", ""); err != nil {
		t.Fatal(err)
	}
	if !m.HasCloudflareToken() {
		t.Fatal("expected Cloudflare API token to be present")
	}
	tokInfo, err := os.Stat(m.cloudflareTokenPath())
	if err != nil || tokInfo.Mode().Perm() != 0600 {
		t.Fatalf("cloudflare token permissions wrong: %v %v", tokInfo.Mode().Perm(), err)
	}
	s, err := m.Read()
	if err != nil {
		t.Fatal(err)
	}
	if s.CloudflareAccountID != "acc_test_12345" || !strings.HasPrefix(s.TunnelClientID, "mac-") {
		t.Fatalf("unexpected saved Cloudflare settings: %+v", s)
	}

	// Neither tunnel.key nor cloudflare.key may appear in state.json or diagnostics.
	rawState, err := os.ReadFile(filepath.Join(m.Root, "state.json"))
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(rawState), secret) || strings.Contains(string(rawState), apiToken) {
		t.Fatal("state.json exposed secret or API token")
	}
	report, err := m.Diagnose(ctx)
	if err != nil {
		t.Fatal(err)
	}
	redacted, err := report.RedactedJSON()
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(redacted), secret) || strings.Contains(string(redacted), apiToken) {
		t.Fatal("diagnostics exposed secret or API token")
	}

	// Validate per-rule worker names, URLs, custom domain validation, and collision checks.
	if got := DefaultTunnelWorkerName("api.app.test"); got != "devbase-api-app" {
		t.Fatalf("DefaultTunnelWorkerName = %q", got)
	}
	longWorker := DefaultTunnelWorkerName(strings.Repeat("ab.", 25) + "test")
	if len(longWorker) > 63 || strings.HasSuffix(longWorker, "-") || !strings.HasPrefix(longWorker, "devbase-") {
		t.Fatalf("invalid truncated worker name %q (len=%d)", longWorker, len(longWorker))
	}
	if got := (ProxyRule{Domain: "api.test", Tunnel: true}).PreviewTunnelURL("demo"); got != "https://devbase-api.demo.workers.dev" {
		t.Fatalf("workers.dev preview URL = %q", got)
	}
	if got := (ProxyRule{Domain: "api.test", Tunnel: true, TunnelDomain: "api.example.com"}).PreviewTunnelURL("demo"); got != "https://api.example.com" {
		t.Fatalf("custom domain preview URL = %q", got)
	}

	if err := m.ReplaceProxyRules(ctx, 0, 8443, []ProxyRule{
		{Domain: "a.b.test", LocalPort: 3000, Tunnel: true},
		{Domain: "a-b.test", LocalPort: 3001, Tunnel: true},
	}); err == nil || !strings.Contains(err.Error(), "duplicate tunnel worker") {
		t.Fatalf("accepted duplicate resolved worker name: %v", err)
	}
	if err := m.ReplaceProxyRules(ctx, 0, 8443, []ProxyRule{
		{Domain: "app.test", LocalPort: 3000, Tunnel: true, TunnelWorker: "other-worker"},
	}); err == nil || !strings.Contains(err.Error(), "devbase-") {
		t.Fatalf("accepted non-devbase worker name: %v", err)
	}
	for _, badDomain := range []string{"app.test", "devbase-app.demo.workers.dev", "singlelabel", "bad/path.example.com"} {
		if err := m.ReplaceProxyRules(ctx, 0, 8443, []ProxyRule{
			{Domain: "app.test", LocalPort: 3000, Tunnel: true, TunnelDomain: badDomain},
		}); err == nil {
			t.Fatalf("accepted invalid custom domain %q", badDomain)
		}
	}
	if err := m.ReplaceProxyRules(ctx, 0, 8443, []ProxyRule{
		{Domain: "a.test", LocalPort: 3000, Tunnel: true, TunnelDomain: "same.example.com"},
		{Domain: "b.test", LocalPort: 3001, Tunnel: true, TunnelDomain: "same.example.com"},
	}); err == nil || !strings.Contains(err.Error(), "duplicate tunnel custom domain") {
		t.Fatalf("accepted duplicate custom domain: %v", err)
	}
	if err := m.ReplaceProxyRules(ctx, 0, 8443, []ProxyRule{
		{Domain: "a.b.test", LocalPort: 3000, Tunnel: true, TunnelWorker: "devbase-api", TunnelDomain: "api.example.com"},
		{Domain: "a-b.test", LocalPort: 3001, Tunnel: true},
	}); err != nil {
		t.Fatalf("valid disambiguated tunnel rules failed: %v", err)
	}
}

func TestTunnelAuthAndWebSocketFraming(t *testing.T) {
	now := time.Unix(1700000000, 0)
	secret := "0123456789abcdef0123456789abcdef"
	sig := signTunnelHandshake(secret, "mac-1", "1700000010", "nonce-1")
	if !verifyTunnelHandshake(secret, "mac-1", "1700000010", "nonce-1", sig, now) {
		t.Fatal("valid handshake rejected")
	}
	if verifyTunnelHandshake(secret, "mac-2", "1700000010", "nonce-1", sig, now) {
		t.Fatal("wrong client accepted")
	}
	if verifyTunnelHandshake("wrong-secret-0123456789", "mac-1", "1700000010", "nonce-1", sig, now) {
		t.Fatal("wrong secret accepted")
	}
	if verifyTunnelHandshake(secret, "mac-1", "1699999900", "nonce-1", sig, now) {
		t.Fatal("expired timestamp accepted")
	}

	raw := encodeTunnelFrame(tunnelFrame{
		Type:     frameHTTPResHead,
		Flags:    tunnelFlagFIN | tunnelFlagText,
		StreamID: 42,
		Code:     201,
		Payload:  []byte("hello"),
	})
	decoded, err := decodeTunnelFrame(raw)
	if err != nil {
		t.Fatal(err)
	}
	if decoded.Type != frameHTTPResHead || decoded.Flags != (tunnelFlagFIN|tunnelFlagText) || decoded.StreamID != 42 || decoded.Code != 201 || string(decoded.Payload) != "hello" {
		t.Fatalf("decoded frame mismatch: %+v", decoded)
	}

	// Verify fragmented WebSocket frame reassembly and Ping/Pong over loopback TCP.
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer ln.Close()
	acceptCh := make(chan net.Conn, 1)
	go func() {
		conn, _ := ln.Accept()
		acceptCh <- conn
	}()
	c1, err := net.Dial("tcp", ln.Addr().String())
	if err != nil {
		t.Fatal(err)
	}
	defer c1.Close()
	c2 := <-acceptCh
	if c2 == nil {
		t.Fatal("failed to accept loopback conn")
	}
	defer c2.Close()
	clientWS := &wsConn{conn: c1, br: bufio.NewReader(c1), isClient: true}
	serverWS := &wsConn{conn: c2, br: bufio.NewReader(c2), isClient: false}

	errCh := make(chan error, 1)
	go func() {
		// Send fragmented binary message: first chunk (fin=0), ping, continuation (fin=1).
		chunk1 := []byte("hello-")
		chunk2 := []byte("world")
		var hdr [2]byte
		hdr[0] = wsOpBinary // fin=0
		hdr[1] = 0x80 | byte(len(chunk1))
		var mask [4]byte
		_, _ = rand.Read(mask[:])
		masked1 := append([]byte(nil), chunk1...)
		for i := range masked1 {
			masked1[i] ^= mask[i%4]
		}
		pkt := append(hdr[:], mask[:]...)
		pkt = append(pkt, masked1...)
		if _, e := c1.Write(pkt); e != nil {
			errCh <- e
			return
		}
		if e := clientWS.WritePing([]byte("hb")); e != nil {
			errCh <- e
			return
		}
		hdr[0] = 0x80 | wsOpCont // fin=1
		hdr[1] = 0x80 | byte(len(chunk2))
		_, _ = rand.Read(mask[:])
		masked2 := append([]byte(nil), chunk2...)
		for i := range masked2 {
			masked2[i] ^= mask[i%4]
		}
		pkt2 := append(hdr[:], mask[:]...)
		pkt2 = append(pkt2, masked2...)
		_, e := c1.Write(pkt2)
		errCh <- e
	}()

	op, msg, err := serverWS.ReadMessage()
	if err != nil {
		t.Fatal(err)
	}
	if op != wsOpBinary || string(msg) != "hello-world" {
		t.Fatalf("fragmented read mismatch: op=%d msg=%q", op, msg)
	}
	if err := <-errCh; err != nil {
		t.Fatal(err)
	}
}

type mockGatewayServer struct {
	t            *testing.T
	secret       string
	server       *httptest.Server
	mu           sync.Mutex
	agentWS      *wsConn
	ownerID      string
	claimedOwner string // externally owned worker for conflict testing
	httpWait     map[uint32]*mockHTTPWaiter
	wsWait       map[uint32]chan tunnelWSOpenAckResult
	visitors     map[uint32]*wsConn
	nextID       atomic.Uint32
	conns        atomic.Int32
}

type mockHTTPWaiter struct {
	respCh chan *http.Response
	pw     *io.PipeWriter
}

type tunnelWSOpenAckResult struct {
	code     int
	protocol string
	errMsg   string
}

func newMockGatewayServer(t *testing.T, secret string) *mockGatewayServer {
	t.Helper()
	g := &mockGatewayServer{
		t:        t,
		secret:   secret,
		httpWait: map[uint32]*mockHTTPWaiter{},
		wsWait:   map[uint32]chan tunnelWSOpenAckResult{},
		visitors: map[uint32]*wsConn{},
	}
	g.server = httptest.NewTLSServer(http.HandlerFunc(g.serveHTTP))
	t.Cleanup(g.server.Close)
	return g
}

func (g *mockGatewayServer) tlsConfig() *tls.Config {
	pool := x509.NewCertPool()
	pool.AddCert(g.server.Certificate())
	return &tls.Config{RootCAs: pool, MinVersion: tls.VersionTLS12}
}

func (g *mockGatewayServer) dropClient(clientID string) {
	g.mu.Lock()
	var ws *wsConn
	if g.ownerID == clientID {
		ws = g.agentWS
	}
	g.mu.Unlock()
	if ws != nil {
		_ = ws.Close()
	}
}

func (g *mockGatewayServer) serveHTTP(w http.ResponseWriter, r *http.Request) {
	if r.URL.Path == "/__devbase/connect" {
		clientID := r.Header.Get("X-Devbase-Client")
		ts := r.Header.Get("X-Devbase-Timestamp")
		nonce := r.Header.Get("X-Devbase-Nonce")
		sig := r.Header.Get("X-Devbase-Signature")
		if !verifyTunnelHandshake(g.secret, clientID, ts, nonce, sig, time.Now()) {
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}
		ws, err := upgradeWebSocket(w, r, nil)
		if err != nil {
			return
		}
		g.conns.Add(1)
		g.serveAgent(clientID, ws)
		return
	}

	host := strings.ToLower(r.Host)
	if h, _, err := net.SplitHostPort(host); err == nil {
		host = h
	}
	g.mu.Lock()
	agentWS := g.agentWS
	g.mu.Unlock()
	if agentWS == nil {
		http.Error(w, "tunnel offline", http.StatusBadGateway)
		return
	}

	if strings.EqualFold(r.Header.Get("Upgrade"), "websocket") {
		g.handleVisitorWS(w, r, host, agentWS)
		return
	}
	g.handleVisitorHTTP(w, r, host, agentWS)
}

func (g *mockGatewayServer) serveAgent(clientID string, ws *wsConn) {
	defer func() {
		ws.Close()
		g.mu.Lock()
		if g.agentWS == ws {
			g.agentWS = nil
			g.ownerID = ""
		}
		g.mu.Unlock()
	}()
	for {
		op, data, err := ws.ReadMessage()
		if err != nil {
			return
		}
		if op == wsOpText {
			if string(data) == tunnelControlPing {
				_ = ws.WriteText([]byte(tunnelControlPong))
			}
			continue
		}
		frame, err := decodeTunnelFrame(data)
		if err != nil {
			return
		}
		switch frame.Type {
		case frameRegister:
			var reg tunnelRegisterPayload
			if err := json.Unmarshal(frame.Payload, &reg); err != nil {
				return
			}
			g.mu.Lock()
			var ackPayload tunnelRegisterAckPayload
			if g.claimedOwner != "" && g.claimedOwner != reg.ClientID {
				ackPayload.ConflictOwner = g.claimedOwner
			} else if g.ownerID != "" && g.ownerID != reg.ClientID && g.agentWS != nil {
				ackPayload.ConflictOwner = g.ownerID
			} else {
				g.agentWS = ws
				g.ownerID = reg.ClientID
				ackPayload.OK = true
			}
			g.mu.Unlock()
			ack, _ := encodeTunnelJSONFrame(frameRegisterAck, tunnelFlagFIN, 0, 0, ackPayload)
			_ = ws.WriteBinary(ack)
		case frameHTTPResHead:
			var head tunnelHTTPResHead
			if err := json.Unmarshal(frame.Payload, &head); err != nil {
				continue
			}
			g.mu.Lock()
			waiter := g.httpWait[frame.StreamID]
			g.mu.Unlock()
			if waiter == nil {
				continue
			}
			pr, pw := io.Pipe()
			resp := &http.Response{
				StatusCode: head.Status,
				Header:     make(http.Header),
				Body:       pr,
			}
			for _, kv := range head.Headers {
				resp.Header.Add(kv[0], kv[1])
			}
			if frame.Flags&tunnelFlagFIN != 0 {
				pw.Close()
				g.mu.Lock()
				delete(g.httpWait, frame.StreamID)
				g.mu.Unlock()
			} else {
				waiter.pw = pw
			}
			waiter.respCh <- resp
		case frameHTTPResData:
			g.mu.Lock()
			waiter := g.httpWait[frame.StreamID]
			g.mu.Unlock()
			if waiter == nil || waiter.pw == nil {
				continue
			}
			if len(frame.Payload) > 0 {
				_, _ = waiter.pw.Write(frame.Payload)
			}
			if frame.Flags&tunnelFlagFIN != 0 {
				waiter.pw.Close()
				g.mu.Lock()
				delete(g.httpWait, frame.StreamID)
				g.mu.Unlock()
			}
		case frameStreamReset:
			g.mu.Lock()
			waiter := g.httpWait[frame.StreamID]
			delete(g.httpWait, frame.StreamID)
			vws := g.visitors[frame.StreamID]
			delete(g.visitors, frame.StreamID)
			g.mu.Unlock()
			if waiter != nil && waiter.pw != nil {
				waiter.pw.CloseWithError(errors.New("stream reset"))
			}
			if vws != nil {
				_ = vws.Close()
			}
		case frameWSOpenAck:
			var ack tunnelWSOpenAckHead
			_ = json.Unmarshal(frame.Payload, &ack)
			g.mu.Lock()
			ch := g.wsWait[frame.StreamID]
			delete(g.wsWait, frame.StreamID)
			g.mu.Unlock()
			if ch != nil {
				ch <- tunnelWSOpenAckResult{code: int(frame.Code), protocol: ack.Protocol, errMsg: ack.Error}
			}
		case frameWSData:
			g.mu.Lock()
			vws := g.visitors[frame.StreamID]
			g.mu.Unlock()
			if vws != nil {
				op := wsOpBinary
				if frame.Flags&tunnelFlagText != 0 {
					op = wsOpText
				}
				// Reassembly for mock server test: accumulate or send when FIN.
				g.deliverWSChunk(frame.StreamID, vws, op, frame.Flags&tunnelFlagFIN != 0, frame.Payload)
			}
		case frameWSClose:
			g.mu.Lock()
			vws := g.visitors[frame.StreamID]
			delete(g.visitors, frame.StreamID)
			g.mu.Unlock()
			if vws != nil {
				_ = vws.WriteClose(frame.Code, string(frame.Payload))
				_ = vws.Close()
			}
		}
	}
}

var mockWSChunks sync.Map

func (g *mockGatewayServer) deliverWSChunk(streamID uint32, vws *wsConn, op byte, fin bool, payload []byte) {
	v, _ := mockWSChunks.LoadOrStore(streamID, &bytes.Buffer{})
	buf := v.(*bytes.Buffer)
	buf.Write(payload)
	if fin {
		mockWSChunks.Delete(streamID)
		_ = vws.WriteMessage(op, append([]byte(nil), buf.Bytes()...))
	}
}

func (g *mockGatewayServer) handleVisitorHTTP(w http.ResponseWriter, r *http.Request, host string, agentWS *wsConn) {
	streamID := g.nextID.Add(1)
	var headers [][2]string
	for k, vals := range r.Header {
		for _, v := range vals {
			headers = append(headers, [2]string{k, v})
		}
	}
	waiter := &mockHTTPWaiter{respCh: make(chan *http.Response, 1)}
	g.mu.Lock()
	g.httpWait[streamID] = waiter
	g.mu.Unlock()

	bodyless := r.Body == nil || r.Method == http.MethodGet || r.Method == http.MethodHead
	var headFlags byte
	if bodyless {
		headFlags = tunnelFlagFIN
	}
	headFrame, _ := encodeTunnelJSONFrame(frameHTTPReqHead, headFlags, streamID, 0, tunnelHTTPReqHead{
		Method:     r.Method,
		Host:       host,
		Path:       r.URL.RequestURI(),
		Headers:    headers,
		RemoteAddr: "203.0.113.10",
	})
	_ = agentWS.WriteBinary(headFrame)
	if !bodyless {
		buf := make([]byte, tunnelMaxChunk)
		for {
			n, err := r.Body.Read(buf)
			if n > 0 {
				var flags byte
				if err == io.EOF {
					flags = tunnelFlagFIN
				}
				_ = agentWS.WriteBinary(encodeTunnelFrame(tunnelFrame{
					Type:     frameHTTPReqData,
					Flags:    flags,
					StreamID: streamID,
					Payload:  buf[:n],
				}))
				if err == io.EOF {
					break
				}
			} else if err == io.EOF {
				_ = agentWS.WriteBinary(encodeTunnelFrame(tunnelFrame{
					Type:     frameHTTPReqData,
					Flags:    tunnelFlagFIN,
					StreamID: streamID,
				}))
				break
			} else if err != nil {
				break
			}
		}
	}

	resp := <-waiter.respCh
	defer resp.Body.Close()
	for k, vals := range resp.Header {
		for _, v := range vals {
			w.Header().Add(k, v)
		}
	}
	w.WriteHeader(resp.StatusCode)
	if fl, ok := w.(http.Flusher); ok {
		fl.Flush()
		buf := make([]byte, 32<<10)
		for {
			n, err := resp.Body.Read(buf)
			if n > 0 {
				_, _ = w.Write(buf[:n])
				fl.Flush()
			}
			if err != nil {
				break
			}
		}
		return
	}
	_, _ = io.Copy(w, resp.Body)
}

func (g *mockGatewayServer) handleVisitorWS(w http.ResponseWriter, r *http.Request, host string, agentWS *wsConn) {
	streamID := g.nextID.Add(1)
	var protos []string
	if p := r.Header.Get("Sec-WebSocket-Protocol"); p != "" {
		for _, s := range strings.Split(p, ",") {
			if s = strings.TrimSpace(s); s != "" {
				protos = append(protos, s)
			}
		}
	}
	var headers [][2]string
	for k, vals := range r.Header {
		for _, v := range vals {
			headers = append(headers, [2]string{k, v})
		}
	}
	ch := make(chan tunnelWSOpenAckResult, 1)
	g.mu.Lock()
	g.wsWait[streamID] = ch
	g.mu.Unlock()
	openFrame, _ := encodeTunnelJSONFrame(frameWSOpen, tunnelFlagFIN, streamID, 0, tunnelWSOpenHead{
		Host:      host,
		Path:      r.URL.RequestURI(),
		Protocols: protos,
		Headers:   headers,
	})
	_ = agentWS.WriteBinary(openFrame)
	ack := <-ch
	if ack.code != http.StatusSwitchingProtocols {
		http.Error(w, ack.errMsg, ack.code)
		return
	}
	respH := http.Header{}
	if ack.protocol != "" {
		respH.Set("Sec-WebSocket-Protocol", ack.protocol)
	}
	vws, err := upgradeWebSocket(w, r, respH)
	if err != nil {
		return
	}
	g.mu.Lock()
	g.visitors[streamID] = vws
	g.mu.Unlock()

	for {
		op, msg, err := vws.ReadMessage()
		if err != nil {
			var closeErr *wsCloseError
			code := uint16(1000)
			reason := ""
			if errors.As(err, &closeErr) {
				code = closeErr.Code
				reason = closeErr.Reason
			}
			_ = agentWS.WriteBinary(encodeTunnelFrame(tunnelFrame{
				Type:     frameWSClose,
				Flags:    tunnelFlagFIN,
				StreamID: streamID,
				Code:     code,
				Payload:  []byte(reason),
			}))
			return
		}
		var textFlag byte
		if op == wsOpText {
			textFlag = tunnelFlagText
		}
		for off := 0; off < len(msg); off += tunnelMaxChunk {
			end := off + tunnelMaxChunk
			flags := textFlag
			if end >= len(msg) {
				end = len(msg)
				flags |= tunnelFlagFIN
			}
			_ = agentWS.WriteBinary(encodeTunnelFrame(tunnelFrame{
				Type:     frameWSData,
				Flags:    flags,
				StreamID: streamID,
				Payload:  msg[off:end],
			}))
		}
	}
}

func TestCloudflareAPISyncDiscoveryAndCleanup(t *testing.T) {
	type mockScript struct {
		migrationTag     string
		subdomainEnabled bool
		customDomains    []string
		secretText       string
		sawMigrations    []bool
	}
	var mu sync.Mutex
	scripts := map[string]*mockScript{
		"existing-blog": {migrationTag: "v1", subdomainEnabled: true},
	}
	var deletedScripts []string
	var touchedUnrelated bool
	wantToken := "cf-api-token-test-0123456789"

	cfSrv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Authorization") != "Bearer "+wantToken {
			w.WriteHeader(http.StatusUnauthorized)
			_ = json.NewEncoder(w).Encode(map[string]any{"success": false, "errors": []map[string]any{{"code": 10000, "message": "Authentication error"}}})
			return
		}
		w.Header().Set("Content-Type", "application/json")
		switch {
		case r.Method == http.MethodGet && r.URL.Path == "/accounts":
			_ = json.NewEncoder(w).Encode(map[string]any{"success": true, "result": []map[string]string{{"id": "acc-cf-999", "name": "Dev"}}})
		case r.Method == http.MethodGet && r.URL.Path == "/accounts/acc-cf-999/workers/subdomain":
			_ = json.NewEncoder(w).Encode(map[string]any{"success": true, "result": map[string]string{"subdomain": "my-dev"}})
		case r.Method == http.MethodGet && r.URL.Path == "/zones":
			_ = json.NewEncoder(w).Encode(map[string]any{"success": true, "result": []map[string]string{{"id": "zone-1", "name": "example.com"}}})
		case r.Method == http.MethodGet && r.URL.Path == "/zones/zone-1/dns_records":
			_ = json.NewEncoder(w).Encode(map[string]any{
				"success": true,
				"result": []map[string]string{
					{"id": "r1", "name": "web.example.com", "type": "CNAME"},
					{"id": "r2", "name": "mail.example.com", "type": "MX"},
				},
			})
		case r.Method == http.MethodGet && strings.HasPrefix(r.URL.Path, "/accounts/acc-cf-999/workers/services/"):
			name := strings.TrimPrefix(r.URL.Path, "/accounts/acc-cf-999/workers/services/")
			mu.Lock()
			sc := scripts[name]
			mu.Unlock()
			if sc == nil {
				w.WriteHeader(http.StatusNotFound)
				_ = json.NewEncoder(w).Encode(map[string]any{"success": false, "errors": []map[string]any{{"code": 10007, "message": "not found"}}})
				return
			}
			_ = json.NewEncoder(w).Encode(map[string]any{"success": true, "result": map[string]any{"default_environment": map[string]any{"script": map[string]string{"migration_tag": sc.migrationTag}}}})
		case r.Method == http.MethodPut && strings.HasPrefix(r.URL.Path, "/accounts/acc-cf-999/workers/scripts/") && !strings.HasSuffix(r.URL.Path, "/domains/records"):
			name := strings.TrimPrefix(r.URL.Path, "/accounts/acc-cf-999/workers/scripts/")
			if name == "existing-blog" {
				mu.Lock()
				touchedUnrelated = true
				mu.Unlock()
			}
			_, params, err := mime.ParseMediaType(r.Header.Get("Content-Type"))
			if err != nil {
				t.Fatalf("bad multipart header: %v", err)
			}
			mr := multipart.NewReader(r.Body, params["boundary"])
			var metaRaw, codeRaw []byte
			for {
				p, err := mr.NextPart()
				if err == io.EOF {
					break
				}
				if err != nil {
					t.Fatalf("multipart read error: %v", err)
				}
				data, _ := io.ReadAll(p)
				switch p.FormName() {
				case "metadata":
					metaRaw = data
				case "index.mjs":
					codeRaw = data
				}
			}
			if !strings.Contains(string(codeRaw), "TunnelGateway") {
				t.Fatalf("uploaded script missing TunnelGateway: %s", string(codeRaw))
			}
			var meta struct {
				MainModule string           `json:"main_module"`
				Bindings   []map[string]any `json:"bindings"`
				Migrations any              `json:"migrations"`
			}
			if err := json.Unmarshal(metaRaw, &meta); err != nil {
				t.Fatalf("decode metadata: %v", err)
			}
			var secretVal string
			for _, b := range meta.Bindings {
				if b["name"] == "TUNNEL_SECRET" {
					secretVal, _ = b["text"].(string)
				}
			}
			mu.Lock()
			sc := scripts[name]
			if sc == nil {
				sc = &mockScript{}
				scripts[name] = sc
			}
			sc.migrationTag = "v1"
			sc.secretText = secretVal
			sc.sawMigrations = append(sc.sawMigrations, meta.Migrations != nil)
			mu.Unlock()
			_ = json.NewEncoder(w).Encode(map[string]any{"success": true, "result": map[string]any{"id": name}})
		case r.Method == http.MethodPost && strings.HasSuffix(r.URL.Path, "/subdomain"):
			trimmed := strings.TrimPrefix(r.URL.Path, "/accounts/acc-cf-999/workers/scripts/")
			name := strings.TrimSuffix(trimmed, "/subdomain")
			var body struct {
				Enabled bool `json:"enabled"`
			}
			_ = json.NewDecoder(r.Body).Decode(&body)
			mu.Lock()
			if scripts[name] != nil {
				scripts[name].subdomainEnabled = body.Enabled
			}
			mu.Unlock()
			_ = json.NewEncoder(w).Encode(map[string]any{"success": true, "result": map[string]any{"enabled": body.Enabled}})
		case r.Method == http.MethodPut && strings.HasSuffix(r.URL.Path, "/domains/records"):
			trimmed := strings.TrimPrefix(r.URL.Path, "/accounts/acc-cf-999/workers/scripts/")
			name := strings.TrimSuffix(trimmed, "/domains/records")
			var body struct {
				Origins []struct {
					Hostname string `json:"hostname"`
					ZoneID   string `json:"zone_id"`
				} `json:"origins"`
			}
			_ = json.NewDecoder(r.Body).Decode(&body)
			var hosts []string
			for _, o := range body.Origins {
				hosts = append(hosts, o.Hostname)
			}
			mu.Lock()
			if scripts[name] != nil {
				scripts[name].customDomains = hosts
			}
			mu.Unlock()
			_ = json.NewEncoder(w).Encode(map[string]any{"success": true, "result": []any{}})
		case r.Method == http.MethodDelete && strings.HasPrefix(r.URL.Path, "/accounts/acc-cf-999/workers/scripts/"):
			name := strings.TrimPrefix(r.URL.Path, "/accounts/acc-cf-999/workers/scripts/")
			mu.Lock()
			if name == "existing-blog" {
				touchedUnrelated = true
			}
			delete(scripts, name)
			deletedScripts = append(deletedScripts, name)
			mu.Unlock()
			_ = json.NewEncoder(w).Encode(map[string]any{"success": true, "result": map[string]any{}})
		default:
			t.Fatalf("unexpected Cloudflare API request: %s %s", r.Method, r.URL.String())
		}
	}))
	defer cfSrv.Close()

	m := testManager(t)
	m.CloudflareAPIBase = cfSrv.URL
	ctx := context.Background()

	if err := m.SaveCloudflareSettings(ctx, wantToken, "", "mac-cf-test"); err != nil {
		t.Fatal(err)
	}
	if err := m.RefreshCloudflareMetadata(ctx); err != nil {
		t.Fatal(err)
	}
	s, err := m.Read()
	if err != nil {
		t.Fatal(err)
	}
	if s.CloudflareAccountID != "acc-cf-999" || s.CloudflareSubdomain != "my-dev" || len(s.CloudflareZones) != 1 || len(s.CloudflareDNS) != 1 || s.CloudflareDNS[0].Name != "web.example.com" {
		t.Fatalf("unexpected metadata: %+v", s)
	}

	// Deploy 1 workers.dev rule + 1 Custom Domain rule.
	if err := m.ReplaceProxyRules(ctx, s.ProxyRulesRevision, 8443, []ProxyRule{
		{Domain: "api.test", LocalPort: 3000, Tunnel: true},
		{Domain: "web.test", LocalPort: 4000, Tunnel: true, TunnelDomain: "web.example.com"},
	}); err != nil {
		t.Fatal(err)
	}
	if err := m.SyncTunnels(ctx); err != nil {
		t.Fatal(err)
	}
	s, err = m.Read()
	if err != nil {
		t.Fatal(err)
	}
	if s.ProxyRules[0].TunnelURL != "https://devbase-api.my-dev.workers.dev" || s.ProxyRules[1].TunnelURL != "https://web.example.com" {
		t.Fatalf("unexpected synced rule URLs: %+v", s.ProxyRules)
	}
	if len(s.TunnelWorkers) != 2 || !s.TunnelWorkers["devbase-api"].PublicURLEquals("https://devbase-api.my-dev.workers.dev") {
		t.Fatalf("unexpected TunnelWorkers state: %+v", s.TunnelWorkers)
	}
	tunnelSecret, err := m.ReadTunnelSecret()
	if err != nil {
		t.Fatal(err)
	}
	mu.Lock()
	apiScript := scripts["devbase-api"]
	webScript := scripts["devbase-web"]
	mu.Unlock()
	if apiScript == nil || !apiScript.subdomainEnabled || apiScript.secretText != tunnelSecret || !apiScript.sawMigrations[0] {
		t.Fatalf("unexpected devbase-api script state: %+v", apiScript)
	}
	if webScript == nil || webScript.subdomainEnabled || len(webScript.customDomains) != 1 || webScript.customDomains[0] != "web.example.com" || !webScript.sawMigrations[0] {
		t.Fatalf("unexpected devbase-web script state: %+v", webScript)
	}
	cfg, err := readPublishedTunnelConfig(m.Root)
	if err != nil || len(cfg.Routes) != 2 {
		t.Fatalf("unexpected published tunnel.json: %+v err=%v", cfg, err)
	}

	// Second sync should omit migrations because existing migration_tag == "v1".
	if err := m.SyncTunnels(ctx); err != nil {
		t.Fatal(err)
	}
	mu.Lock()
	if len(scripts["devbase-api"].sawMigrations) != 2 || scripts["devbase-api"].sawMigrations[1] {
		t.Fatalf("expected migrations omitted on update, got %v", scripts["devbase-api"].sawMigrations)
	}
	mu.Unlock()

	// Invalid custom domain outside active zones must fail before mutating deployed workers.
	if err := m.ReplaceProxyRules(ctx, s.ProxyRulesRevision, 8443, []ProxyRule{
		{Domain: "api.test", LocalPort: 3000, Tunnel: true},
		{Domain: "web.test", LocalPort: 4000, Tunnel: true, TunnelDomain: "bad.unowned.org"},
	}); err != nil {
		t.Fatal(err)
	}
	if err := m.SyncTunnels(ctx); err == nil || !strings.Contains(err.Error(), "active Cloudflare zone") {
		t.Fatalf("expected unowned zone rejection, got %v", err)
	}

	// Disabling Tunnel on web.test deletes devbase-web cleanly and leaves devbase-api + unrelated existing-blog untouched.
	s, _ = m.Read()
	if err := m.ReplaceProxyRules(ctx, s.ProxyRulesRevision, 8443, []ProxyRule{
		{Domain: "api.test", LocalPort: 3000, Tunnel: true},
		{Domain: "web.test", LocalPort: 4000, Tunnel: false},
	}); err != nil {
		t.Fatal(err)
	}
	if err := m.SyncTunnels(ctx); err != nil {
		t.Fatal(err)
	}
	s, _ = m.Read()
	if len(s.TunnelWorkers) != 1 || s.TunnelWorkers["devbase-api"].ScriptName != "devbase-api" || s.ProxyRules[1].TunnelURL != "" {
		t.Fatalf("orphan cleanup state mismatch: %+v", s)
	}
	mu.Lock()
	if len(deletedScripts) != 1 || deletedScripts[0] != "devbase-web" || scripts["devbase-web"] != nil || touchedUnrelated {
		t.Fatalf("unexpected delete history=%v touchedUnrelated=%v", deletedScripts, touchedUnrelated)
	}
	mu.Unlock()
}

func (w TunnelWorkerState) PublicURLEquals(want string) bool {
	return w.PublicURL == want
}

func TestTunnelEndToEndHTTPWebSocketHotReloadAndConflict(t *testing.T) {
	sseRelease := make(chan struct{})
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if strings.EqualFold(r.Header.Get("Upgrade"), "websocket") {
			ws, err := upgradeWebSocket(w, r, http.Header{"Sec-WebSocket-Protocol": []string{"vite-hmr"}})
			if err != nil {
				return
			}
			defer ws.Close()
			for {
				op, data, err := ws.ReadMessage()
				if err != nil {
					return
				}
				if op == wsOpText {
					_ = ws.WriteText([]byte(fmt.Sprintf("%s|host=%s|origin=%s", string(data), r.Host, r.Header.Get("Origin"))))
				} else {
					_ = ws.WriteBinary(data)
				}
			}
		}
		switch r.URL.Path {
		case "/hello":
			w.Header().Add("Set-Cookie", "a=1; Path=/")
			w.Header().Add("Set-Cookie", "b=2; Path=/")
			_ = json.NewEncoder(w).Encode(map[string]string{
				"host":    r.Host,
				"fhost":   r.Header.Get("X-Forwarded-Host"),
				"fproto":  r.Header.Get("X-Forwarded-Proto"),
				"forigin": r.Header.Get("X-Forwarded-Origin"),
				"origin":  r.Header.Get("Origin"),
				"ffor":    r.Header.Get("X-Forwarded-For"),
				"query":   r.URL.RawQuery,
			})
		case "/large":
			body, err := io.ReadAll(r.Body)
			if err != nil {
				http.Error(w, err.Error(), http.StatusBadRequest)
				return
			}
			w.WriteHeader(http.StatusOK)
			_, _ = w.Write(body)
		case "/sse":
			w.Header().Set("Content-Type", "text/event-stream")
			fl, _ := w.(http.Flusher)
			_, _ = io.WriteString(w, "data: first\n\n")
			fl.Flush()
			<-sseRelease
			_, _ = io.WriteString(w, "data: second\n\n")
			fl.Flush()
		default:
			w.WriteHeader(http.StatusNoContent)
		}
	}))
	defer upstream.Close()
	localPort := upstream.Listener.Addr().(*net.TCPAddr).Port

	secondUpstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = io.WriteString(w, "second-upstream-ok")
	}))
	defer secondUpstream.Close()
	secondPort := secondUpstream.Listener.Addr().(*net.TCPAddr).Port

	m := testManager(t)
	ctx := context.Background()
	secret, err := m.ensureTunnelSecret()
	if err != nil {
		t.Fatal(err)
	}

	gwApp := newMockGatewayServer(t, secret)
	gwAdmin := newMockGatewayServer(t, secret)
	gwConflict := newMockGatewayServer(t, secret)
	gwConflict.claimedOwner = "other-mac"

	pool := x509.NewCertPool()
	pool.AddCert(gwApp.server.Certificate())
	pool.AddCert(gwAdmin.server.Certificate())
	pool.AddCert(gwConflict.server.Certificate())
	tlsCfg := &tls.Config{RootCAs: pool, MinVersion: tls.VersionTLS12}

	m.tunnelTLSConfig = tlsCfg
	m.tunnelPingInterval = 250 * time.Millisecond
	m.tunnelReadTimeout = 2 * time.Second

	if err := m.SaveCloudflareSettings(ctx, "", "", "mac-test"); err != nil {
		t.Fatal(err)
	}
	if err := m.ReplaceProxyRules(ctx, 0, 8443, []ProxyRule{
		{Domain: "app.test", LocalPort: localPort, Tunnel: true, TunnelURL: "https://devbase-app.demo.workers.dev"},
	}); err != nil {
		t.Fatal(err)
	}
	if err := writeJSON(m.tunnelConfigPath(), tunnelPublishedConfig{
		SchemaVersion: 1,
		ClientID:      "mac-test",
		Routes: []TunnelRoute{
			{WorkerName: "devbase-app", PublicURL: "https://devbase-app.demo.workers.dev", ConnectURL: gwApp.server.URL, LocalDomain: "app.test", LocalPort: localPort},
		},
	}); err != nil {
		t.Fatal(err)
	}

	tunnelCtx, tunnelCancel := context.WithCancel(context.Background())
	tunnelDone := make(chan error, 1)
	go func() { tunnelDone <- m.ServeTunnel(tunnelCtx) }()
	defer func() {
		tunnelCancel()
		select {
		case err := <-tunnelDone:
			if err != nil {
				t.Errorf("ServeTunnel returned error: %v", err)
			}
		case <-time.After(3 * time.Second):
			t.Error("ServeTunnel did not exit")
		}
	}()

	waitForTunnel := func(cond func(TunnelStatusSnapshot) bool) TunnelStatusSnapshot {
		t.Helper()
		deadline := time.Now().Add(5 * time.Second)
		for time.Now().Before(deadline) {
			if st, ok := m.readTunnelStatus(); ok && cond(st) {
				return st
			}
			time.Sleep(25 * time.Millisecond)
		}
		st, _ := m.readTunnelStatus()
		t.Fatalf("timed out waiting for tunnel status, last=%+v", st)
		return TunnelStatusSnapshot{}
	}

	waitForTunnel(func(st TunnelStatusSnapshot) bool {
		return st.Phase == "Connected" && st.Connected == 1 && st.Total == 1 && len(st.Conflicts) == 0
	})

	visitorClient := gwApp.server.Client()

	// 1. Standard HTTP request + multi-value headers + Origin/Host rewriting.
	req, _ := http.NewRequest("GET", gwApp.server.URL+"/hello?x=1&y=2", nil)
	req.Host = "devbase-app.demo.workers.dev"
	req.Header.Set("Origin", "https://devbase-app.demo.workers.dev")
	resp, err := visitorClient.Do(req)
	if err != nil {
		t.Fatal(err)
	}
	var echo map[string]string
	_ = json.NewDecoder(resp.Body).Decode(&echo)
	resp.Body.Close()
	if resp.StatusCode != 200 || len(resp.Cookies()) != 2 {
		t.Fatalf("unexpected response: status=%d cookies=%v", resp.StatusCode, resp.Cookies())
	}
	wantLocalHost := fmt.Sprintf("localhost:%d", localPort)
	if echo["host"] != wantLocalHost || echo["fhost"] != "devbase-app.demo.workers.dev" || echo["fproto"] != "https" || echo["origin"] != "http://"+wantLocalHost || echo["forigin"] != "https://devbase-app.demo.workers.dev" || echo["query"] != "x=1&y=2" {
		t.Fatalf("header forwarding mismatch: %+v", echo)
	}

	// 2. Large (>1 MiB) streaming POST & response body.
	big := bytes.Repeat([]byte("DevbaseTunnelPayload"), (1500<<10)/20) // ~1.5 MiB
	reqBig, _ := http.NewRequest("POST", gwApp.server.URL+"/large", bytes.NewReader(big))
	reqBig.Host = "devbase-app.demo.workers.dev"
	respBig, err := visitorClient.Do(reqBig)
	if err != nil {
		t.Fatal(err)
	}
	gotBig, err := io.ReadAll(respBig.Body)
	respBig.Body.Close()
	if err != nil || !bytes.Equal(gotBig, big) {
		t.Fatalf("1.5 MiB body mismatch: got=%d want=%d err=%v", len(gotBig), len(big), err)
	}

	// 3. SSE streaming flush before upstream completes.
	reqSSE, _ := http.NewRequest("GET", gwApp.server.URL+"/sse", nil)
	reqSSE.Host = "devbase-app.demo.workers.dev"
	respSSE, err := visitorClient.Do(reqSSE)
	if err != nil {
		t.Fatal(err)
	}
	sseReader := bufio.NewReader(respSSE.Body)
	line1, err := sseReader.ReadString('\n')
	if err != nil || strings.TrimSpace(line1) != "data: first" {
		respSSE.Body.Close()
		t.Fatalf("SSE did not flush first event immediately: %q err=%v", line1, err)
	}
	close(sseRelease)
	restSSE, _ := io.ReadAll(sseReader)
	respSSE.Body.Close()
	if !strings.Contains(string(restSSE), "data: second") {
		t.Fatalf("SSE missed second event: %q", string(restSSE))
	}

	// 4. Proxied WebSocket with vite-hmr subprotocol and >256 KiB binary frame.
	wsURL := "wss://" + strings.TrimPrefix(gwApp.server.URL, "https://") + "/hmr"
	wsHeaders := http.Header{
		"Host":   []string{"devbase-app.demo.workers.dev"},
		"Origin": []string{"https://devbase-app.demo.workers.dev"},
	}
	vws, wsResp, err := dialWebSocket(ctx, wsURL, wsHeaders, []string{"vite-hmr"}, tlsCfg)
	if err != nil {
		t.Fatal(err)
	}
	if wsResp.Header.Get("Sec-WebSocket-Protocol") != "vite-hmr" {
		t.Fatalf("missing negotiated subprotocol: %q", wsResp.Header.Get("Sec-WebSocket-Protocol"))
	}
	if err := vws.WriteText([]byte("hmr-ping")); err != nil {
		t.Fatal(err)
	}
	op, wsData, err := vws.ReadMessage()
	if err != nil || op != wsOpText || string(wsData) != fmt.Sprintf("hmr-ping|host=%s|origin=http://%s", wantLocalHost, wantLocalHost) {
		t.Fatalf("WS text mismatch: op=%d data=%q err=%v", op, wsData, err)
	}
	bigBin := bytes.Repeat([]byte{0xAB, 0xCD, 0xEF, 0x01}, 80*1024) // 320 KiB > 256 KiB chunk
	if err := vws.WriteBinary(bigBin); err != nil {
		t.Fatal(err)
	}
	op, gotBin, err := vws.ReadMessage()
	if err != nil || op != wsOpBinary || !bytes.Equal(gotBin, bigBin) {
		t.Fatalf("WS 320 KiB binary mismatch: op=%d len=%d err=%v", op, len(gotBin), err)
	}
	_ = vws.WriteClose(1000, "done")
	_ = vws.Close()

	// 5. Hot-add workers via proxy/tunnel.json without dropping existing WS, and report conflicts.
	initialConns := gwApp.conns.Load()
	if err := writeJSON(m.tunnelConfigPath(), tunnelPublishedConfig{
		SchemaVersion: 1,
		ClientID:      "mac-test",
		Routes: []TunnelRoute{
			{WorkerName: "devbase-app", PublicURL: "https://devbase-app.demo.workers.dev", ConnectURL: gwApp.server.URL, LocalDomain: "app.test", LocalPort: localPort},
			{WorkerName: "devbase-admin", PublicURL: "https://admin.example.com", ConnectURL: gwAdmin.server.URL, LocalDomain: "admin.test", LocalPort: secondPort},
			{WorkerName: "devbase-conflict", PublicURL: "https://conflict.example.com", ConnectURL: gwConflict.server.URL, LocalDomain: "conflict.test", LocalPort: secondPort},
		},
	}); err != nil {
		t.Fatal(err)
	}
	st := waitForTunnel(func(st TunnelStatusSnapshot) bool {
		return st.Phase == "Connected" && st.Connected == 2 && st.Total == 3 && len(st.Conflicts) == 1
	})
	if st.Conflicts[0].Host != "conflict.example.com" || st.Conflicts[0].Owner != "other-mac" {
		t.Fatalf("unexpected conflict report: %+v", st.Conflicts)
	}
	if detail := m.formatTunnelDetail(); detail != "Conflict: 1 worker" {
		t.Fatalf("unexpected formatTunnelDetail: %q", detail)
	}
	if gwApp.conns.Load() != initialConns {
		t.Fatalf("adding new worker dropped existing WS: before=%d after=%d", initialConns, gwApp.conns.Load())
	}
	reqAdmin, _ := http.NewRequest("GET", gwAdmin.server.URL+"/", nil)
	reqAdmin.Host = "admin.example.com"
	respAdmin, err := gwAdmin.server.Client().Do(reqAdmin)
	if err != nil {
		t.Fatal(err)
	}
	adminBody, _ := io.ReadAll(respAdmin.Body)
	respAdmin.Body.Close()
	if respAdmin.StatusCode != 200 || string(adminBody) != "second-upstream-ok" {
		t.Fatalf("hot-added worker failed: %d %q", respAdmin.StatusCode, string(adminBody))
	}

	// 6. Auto-reconnect after gateway drops one worker's connection.
	gwApp.dropClient("mac-test")
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) && gwApp.conns.Load() == initialConns {
		time.Sleep(25 * time.Millisecond)
	}
	if gwApp.conns.Load() <= initialConns {
		t.Fatal("ServeTunnel did not reconnect after connection drop")
	}
	waitForTunnel(func(st TunnelStatusSnapshot) bool {
		return st.Phase == "Connected" && st.Connected == 2 && st.Total == 3
	})

	// 7. Removing a worker from tunnel.json closes its WS cleanly while keeping others connected.
	appConnsBeforeRemove := gwApp.conns.Load()
	if err := writeJSON(m.tunnelConfigPath(), tunnelPublishedConfig{
		SchemaVersion: 1,
		ClientID:      "mac-test",
		Routes: []TunnelRoute{
			{WorkerName: "devbase-app", PublicURL: "https://devbase-app.demo.workers.dev", ConnectURL: gwApp.server.URL, LocalDomain: "app.test", LocalPort: localPort},
		},
	}); err != nil {
		t.Fatal(err)
	}
	waitForTunnel(func(st TunnelStatusSnapshot) bool {
		return st.Phase == "Connected" && st.Connected == 1 && st.Total == 1
	})
	if gwApp.conns.Load() != appConnsBeforeRemove {
		t.Fatalf("removing devbase-admin reconnected devbase-app: before=%d after=%d", appConnsBeforeRemove, gwApp.conns.Load())
	}
}

func TestTunnelWorkerScriptSyntax(t *testing.T) {
	if !strings.Contains(cloudflareWorkerIndexMJS, "TunnelGateway") || strings.Contains(cloudflareWorkerIndexMJS, "this.routes") {
		t.Fatal("worker script should have TunnelGateway without legacy multi-route map")
	}
	scriptPath := filepath.Join(t.TempDir(), "index.mjs")
	if err := os.WriteFile(scriptPath, []byte(cloudflareWorkerIndexMJS), 0600); err != nil {
		t.Fatal(err)
	}
	if nodeBin, lookErr := exec.LookPath("node"); lookErr == nil {
		out, err := exec.Command(nodeBin, "--check", scriptPath).CombinedOutput()
		if err != nil {
			t.Fatalf("node --check failed: %v\n%s", err, string(out))
		}
	}
}

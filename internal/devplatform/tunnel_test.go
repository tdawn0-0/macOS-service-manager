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
	if got := DefaultTunnelWorkerName("api.app.devbase.localhost"); got != "devbase-api-app" {
		t.Fatalf("DefaultTunnelWorkerName = %q", got)
	}
	longWorker := DefaultTunnelWorkerName(strings.Repeat("ab.", 25) + "test")
	if len(longWorker) > 63 || strings.HasSuffix(longWorker, "-") || !strings.HasPrefix(longWorker, "devbase-") {
		t.Fatalf("invalid truncated worker name %q (len=%d)", longWorker, len(longWorker))
	}
	if got := (ProxyRule{Domain: "api.devbase.localhost", Tunnel: true}).PreviewTunnelURL("demo"); got != "https://devbase-api.demo.workers.dev" {
		t.Fatalf("workers.dev preview URL = %q", got)
	}
	if got := (ProxyRule{Domain: "api.devbase.localhost", Tunnel: true, TunnelDomain: "api.example.com"}).PreviewTunnelURL("demo"); got != "https://api.example.com" {
		t.Fatalf("custom domain preview URL = %q", got)
	}

	if err := m.ReplaceProxyRules(ctx, 0, 8443, []ProxyRule{
		{Domain: "a.b.devbase.localhost", LocalPort: 3000, Tunnel: true},
		{Domain: "a-b.devbase.localhost", LocalPort: 3001, Tunnel: true},
	}); err == nil || !strings.Contains(err.Error(), "duplicate tunnel worker") {
		t.Fatalf("accepted duplicate resolved worker name: %v", err)
	}
	if err := m.ReplaceProxyRules(ctx, 0, 8443, []ProxyRule{
		{Domain: "app.devbase.localhost", LocalPort: 3000, Tunnel: true, TunnelWorker: "other-worker"},
	}); err == nil || !strings.Contains(err.Error(), "devbase-") {
		t.Fatalf("accepted non-devbase worker name: %v", err)
	}
	for _, badDomain := range []string{"app.devbase.localhost", "devbase-app.demo.workers.dev", "singlelabel", "bad/path.example.com"} {
		if err := m.ReplaceProxyRules(ctx, 0, 8443, []ProxyRule{
			{Domain: "app.devbase.localhost", LocalPort: 3000, Tunnel: true, TunnelDomain: badDomain},
		}); err == nil {
			t.Fatalf("accepted invalid custom domain %q", badDomain)
		}
	}
	if err := m.ReplaceProxyRules(ctx, 0, 8443, []ProxyRule{
		{Domain: "a.devbase.localhost", LocalPort: 3000, Tunnel: true, TunnelDomain: "same.example.com"},
		{Domain: "b.devbase.localhost", LocalPort: 3001, Tunnel: true, TunnelDomain: "same.example.com"},
	}); err == nil || !strings.Contains(err.Error(), "duplicate tunnel custom domain") {
		t.Fatalf("accepted duplicate custom domain: %v", err)
	}
	if err := m.ReplaceProxyRules(ctx, 0, 8443, []ProxyRule{
		{Domain: "a.b.devbase.localhost", LocalPort: 3000, Tunnel: true, TunnelWorker: "devbase-api", TunnelDomain: "api.example.com"},
		{Domain: "a-b.devbase.localhost", LocalPort: 3001, Tunnel: true},
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
	t             *testing.T
	secret        string
	server        *httptest.Server
	mu            sync.Mutex
	agentWS       *wsConn
	ownerID       string
	claimedOwner  string // externally owned worker for conflict testing
	httpWait      map[uint32]*mockHTTPWaiter
	wsWait        map[uint32]chan tunnelWSOpenAckResult
	visitors      map[uint32]*wsConn
	recordFrames  bool
	agentFrames   []tunnelFrame
	nextID        atomic.Uint32
	conns         atomic.Int32
	protocolPings atomic.Int32
	textPings     atomic.Int32
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
	ws.onPing = func() {
		g.protocolPings.Add(1)
	}
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
				g.textPings.Add(1)
				_ = ws.WriteText([]byte(tunnelControlPong))
			}
			continue
		}
		frame, err := decodeTunnelFrame(data)
		if err != nil {
			return
		}
		if g.recordFrames {
			cp := frame
			cp.Payload = append([]byte(nil), frame.Payload...)
			g.mu.Lock()
			g.agentFrames = append(g.agentFrames, cp)
			g.mu.Unlock()
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
		{Domain: "api.devbase.localhost", LocalPort: 3000, Tunnel: true},
		{Domain: "web.devbase.localhost", LocalPort: 4000, Tunnel: true, TunnelDomain: "web.example.com"},
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
		{Domain: "api.devbase.localhost", LocalPort: 3000, Tunnel: true},
		{Domain: "web.devbase.localhost", LocalPort: 4000, Tunnel: true, TunnelDomain: "bad.unowned.org"},
	}); err != nil {
		t.Fatal(err)
	}
	if err := m.SyncTunnels(ctx); err == nil || !strings.Contains(err.Error(), "active Cloudflare zone") {
		t.Fatalf("expected unowned zone rejection, got %v", err)
	}

	// Disabling Tunnel on web.devbase.localhost deletes devbase-web cleanly and leaves devbase-api + unrelated existing-blog untouched.
	s, _ = m.Read()
	if err := m.ReplaceProxyRules(ctx, s.ProxyRulesRevision, 8443, []ProxyRule{
		{Domain: "api.devbase.localhost", LocalPort: 3000, Tunnel: true},
		{Domain: "web.devbase.localhost", LocalPort: 4000, Tunnel: false},
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
		{Domain: "app.devbase.localhost", LocalPort: localPort, Tunnel: true, TunnelURL: "https://devbase-app.demo.workers.dev"},
	}); err != nil {
		t.Fatal(err)
	}
	if err := writeJSON(m.tunnelConfigPath(), tunnelPublishedConfig{
		SchemaVersion: 1,
		ClientID:      "mac-test",
		Routes: []TunnelRoute{
			{WorkerName: "devbase-app", PublicURL: "https://devbase-app.demo.workers.dev", ConnectURL: gwApp.server.URL, LocalDomain: "app.devbase.localhost", LocalPort: localPort},
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
			{WorkerName: "devbase-app", PublicURL: "https://devbase-app.demo.workers.dev", ConnectURL: gwApp.server.URL, LocalDomain: "app.devbase.localhost", LocalPort: localPort},
			{WorkerName: "devbase-admin", PublicURL: "https://admin.example.com", ConnectURL: gwAdmin.server.URL, LocalDomain: "admin.devbase.localhost", LocalPort: secondPort},
			{WorkerName: "devbase-conflict", PublicURL: "https://conflict.example.com", ConnectURL: gwConflict.server.URL, LocalDomain: "conflict.devbase.localhost", LocalPort: secondPort},
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
			{WorkerName: "devbase-app", PublicURL: "https://devbase-app.demo.workers.dev", ConnectURL: gwApp.server.URL, LocalDomain: "app.devbase.localhost", LocalPort: localPort},
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

	// 8. Verify keep-alive uses zero-cost RFC 6455 protocol pings, not application text pings.
	pingDeadline := time.Now().Add(2 * time.Second)
	for time.Now().Before(pingDeadline) && gwApp.protocolPings.Load() == 0 {
		time.Sleep(25 * time.Millisecond)
	}
	if gwApp.protocolPings.Load() == 0 || gwApp.textPings.Load() != 0 {
		t.Fatalf("unexpected keep-alive pings: protocol=%d text=%d", gwApp.protocolPings.Load(), gwApp.textPings.Load())
	}
}

func TestTunnelHTTPFrameCoalescingAndScriptDigestSync(t *testing.T) {
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/empty":
			w.Header().Set("Content-Length", "0")
			w.WriteHeader(http.StatusOK)
		case "/small":
			w.Header().Set("Content-Length", "11")
			w.WriteHeader(http.StatusOK)
			_, _ = io.WriteString(w, "hello-world")
		default:
			w.WriteHeader(http.StatusNotFound)
		}
	}))
	defer upstream.Close()
	localPort := upstream.Listener.Addr().(*net.TCPAddr).Port

	m := testManager(t)
	ctx := context.Background()
	secret, err := m.ensureTunnelSecret()
	if err != nil {
		t.Fatal(err)
	}
	gw := newMockGatewayServer(t, secret)
	gw.recordFrames = true
	pool := x509.NewCertPool()
	pool.AddCert(gw.server.Certificate())
	m.tunnelTLSConfig = &tls.Config{RootCAs: pool, MinVersion: tls.VersionTLS12}

	if err := m.SaveCloudflareSettings(ctx, "", "", "mac-frame"); err != nil {
		t.Fatal(err)
	}
	if err := m.ReplaceProxyRules(ctx, 0, 8443, []ProxyRule{
		{Domain: "app.devbase.localhost", LocalPort: localPort, Tunnel: true, TunnelURL: "https://devbase-app.demo.workers.dev"},
	}); err != nil {
		t.Fatal(err)
	}
	if err := writeJSON(m.tunnelConfigPath(), tunnelPublishedConfig{
		SchemaVersion: 1,
		ClientID:      "mac-frame",
		Routes: []TunnelRoute{
			{WorkerName: "devbase-app", PublicURL: "https://devbase-app.demo.workers.dev", ConnectURL: gw.server.URL, LocalDomain: "app.devbase.localhost", LocalPort: localPort},
		},
	}); err != nil {
		t.Fatal(err)
	}

	tunnelCtx, tunnelCancel := context.WithCancel(context.Background())
	tunnelDone := make(chan error, 1)
	go func() { tunnelDone <- m.ServeTunnel(tunnelCtx) }()
	defer func() {
		tunnelCancel()
		<-tunnelDone
	}()

	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		if st, ok := m.readTunnelStatus(); ok && st.Phase == "Connected" && st.Connected == 1 {
			break
		}
		time.Sleep(25 * time.Millisecond)
	}

	client := gw.server.Client()
	// Content-Length: 0 must emit only 1 binary frame (frameHTTPResHead | tunnelFlagFIN).
	reqEmpty, _ := http.NewRequest("GET", gw.server.URL+"/empty", nil)
	reqEmpty.Host = "devbase-app.demo.workers.dev"
	respEmpty, err := client.Do(reqEmpty)
	if err != nil {
		t.Fatal(err)
	}
	_, _ = io.ReadAll(respEmpty.Body)
	respEmpty.Body.Close()

	// Fixed Content-Length > 0 small response must emit exactly 2 binary frames (HEAD without FIN + DATA with FIN).
	reqSmall, _ := http.NewRequest("GET", gw.server.URL+"/small", nil)
	reqSmall.Host = "devbase-app.demo.workers.dev"
	respSmall, err := client.Do(reqSmall)
	if err != nil {
		t.Fatal(err)
	}
	smallBody, _ := io.ReadAll(respSmall.Body)
	respSmall.Body.Close()
	if string(smallBody) != "hello-world" {
		t.Fatalf("unexpected small body: %q", string(smallBody))
	}

	gw.mu.Lock()
	frames := append([]tunnelFrame(nil), gw.agentFrames...)
	gw.mu.Unlock()

	byStream := map[uint32][]tunnelFrame{}
	for _, f := range frames {
		if f.StreamID != 0 {
			byStream[f.StreamID] = append(byStream[f.StreamID], f)
		}
	}
	if len(byStream) != 2 {
		t.Fatalf("expected 2 streams, got %d (%+v)", len(byStream), byStream)
	}
	s1 := byStream[1]
	if len(s1) != 1 || s1[0].Type != frameHTTPResHead || s1[0].Flags&tunnelFlagFIN == 0 {
		t.Fatalf("expected 1 HEAD|FIN frame for empty response, got %+v", s1)
	}
	s2 := byStream[2]
	if len(s2) != 2 || s2[0].Type != frameHTTPResHead || s2[0].Flags&tunnelFlagFIN != 0 ||
		s2[1].Type != frameHTTPResData || s2[1].Flags&tunnelFlagFIN == 0 || string(s2[1].Payload) != "hello-world" {
		t.Fatalf("expected 2 frames (HEAD + DATA|FIN) for small response, got %+v", s2)
	}

	// Verify scriptDigest validation, needsTunnelSync, and compatibility_date upload.
	var compatDates []string
	wantToken := "cf-digest-token-0123456789"
	cfSrv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		switch {
		case r.Method == http.MethodGet && r.URL.Path == "/accounts":
			_ = json.NewEncoder(w).Encode(map[string]any{"success": true, "result": []map[string]string{{"id": "acc-dig-123", "name": "Dev"}}})
		case r.Method == http.MethodGet && r.URL.Path == "/accounts/acc-dig-123/workers/subdomain":
			_ = json.NewEncoder(w).Encode(map[string]any{"success": true, "result": map[string]string{"subdomain": "dig-dev"}})
		case r.Method == http.MethodGet && r.URL.Path == "/zones":
			_ = json.NewEncoder(w).Encode(map[string]any{"success": true, "result": []map[string]string{}})
		case r.Method == http.MethodGet && strings.HasPrefix(r.URL.Path, "/accounts/acc-dig-123/workers/services/"):
			_ = json.NewEncoder(w).Encode(map[string]any{"success": true, "result": map[string]any{"default_environment": map[string]any{"script": map[string]string{"migration_tag": "v1"}}}})
		case r.Method == http.MethodPut && strings.HasPrefix(r.URL.Path, "/accounts/acc-dig-123/workers/scripts/"):
			_, params, _ := mime.ParseMediaType(r.Header.Get("Content-Type"))
			mr := multipart.NewReader(r.Body, params["boundary"])
			for {
				p, err := mr.NextPart()
				if err != nil {
					break
				}
				if p.FormName() == "metadata" {
					var meta struct {
						CompatibilityDate string `json:"compatibility_date"`
					}
					_ = json.NewDecoder(p).Decode(&meta)
					compatDates = append(compatDates, meta.CompatibilityDate)
				}
			}
			_ = json.NewEncoder(w).Encode(map[string]any{"success": true, "result": map[string]any{"id": "devbase-app"}})
		case r.Method == http.MethodPost && strings.HasSuffix(r.URL.Path, "/subdomain"):
			_ = json.NewEncoder(w).Encode(map[string]any{"success": true, "result": map[string]any{"enabled": true}})
		default:
			w.WriteHeader(http.StatusNotFound)
		}
	}))
	defer cfSrv.Close()

	m2 := testManager(t)
	m2.CloudflareAPIBase = cfSrv.URL
	if err := m2.SaveCloudflareSettings(ctx, wantToken, "", "mac-dig"); err != nil {
		t.Fatal(err)
	}
	if err := m2.ReplaceProxyRules(ctx, 0, 8443, []ProxyRule{
		{Domain: "app.devbase.localhost", LocalPort: localPort, Tunnel: true, TunnelURL: "https://devbase-app.dig-dev.workers.dev"},
	}); err != nil {
		t.Fatal(err)
	}
	// Empty or stale ScriptDigest must trigger needsTunnelSync.
	legacyState, err := m2.Read()
	if err != nil {
		t.Fatal(err)
	}
	legacyState.TunnelWorkers = map[string]TunnelWorkerState{
		"devbase-app": {
			ScriptName:  "devbase-app",
			PublicURL:   "https://devbase-app.dig-dev.workers.dev",
			LocalDomain: "app.devbase.localhost",
			LocalPort:   localPort,
		},
	}
	if !m2.needsTunnelSync(legacyState) {
		t.Fatal("expected needsTunnelSync=true when ScriptDigest is empty")
	}
	staleState := legacyState
	staleState.TunnelWorkers = map[string]TunnelWorkerState{
		"devbase-app": {
			ScriptName:   "devbase-app",
			PublicURL:    "https://devbase-app.dig-dev.workers.dev",
			LocalDomain:  "app.devbase.localhost",
			LocalPort:    localPort,
			ScriptDigest: "0000000000000000",
		},
	}
	if !m2.needsTunnelSync(staleState) {
		t.Fatal("expected needsTunnelSync=true when ScriptDigest is stale")
	}
	if err := validateTunnelState(&legacyState); err != nil {
		t.Fatalf("expected legacy empty ScriptDigest to validate: %v", err)
	}
	badState := legacyState
	badState.TunnelWorkers = map[string]TunnelWorkerState{
		"devbase-app": {
			ScriptName:   "devbase-app",
			PublicURL:    "https://devbase-app.dig-dev.workers.dev",
			LocalDomain:  "app.devbase.localhost",
			LocalPort:    localPort,
			ScriptDigest: "not-hex!",
		},
	}
	if err := validateTunnelState(&badState); err == nil {
		t.Fatal("expected malformed ScriptDigest to fail validation")
	}

	if err := m2.SyncTunnels(ctx); err != nil {
		t.Fatal(err)
	}
	synced, err := m2.Read()
	if err != nil {
		t.Fatal(err)
	}
	if len(compatDates) != 1 || compatDates[0] != cloudflareWorkerCompatDate {
		t.Fatalf("expected compatibility_date %q, got %v", cloudflareWorkerCompatDate, compatDates)
	}
	if synced.TunnelWorkers["devbase-app"].ScriptDigest != currentTunnelWorkerDigest() {
		t.Fatalf("unexpected ScriptDigest: got=%q want=%q", synced.TunnelWorkers["devbase-app"].ScriptDigest, currentTunnelWorkerDigest())
	}
	if m2.needsTunnelSync(synced) {
		t.Fatal("expected needsTunnelSync=false after SyncTunnels")
	}
}

func TestTunnelWorkerHibernationLifecycle(t *testing.T) {
	nodeBin, lookErr := exec.LookPath("node")
	if lookErr != nil {
		t.Skip("node not installed")
	}
	dir := t.TempDir()
	cfMockJS := `const OrigResponse = globalThis.Response;

export class MockWebSocket {
  constructor() {
    this.readyState = 1;
    this.sent = [];
    this.closeCalls = [];
    this._attachment = null;
    this._peer = null;
  }
  send(data) {
    if (this.readyState !== 1) {
      throw new Error("WebSocket is not open");
    }
    this.sent.push(typeof data === "string" ? data : new Uint8Array(data));
  }
  close(code, reason) {
    if (code !== undefined) {
      const valid = Number.isInteger(code) &&
        ((code >= 1000 && code <= 1014 && code !== 1004 && code !== 1005 && code !== 1006) ||
         (code >= 3000 && code <= 4999));
      if (!valid) {
        throw new RangeError("Invalid close code: " + code);
      }
    }
    if (reason !== undefined) {
      const str = String(reason);
      if (Buffer.byteLength(str, "utf8") > 123) {
        throw new SyntaxError("Close reason exceeds 123 UTF-8 bytes");
      }
    }
    this.closeCalls.push({ code: code ?? 1005, reason: reason ?? "" });
    this.readyState = 3;
  }
  serializeAttachment(val) {
    this._attachment = val === undefined ? null : structuredClone(val);
  }
  deserializeAttachment() {
    return this._attachment === null ? null : structuredClone(this._attachment);
  }
}

globalThis.WebSocket = {
  CONNECTING: 0,
  OPEN: 1,
  CLOSING: 2,
  CLOSED: 3,
  READY_STATE_OPEN: 1,
};

globalThis.WebSocketPair = class WebSocketPair {
  constructor() {
    const a = new MockWebSocket();
    const b = new MockWebSocket();
    a._peer = b;
    b._peer = a;
    this[0] = a;
    this[1] = b;
  }
};

globalThis.WebSocketRequestResponsePair = class WebSocketRequestResponsePair {
  constructor(request, response) {
    this._req = request;
    this._res = response;
  }
  getRequest() { return this._req; }
  getResponse() { return this._res; }
};

globalThis.Response = class MockResponse extends OrigResponse {
  constructor(body, init) {
    if (init && init.status === 101 && init.webSocket) {
      super(null, { status: 200, headers: init.headers });
      this._status101 = true;
      this._webSocket = init.webSocket;
      return;
    }
    super(body, init);
  }
  get status() {
    return this._status101 ? 101 : super.status;
  }
  get webSocket() {
    return this._webSocket ?? null;
  }
};

export class DurableObject {
  constructor(ctx, env) {
    this.ctx = ctx;
    this.env = env;
  }
}
`
	transformedWorker := strings.Replace(cloudflareWorkerIndexMJS, `import { DurableObject } from "cloudflare:workers";`, `import { DurableObject } from "./cf_mock.mjs";`, 1)
	if transformedWorker == cloudflareWorkerIndexMJS {
		t.Fatal("failed to rewrite cloudflare:workers import")
	}

	testScript := `import assert from "node:assert/strict";
import "./cf_mock.mjs";

const mod = await import("./index.mjs");
const worker = mod.default;
const { TunnelGateway } = mod;

const enc = new TextEncoder();
const dec = new TextDecoder();

function encodeFrame(type, flags, streamId, code, payload) {
  const body = payload instanceof Uint8Array
    ? payload
    : typeof payload === "string"
      ? enc.encode(payload)
      : payload !== undefined
        ? enc.encode(JSON.stringify(payload))
        : new Uint8Array(0);
  const out = new Uint8Array(9 + body.byteLength);
  const view = new DataView(out.buffer);
  out[0] = 1;
  out[1] = type;
  out[2] = flags;
  view.setUint32(3, streamId, false);
  view.setUint16(7, code, false);
  if (body.byteLength > 0) out.set(body, 9);
  return out;
}

function decodeFrame(raw) {
  const bytes = raw instanceof Uint8Array ? raw : new Uint8Array(raw);
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  return {
    type: bytes[1],
    flags: bytes[2],
    streamId: view.getUint32(3, false),
    code: view.getUint16(7, false),
    payload: bytes.subarray(9),
  };
}

class PersistedDOContext {
  constructor() {
    this.sockets = [];
    this.autoResponse = null;
    this.autoResponseCalls = 0;
  }
  acceptWebSocket(ws, tags = []) {
    ws.readyState = 1;
    this.sockets.push({ ws, tags: [...tags] });
  }
  getWebSockets(tag) {
    return this.sockets
      .filter((e) => e.ws.readyState !== 3 && (!tag || e.tags.includes(tag)))
      .map((e) => e.ws);
  }
  getTags(ws) {
    const found = this.sockets.find((e) => e.ws === ws);
    return found ? [...found.tags] : [];
  }
  setWebSocketAutoResponse(pair) {
    this.autoResponseCalls++;
    this.autoResponse = pair ?? null;
  }
  getWebSocketAutoResponse() {
    return this.autoResponse;
  }
}

async function makeSignedConnectReq(secret, clientId) {
  const ts = String(Math.floor(Date.now() / 1000));
  const nonce = "nonce-" + Math.random().toString(16).slice(2);
  const key = await crypto.subtle.importKey(
    "raw",
    enc.encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const mac = await crypto.subtle.sign("HMAC", key, enc.encode("v1\n" + clientId + "\n" + ts + "\n" + nonce));
  const sig = Array.from(new Uint8Array(mac)).map((b) => b.toString(16).padStart(2, "0")).join("");
  return new Request("https://devbase-app.demo.workers.dev/__devbase/connect", {
    method: "GET",
    headers: {
      Upgrade: "websocket",
      "X-Devbase-Client": clientId,
      "X-Devbase-Timestamp": ts,
      "X-Devbase-Nonce": nonce,
      "X-Devbase-Signature": sig,
    },
  });
}

const secret = "super-secret-tunnel-key-0123456789";
const persistedCtx = new PersistedDOContext();
let currentGw = null;
let stubGetCalls = 0;
const env = {
  TUNNEL_SECRET: secret,
  TUNNEL_GATEWAY: {
    idFromName: (n) => n,
    get: () => {
      stubGetCalls++;
      return { fetch: (req) => currentGw.fetch(req) };
    },
  },
};

currentGw = new TunnelGateway(persistedCtx, env);
assert.equal(persistedCtx.autoResponseCalls, 1);

// 1. Outer Worker early filtering must reject /__devbase/* probes without hitting DO.
const r404 = await worker.fetch(new Request("https://devbase-app.demo.workers.dev/__devbase/unknown"), env);
assert.equal(r404.status, 404);
const r405 = await worker.fetch(new Request("https://devbase-app.demo.workers.dev/__devbase/connect", { method: "POST" }), env);
assert.equal(r405.status, 405);
const r426 = await worker.fetch(new Request("https://devbase-app.demo.workers.dev/__devbase/connect", { method: "GET" }), env);
assert.equal(r426.status, 426);
assert.equal(stubGetCalls, 0, "early filtering should never wake DO");

// 2. Connect and register agent, open visitor 1 WebSocket (streamId === 1).
const connRes1 = await worker.fetch(await makeSignedConnectReq(secret, "mac-1"), env);
assert.equal(connRes1.status, 101);
const agent1 = persistedCtx.getWebSockets("agent")[0];
await currentGw.webSocketMessage(agent1, encodeFrame(0x01, 0x01, 0, 0, { clientId: "mac-1" }));
assert.equal(agent1.sent.length, 1);
const regAck = JSON.parse(dec.decode(decodeFrame(agent1.sent.pop()).payload));
assert.equal(regAck.ok, true);

const v1Req = new Request("https://devbase-app.demo.workers.dev/hmr", {
  headers: { Upgrade: "websocket", "Sec-WebSocket-Protocol": "vite-hmr" },
});
const v1Promise = currentGw.fetch(v1Req);
await new Promise((r) => setTimeout(r, 10));
const v1OpenFrame = decodeFrame(agent1.sent.pop());
assert.equal(v1OpenFrame.type, 0x20);
assert.equal(v1OpenFrame.streamId, 1);
await currentGw.webSocketMessage(agent1, encodeFrame(0x21, 0x01, 1, 101, { protocol: "vite-hmr" }));
const v1Res = await v1Promise;
assert.equal(v1Res.status, 101);
const visitor1 = persistedCtx.getWebSockets("visitor")[0];

// 3. Simulate hibernation wakeup: create brand-new TunnelGateway over persistedCtx only.
currentGw = new TunnelGateway(persistedCtx, env);
assert.equal(persistedCtx.autoResponseCalls, 1, "wakeup must not re-register auto-response");
assert.equal(currentGw.agentWs, agent1);
assert.equal(currentGw.wsVisitors.get(1), visitor1);
assert.ok(currentGw.nextStreamId >= 2);

// 4. After wakeup, open visitor 2 and HTTP request; verify streamIds do NOT collide with 1.
const v2Promise = currentGw.fetch(new Request("https://devbase-app.demo.workers.dev/ws2", {
  headers: { Upgrade: "websocket" },
}));
await new Promise((r) => setTimeout(r, 10));
const v2OpenFrame = decodeFrame(agent1.sent.pop());
assert.equal(v2OpenFrame.type, 0x20);
assert.ok(v2OpenFrame.streamId >= 2, "post-hibernation streamId must not collide with 1");
const v2StreamId = v2OpenFrame.streamId;
await currentGw.webSocketMessage(agent1, encodeFrame(0x21, 0x01, v2StreamId, 101, {}));
const v2Res = await v2Promise;
assert.equal(v2Res.status, 101);
const visitor2 = currentGw.wsVisitors.get(v2StreamId);
assert.ok(visitor2 && visitor2 !== visitor1);

// Verify routing still works independently for both visitor 1 and visitor 2.
await currentGw.webSocketMessage(visitor1, "ping-v1");
assert.equal(decodeFrame(agent1.sent.pop()).streamId, 1);
await currentGw.webSocketMessage(agent1, encodeFrame(0x22, 0x03, 1, 0, "reply-v1"));
assert.equal(visitor1.sent.pop(), "reply-v1");

await currentGw.webSocketMessage(visitor2, "ping-v2");
assert.equal(decodeFrame(agent1.sent.pop()).streamId, v2StreamId);
await currentGw.webSocketMessage(agent1, encodeFrame(0x22, 0x03, v2StreamId, 0, "reply-v2"));
assert.equal(visitor2.sent.pop(), "reply-v2");

const httpPromise = currentGw.fetch(new Request("https://devbase-app.demo.workers.dev/ok"));
await new Promise((r) => setTimeout(r, 10));
const httpHeadFrame = decodeFrame(agent1.sent.pop());
assert.equal(httpHeadFrame.type, 0x10);
assert.ok(httpHeadFrame.streamId > v2StreamId);
await currentGw.webSocketMessage(agent1, encodeFrame(0x12, 0x01, httpHeadFrame.streamId, 200, { status: 200, headers: [] }));
const httpRes = await httpPromise;
assert.equal(httpRes.status, 200);

// 5. Streaming HTTP response body.cancel() must send FRAME_STREAM_RESET and clean httpStreams.
const ssePromise = currentGw.fetch(new Request("https://devbase-app.demo.workers.dev/sse"));
await new Promise((r) => setTimeout(r, 10));
const sseStreamId = decodeFrame(agent1.sent.pop()).streamId;
await currentGw.webSocketMessage(agent1, encodeFrame(0x12, 0x00, sseStreamId, 200, { status: 200, headers: [["Content-Type", "text/event-stream"]] }));
const sseRes = await ssePromise;
await currentGw.webSocketMessage(agent1, encodeFrame(0x13, 0x00, sseStreamId, 0, "data: 1\n\n"));
const reader = sseRes.body.getReader();
const chunk1 = await reader.read();
assert.equal(dec.decode(chunk1.value), "data: 1\n\n");
await reader.cancel("tab closed");
const resetFrame = decodeFrame(agent1.sent.pop());
assert.equal(resetFrame.type, 0x14);
assert.equal(resetFrame.streamId, sseStreamId);
assert.equal(currentGw.httpStreams.size, 0);

// 6. handleVisitorWebSocket abort and synchronous agentWs.send throw must not leak wsPending.
const ac = new AbortController();
const abortWsPromise = currentGw.fetch(new Request("https://devbase-app.demo.workers.dev/abort", {
  headers: { Upgrade: "websocket" },
  signal: ac.signal,
}));
await new Promise((r) => setTimeout(r, 10));
const abortedStreamId = decodeFrame(agent1.sent.pop()).streamId;
ac.abort();
const abortWsRes = await abortWsPromise;
assert.equal(abortWsRes.status, 502);
const abortReset = decodeFrame(agent1.sent.pop());
assert.equal(abortReset.type, 0x14);
assert.equal(abortReset.streamId, abortedStreamId);
assert.equal(currentGw.wsPending.size, 0);

const origSend = agent1.send.bind(agent1);
agent1.send = () => { throw new Error("sync send error"); };
const throwWsRes = await currentGw.fetch(new Request("https://devbase-app.demo.workers.dev/throw", {
  headers: { Upgrade: "websocket" },
}));
assert.equal(throwWsRes.status, 502);
assert.equal(currentGw.wsPending.size, 0);
const throwHttpRes = await currentGw.fetch(new Request("https://devbase-app.demo.workers.dev/throw-http"));
assert.equal(throwHttpRes.status, 502);
assert.equal(currentGw.httpStreams.size, 0);
agent1.send = origSend;

// 7. webSocketClose with invalid close code 1005 and >123-byte multi-byte UTF-8 reason must sanitize and succeed.
const longEmojiReason = "🚀".repeat(50);
await currentGw.webSocketClose(visitor1, 1005, longEmojiReason);
assert.equal(visitor1.closeCalls.length, 1);
assert.equal(visitor1.closeCalls[0].code, 1000);
assert.ok(Buffer.byteLength(visitor1.closeCalls[0].reason, "utf8") <= 123);
assert.equal(visitor1.closeCalls[0].reason, "🚀".repeat(30));
const wsCloseFrame = decodeFrame(agent1.sent.pop());
assert.equal(wsCloseFrame.type, 0x23);
assert.equal(wsCloseFrame.streamId, 1);

// 8. Reconnect by same clientId supersedes old active agent (active=false + closed) while keeping conflicting mac-other open.
const conflictRes = await worker.fetch(await makeSignedConnectReq(secret, "mac-other"), env);
assert.equal(conflictRes.status, 101);
const conflictWs = persistedCtx.getWebSockets("agent").find((s) => s !== agent1);
await currentGw.webSocketMessage(conflictWs, encodeFrame(0x01, 0x01, 0, 0, { clientId: "mac-other" }));
const conflictAck = JSON.parse(dec.decode(decodeFrame(conflictWs.sent.pop()).payload));
assert.equal(conflictAck.ok, false);
assert.equal(conflictAck.conflictOwner, "mac-1");
assert.equal(conflictWs.readyState, 1);

const reconnRes = await worker.fetch(await makeSignedConnectReq(secret, "mac-1"), env);
assert.equal(reconnRes.status, 101);
const agent2 = persistedCtx.getWebSockets("agent").find((s) => s !== agent1 && s !== conflictWs);
await currentGw.webSocketMessage(agent2, encodeFrame(0x01, 0x01, 0, 0, { clientId: "mac-1" }));
const reconnAck = JSON.parse(dec.decode(decodeFrame(agent2.sent.pop()).payload));
assert.equal(reconnAck.ok, true);
assert.equal(agent1.deserializeAttachment().active, false);
assert.equal(agent1.readyState, 3);
assert.equal(visitor2.readyState, 3);
assert.equal(conflictWs.readyState, 1, "conflicting client socket must stay open");
assert.equal(agent2.deserializeAttachment().active, true);

// 9. Orphaned visitor socket on wakeup without an active agent must be closed cleanly.
agent2.readyState = 3;
const orphanVisitorRes = await currentGw.fetch(new Request("https://devbase-app.demo.workers.dev/offline"));
assert.equal(orphanVisitorRes.status, 502);
const ctxOrphan = new PersistedDOContext();
const orphanV = new (Object.getPrototypeOf(visitor2).constructor)();
ctxOrphan.acceptWebSocket(orphanV, ["visitor"]);
orphanV.serializeAttachment({ streamId: 99 });
const gwOrphan = new TunnelGateway(ctxOrphan, env);
assert.equal(gwOrphan.wsVisitors.size, 0);
assert.equal(orphanV.readyState, 3);
`
	if err := os.WriteFile(filepath.Join(dir, "cf_mock.mjs"), []byte(cfMockJS), 0600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "index.mjs"), []byte(transformedWorker), 0600); err != nil {
		t.Fatal(err)
	}
	testPath := filepath.Join(dir, "hibernation_test.mjs")
	if err := os.WriteFile(testPath, []byte(testScript), 0600); err != nil {
		t.Fatal(err)
	}
	cmd := exec.Command(nodeBin, testPath)
	cmd.Dir = dir
	out, err := cmd.CombinedOutput()
	if err != nil {
		t.Fatalf("node hibernation test failed: %v\n%s", err, string(out))
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

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

	if err := m.GenerateTunnelSecret(ctx); err != nil {
		t.Fatal(err)
	}
	if !m.HasTunnelSecret() {
		t.Fatal("expected tunnel secret to be present")
	}
	secret, err := m.ReadTunnelSecret()
	if err != nil || len(secret) != 64 {
		t.Fatalf("unexpected secret: len=%d err=%v", len(secret), err)
	}
	info, err := os.Stat(m.tunnelKeyPath())
	if err != nil || info.Mode().Perm() != 0600 {
		t.Fatalf("tunnel key permissions wrong: %v %v", info.Mode().Perm(), err)
	}

	if err := m.SaveTunnelSettings(ctx, " *.DEV.Example.COM ", "", "https://gw.example.com/", ""); err != nil {
		t.Fatal(err)
	}
	s, err := m.Read()
	if err != nil {
		t.Fatal(err)
	}
	if s.TunnelDomain != "dev.example.com" || s.TunnelEndpoint != "https://gw.example.com" || !strings.HasPrefix(s.TunnelClientID, "mac-") {
		t.Fatalf("unexpected saved tunnel settings: %+v", s)
	}

	// Secret must never appear in state.json or redacted diagnostics.
	rawState, err := os.ReadFile(filepath.Join(m.Root, "state.json"))
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(rawState), secret) {
		t.Fatal("state.json exposed tunnel secret")
	}
	report, err := m.Diagnose(ctx)
	if err != nil {
		t.Fatal(err)
	}
	redacted, err := report.RedactedJSON()
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(redacted), secret) {
		t.Fatal("diagnostics exposed tunnel secret")
	}

	// Reject insecure remote http:// endpoints while allowing loopback http:// for tests.
	if err := m.SaveTunnelSettings(ctx, "dev.example.com", "mac-1", "http://remote.example.com", ""); err == nil {
		t.Fatal("accepted non-loopback http:// worker URL")
	}
	if err := m.SaveTunnelSettings(ctx, "dev.example.com", "mac-1", "http://127.0.0.1:8787", ""); err != nil {
		t.Fatalf("rejected loopback http:// worker URL: %v", err)
	}

	// Validate ProxyRule tunnel subdomain normalization & collision detection.
	if got := DefaultTunnelSubdomain("api.app.test"); got != "api-app" {
		t.Fatalf("DefaultTunnelSubdomain = %q", got)
	}
	if err := m.ReplaceProxyRules(ctx, 0, 8443, []ProxyRule{
		{Domain: "a.b.test", LocalPort: 3000, Tunnel: true},
		{Domain: "a-b.test", LocalPort: 3001, Tunnel: true},
	}); err == nil || !strings.Contains(err.Error(), "duplicate tunnel subdomain") {
		t.Fatalf("accepted duplicate resolved tunnel subdomain: %v", err)
	}
	if err := m.ReplaceProxyRules(ctx, 0, 8443, []ProxyRule{
		{Domain: "devbase-gateway.test", LocalPort: 3000, Tunnel: true},
	}); err == nil || !strings.Contains(err.Error(), "reserved") {
		t.Fatalf("accepted reserved tunnel subdomain: %v", err)
	}
	if err := m.ReplaceProxyRules(ctx, 0, 8443, []ProxyRule{
		{Domain: "a.b.test", LocalPort: 3000, Tunnel: true, TunnelSubdomain: " API "},
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
	t        *testing.T
	secret   string
	server   *httptest.Server
	mu       sync.Mutex
	agents   map[string]*wsConn
	routes   map[string]string
	claimed  map[string]string // externally owned hosts for conflict testing
	httpWait map[uint32]*mockHTTPWaiter
	wsWait   map[uint32]chan tunnelWSOpenAckResult
	visitors map[uint32]*wsConn
	nextID   atomic.Uint32
	conns    atomic.Int32
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
		agents:   map[string]*wsConn{},
		routes:   map[string]string{},
		claimed:  map[string]string{},
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
	ws := g.agents[clientID]
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
	owner := g.routes[host]
	agentWS := g.agents[owner]
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
		if g.agents[clientID] == ws {
			delete(g.agents, clientID)
			for h, owner := range g.routes {
				if owner == clientID {
					delete(g.routes, h)
				}
			}
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
			g.agents[reg.ClientID] = ws
			for h, owner := range g.routes {
				if owner == reg.ClientID {
					delete(g.routes, h)
				}
			}
			var accepted []string
			var conflicts []TunnelConflict
			for _, h := range reg.Routes {
				if other, taken := g.claimed[h]; taken && other != reg.ClientID {
					conflicts = append(conflicts, TunnelConflict{Host: h, Owner: other})
					continue
				}
				g.routes[h] = reg.ClientID
				accepted = append(accepted, h)
			}
			g.mu.Unlock()
			ack, _ := encodeTunnelJSONFrame(frameRegisterAck, tunnelFlagFIN, 0, 0, tunnelRegisterAckPayload{
				Accepted:  accepted,
				Conflicts: conflicts,
			})
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
				"host":      r.Host,
				"fhost":     r.Header.Get("X-Forwarded-Host"),
				"fproto":    r.Header.Get("X-Forwarded-Proto"),
				"forigin":   r.Header.Get("X-Forwarded-Origin"),
				"origin":    r.Header.Get("Origin"),
				"ffor":      r.Header.Get("X-Forwarded-For"),
				"query":     r.URL.RawQuery,
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
	if err := m.GenerateTunnelSecret(ctx); err != nil {
		t.Fatal(err)
	}
	secret, err := m.ReadTunnelSecret()
	if err != nil {
		t.Fatal(err)
	}

	gw := newMockGatewayServer(t, secret)
	gw.claimed["conflict.dev.example.com"] = "other-mac"
	m.tunnelTLSConfig = gw.tlsConfig()
	m.tunnelPingInterval = 250 * time.Millisecond
	m.tunnelReadTimeout = 2 * time.Second

	if err := m.SaveTunnelSettings(ctx, "dev.example.com", "mac-test", gw.server.URL, ""); err != nil {
		t.Fatal(err)
	}
	if err := m.ReplaceProxyRules(ctx, 0, 8443, []ProxyRule{
		{Domain: "app.test", LocalPort: localPort, Tunnel: true},
	}); err != nil {
		t.Fatal(err)
	}
	s, err := m.Read()
	if err != nil {
		t.Fatal(err)
	}
	if err := m.writePublishedTunnelConfig(&s); err != nil {
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
		return st.Phase == "Connected" && len(st.Accepted) == 1 && st.Accepted[0] == "app.dev.example.com"
	})

	visitorClient := gw.server.Client()

	// 1. Standard HTTP request + multi-value headers + Origin/Host rewriting.
	req, _ := http.NewRequest("GET", gw.server.URL+"/hello?x=1&y=2", nil)
	req.Host = "app.dev.example.com"
	req.Header.Set("Origin", "https://app.dev.example.com")
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
	if echo["host"] != wantLocalHost || echo["fhost"] != "app.dev.example.com" || echo["fproto"] != "https" || echo["origin"] != "http://"+wantLocalHost || echo["forigin"] != "https://app.dev.example.com" || echo["query"] != "x=1&y=2" {
		t.Fatalf("header forwarding mismatch: %+v", echo)
	}

	// 2. Large (>1 MiB) streaming POST & response body.
	big := bytes.Repeat([]byte("DevbaseTunnelPayload"), (1500<<10)/20) // ~1.5 MiB
	reqBig, _ := http.NewRequest("POST", gw.server.URL+"/large", bytes.NewReader(big))
	reqBig.Host = "app.dev.example.com"
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
	reqSSE, _ := http.NewRequest("GET", gw.server.URL+"/sse", nil)
	reqSSE.Host = "app.dev.example.com"
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
	wsURL := "wss://" + strings.TrimPrefix(gw.server.URL, "https://") + "/hmr"
	wsHeaders := http.Header{
		"Host":   []string{"app.dev.example.com"},
		"Origin": []string{"https://app.dev.example.com"},
	}
	vws, wsResp, err := dialWebSocket(ctx, wsURL, wsHeaders, []string{"vite-hmr"}, gw.tlsConfig())
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

	// 5. Hot reload routes via proxy/tunnel.json without reconnecting WS, and report conflicts.
	initialConns := gw.conns.Load()
	if err := m.ReplaceProxyRules(ctx, 1, 8443, []ProxyRule{
		{Domain: "app.test", LocalPort: localPort, Tunnel: true},
		{Domain: "admin.test", LocalPort: secondPort, Tunnel: true},
		{Domain: "conflict.test", LocalPort: secondPort, Tunnel: true},
	}); err != nil {
		t.Fatal(err)
	}
	s, _ = m.Read()
	if err := m.writePublishedTunnelConfig(&s); err != nil {
		t.Fatal(err)
	}
	st := waitForTunnel(func(st TunnelStatusSnapshot) bool {
		return st.Phase == "Connected" && len(st.Accepted) == 2 && len(st.Conflicts) == 1
	})
	if st.Conflicts[0].Host != "conflict.dev.example.com" || st.Conflicts[0].Owner != "other-mac" {
		t.Fatalf("unexpected conflict report: %+v", st.Conflicts)
	}
	if detail := m.formatTunnelDetail(); detail != "Conflict: 1 route" {
		t.Fatalf("unexpected formatTunnelDetail: %q", detail)
	}
	if gw.conns.Load() != initialConns {
		t.Fatalf("route hot reload dropped and reconnected WS: before=%d after=%d", initialConns, gw.conns.Load())
	}
	reqAdmin, _ := http.NewRequest("GET", gw.server.URL+"/", nil)
	reqAdmin.Host = "admin.dev.example.com"
	respAdmin, err := visitorClient.Do(reqAdmin)
	if err != nil {
		t.Fatal(err)
	}
	adminBody, _ := io.ReadAll(respAdmin.Body)
	respAdmin.Body.Close()
	if respAdmin.StatusCode != 200 || string(adminBody) != "second-upstream-ok" {
		t.Fatalf("hot-reloaded route failed: %d %q", respAdmin.StatusCode, string(adminBody))
	}

	// 6. Auto-reconnect after gateway drops the agent connection.
	gw.dropClient("mac-test")
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) && gw.conns.Load() == initialConns {
		time.Sleep(25 * time.Millisecond)
	}
	if gw.conns.Load() <= initialConns {
		t.Fatal("ServeTunnel did not reconnect after connection drop")
	}
	waitForTunnel(func(st TunnelStatusSnapshot) bool {
		return st.Phase == "Connected" && len(st.Accepted) == 2
	})
}

func TestExportTunnelWorkerTemplate(t *testing.T) {
	m := testManager(t)
	ctx := context.Background()
	if err := m.SaveTunnelSettings(ctx, "edge.example.com", "mac-1", "", ""); err != nil {
		t.Fatal(err)
	}
	dir, err := m.ExportTunnelWorker(ctx)
	if err != nil {
		t.Fatal(err)
	}
	for _, rel := range []string{"wrangler.toml", "src/index.mjs", "README.md"} {
		p := filepath.Join(dir, rel)
		info, err := os.Stat(p)
		if err != nil || info.Mode().Perm() != 0600 {
			t.Fatalf("missing or wrong permissions for %s: %v", rel, err)
		}
	}
	tomlBytes, _ := os.ReadFile(filepath.Join(dir, "wrangler.toml"))
	if !strings.Contains(string(tomlBytes), "TunnelGateway") || !strings.Contains(string(tomlBytes), "*.edge.example.com/*") {
		t.Fatalf("unexpected wrangler.toml:\n%s", string(tomlBytes))
	}
	if nodeBin, lookErr := exec.LookPath("node"); lookErr == nil {
		out, err := exec.Command(nodeBin, "--check", filepath.Join(dir, "src", "index.mjs")).CombinedOutput()
		if err != nil {
			t.Fatalf("node --check failed: %v\n%s", err, string(out))
		}
	}
}

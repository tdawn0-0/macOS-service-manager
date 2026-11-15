package devplatform

import (
	"bytes"
	"context"
	"crypto/sha256"
	"crypto/tls"
	"encoding/json"
	"errors"
	"fmt"
	"html/template"
	"io"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"
)

const (
	tunnelKeepAliveInterval = 20 * time.Second
	tunnelReadTimeout       = 45 * time.Second
	tunnelMaxStreamBuffer   = 16 << 20 // 16 MiB per stream upload buffer
	tunnelMaxStreams        = 256
)

// ApplyTunnel reconciles any unsynced Cloudflare Workers if an API token is
// configured, publishes the tunnel routing snapshot, and ensures the background
// launchd tunnel job is running.
func (m *Manager) ApplyTunnel(ctx context.Context) error {
	if s, err := m.Read(); err == nil && m.needsTunnelSync(s) && m.HasCloudflareToken() {
		if err := m.SyncTunnels(ctx); err != nil {
			return err
		}
	}
	return m.withState(ctx, func(s *State) error {
		return m.startTunnel(ctx, s)
	})
}

func (m *Manager) needsTunnelSync(s State) bool {
	desired := map[string]ProxyRule{}
	for _, rule := range s.ProxyRules {
		if !rule.Tunnel {
			continue
		}
		name := rule.ResolvedTunnelWorker()
		desired[name] = rule
		w, ok := s.TunnelWorkers[name]
		if !ok {
			if rule.TunnelURL == "" {
				return true
			}
			continue
		}
		if w.PublicURL == "" || w.CustomDomain != rule.EffectiveTunnelDomain() {
			return true
		}
	}
	for name := range s.TunnelWorkers {
		if _, keep := desired[name]; !keep {
			return true
		}
	}
	return false
}

// StartTunnel ensures the background Cloudflare tunnel launchd job is running.
func (m *Manager) StartTunnel(ctx context.Context) error {
	return m.ApplyTunnel(ctx)
}

// StopTunnel stops the background Cloudflare tunnel launchd job.
func (m *Manager) StopTunnel(ctx context.Context) error {
	return m.withState(ctx, func(*State) error {
		if err := m.stopJob(ctx, "tunnel"); err != nil {
			return err
		}
		m.writeTunnelStatus(TunnelStatusSnapshot{Phase: "Stopped"})
		return nil
	})
}

// RestartTunnel restarts the background Cloudflare tunnel launchd job.
func (m *Manager) RestartTunnel(ctx context.Context) error {
	return m.withState(ctx, func(s *State) error {
		if err := m.stopJob(ctx, "tunnel"); err != nil {
			return err
		}
		return m.startTunnel(ctx, s)
	})
}

func (m *Manager) startTunnel(ctx context.Context, s *State) error {
	if err := m.ensureLogMaintenance(ctx, s); err != nil {
		return err
	}
	if _, err := m.ensureTunnelSecret(); err != nil {
		return err
	}
	cfg, err := tunnelConfigFromState(s)
	if err != nil {
		return err
	}
	if len(cfg.Routes) == 0 {
		return errors.New("enable Tunnel on at least one HTTPS rule first")
	}
	if err := m.writePublishedTunnelConfig(s); err != nil {
		return err
	}
	if job, err := m.readJobState(ctx, "tunnel"); err == nil && job.Running && job.PID > 0 {
		return nil
	}
	exe, err := m.helperExecutable("tunnel")
	if err != nil {
		return err
	}
	args := []string{exe, "--dev", "tunnel", m.Root}
	fresh, err := m.startJobPolicy(ctx, "tunnel", args, m.Root, nil, true, 5)
	if err != nil {
		return err
	}
	if err = m.waitService(ctx, "tunnel", processProbe{ID: "tunnel", Health: "process", ReadinessTimeoutSeconds: 10}); err != nil {
		if fresh {
			_ = m.stopJob(context.Background(), "tunnel")
		}
		return err
	}
	return nil
}

// ServeTunnel supervises one outbound WebSocket connection per deployed
// Cloudflare Worker route defined in proxy/tunnel.json.
func (m *Manager) ServeTunnel(ctx context.Context) error {
	if err := m.init(); err != nil {
		return err
	}
	cfg, cfgDigest, err := readTunnelConfigWithDigest(m.Root)
	if err != nil {
		return err
	}
	secret, keyDigest, err := m.readTunnelSecretWithDigest()
	if err != nil {
		return err
	}
	sup := &tunnelSupervisor{
		manager:   m,
		transport: newTunnelUpstreamTransport(),
		cfg:       cfg,
		cfgDigest: cfgDigest,
		secret:    secret,
		keyDigest: keyDigest,
		runners:   map[string]*managedWorkerRunner{},
		statuses:  map[string]tunnelWorkerStatus{},
	}
	sup.reconcile(ctx)
	defer func() {
		sup.stopAll()
		m.writeTunnelStatus(TunnelStatusSnapshot{Phase: "Stopped"})
	}()

	tick := time.NewTicker(time.Second)
	defer tick.Stop()
	for {
		select {
		case <-ctx.Done():
			return nil
		case <-tick.C:
			sup.checkUpdates(ctx)
		}
	}
}

func newTunnelUpstreamTransport() *http.Transport {
	return &http.Transport{
		Proxy:                 nil,
		DisableCompression:    true,
		IdleConnTimeout:       90 * time.Second,
		MaxIdleConns:          100,
		DialContext:           loopbackUpstreamDialer,
		TLSHandshakeTimeout:   10 * time.Second,
		ResponseHeaderTimeout: 60 * time.Second,
	}
}

type tunnelWorkerStatus struct {
	phase     string
	conflicts []TunnelConflict
}

type managedWorkerRunner struct {
	route  TunnelRoute
	cancel context.CancelFunc
	done   chan struct{}
}

type tunnelSupervisor struct {
	manager   *Manager
	transport *http.Transport

	mu        sync.Mutex
	cfg       tunnelPublishedConfig
	cfgDigest [32]byte
	secret    string
	keyDigest [32]byte
	runners   map[string]*managedWorkerRunner
	statuses  map[string]tunnelWorkerStatus
}

type tunnelWorkerRunner struct {
	sup       *tunnelSupervisor
	manager   *Manager
	transport *http.Transport
	clientID  string
	secret    string
	route     TunnelRoute
}

func readTunnelConfigWithDigest(root string) (tunnelPublishedConfig, [32]byte, error) {
	b, err := os.ReadFile(filepath.Join(root, "proxy", "tunnel.json"))
	if err != nil {
		return tunnelPublishedConfig{}, [32]byte{}, err
	}
	cfg, err := readPublishedTunnelConfig(root)
	if err != nil {
		return tunnelPublishedConfig{}, [32]byte{}, err
	}
	return cfg, sha256.Sum256(b), nil
}

func (m *Manager) readTunnelSecretWithDigest() (string, [32]byte, error) {
	b, err := os.ReadFile(m.tunnelKeyPath())
	if err != nil {
		return "", [32]byte{}, err
	}
	secret, err := validateTunnelSecret(string(b))
	if err != nil {
		return "", [32]byte{}, err
	}
	return secret, sha256.Sum256(b), nil
}

func (s *tunnelSupervisor) checkUpdates(ctx context.Context) {
	cfg, cfgDigest, cfgErr := readTunnelConfigWithDigest(s.manager.Root)
	secret, keyDigest, keyErr := s.manager.readTunnelSecretWithDigest()

	var toStop []*managedWorkerRunner
	var restartAll, cfgChanged bool

	s.mu.Lock()
	if keyErr == nil && keyDigest != s.keyDigest {
		s.secret = secret
		s.keyDigest = keyDigest
		restartAll = true
	}
	if cfgErr == nil && cfgDigest != s.cfgDigest {
		if cfg.ClientID != s.cfg.ClientID {
			restartAll = true
		} else {
			cfgChanged = true
		}
		s.cfg = cfg
		s.cfgDigest = cfgDigest
	}
	if restartAll {
		for name, mr := range s.runners {
			toStop = append(toStop, mr)
			delete(s.runners, name)
			delete(s.statuses, name)
		}
	}
	s.mu.Unlock()

	for _, mr := range toStop {
		mr.cancel()
		<-mr.done
	}
	if restartAll || cfgChanged {
		s.reconcile(ctx)
	}
}

func (s *tunnelSupervisor) reconcile(ctx context.Context) {
	s.mu.Lock()
	desired := make(map[string]TunnelRoute, len(s.cfg.Routes))
	for _, r := range s.cfg.Routes {
		desired[r.WorkerName] = r
	}
	var toStop []*managedWorkerRunner
	for name, mr := range s.runners {
		want, keep := desired[name]
		if !keep || mr.route != want {
			toStop = append(toStop, mr)
			delete(s.runners, name)
			delete(s.statuses, name)
		}
	}
	s.mu.Unlock()

	for _, mr := range toStop {
		mr.cancel()
		<-mr.done
	}

	s.mu.Lock()
	for name, route := range desired {
		if _, exists := s.runners[name]; exists {
			continue
		}
		rCtx, rCancel := context.WithCancel(ctx)
		done := make(chan struct{})
		s.runners[name] = &managedWorkerRunner{route: route, cancel: rCancel, done: done}
		s.statuses[name] = tunnelWorkerStatus{phase: "Connecting"}
		runner := &tunnelWorkerRunner{
			sup:       s,
			manager:   s.manager,
			transport: s.transport,
			clientID:  s.cfg.ClientID,
			secret:    s.secret,
			route:     route,
		}
		go func() {
			defer close(done)
			runner.loop(rCtx)
		}()
	}
	s.manager.writeTunnelStatus(s.buildStatusLocked())
	s.mu.Unlock()
}

func (s *tunnelSupervisor) stopAll() {
	s.mu.Lock()
	toStop := make([]*managedWorkerRunner, 0, len(s.runners))
	for name, mr := range s.runners {
		toStop = append(toStop, mr)
		delete(s.runners, name)
		delete(s.statuses, name)
	}
	s.mu.Unlock()

	for _, mr := range toStop {
		mr.cancel()
		<-mr.done
	}
}

func (s *tunnelSupervisor) updateStatus(workerName string, st tunnelWorkerStatus) {
	s.mu.Lock()
	if _, active := s.runners[workerName]; !active {
		s.mu.Unlock()
		return
	}
	s.statuses[workerName] = st
	s.manager.writeTunnelStatus(s.buildStatusLocked())
	s.mu.Unlock()
}

func (s *tunnelSupervisor) buildStatusLocked() TunnelStatusSnapshot {
	total := len(s.runners)
	if total == 0 {
		return TunnelStatusSnapshot{PID: os.Getpid(), Phase: "Stopped", Total: 0}
	}
	var connected, connecting, authFailed int
	var conflicts []TunnelConflict
	for name := range s.runners {
		st := s.statuses[name]
		switch st.phase {
		case "Connected":
			if len(st.conflicts) > 0 {
				conflicts = append(conflicts, st.conflicts...)
			} else {
				connected++
			}
		case "AuthFailed":
			authFailed++
		case "Connecting":
			connecting++
		}
	}
	phase := "Reconnecting"
	switch {
	case connected > 0 || len(conflicts) > 0:
		phase = "Connected"
	case authFailed > 0:
		phase = "AuthFailed"
	case connecting == total:
		phase = "Connecting"
	}
	return TunnelStatusSnapshot{
		PID:       os.Getpid(),
		Phase:     phase,
		Connected: connected,
		Total:     total,
		Conflicts: conflicts,
	}
}

func (r *tunnelWorkerRunner) loop(ctx context.Context) {
	backoff := 500 * time.Millisecond
	for {
		if ctx.Err() != nil {
			return
		}
		startedAt := time.Now()
		authFailed, runErr := r.runOnce(ctx)
		if ctx.Err() != nil {
			return
		}
		if time.Since(startedAt) >= 15*time.Second {
			backoff = 500 * time.Millisecond
		}
		wait := backoff
		if authFailed {
			r.sup.updateStatus(r.route.WorkerName, tunnelWorkerStatus{phase: "AuthFailed"})
			wait = 15 * time.Second
		} else {
			r.sup.updateStatus(r.route.WorkerName, tunnelWorkerStatus{phase: "Reconnecting"})
			backoff *= 2
			if backoff > 15*time.Second {
				backoff = 15 * time.Second
			}
		}
		if runErr != nil {
			r.manager.emit("Tunnel (" + r.route.WorkerName + ") disconnected: " + runErr.Error())
		}
		timer := time.NewTimer(wait)
		select {
		case <-ctx.Done():
			timer.Stop()
			return
		case <-timer.C:
		}
	}
}

func (r *tunnelWorkerRunner) runOnce(ctx context.Context) (authFailed bool, err error) {
	wsURL, err := r.route.WebSocketURL()
	if err != nil {
		return false, err
	}

	ts := strconv.FormatInt(time.Now().Unix(), 10)
	nonce := randomID() + randomID()
	sig := signTunnelHandshake(r.secret, r.clientID, ts, nonce)
	headers := http.Header{}
	headers.Set("X-Devbase-Client", r.clientID)
	headers.Set("X-Devbase-Timestamp", ts)
	headers.Set("X-Devbase-Nonce", nonce)
	headers.Set("X-Devbase-Signature", sig)

	dialCtx, dialCancel := context.WithTimeout(ctx, 15*time.Second)
	var tlsCfg *tls.Config
	if r.manager.tunnelTLSConfig != nil {
		tlsCfg = r.manager.tunnelTLSConfig
	}
	ws, _, err := dialWebSocket(dialCtx, wsURL, headers, nil, tlsCfg)
	dialCancel()
	if err != nil {
		var hsErr *wsHandshakeError
		if errors.As(err, &hsErr) && (hsErr.StatusCode == http.StatusUnauthorized || hsErr.StatusCode == http.StatusForbidden) {
			return true, err
		}
		return false, err
	}
	defer ws.Close()

	readTimeout := r.manager.tunnelReadTimeout
	if readTimeout <= 0 {
		readTimeout = tunnelReadTimeout
	}
	pingInterval := r.manager.tunnelPingInterval
	if pingInterval <= 0 {
		pingInterval = tunnelKeepAliveInterval
	}

	ws.onActivity = func() {
		_ = ws.SetReadDeadline(time.Now().Add(readTimeout))
	}
	_ = ws.SetReadDeadline(time.Now().Add(readTimeout))

	sessionCtx, sessionCancel := context.WithCancel(ctx)
	defer sessionCancel()

	sess := &tunnelSession{
		runner:      r,
		ws:          ws,
		sem:         make(chan struct{}, tunnelMaxStreams),
		httpStreams: map[uint32]*tunnelHTTPStream{},
		wsStreams:   map[uint32]*tunnelWSStream{},
	}
	defer sess.closeAll()

	if err = sess.sendRegister(r.clientID); err != nil {
		return false, err
	}
	r.sup.updateStatus(r.route.WorkerName, tunnelWorkerStatus{phase: "Connecting"})

	readErrCh := make(chan error, 1)
	go func() {
		readErrCh <- sess.readLoop(sessionCtx)
	}()

	pingTick := time.NewTicker(pingInterval)
	defer pingTick.Stop()

	for {
		select {
		case <-ctx.Done():
			_ = ws.WriteClose(1000, "shutdown")
			sessionCancel()
			<-readErrCh
			return false, nil
		case readErr := <-readErrCh:
			sessionCancel()
			return false, readErr
		case <-pingTick.C:
			if err := ws.WriteText([]byte(tunnelControlPing)); err != nil {
				sessionCancel()
				<-readErrCh
				return false, err
			}
		}
	}
}

type tunnelSession struct {
	runner      *tunnelWorkerRunner
	ws          *wsConn
	sem         chan struct{}
	streamsMu   sync.Mutex
	httpStreams map[uint32]*tunnelHTTPStream
	wsStreams   map[uint32]*tunnelWSStream
}

func (s *tunnelSession) sendRegister(clientID string) error {
	frame, err := encodeTunnelJSONFrame(frameRegister, tunnelFlagFIN, 0, 0, tunnelRegisterPayload{
		ClientID: clientID,
	})
	if err != nil {
		return err
	}
	return s.ws.WriteBinary(frame)
}

func (s *tunnelSession) closeAll() {
	s.streamsMu.Lock()
	httpList := make([]*tunnelHTTPStream, 0, len(s.httpStreams))
	for id, st := range s.httpStreams {
		httpList = append(httpList, st)
		delete(s.httpStreams, id)
	}
	wsList := make([]*tunnelWSStream, 0, len(s.wsStreams))
	for id, st := range s.wsStreams {
		wsList = append(wsList, st)
		delete(s.wsStreams, id)
	}
	s.streamsMu.Unlock()

	for _, st := range httpList {
		st.cancel()
		st.body.CloseWithError(io.ErrClosedPipe)
	}
	for _, st := range wsList {
		st.close(1001, "tunnel closed")
	}
}

func (s *tunnelSession) readLoop(ctx context.Context) error {
	for {
		op, data, err := s.ws.ReadMessage()
		if err != nil {
			return err
		}
		if op == wsOpText {
			if string(data) == tunnelControlPong {
				continue
			}
			continue
		}
		frame, err := decodeTunnelFrame(data)
		if err != nil {
			return err
		}
		switch frame.Type {
		case frameRegisterAck:
			var ack tunnelRegisterAckPayload
			if err := json.Unmarshal(frame.Payload, &ack); err == nil {
				var conflicts []TunnelConflict
				if !ack.OK && ack.ConflictOwner != "" {
					conflicts = []TunnelConflict{{Host: s.runner.route.PublicHost(), Owner: ack.ConflictOwner}}
				}
				s.runner.sup.updateStatus(s.runner.route.WorkerName, tunnelWorkerStatus{
					phase:     "Connected",
					conflicts: conflicts,
				})
			}
		case frameHTTPReqHead:
			s.handleHTTPHead(ctx, frame)
		case frameHTTPReqData:
			s.handleHTTPData(frame)
		case frameStreamReset:
			s.handleStreamReset(frame.StreamID)
		case frameWSOpen:
			s.handleWSOpen(ctx, frame)
		case frameWSData:
			s.handleWSData(frame)
		case frameWSClose:
			s.handleWSClose(frame)
		}
	}
}

func (s *tunnelSession) handleHTTPHead(ctx context.Context, frame tunnelFrame) {
	var head tunnelHTTPReqHead
	if err := json.Unmarshal(frame.Payload, &head); err != nil {
		s.sendReset(frame.StreamID)
		return
	}
	route := s.runner.route
	select {
	case s.sem <- struct{}{}:
	default:
		s.sendSimpleHTTPResponse(frame.StreamID, http.StatusServiceUnavailable, "text/plain; charset=utf-8", []byte("Tunnel concurrency limit reached"))
		return
	}

	streamCtx, cancel := context.WithCancel(ctx)
	var bodyReader io.ReadCloser = http.NoBody
	var bodyBuf *streamBodyBuffer
	if frame.Flags&tunnelFlagFIN == 0 {
		bodyBuf = newStreamBodyBuffer(tunnelMaxStreamBuffer)
		bodyReader = bodyBuf
		st := &tunnelHTTPStream{body: bodyBuf, cancel: cancel}
		s.streamsMu.Lock()
		s.httpStreams[frame.StreamID] = st
		s.streamsMu.Unlock()
	}

	go func() {
		defer func() {
			cancel()
			if bodyBuf != nil {
				bodyBuf.CloseWithError(io.ErrClosedPipe)
				s.streamsMu.Lock()
				delete(s.httpStreams, frame.StreamID)
				s.streamsMu.Unlock()
			}
			<-s.sem
		}()
		s.forwardHTTP(streamCtx, frame.StreamID, route, head, bodyReader)
	}()
}

func (s *tunnelSession) handleHTTPData(frame tunnelFrame) {
	s.streamsMu.Lock()
	st := s.httpStreams[frame.StreamID]
	s.streamsMu.Unlock()
	if st == nil {
		return
	}
	if len(frame.Payload) > 0 {
		if !st.body.Push(frame.Payload) {
			st.cancel()
			s.sendReset(frame.StreamID)
			return
		}
	}
	if frame.Flags&tunnelFlagFIN != 0 {
		st.body.Finish()
	}
}

func (s *tunnelSession) handleStreamReset(streamID uint32) {
	s.streamsMu.Lock()
	hs := s.httpStreams[streamID]
	delete(s.httpStreams, streamID)
	ws := s.wsStreams[streamID]
	delete(s.wsStreams, streamID)
	s.streamsMu.Unlock()
	if hs != nil {
		hs.cancel()
		hs.body.CloseWithError(errors.New("stream reset by peer"))
	}
	if ws != nil {
		ws.close(1001, "stream reset")
	}
}

func (s *tunnelSession) forwardHTTP(ctx context.Context, streamID uint32, route TunnelRoute, head tunnelHTTPReqHead, body io.ReadCloser) {
	path := head.Path
	if path == "" || !strings.HasPrefix(path, "/") {
		path = "/" + strings.TrimPrefix(path, "/")
	}
	targetURL := fmt.Sprintf("http://127.0.0.1:%d%s", route.LocalPort, path)
	req, err := http.NewRequestWithContext(ctx, head.Method, targetURL, body)
	if err != nil {
		s.sendSimpleHTTPResponse(streamID, http.StatusBadRequest, "text/plain; charset=utf-8", []byte("Invalid tunneled request"))
		return
	}
	localHost := fmt.Sprintf("localhost:%d", route.LocalPort)
	req.Host = localHost
	applyTunnelForwardedHeaders(req.Header, head.Headers, route.PublicHost(), localHost, head.RemoteAddr)

	resp, err := s.runner.transport.RoundTrip(req)
	if err != nil {
		if ctx.Err() != nil {
			s.sendReset(streamID)
			return
		}
		status, bodyBytes := renderTunnelFailure(route, err)
		s.sendSimpleHTTPResponse(streamID, status, "text/html; charset=utf-8", bodyBytes)
		return
	}
	defer resp.Body.Close()

	respHeaders := filterResponseHeaders(resp.Header)
	bodyless := strings.EqualFold(head.Method, http.MethodHead) || resp.StatusCode == http.StatusNoContent || resp.StatusCode == http.StatusNotModified || resp.StatusCode == http.StatusSwitchingProtocols
	var headFlags byte
	if bodyless {
		headFlags = tunnelFlagFIN
	}
	headFrame, err := encodeTunnelJSONFrame(frameHTTPResHead, headFlags, streamID, uint16(resp.StatusCode), tunnelHTTPResHead{
		Status:  resp.StatusCode,
		Headers: respHeaders,
	})
	if err != nil {
		s.sendReset(streamID)
		return
	}
	if err = s.ws.WriteBinary(headFrame); err != nil {
		return
	}
	if bodyless {
		return
	}

	buf := make([]byte, 64<<10)
	for {
		n, readErr := resp.Body.Read(buf)
		if n > 0 {
			for off := 0; off < n; off += tunnelMaxChunk {
				end := off + tunnelMaxChunk
				if end > n {
					end = n
				}
				var flags byte
				if readErr == io.EOF && end == n {
					flags = tunnelFlagFIN
				}
				frame := encodeTunnelFrame(tunnelFrame{
					Type:     frameHTTPResData,
					Flags:    flags,
					StreamID: streamID,
					Payload:  buf[off:end],
				})
				if writeErr := s.ws.WriteBinary(frame); writeErr != nil {
					return
				}
			}
			if readErr == io.EOF {
				return
			}
		}
		if readErr == io.EOF {
			finFrame := encodeTunnelFrame(tunnelFrame{
				Type:     frameHTTPResData,
				Flags:    tunnelFlagFIN,
				StreamID: streamID,
			})
			_ = s.ws.WriteBinary(finFrame)
			return
		}
		if readErr != nil {
			s.sendReset(streamID)
			return
		}
	}
}

func (s *tunnelSession) sendSimpleHTTPResponse(streamID uint32, status int, contentType string, body []byte) {
	headers := [][2]string{
		{"Content-Type", contentType},
		{"Cache-Control", "no-store"},
		{"Content-Length", strconv.Itoa(len(body))},
	}
	var headFlags byte
	if len(body) == 0 {
		headFlags = tunnelFlagFIN
	}
	headFrame, err := encodeTunnelJSONFrame(frameHTTPResHead, headFlags, streamID, uint16(status), tunnelHTTPResHead{
		Status:  status,
		Headers: headers,
	})
	if err != nil {
		return
	}
	if err = s.ws.WriteBinary(headFrame); err != nil || len(body) == 0 {
		return
	}
	for off := 0; off < len(body); off += tunnelMaxChunk {
		end := off + tunnelMaxChunk
		var flags byte
		if end >= len(body) {
			end = len(body)
			flags = tunnelFlagFIN
		}
		_ = s.ws.WriteBinary(encodeTunnelFrame(tunnelFrame{
			Type:     frameHTTPResData,
			Flags:    flags,
			StreamID: streamID,
			Payload:  body[off:end],
		}))
	}
}

func (s *tunnelSession) sendReset(streamID uint32) {
	_ = s.ws.WriteBinary(encodeTunnelFrame(tunnelFrame{
		Type:     frameStreamReset,
		Flags:    tunnelFlagFIN,
		StreamID: streamID,
	}))
}

func (s *tunnelSession) handleWSOpen(ctx context.Context, frame tunnelFrame) {
	var head tunnelWSOpenHead
	if err := json.Unmarshal(frame.Payload, &head); err != nil {
		s.sendWSOpenError(frame.StreamID, http.StatusBadRequest, "invalid websocket open payload")
		return
	}
	route := s.runner.route
	select {
	case s.sem <- struct{}{}:
	default:
		s.sendWSOpenError(frame.StreamID, http.StatusServiceUnavailable, "tunnel concurrency limit reached")
		return
	}

	path := head.Path
	if path == "" || !strings.HasPrefix(path, "/") {
		path = "/" + strings.TrimPrefix(path, "/")
	}
	localHost := fmt.Sprintf("localhost:%d", route.LocalPort)
	reqHeaders := http.Header{}
	reqHeaders.Set("Host", localHost)
	applyTunnelForwardedHeaders(reqHeaders, head.Headers, route.PublicHost(), localHost, "")

	dialCtx, dialCancel := context.WithTimeout(ctx, 10*time.Second)
	localWS, resp, err := dialWebSocketVia(dialCtx, fmt.Sprintf("ws://127.0.0.1:%d%s", route.LocalPort, path), reqHeaders, head.Protocols, nil, loopbackUpstreamDialer)
	dialCancel()
	if err != nil {
		<-s.sem
		status := http.StatusBadGateway
		var hsErr *wsHandshakeError
		if errors.As(err, &hsErr) {
			status = hsErr.StatusCode
		}
		s.sendWSOpenError(frame.StreamID, status, "local websocket upstream unavailable")
		return
	}

	negotiatedProto := ""
	if resp != nil {
		negotiatedProto = resp.Header.Get("Sec-WebSocket-Protocol")
	}
	ackFrame, err := encodeTunnelJSONFrame(frameWSOpenAck, tunnelFlagFIN, frame.StreamID, http.StatusSwitchingProtocols, tunnelWSOpenAckHead{
		Protocol: negotiatedProto,
	})
	if err != nil {
		localWS.Close()
		<-s.sem
		return
	}

	wsStream := &tunnelWSStream{
		session:  s,
		streamID: frame.StreamID,
		local:    localWS,
		sendCh:   make(chan wsOutboundMsg, 64),
	}
	s.streamsMu.Lock()
	s.wsStreams[frame.StreamID] = wsStream
	s.streamsMu.Unlock()

	if err = s.ws.WriteBinary(ackFrame); err != nil {
		s.streamsMu.Lock()
		delete(s.wsStreams, frame.StreamID)
		s.streamsMu.Unlock()
		wsStream.close(1001, "tunnel write error")
		<-s.sem
		return
	}

	go func() {
		defer func() {
			s.streamsMu.Lock()
			delete(s.wsStreams, frame.StreamID)
			s.streamsMu.Unlock()
			wsStream.close(1000, "")
			<-s.sem
		}()
		wsStream.pump(ctx)
	}()
}

func (s *tunnelSession) sendWSOpenError(streamID uint32, status int, msg string) {
	b, err := encodeTunnelJSONFrame(frameWSOpenAck, tunnelFlagFIN, streamID, uint16(status), tunnelWSOpenAckHead{Error: msg})
	if err == nil {
		_ = s.ws.WriteBinary(b)
	}
}

func (s *tunnelSession) handleWSData(frame tunnelFrame) {
	s.streamsMu.Lock()
	st := s.wsStreams[frame.StreamID]
	s.streamsMu.Unlock()
	if st == nil {
		return
	}
	st.mu.Lock()
	if len(st.fragBuf)+len(frame.Payload) > wsMaxMessageSize {
		st.mu.Unlock()
		st.close(1009, "message too big")
		s.sendReset(frame.StreamID)
		return
	}
	st.fragBuf = append(st.fragBuf, frame.Payload...)
	if frame.Flags&tunnelFlagFIN == 0 {
		st.mu.Unlock()
		return
	}
	msg := st.fragBuf
	st.fragBuf = nil
	st.mu.Unlock()

	opcode := wsOpBinary
	if frame.Flags&tunnelFlagText != 0 {
		opcode = wsOpText
	}
	if !st.enqueue(opcode, msg) {
		st.close(1001, "local websocket backpressure")
		s.sendReset(frame.StreamID)
	}
}

func (s *tunnelSession) handleWSClose(frame tunnelFrame) {
	s.streamsMu.Lock()
	st := s.wsStreams[frame.StreamID]
	delete(s.wsStreams, frame.StreamID)
	s.streamsMu.Unlock()
	if st != nil {
		code := frame.Code
		if code == 0 {
			code = 1000
		}
		st.close(code, string(frame.Payload))
	}
}

type wsOutboundMsg struct {
	opcode byte
	data   []byte
}

type tunnelWSStream struct {
	session  *tunnelSession
	streamID uint32
	local    *wsConn

	mu        sync.Mutex
	fragBuf   []byte
	closed    bool
	closeOnce sync.Once
	sendCh    chan wsOutboundMsg
}

func (st *tunnelWSStream) enqueue(opcode byte, data []byte) bool {
	st.mu.Lock()
	closed := st.closed
	st.mu.Unlock()
	if closed {
		return false
	}
	select {
	case st.sendCh <- wsOutboundMsg{opcode: opcode, data: data}:
		return true
	default:
		return false
	}
}

func (st *tunnelWSStream) close(code uint16, reason string) {
	st.closeOnce.Do(func() {
		st.mu.Lock()
		st.closed = true
		st.mu.Unlock()
		if code != 0 {
			_ = st.local.WriteClose(code, reason)
		}
		_ = st.local.Close()
		close(st.sendCh)
	})
}

func (st *tunnelWSStream) pump(ctx context.Context) {
	writerDone := make(chan struct{})
	go func() {
		defer close(writerDone)
		for msg := range st.sendCh {
			if err := st.local.WriteMessage(msg.opcode, msg.data); err != nil {
				st.close(1011, "local write error")
				return
			}
		}
	}()

	for {
		op, data, err := st.local.ReadMessage()
		if err != nil {
			var closeErr *wsCloseError
			code := uint16(1000)
			reason := ""
			if errors.As(err, &closeErr) {
				code = closeErr.Code
				reason = closeErr.Reason
			}
			if ctx.Err() == nil {
				_ = st.session.ws.WriteBinary(encodeTunnelFrame(tunnelFrame{
					Type:     frameWSClose,
					Flags:    tunnelFlagFIN,
					StreamID: st.streamID,
					Code:     code,
					Payload:  []byte(reason),
				}))
			}
			st.close(0, "")
			<-writerDone
			return
		}
		var textFlag byte
		if op == wsOpText {
			textFlag = tunnelFlagText
		}
		if len(data) == 0 {
			_ = st.session.ws.WriteBinary(encodeTunnelFrame(tunnelFrame{
				Type:     frameWSData,
				Flags:    textFlag | tunnelFlagFIN,
				StreamID: st.streamID,
			}))
			continue
		}
		for off := 0; off < len(data); off += tunnelMaxChunk {
			end := off + tunnelMaxChunk
			flags := textFlag
			if end >= len(data) {
				end = len(data)
				flags |= tunnelFlagFIN
			}
			if writeErr := st.session.ws.WriteBinary(encodeTunnelFrame(tunnelFrame{
				Type:     frameWSData,
				Flags:    flags,
				StreamID: st.streamID,
				Payload:  data[off:end],
			})); writeErr != nil {
				st.close(1001, "tunnel write error")
				<-writerDone
				return
			}
		}
	}
}

type tunnelHTTPStream struct {
	body   *streamBodyBuffer
	cancel context.CancelFunc
}

type streamBodyBuffer struct {
	mu      sync.Mutex
	chunks  [][]byte
	queued  int
	maxSize int
	eof     bool
	err     error
	notify  chan struct{}
}

func newStreamBodyBuffer(maxSize int) *streamBodyBuffer {
	return &streamBodyBuffer{
		maxSize: maxSize,
		notify:  make(chan struct{}, 1),
	}
}

func (b *streamBodyBuffer) signal() {
	select {
	case b.notify <- struct{}{}:
	default:
	}
}

func (b *streamBodyBuffer) Push(p []byte) bool {
	if len(p) == 0 {
		return true
	}
	cp := append([]byte(nil), p...)
	b.mu.Lock()
	defer b.mu.Unlock()
	if b.eof || b.err != nil || b.queued+len(cp) > b.maxSize {
		return false
	}
	b.chunks = append(b.chunks, cp)
	b.queued += len(cp)
	b.signal()
	return true
}

func (b *streamBodyBuffer) Finish() {
	b.mu.Lock()
	b.eof = true
	b.signal()
	b.mu.Unlock()
}

func (b *streamBodyBuffer) CloseWithError(err error) {
	b.mu.Lock()
	if b.err == nil {
		b.err = err
	}
	b.signal()
	b.mu.Unlock()
}

func (b *streamBodyBuffer) Close() error {
	b.CloseWithError(io.ErrClosedPipe)
	return nil
}

func (b *streamBodyBuffer) Read(p []byte) (int, error) {
	for {
		b.mu.Lock()
		if len(b.chunks) > 0 {
			head := b.chunks[0]
			n := copy(p, head)
			if n < len(head) {
				b.chunks[0] = head[n:]
			} else {
				b.chunks = b.chunks[1:]
			}
			b.queued -= n
			b.mu.Unlock()
			return n, nil
		}
		if b.err != nil {
			err := b.err
			b.mu.Unlock()
			return 0, err
		}
		if b.eof {
			b.mu.Unlock()
			return 0, io.EOF
		}
		b.mu.Unlock()
		<-b.notify
	}
}

func applyTunnelForwardedHeaders(dst http.Header, incoming [][2]string, publicHost, localHost, remoteAddr string) {
	hopByHop := map[string]bool{
		"connection":                true,
		"keep-alive":                true,
		"proxy-authenticate":        true,
		"proxy-authorization":       true,
		"te":                        true,
		"trailer":                   true,
		"transfer-encoding":         true,
		"upgrade":                   true,
		"host":                      true,
		"forwarded":                 true,
		"x-forwarded-for":           true,
		"x-forwarded-host":          true,
		"x-forwarded-proto":         true,
		"x-real-ip":                 true,
		"sec-websocket-key":         true,
		"sec-websocket-version":     true,
		"sec-websocket-extensions":  true,
		"sec-websocket-protocol":    true,
		"cf-connecting-ip":          false,
	}
	var connTokens []string
	for _, kv := range incoming {
		if strings.EqualFold(kv[0], "connection") {
			for _, tok := range strings.Split(kv[1], ",") {
				if t := strings.ToLower(strings.TrimSpace(tok)); t != "" {
					connTokens = append(connTokens, t)
				}
			}
		}
	}
	for _, t := range connTokens {
		hopByHop[t] = true
	}
	for _, kv := range incoming {
		lk := strings.ToLower(kv[0])
		if hopByHop[lk] {
			continue
		}
		if lk == "origin" && publicHost != "" {
			origin := strings.TrimSpace(kv[1])
			if strings.EqualFold(origin, "https://"+publicHost) || strings.EqualFold(origin, "https://"+publicHost+":443") {
				dst.Set("X-Forwarded-Origin", origin)
				dst.Set("Origin", "http://"+localHost)
				continue
			}
		}
		dst.Add(kv[0], kv[1])
	}
	if publicHost != "" {
		dst.Set("X-Forwarded-Host", publicHost)
	}
	dst.Set("X-Forwarded-Proto", "https")
	if remoteAddr != "" {
		dst.Set("X-Forwarded-For", remoteAddr)
		dst.Set("X-Real-IP", remoteAddr)
	}
}

func filterResponseHeaders(h http.Header) [][2]string {
	hopByHop := map[string]bool{
		"connection":          true,
		"keep-alive":          true,
		"proxy-authenticate":  true,
		"proxy-authorization": true,
		"te":                  true,
		"trailer":             true,
		"transfer-encoding":   true,
		"upgrade":             true,
	}
	for _, v := range h.Values("Connection") {
		for _, tok := range strings.Split(v, ",") {
			if t := strings.ToLower(strings.TrimSpace(tok)); t != "" {
				hopByHop[t] = true
			}
		}
	}
	var out [][2]string
	keys := make([]string, 0, len(h))
	for k := range h {
		if !hopByHop[strings.ToLower(k)] {
			keys = append(keys, k)
		}
	}
	sort.Strings(keys)
	for _, k := range keys {
		for _, v := range h[k] {
			out = append(out, [2]string{k, v})
		}
	}
	return out
}

var tunnelFailurePage = template.Must(template.New("tunnel-failure").Parse(`<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>{{.Title}}</title><style>body{font:16px/1.6 system-ui;margin:12vh auto;padding:24px;max-width:640px;color:#202124;background:#f5f6f8}main{background:white;padding:32px;border-radius:16px}h1{line-height:1.2;font-size:28px}code{word-break:break-all}p{color:#555}</style></head><body><main><h1>{{.Title}}</h1><p>{{.Message}}</p><p><strong>{{.PublicHost}}</strong> → <code>http://127.0.0.1:{{.LocalPort}}</code></p><p>Open Devbase to check the local service and tunnel status.</p></main></body></html>`))

func renderTunnelFailure(route TunnelRoute, err error) (int, []byte) {
	status := http.StatusBadGateway
	title := "Local service is unavailable"
	message := "The local server did not accept this tunneled request. Start the local application or check its port."
	var netErr net.Error
	if errors.As(err, &netErr) && netErr.Timeout() {
		status = http.StatusGatewayTimeout
		title = "Local service timed out"
		message = "The local server did not respond within the tunnel timeout."
	}
	var buf bytes.Buffer
	_ = tunnelFailurePage.Execute(&buf, struct {
		Title, Message, PublicHost string
		LocalPort                  int
	}{title, message, route.PublicHost(), route.LocalPort})
	return status, buf.Bytes()
}

package devplatform

import (
	"context"
	"crypto/sha256"
	"crypto/tls"
	"errors"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"sync"
	"time"
)

const (
	tunnelKeepAliveInterval = 30 * time.Second
	tunnelReadTimeout       = 75 * time.Second
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
		wantAuthDigest := ""
		if rule.TunnelAuth {
			tok, err := m.TunnelAccessToken(rule.Domain)
			if err != nil || tok == "" {
				return true
			}
			wantAuthDigest = tunnelAuthDigest(tok)
		}
		if w.PublicURL == "" || w.CustomDomain != rule.EffectiveTunnelDomain() || w.ScriptDigest != currentTunnelWorkerDigest() || w.AuthDigest != wantAuthDigest {
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
		DialContext:           upstreamDialer,
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
	workerStates := make(map[string]string, total)
	for name := range s.runners {
		st := s.statuses[name]
		switch st.phase {
		case "Connected":
			if len(st.conflicts) > 0 {
				conflicts = append(conflicts, st.conflicts...)
				workerStates[name] = "Conflict"
			} else {
				connected++
				workerStates[name] = "Connected"
			}
		case "AuthFailed":
			authFailed++
			workerStates[name] = "Reconnecting"
		case "Connecting":
			connecting++
			workerStates[name] = "Reconnecting"
		default:
			workerStates[name] = "Reconnecting"
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
		PID:          os.Getpid(),
		Phase:        phase,
		Connected:    connected,
		Total:        total,
		Conflicts:    conflicts,
		WorkerStates: workerStates,
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
			if err := ws.WritePing(nil); err != nil {
				sessionCancel()
				<-readErrCh
				return false, err
			}
		}
	}
}

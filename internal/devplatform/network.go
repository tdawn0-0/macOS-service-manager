package devplatform

import (
	"context"
	"crypto/sha256"
	"crypto/tls"
	"crypto/x509"
	"encoding/hex"
	"errors"
	"fmt"
	"howett.net/plist"
	"io"
	"devbase/internal/privhelper"
	"net"
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"strconv"
	"strings"
	"syscall"
	"time"
)

func proxyListenerPorts(sites []proxyRoute) []int {
	seen := map[int]bool{}
	var out []int
	for _, s := range sites {
		if !seen[s.Port] {
			seen[s.Port] = true
			out = append(out, s.Port)
		}
	}
	sort.Ints(out)
	return out
}
func privilegedPorts(sites []proxyRoute) bool {
	for _, s := range sites {
		if s.Port < 1024 {
			return true
		}
	}
	return false
}
func (m *Manager) socketJobPath() string {
	dir := m.SocketJobDir
	if dir == "" {
		dir = "/Library/LaunchDaemons"
	}
	return filepath.Join(dir, m.label("proxy-sockets")+".plist")
}
func (m *Manager) socketTarget() string { return "system/" + m.label("proxy-sockets") }
func (m *Manager) socketLoaded(ctx context.Context) bool {
	_, e := m.Run(ctx, "/bin/launchctl", "print", m.socketTarget())
	return e == nil
}
func (m *Manager) socketJob(sites []proxyRoute, exe, username string) ([]byte, error) {
	if username == "" || username == "root" || os.Getuid() == 0 {
		return nil, errors.New("the development proxy must run as a normal user")
	}
	if !filepath.IsAbs(exe) {
		return nil, errors.New("proxy executable must be absolute")
	}
	sockets := map[string]any{}
	for _, port := range proxyListenerPorts(sites) {
		sockets["HTTPS-"+strconv.Itoa(port)] = map[string]any{"SockServiceName": strconv.Itoa(port), "SockNodeName": "127.0.0.1", "SockFamily": "IPv4", "SockType": "stream", "SockProtocol": "TCP", "SockPassive": true}
	}
	job := map[string]any{"Label": m.label("proxy-sockets"), "UserName": username, "ProgramArguments": []string{exe, "--dev", "serve", m.Root}, "WorkingDirectory": m.Root, "EnvironmentVariables": map[string]string{"MSM_DEV_SOCKET_ACTIVATION": "1", "HOME": os.Getenv("HOME")}, "Sockets": sockets, "RunAtLoad": true, "ProcessType": "Background", "StandardOutPath": "/dev/null", "StandardErrorPath": "/dev/null"}
	return plist.MarshalIndent(job, plist.XMLFormat, "\t")
}
func equivalentPlists(a, b []byte) bool {
	var x, y map[string]any
	if _, e := plist.Unmarshal(a, &x); e != nil {
		return false
	}
	if _, e := plist.Unmarshal(b, &y); e != nil {
		return false
	}
	return reflect.DeepEqual(x, y)
}
func (m *Manager) installSocketJob(ctx context.Context, sites []proxyRoute) error {
	exe, e := m.proxyExecutable()
	if e != nil {
		return e
	}
	account, e := m.Run(ctx, "/usr/bin/id", "-un")
	if e != nil {
		return e
	}
	data, e := m.socketJob(sites, exe, strings.TrimSpace(account))
	if e != nil {
		return e
	}
	old, _ := os.ReadFile(m.socketJobPath())
	if equivalentPlists(old, data) && m.socketLoaded(ctx) {
		return nil
	}
	if e = ctx.Err(); e != nil {
		return e
	}
	hc, e := privhelper.Require()
	if e != nil {
		return e
	}
	m.emit("Configuring local HTTPS ports via privileged helper; the proxy runs as your user")
	return hc.InstallProxySocket(ctx, privhelper.ProxySocketInstallRequest{
		Plist:   data,
		JobPath: m.socketJobPath(),
		Target:  m.socketTarget(),
	})
}
func (m *Manager) stopSocketJob(ctx context.Context) error {
	_, e := os.Stat(m.socketJobPath())
	if errors.Is(e, os.ErrNotExist) {
		return nil
	}
	if e != nil {
		return e
	}
	if e = ctx.Err(); e != nil {
		return e
	}
	hc, e := privhelper.Require()
	if e != nil {
		return e
	}
	return hc.StopProxySocket(ctx, privhelper.ProxySocketStopRequest{
		JobPath: m.socketJobPath(),
		Target:  m.socketTarget(),
	})
}
func (m *Manager) stopProxy(ctx context.Context) error {
	if e := m.stopJob(ctx, "proxy"); e != nil {
		return e
	}
	return m.stopSocketJob(ctx)
}
func (m *Manager) startSocketProxy(ctx context.Context, sites, old []proxyRoute) (err error) {
	userWasLoaded, systemWasLoaded := m.loaded(ctx, "proxy"), m.socketLoaded(ctx)
	if err = m.preflightProxyPorts(ctx, sites, old, userWasLoaded || systemWasLoaded); err != nil {
		return err
	}
	oldJob, _ := os.ReadFile(m.socketJobPath())
	if err = m.stopJob(ctx, "proxy"); err != nil {
		return err
	}
	path := filepath.Join(m.Root, "proxy", "rules.json")
	if err = writeJSON(path, proxyConfigFromRoutes(sites)); err != nil {
		return err
	}
	defer func() {
		if err == nil {
			return
		}
		_ = writeJSON(path, proxyConfigFromRoutes(old))
		if systemWasLoaded && len(oldJob) > 0 {
			current, _ := os.ReadFile(m.socketJobPath())
			if equivalentPlists(current, oldJob) && m.socketLoaded(context.Background()) {
				return
			}
			var restoreErr error
			if hc, reqErr := privhelper.Require(); reqErr != nil {
				restoreErr = reqErr
			} else {
				restoreErr = hc.InstallProxySocket(context.Background(), privhelper.ProxySocketInstallRequest{
					Plist:   oldJob,
					JobPath: m.socketJobPath(),
					Target:  m.socketTarget(),
				})
			}
			if restoreErr != nil {
				err = fmt.Errorf("%w; restoring previous socket job: %v", err, restoreErr)
			}
		} else {
			_ = m.stopSocketJob(context.Background())
			if userWasLoaded {
				exe, e := m.executable()
				if e == nil {
					_, e = m.startJob(context.Background(), "proxy", []string{exe, "--dev", "serve", m.Root}, m.Root, nil, false)
				}
				if e != nil {
					err = fmt.Errorf("%w; restoring previous proxy: %v", err, e)
				}
			}
		}
	}()
	if err = m.installSocketJob(ctx, sites); err != nil {
		return err
	}
	for _, site := range sites {
		if err = m.verifyProxyTLS(ctx, site); err != nil {
			return err
		}
	}
	return nil
}
func (m *Manager) verifyProxyTLS(ctx context.Context, site proxyRoute) error {
	ca, _, e := m.ca()
	if e != nil {
		return e
	}
	roots := x509.NewCertPool()
	roots.AddCert(ca)
	ctx, cancel := context.WithTimeout(ctx, 20*time.Second)
	defer cancel()
	for {
		dialer := tls.Dialer{NetDialer: &net.Dialer{Timeout: time.Second}, Config: &tls.Config{RootCAs: roots, ServerName: site.Domain, MinVersion: tls.VersionTLS12}}
		attempt, done := context.WithTimeout(ctx, time.Second)
		conn, err := dialer.DialContext(attempt, "tcp", fmt.Sprintf("127.0.0.1:%d", site.Port))
		done()
		if err == nil {
			conn.Close()
			return nil
		}
		select {
		case <-ctx.Done():
			return fmt.Errorf("HTTPS proxy %s:%d did not complete a trusted TLS handshake: %w", site.Domain, site.Port, ctx.Err())
		case <-time.After(100 * time.Millisecond):
		}
	}
}

func (m *Manager) RestartProxy(ctx context.Context) error {
	return m.withState(ctx, func(s *State) error {
		sites := proxyRoutesFromState(s)
		if len(sites) == 0 {
			return errors.New("no HTTPS proxy rules configured")
		}
		if m.socketLoaded(ctx) {
			hc, e := privhelper.Require()
			if e != nil {
				return e
			}
			if e := hc.KickstartProxySocket(ctx, m.socketTarget()); e != nil {
				return e
			}
			for _, site := range sites {
				if e := m.verifyProxyTLS(ctx, site); e != nil {
					return e
				}
			}
			return nil
		}
		if e := m.stopJob(ctx, "proxy"); e != nil {
			return e
		}
		return m.startProxy(ctx, s)
	})
}
func (m *Manager) StopProxy(ctx context.Context) error {
	return m.withState(ctx, func(s *State) error { return m.stopProxy(ctx) })
}

// Keep the socket-activated entry point independent of build directories and
// application bundle moves. launchd always drops to this user's identity.
func (m *Manager) proxyExecutable() (string, error) {
	return m.helperExecutable("proxy")
}
func (m *Manager) helperExecutable(name string) (string, error) {
	source, e := m.executable()
	if e != nil {
		return "", e
	}
	dir := filepath.Join(m.Root, "helpers")
	if e = os.MkdirAll(dir, 0700); e != nil {
		return "", e
	}
	dest := filepath.Join(dir, name)
	if source == dest {
		return dest, nil
	}
	f, e := os.Open(source)
	if e != nil {
		return "", e
	}
	defer f.Close()
	data, e := io.ReadAll(io.LimitReader(f, (128<<20)+1))
	if e != nil {
		return "", e
	}
	if len(data) > 128<<20 {
		return "", errors.New("helper executable exceeds size limit")
	}
	sum := sha256.Sum256(data)
	if executable, bundled, err := m.bundleHelperExecutable(source, hex.EncodeToString(sum[:])); bundled {
		return executable, err
	}
	if !hasDigest(dest, hex.EncodeToString(sum[:])) {
		if e = atomicWrite(dest, data, 0700); e != nil {
			return "", e
		}
	}
	return dest, nil
}

// VerifySocketLifecycle delegates the privileged socket lifecycle verification
// to the privileged helper. Its cleanup restores the existing job even if a check fails.
func (m *Manager) VerifySocketLifecycle(ctx context.Context) error {
	return m.withState(ctx, func(s *State) error {
		sites := proxyRoutesFromState(s)
		if !m.socketLoaded(ctx) || len(sites) == 0 {
			return errors.New("start the socket-activated HTTPS proxy before running this test")
		}
		hc, err := privhelper.Require()
		if err != nil {
			return err
		}
		if err := m.verifySocketLifecycleViaHelper(ctx, hc, sites); err != nil {
			return err
		}
		for _, site := range sites {
			if e := m.verifyProxyTLS(ctx, site); e != nil {
				return e
			}
		}
		return nil
	})
}

func (m *Manager) verifySocketLifecycleViaHelper(ctx context.Context, hc *privhelper.Client, sites []proxyRoute) error {
	target, jobPath := m.socketTarget(), m.socketJobPath()
	printResp, _ := hc.PrintProxySocket(ctx, target)
	pid := parseLaunchctlPID(string(printResp.Output))

	restored := false
	defer func() {
		if !restored {
			_ = hc.LaunchdControl(context.Background(), [][]string{{"bootstrap", "system", jobPath}})
		}
	}()

	if err := hc.LaunchdControl(ctx, [][]string{{"bootout", target}}); err != nil {
		return err
	}
	if pid > 0 {
		deadline := time.Now().Add(10 * time.Second)
		for syscall.Kill(pid, 0) == nil {
			if time.Now().After(deadline) {
				return errors.New("proxy did not stop")
			}
			select {
			case <-ctx.Done():
				return ctx.Err()
			case <-time.After(100 * time.Millisecond):
			}
		}
	}
	for _, port := range proxyListenerPorts(sites) {
		conn, err := net.DialTimeout("tcp4", fmt.Sprintf("127.0.0.1:%d", port), time.Second)
		if err == nil {
			conn.Close()
			return errors.New("HTTPS port was not released")
		}
	}
	if err := hc.LaunchdControl(ctx, [][]string{
		{"bootstrap", "system", jobPath},
		{"kickstart", "-k", target},
	}); err != nil {
		return err
	}
	restored = true
	return nil
}

func parseLaunchctlPID(out string) int {
	for _, line := range strings.Split(out, "\n") {
		fields := strings.Fields(line)
		if len(fields) == 3 && fields[0] == "pid" && fields[1] == "=" {
			if pid, err := strconv.Atoi(fields[2]); err == nil && pid > 0 {
				return pid
			}
		}
	}
	return 0
}

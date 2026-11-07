package devplatform

import (
	"context"
	"crypto/sha256"
	"crypto/tls"
	"crypto/x509"
	"encoding/base64"
	"encoding/hex"
	"errors"
	"fmt"
	"howett.net/plist"
	"io"
	"macos-service-manager/internal/system"
	"net"
	"os"
	"path/filepath"
	"reflect"
	"sort"
	"strconv"
	"strings"
	"time"
)

func sitePorts(sites []Site) []int {
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
func privilegedPorts(sites []Site) bool {
	for _, s := range sites {
		if s.Port < 1024 {
			return true
		}
	}
	return false
}
func (m *Manager) socketJobPath() string {
	return filepath.Join("/Library/LaunchDaemons", m.label("proxy-sockets")+".plist")
}
func (m *Manager) socketTarget() string { return "system/" + m.label("proxy-sockets") }
func (m *Manager) socketLoaded(ctx context.Context) bool {
	_, e := m.Run(ctx, "/bin/launchctl", "print", m.socketTarget())
	return e == nil
}
func (m *Manager) socketJob(sites []Site, exe, username string) ([]byte, error) {
	if username == "" || username == "root" || os.Getuid() == 0 {
		return nil, errors.New("the development proxy must run as a normal user")
	}
	if !filepath.IsAbs(exe) {
		return nil, errors.New("proxy executable must be absolute")
	}
	sockets := map[string]any{}
	for _, port := range sitePorts(sites) {
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
func socketInstallScript(data []byte, path, target string) string {
	// Embed immutable bytes in the authorized command. No user-writable staging
	// file is read as root after the authorization dialog.
	encoded := base64.StdEncoding.EncodeToString(data)
	return "set -eu; tmp=$(/usr/bin/mktemp /Library/LaunchDaemons/.msm-socket.XXXXXX); trap '/bin/rm -f \"$tmp\"' EXIT; /usr/bin/printf '%s' " + system.ShellQuote(encoded) + " | /usr/bin/base64 -D > \"$tmp\"; /usr/bin/plutil -lint \"$tmp\" >/dev/null; /usr/sbin/chown root:wheel \"$tmp\"; /bin/chmod 644 \"$tmp\"; if /bin/launchctl print " + system.ShellQuote(target) + " >/dev/null 2>&1; then /bin/launchctl bootout " + system.ShellQuote(target) + "; fi; /bin/mv -f \"$tmp\" " + system.ShellQuote(path) + "; /bin/launchctl bootstrap system " + system.ShellQuote(path)
}
func (m *Manager) installSocketJob(ctx context.Context, sites []Site) error {
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
	m.emit("Authorizing local HTTPS ports; the proxy runs as your user")
	_, e = system.RunAdmin("/bin/sh", "-c", socketInstallScript(data, m.socketJobPath(), m.socketTarget()))
	return e
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
	script := "set -eu; if /bin/launchctl print " + system.ShellQuote(m.socketTarget()) + " >/dev/null 2>&1; then /bin/launchctl bootout " + system.ShellQuote(m.socketTarget()) + "; fi; /bin/rm -f " + system.ShellQuote(m.socketJobPath())
	_, e = system.RunAdmin("/bin/sh", "-c", script)
	return e
}
func (m *Manager) stopProxy(ctx context.Context) error {
	if e := m.stopJob(ctx, "proxy"); e != nil {
		return e
	}
	return m.stopSocketJob(ctx)
}
func (m *Manager) startSocketProxy(ctx context.Context, sites, old []Site) (err error) {
	userWasLoaded, systemWasLoaded := m.loaded(ctx, "proxy"), m.socketLoaded(ctx)
	if err = preflightProxyPorts(sites, old, userWasLoaded || systemWasLoaded); err != nil {
		return err
	}
	oldJob, _ := os.ReadFile(m.socketJobPath())
	if err = m.stopJob(ctx, "proxy"); err != nil {
		return err
	}
	path := filepath.Join(m.Root, "proxy", "sites.json")
	if err = writeJSON(path, sites); err != nil {
		return err
	}
	defer func() {
		if err == nil {
			return
		}
		_ = writeJSON(path, old)
		if systemWasLoaded && len(oldJob) > 0 {
			current, _ := os.ReadFile(m.socketJobPath())
			if equivalentPlists(current, oldJob) && m.socketLoaded(context.Background()) {
				return
			}
			if _, restoreErr := system.RunAdmin("/bin/sh", "-c", socketInstallScript(oldJob, m.socketJobPath(), m.socketTarget())); restoreErr != nil {
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
		if err = m.verifySiteTLS(ctx, site); err != nil {
			return err
		}
	}
	return nil
}
func (m *Manager) verifySiteTLS(ctx context.Context, site Site) error {
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
		sites := allSites(s)
		if len(sites) == 0 {
			return errors.New("no HTTPS sites configured")
		}
		if m.socketLoaded(ctx) {
			if _, e := system.RunAdmin("/bin/launchctl", "kickstart", "-k", m.socketTarget()); e != nil {
				return e
			}
			for _, site := range sites {
				if e := m.verifySiteTLS(ctx, site); e != nil {
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

// VerifySocketLifecycle batches the complete privileged test into one macOS
// authorization. Its trap restores the existing job even if a check fails.
func (m *Manager) VerifySocketLifecycle(ctx context.Context) error {
	return m.withState(ctx, func(s *State) error {
		sites := allSites(s)
		if !m.socketLoaded(ctx) || len(sites) == 0 {
			return errors.New("start the socket-activated HTTPS proxy before running this test")
		}
		target, path := system.ShellQuote(m.socketTarget()), system.ShellQuote(m.socketJobPath())
		script := "set -eu; restore(){ /bin/launchctl print " + target + " >/dev/null 2>&1 || /bin/launchctl bootstrap system " + path + "; }; trap restore EXIT; pid=$(/bin/launchctl print " + target + " | /usr/bin/awk '$1 == \"pid\" && $2 == \"=\" {print $3; exit}'); /bin/launchctl bootout " + target + "; count=0; if [ -n \"$pid\" ]; then while /bin/kill -0 \"$pid\" 2>/dev/null; do count=$((count+1)); if [ \"$count\" -gt 100 ]; then echo 'proxy did not stop' >&2; exit 1; fi; /bin/sleep 0.1; done; fi; "
		for _, port := range sitePorts(sites) {
			script += fmt.Sprintf("if /usr/bin/nc -z -G 1 127.0.0.1 %d >/dev/null 2>&1; then echo 'HTTPS port was not released' >&2; exit 1; fi; ", port)
		}
		script += "/bin/launchctl bootstrap system " + path + "; /bin/launchctl kickstart -k " + target
		if _, e := system.RunAdmin("/bin/sh", "-c", script); e != nil {
			return e
		}
		for _, site := range sites {
			if e := m.verifySiteTLS(ctx, site); e != nil {
				return e
			}
		}
		return nil
	})
}

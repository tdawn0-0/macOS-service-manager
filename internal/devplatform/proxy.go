package devplatform

import (
	"context"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/sha1"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"macos-service-manager/internal/system"
	"math/big"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"
)

func allSites(s *State) []Site {
	var sites []Site
	for _, site := range s.Sites {
		if !site.Disabled && !site.NeedsReview {
			sites = append(sites, site)
		}
	}
	for _, p := range s.Projects {
		for _, site := range p.Sites {
			if !site.Disabled && !site.NeedsReview {
				sites = append(sites, site)
			}
		}
	}
	sort.Slice(sites, func(i, j int) bool { return sites[i].Domain < sites[j].Domain })
	return sites
}
func validateSites(sites []Site) error {
	seen := map[string]bool{}
	for _, s := range sites {
		if e := s.Validate(); e != nil {
			return e
		}
		key := fmt.Sprintf("%s:%d", s.Domain, s.Port)
		if seen[key] {
			return fmt.Errorf("duplicate site %s", key)
		}
		seen[key] = true
	}
	return nil
}
func serial() *big.Int { n, _ := rand.Int(rand.Reader, new(big.Int).Lsh(big.NewInt(1), 128)); return n }
func (m *Manager) ca() (*x509.Certificate, *ecdsa.PrivateKey, error) {
	return m.loadOrCreateAuthority()
}
func (m *Manager) leaf(domain string) (tls.Certificate, error) {
	ca, key, e := m.ca()
	if e != nil {
		return tls.Certificate{}, e
	}
	k, e := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if e != nil {
		return tls.Certificate{}, e
	}
	now := time.Now()
	t := &x509.Certificate{SerialNumber: serial(), Subject: pkix.Name{CommonName: domain}, DNSNames: []string{domain}, NotBefore: now.Add(-time.Hour), NotAfter: now.AddDate(0, 1, 0), KeyUsage: x509.KeyUsageDigitalSignature, ExtKeyUsage: []x509.ExtKeyUsage{x509.ExtKeyUsageServerAuth}}
	if t.NotAfter.After(ca.NotAfter) {
		t.NotAfter = ca.NotAfter
	}
	if t.NotBefore.Before(ca.NotBefore) {
		t.NotBefore = ca.NotBefore
	}
	der, e := x509.CreateCertificate(rand.Reader, t, ca, &k.PublicKey, key)
	if e != nil {
		return tls.Certificate{}, e
	}
	return tls.Certificate{Certificate: [][]byte{der, ca.Raw}, PrivateKey: k, Leaf: t}, nil
}
func (m *Manager) SaveSite(ctx context.Context, site Site) error {
	if e := site.Validate(); e != nil {
		return e
	}
	return m.withState(ctx, func(s *State) error {
		s.Sites[site.ID] = site
		if e := validateSites(allSites(s)); e != nil {
			return e
		}
		return nil
	})
}
func (m *Manager) startProxy(ctx context.Context, s *State) error {
	var keys []resourceKey
	for _, port := range sitePorts(allSites(s)) {
		keys = append(keys, resourceKey{"TCP port", fmt.Sprint(port)})
	}
	release, leaseErr := m.resourceLeases(ctx, keys...)
	if leaseErr != nil {
		return leaseErr
	}
	defer release()
	sites := allSites(s)
	if e := validateSites(sites); e != nil {
		return e
	}
	if len(sites) == 0 {
		return m.stopProxy(ctx)
	}
	if _, _, e := m.ca(); e != nil {
		return e
	}
	old, _ := m.proxySites()
	if privilegedPorts(sites) {
		return m.startSocketProxy(ctx, sites, old)
	}
	loaded := m.loaded(ctx, "proxy")
	if e := preflightProxyPorts(sites, old, loaded || m.socketLoaded(ctx)); e != nil {
		return e
	}
	if e := m.stopSocketJob(ctx); e != nil {
		return e
	}
	ports := func(ss []Site) map[int]bool {
		out := map[int]bool{}
		for _, site := range ss {
			out[site.Port] = true
		}
		return out
	}
	before, after := ports(old), ports(sites)
	changed := len(before) != len(after)
	for p := range after {
		if !before[p] {
			changed = true
		}
		if !loaded || !before[p] {
			l, e := net.Listen("tcp4", fmt.Sprintf("127.0.0.1:%d", p))
			if e != nil {
				return &PortUnavailableError{Port: p, Role: "HTTPS", Err: e}
			}
			l.Close()
		}
	}
	exe, e := m.executable()
	if e != nil {
		return e
	}
	args := []string{exe, "--dev", "serve", m.Root}
	if loaded && changed {
		if e = m.stopJob(ctx, "proxy"); e != nil {
			return e
		}
	}
	path := filepath.Join(m.Root, "proxy", "sites.json")
	if e = writeJSON(path, sites); e != nil {
		return e
	}
	fresh, e := m.startJob(ctx, "proxy", args, m.Root, nil, false)
	if e == nil && fresh {
		for p := range after {
			if e = waitHealth(ctx, fmt.Sprintf("tcp://127.0.0.1:%d", p)); e != nil {
				break
			}
		}
	}
	if e == nil {
		for _, site := range sites {
			if e = m.verifySiteTLS(ctx, site); e != nil {
				break
			}
		}
	}
	if e != nil {
		_ = m.stopJob(context.Background(), "proxy")
		_ = writeJSON(path, old)
		if loaded {
			_, _ = m.startJob(context.Background(), "proxy", args, m.Root, nil, false)
		}
	}
	return e
}
func (m *Manager) ApplyProxy(ctx context.Context) error {
	return m.withState(ctx, func(s *State) error { return m.startProxy(ctx, s) })
}

// ServeProxy reloads validated routing snapshots and certificates without exposing
// an unauthenticated administrative TCP endpoint. Unknown hosts are never proxied.
func (m *Manager) ServeProxy(ctx context.Context) error {
	if e := m.init(); e != nil {
		return e
	}
	sites, e := m.proxySites()
	if e != nil {
		return e
	}
	if e = validateSites(sites); e != nil {
		return e
	}
	routes := newLiveProxyRoutes(m, sites)
	defer routes.close()
	ports := map[int]bool{}
	for _, site := range sites {
		ports[site.Port] = true
	}
	if len(ports) == 0 {
		return errors.New("no sites configured")
	}
	var servers []*http.Server
	var listeners []net.Listener
	defer func() {
		for _, server := range servers {
			_ = server.Close()
		}
		for _, l := range listeners {
			_ = l.Close()
		}
	}()
	errc := make(chan error, len(ports))
	for port := range ports {
		port := port
		var l net.Listener
		var e error
		if os.Getenv("MSM_DEV_SOCKET_ACTIVATION") == "1" {
			l, e = activatedListener(fmt.Sprintf("HTTPS-%d", port))
		} else {
			l, e = net.Listen("tcp4", fmt.Sprintf("127.0.0.1:%d", port))
		}
		if e != nil {
			return e
		}
		listeners = append(listeners, l)
		server := &http.Server{ReadHeaderTimeout: 10 * time.Second, Handler: http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			current := routes.snapshot()
			host := r.Host
			if h, _, e := net.SplitHostPort(host); e == nil {
				host = h
			}
			if route := current.routes[net.JoinHostPort(host, fmt.Sprint(port))]; route != nil {
				route.proxy.ServeHTTP(w, r)
				return
			}
			http.NotFound(w, r)
		}), TLSConfig: &tls.Config{MinVersion: tls.VersionTLS12, GetCertificate: func(hello *tls.ClientHelloInfo) (*tls.Certificate, error) {
			current := routes.snapshot()
			if route := current.routes[net.JoinHostPort(hello.ServerName, fmt.Sprint(port))]; route != nil {
				cert, e := m.leaf(route.site.Domain)
				return &cert, e
			}
			return nil, errors.New("unknown local domain")
		}}}
		servers = append(servers, server)
		go func() { errc <- server.Serve(tls.NewListener(l, server.TLSConfig)) }()
	}
	select {
	case <-ctx.Done():
		return nil
	case e := <-errc:
		return e
	}
}

const hostsBegin = "# BEGIN Service Manager Development"
const hostsEnd = "# END Service Manager Development"

func HostsContent(old string, sites []Site) (string, error) {
	if e := validateSites(sites); e != nil {
		return "", e
	}
	start, end := strings.Index(old, hostsBegin), strings.Index(old, hostsEnd)
	if (start < 0) != (end < 0) || end < start {
		return "", errors.New("malformed managed hosts block")
	}
	if start >= 0 {
		old = old[:start] + old[end+len(hostsEnd):]
	}
	domains := map[string]bool{}
	for _, s := range sites {
		domains[s.Domain] = true
	}
	for _, line := range strings.Split(old, "\n") {
		fields := strings.Fields(strings.SplitN(line, "#", 2)[0])
		if len(fields) < 2 {
			continue
		}
		for _, name := range fields[1:] {
			if domains[name] {
				return "", fmt.Errorf("domain %s already appears outside managed block", name)
			}
		}
	}
	names := []string{}
	for d := range domains {
		names = append(names, d)
	}
	sort.Strings(names)
	out := strings.TrimRight(old, "\n") + "\n"
	if len(names) > 0 {
		out += hostsBegin + "\n"
		for _, d := range names {
			out += "127.0.0.1 " + d + "\n"
		}
		out += hostsEnd + "\n"
	}
	return out, nil
}
func (m *Manager) ConfigureTrust(ctx context.Context, remove bool) error {
	return m.withState(ctx, func(*State) error {
		if !remove {
			if _, _, err := m.ca(); err != nil {
				return err
			}
		}
		return m.withAuthorityLock(ctx, func() error {
			active, err := m.readActiveAuthority()
			if err != nil {
				return err
			}
			if remove && active.cert == nil && active.info.Status == "Not created" {
				return nil
			}
			return m.configureAuthorityTrust(ctx, active, remove)
		})
	})
}
func (m *Manager) ConfigureHosts(ctx context.Context) error {
	return m.withState(ctx, func(s *State) error {
		old, e := os.ReadFile("/etc/hosts")
		if e != nil {
			return e
		}
		updated, e := HostsContent(string(old), allSites(s))
		if e != nil {
			return e
		}
		if updated == string(old) {
			return nil
		}
		if e = atomicWrite(filepath.Join(m.Root, "hosts.backup"), old, 0600); e != nil {
			return e
		}
		script, e := hostsInstallScript(old, []byte(updated))
		if e != nil {
			return e
		}
		_, e = system.RunAdmin("/bin/sh", "-c", script)
		return e
	})
}

// hostsInstallScript freezes the reviewed bytes before the authorization dialog.
func hostsInstallScript(old, updated []byte) (string, error) {
	if len(updated) > 64<<10 {
		return "", errors.New("hosts configuration exceeds authorized command size limit")
	}
	sum := sha1.Sum(old)
	check := "[ -f /etc/hosts ] && [ ! -L /etc/hosts ] && test \"$(/usr/bin/shasum /etc/hosts | /usr/bin/cut -d ' ' -f 1)\" = " + system.ShellQuote(hex.EncodeToString(sum[:]))
	return check + " && tmp=$(/usr/bin/mktemp /etc/.msm-hosts.XXXXXX) && trap '/bin/rm -f \"$tmp\"' EXIT && /usr/bin/printf '%s' " + system.ShellQuote(base64.StdEncoding.EncodeToString(updated)) + " | /usr/bin/base64 -D > \"$tmp\" && set -- $(/usr/bin/stat -f '%u %g %Lp' /etc/hosts) && /usr/sbin/chown \"$1:$2\" \"$tmp\" && /bin/chmod \"$3\" \"$tmp\" && " + check + " && /bin/mv -f \"$tmp\" /etc/hosts && /usr/bin/dscacheutil -flushcache", nil
}

func (m *Manager) proxySites() ([]Site, error) {
	var sites []Site
	b, e := os.ReadFile(filepath.Join(m.Root, "proxy", "sites.json"))
	if e != nil {
		return nil, e
	}
	if e = json.Unmarshal(b, &sites); e != nil {
		return nil, e
	}
	return sites, validateSites(sites)
}

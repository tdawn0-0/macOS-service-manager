package devplatform

import (
	"bytes"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"time"
)

var (
	tunnelClientIDRE      = regexp.MustCompile(`^[a-z0-9][a-z0-9-]{1,39}$`)
	cloudflareAccountIDRE = regexp.MustCompile(`^[a-zA-Z0-9_-]{8,64}$`)
)

func normalizeTunnelClientID(raw string) (string, error) {
	id := strings.ToLower(strings.TrimSpace(raw))
	if id == "" {
		return "", nil
	}
	if !tunnelClientIDRE.MatchString(id) {
		return "", errors.New("client ID must be 2–40 lowercase letters, digits, or hyphens")
	}
	return id, nil
}

func normalizeCloudflareAccountID(raw string) (string, error) {
	id := strings.TrimSpace(raw)
	if id == "" {
		return "", nil
	}
	if !cloudflareAccountIDRE.MatchString(id) {
		return "", errors.New("Cloudflare account ID must be 8–64 alphanumeric characters, hyphens, or underscores")
	}
	return id, nil
}

func validateTunnelState(s *State) error {
	nc, err := normalizeTunnelClientID(s.TunnelClientID)
	if err != nil {
		return err
	}
	if nc != s.TunnelClientID {
		return errors.New("tunnel client ID is not normalized")
	}
	na, err := normalizeCloudflareAccountID(s.CloudflareAccountID)
	if err != nil {
		return err
	}
	if na != s.CloudflareAccountID {
		return errors.New("Cloudflare account ID is not normalized")
	}
	if sub := strings.TrimSpace(s.CloudflareSubdomain); sub != "" {
		if strings.ToLower(sub) != sub || !proxyDomainPart.MatchString(sub) {
			return errors.New("Cloudflare workers.dev subdomain is invalid")
		}
	}
	for name, worker := range s.TunnelWorkers {
		if !strings.HasPrefix(name, "devbase-") || worker.ScriptName != name || !proxyDomainPart.MatchString(name) {
			return fmt.Errorf("invalid deployed tunnel worker %q", name)
		}
		if worker.PublicURL == "" || worker.LocalPort < 1 || worker.LocalPort > 65535 {
			return fmt.Errorf("incomplete tunnel worker state %q", name)
		}
	}
	return nil
}

func validateTunnelSecret(secret string) (string, error) {
	trimmed := strings.TrimSpace(secret)
	if len(trimmed) < 16 || len(trimmed) > 512 {
		return "", errors.New("tunnel secret must be 16–512 characters")
	}
	if strings.ContainsAny(trimmed, "\r\n") {
		return "", errors.New("tunnel secret must be a single line")
	}
	return trimmed, nil
}

func validateCloudflareToken(token string) (string, error) {
	trimmed := strings.TrimSpace(token)
	if len(trimmed) < 10 || len(trimmed) > 512 {
		return "", errors.New("Cloudflare API token must be 10–512 characters")
	}
	if strings.ContainsAny(trimmed, " \t\r\n") {
		return "", errors.New("Cloudflare API token cannot contain whitespace")
	}
	return trimmed, nil
}

func (m *Manager) cloudflareTokenPath() string {
	return filepath.Join(m.Root, "proxy", "cloudflare.key")
}

func (m *Manager) tunnelKeyPath() string {
	return filepath.Join(m.Root, "proxy", "tunnel.key")
}

func (m *Manager) tunnelConfigPath() string {
	return filepath.Join(m.Root, "proxy", "tunnel.json")
}

func (m *Manager) tunnelStatusPath() string {
	return filepath.Join(m.Root, "proxy", "tunnel-status.json")
}

// HasCloudflareToken checks whether a Cloudflare API token file is stored
// without exposing its contents.
func (m *Manager) HasCloudflareToken() bool {
	info, err := os.Stat(m.cloudflareTokenPath())
	if err != nil || !info.Mode().IsRegular() {
		return false
	}
	return info.Size() >= 10 && info.Size() <= 4096
}

// ReadCloudflareToken reads the Cloudflare API token from 0600 storage.
func (m *Manager) ReadCloudflareToken() (string, error) {
	return readSecretFile(m.cloudflareTokenPath(), validateCloudflareToken, "Cloudflare API token")
}

func (m *Manager) writeCloudflareToken(token string) error {
	cleaned, err := validateCloudflareToken(token)
	if err != nil {
		return err
	}
	return atomicWrite(m.cloudflareTokenPath(), []byte(cleaned), 0600)
}

// ReadTunnelSecret reads the internal Worker shared HMAC secret from disk.
func (m *Manager) ReadTunnelSecret() (string, error) {
	return readSecretFile(m.tunnelKeyPath(), validateTunnelSecret, "tunnel secret")
}

func readSecretFile(path string, validate func(string) (string, error), label string) (string, error) {
	f, err := os.Open(path)
	if err != nil {
		return "", err
	}
	defer f.Close()
	b, err := io.ReadAll(io.LimitReader(f, 4097))
	if err != nil {
		return "", err
	}
	if len(b) > 4096 {
		return "", fmt.Errorf("%s file exceeds size limit", label)
	}
	return validate(string(b))
}

func (m *Manager) writeTunnelSecret(secret string) error {
	cleaned, err := validateTunnelSecret(secret)
	if err != nil {
		return err
	}
	return atomicWrite(m.tunnelKeyPath(), []byte(cleaned), 0600)
}

func (m *Manager) ensureTunnelSecret() (string, error) {
	if secret, err := m.ReadTunnelSecret(); err == nil {
		return secret, nil
	}
	var raw [32]byte
	if _, err := rand.Read(raw[:]); err != nil {
		return "", err
	}
	secret := hex.EncodeToString(raw[:])
	if err := m.writeTunnelSecret(secret); err != nil {
		return "", err
	}
	return secret, nil
}

// TunnelRoute maps a single deployed Cloudflare Worker to a local loopback port.
type TunnelRoute struct {
	WorkerName  string `json:"workerName"`
	PublicURL   string `json:"publicUrl"`
	ConnectURL  string `json:"connectUrl,omitempty"`
	LocalDomain string `json:"localDomain"`
	LocalPort   int    `json:"localPort"`
}

func (r TunnelRoute) PublicHost() string {
	u, err := url.Parse(r.PublicURL)
	if err != nil {
		return ""
	}
	return strings.ToLower(u.Host)
}

func (r TunnelRoute) WebSocketURL() (string, error) {
	base := strings.TrimSpace(r.ConnectURL)
	if base == "" {
		base = strings.TrimSpace(r.PublicURL)
	}
	u, err := url.Parse(base)
	if err != nil || u.Host == "" {
		return "", errors.New("invalid tunnel route URL")
	}
	switch strings.ToLower(u.Scheme) {
	case "https":
		u.Scheme = "wss"
	case "http":
		u.Scheme = "ws"
	case "wss", "ws":
	default:
		return "", errors.New("unsupported tunnel URL scheme")
	}
	u.Path = "/__devbase/connect"
	u.RawQuery = ""
	u.Fragment = ""
	return u.String(), nil
}

type tunnelPublishedConfig struct {
	SchemaVersion int           `json:"schemaVersion"`
	ClientID      string        `json:"clientId"`
	Routes        []TunnelRoute `json:"routes"`
}

func tunnelConfigFromState(s *State) (tunnelPublishedConfig, error) {
	clientID := s.TunnelClientID
	if clientID == "" {
		clientID = "mac-" + randomID()[:8]
	}
	routes := make([]TunnelRoute, 0, len(s.ProxyRules))
	for _, rule := range s.ProxyRules {
		if !rule.Tunnel {
			continue
		}
		workerName := rule.ResolvedTunnelWorker()
		publicURL := ""
		if w, ok := s.TunnelWorkers[workerName]; ok && w.PublicURL != "" {
			publicURL = w.PublicURL
		} else if rule.TunnelURL != "" {
			publicURL = rule.TunnelURL
		}
		if publicURL == "" {
			return tunnelPublishedConfig{}, fmt.Errorf("deploy Cloudflare Worker before starting tunnel for %s", rule.Domain)
		}
		routes = append(routes, TunnelRoute{
			WorkerName:  workerName,
			PublicURL:   publicURL,
			ConnectURL:  publicURL,
			LocalDomain: rule.Domain,
			LocalPort:   rule.LocalPort,
		})
	}
	sort.Slice(routes, func(i, j int) bool { return routes[i].WorkerName < routes[j].WorkerName })
	return tunnelPublishedConfig{
		SchemaVersion: 1,
		ClientID:      clientID,
		Routes:        routes,
	}, nil
}

func (m *Manager) tunnelPublishedExists() bool {
	_, err := os.Stat(m.tunnelConfigPath())
	return err == nil
}

func (m *Manager) writePublishedTunnelConfig(s *State) error {
	cfg, err := tunnelConfigFromState(s)
	if err != nil {
		return err
	}
	if s.TunnelClientID == "" {
		s.TunnelClientID = cfg.ClientID
	}
	return writeJSON(m.tunnelConfigPath(), cfg)
}

func readPublishedTunnelConfig(root string) (tunnelPublishedConfig, error) {
	b, err := os.ReadFile(filepath.Join(root, "proxy", "tunnel.json"))
	if err != nil {
		return tunnelPublishedConfig{}, err
	}
	var cfg tunnelPublishedConfig
	dec := json.NewDecoder(bytes.NewReader(b))
	dec.DisallowUnknownFields()
	if err = dec.Decode(&cfg); err != nil {
		return cfg, err
	}
	var extra any
	if err = dec.Decode(&extra); err != io.EOF {
		return cfg, errors.New("tunnel snapshot has trailing data")
	}
	if cfg.SchemaVersion != 1 {
		return cfg, errors.New("unsupported tunnel snapshot schema")
	}
	if cfg.ClientID == "" {
		return cfg, errors.New("tunnel snapshot missing client ID")
	}
	if _, err = normalizeTunnelClientID(cfg.ClientID); err != nil {
		return cfg, err
	}
	if cfg.Routes == nil {
		cfg.Routes = []TunnelRoute{}
	}
	seen := map[string]bool{}
	for _, r := range cfg.Routes {
		if !strings.HasPrefix(r.WorkerName, "devbase-") || r.PublicURL == "" || r.LocalPort < 1 || r.LocalPort > 65535 {
			return cfg, errors.New("invalid tunnel route in snapshot")
		}
		if seen[r.WorkerName] {
			return cfg, fmt.Errorf("duplicate tunnel route %s", r.WorkerName)
		}
		seen[r.WorkerName] = true
		if _, err = r.WebSocketURL(); err != nil {
			return cfg, err
		}
	}
	return cfg, nil
}

// TunnelConflict describes a hostname or Worker already claimed by another active client.
type TunnelConflict struct {
	Host  string `json:"host"`
	Owner string `json:"owner"`
}

// TunnelStatusSnapshot is written atomically by the background tunnel process
// so the UI and diagnostics can observe live connection state without IPC.
type TunnelStatusSnapshot struct {
	PID       int              `json:"pid"`
	Phase     string           `json:"phase"`
	Connected int              `json:"connected"`
	Total     int              `json:"total"`
	Conflicts []TunnelConflict `json:"conflicts,omitempty"`
	UpdatedAt time.Time        `json:"updatedAt"`
}

func (m *Manager) writeTunnelStatus(status TunnelStatusSnapshot) {
	status.UpdatedAt = time.Now().UTC()
	_ = writeJSON(m.tunnelStatusPath(), status)
}

func (m *Manager) readTunnelStatus() (TunnelStatusSnapshot, bool) {
	b, err := os.ReadFile(m.tunnelStatusPath())
	if err != nil {
		return TunnelStatusSnapshot{}, false
	}
	var st TunnelStatusSnapshot
	if err := json.Unmarshal(b, &st); err != nil {
		return TunnelStatusSnapshot{}, false
	}
	return st, true
}

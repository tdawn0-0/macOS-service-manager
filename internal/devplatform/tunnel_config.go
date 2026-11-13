package devplatform

import (
	"bytes"
	"context"
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

var tunnelClientIDRE = regexp.MustCompile(`^[a-z0-9][a-z0-9-]{1,39}$`)

func normalizeTunnelDomain(raw string) (string, error) {
	d := strings.ToLower(strings.TrimSpace(raw))
	d = strings.TrimPrefix(d, "*.")
	d = strings.TrimPrefix(d, ".")
	d = strings.TrimSuffix(d, ".")
	if d == "" {
		return "", nil
	}
	if len(d) > 240 {
		return "", errors.New("tunnel domain is too long")
	}
	parts := strings.Split(d, ".")
	if len(parts) < 2 {
		return "", errors.New("tunnel domain must be a full domain such as dev.example.com")
	}
	for _, part := range parts {
		if !proxyDomainPart.MatchString(part) {
			return "", fmt.Errorf("invalid tunnel domain label %q", part)
		}
	}
	return d, nil
}

func normalizeTunnelEndpoint(raw string) (string, error) {
	ep := strings.TrimSpace(raw)
	if ep == "" {
		return "", nil
	}
	u, err := url.Parse(ep)
	if err != nil || u.Host == "" || u.User != nil || u.RawQuery != "" || u.Fragment != "" {
		return "", errors.New("worker URL must be a valid https:// URL without query or credentials")
	}
	scheme := strings.ToLower(u.Scheme)
	host := strings.ToLower(u.Hostname())
	switch scheme {
	case "https":
	case "http":
		if host != "127.0.0.1" && host != "localhost" {
			return "", errors.New("worker URL must use https://")
		}
	default:
		return "", errors.New("worker URL must use https://")
	}
	u.Scheme = scheme
	u.Host = strings.ToLower(u.Host)
	u.Path = strings.TrimRight(u.Path, "/")
	return u.String(), nil
}

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

func validateTunnelSettings(domain, endpoint, clientID string) error {
	nd, err := normalizeTunnelDomain(domain)
	if err != nil {
		return err
	}
	if nd != domain {
		return errors.New("tunnel domain is not normalized")
	}
	ne, err := normalizeTunnelEndpoint(endpoint)
	if err != nil {
		return err
	}
	if ne != endpoint {
		return errors.New("tunnel endpoint is not normalized")
	}
	nc, err := normalizeTunnelClientID(clientID)
	if err != nil {
		return err
	}
	if nc != clientID {
		return errors.New("tunnel client ID is not normalized")
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

func (m *Manager) tunnelKeyPath() string {
	return filepath.Join(m.Root, "proxy", "tunnel.key")
}

func (m *Manager) tunnelConfigPath() string {
	return filepath.Join(m.Root, "proxy", "tunnel.json")
}

func (m *Manager) tunnelStatusPath() string {
	return filepath.Join(m.Root, "proxy", "tunnel-status.json")
}

// HasTunnelSecret checks whether a valid tunnel secret file is stored without
// exposing its bytes.
func (m *Manager) HasTunnelSecret() bool {
	info, err := os.Stat(m.tunnelKeyPath())
	if err != nil || !info.Mode().IsRegular() {
		return false
	}
	return info.Size() >= 16 && info.Size() <= 4096
}

// ReadTunnelSecret reads the local Cloudflare Worker shared secret from disk.
func (m *Manager) ReadTunnelSecret() (string, error) {
	f, err := os.Open(m.tunnelKeyPath())
	if err != nil {
		return "", err
	}
	defer f.Close()
	b, err := io.ReadAll(io.LimitReader(f, 4097))
	if err != nil {
		return "", err
	}
	if len(b) > 4096 {
		return "", errors.New("tunnel secret file exceeds size limit")
	}
	return validateTunnelSecret(string(b))
}

func (m *Manager) writeTunnelSecret(secret string) error {
	cleaned, err := validateTunnelSecret(secret)
	if err != nil {
		return err
	}
	return atomicWrite(m.tunnelKeyPath(), []byte(cleaned), 0600)
}

// GenerateTunnelSecret creates a fresh 256-bit hex secret in proxy/tunnel.key
// and refreshes the published tunnel snapshot if the tunnel is active.
func (m *Manager) GenerateTunnelSecret(ctx context.Context) error {
	var raw [32]byte
	if _, err := rand.Read(raw[:]); err != nil {
		return err
	}
	secret := hex.EncodeToString(raw[:])
	return m.withState(ctx, func(s *State) error {
		if err := m.writeTunnelSecret(secret); err != nil {
			return err
		}
		if m.tunnelPublishedExists() || m.loaded(ctx, "tunnel") {
			return m.writePublishedTunnelConfig(s)
		}
		return nil
	})
}

// SaveTunnelSettings validates and saves Cloudflare tunnel configuration and an
// optional replacement secret.
func (m *Manager) SaveTunnelSettings(ctx context.Context, domain, clientID, endpoint, newSecret string) error {
	nd, err := normalizeTunnelDomain(domain)
	if err != nil {
		return err
	}
	ne, err := normalizeTunnelEndpoint(endpoint)
	if err != nil {
		return err
	}
	nc, err := normalizeTunnelClientID(clientID)
	if err != nil {
		return err
	}
	var cleanedSecret string
	if strings.TrimSpace(newSecret) != "" {
		cleanedSecret, err = validateTunnelSecret(newSecret)
		if err != nil {
			return err
		}
	}
	return m.withState(ctx, func(s *State) error {
		if nc == "" && (nd != "" || ne != "") {
			nc = "mac-" + randomID()[:8]
		}
		s.TunnelDomain = nd
		s.TunnelEndpoint = ne
		s.TunnelClientID = nc
		if cleanedSecret != "" {
			if err := m.writeTunnelSecret(cleanedSecret); err != nil {
				return err
			}
		}
		if m.tunnelPublishedExists() || m.loaded(ctx, "tunnel") {
			return m.writePublishedTunnelConfig(s)
		}
		return nil
	})
}

// TunnelRoute maps a single public hostname to a local loopback port.
type TunnelRoute struct {
	PublicHost  string `json:"publicHost"`
	LocalDomain string `json:"localDomain"`
	LocalPort   int    `json:"localPort"`
}

type tunnelPublishedConfig struct {
	SchemaVersion int           `json:"schemaVersion"`
	Domain        string        `json:"domain"`
	Endpoint      string        `json:"endpoint"`
	ClientID      string        `json:"clientId"`
	Rules         []TunnelRoute `json:"rules"`
}

func (c tunnelPublishedConfig) WebSocketURL() (string, error) {
	if c.Endpoint != "" {
		u, err := url.Parse(c.Endpoint)
		if err != nil {
			return "", err
		}
		switch u.Scheme {
		case "https":
			u.Scheme = "wss"
		case "http":
			u.Scheme = "ws"
		default:
			return "", errors.New("unsupported tunnel endpoint scheme")
		}
		u.Path = "/__devbase/connect"
		return u.String(), nil
	}
	if c.Domain == "" {
		return "", errors.New("tunnel wildcard domain or worker URL is required")
	}
	return "wss://" + reservedTunnelSubdomain + "." + c.Domain + "/__devbase/connect", nil
}

func tunnelConfigFromState(s *State) (tunnelPublishedConfig, error) {
	if s.TunnelDomain == "" && s.TunnelEndpoint == "" {
		return tunnelPublishedConfig{}, errors.New("configure a wildcard domain or worker URL first")
	}
	clientID := s.TunnelClientID
	if clientID == "" {
		clientID = "mac-" + randomID()[:8]
	}
	routes := make([]TunnelRoute, 0, len(s.ProxyRules))
	for _, rule := range s.ProxyRules {
		if !rule.Tunnel {
			continue
		}
		if s.TunnelDomain == "" {
			return tunnelPublishedConfig{}, errors.New("wildcard domain is required when tunneling proxy rules")
		}
		routes = append(routes, TunnelRoute{
			PublicHost:  rule.PublicHost(s.TunnelDomain),
			LocalDomain: rule.Domain,
			LocalPort:   rule.LocalPort,
		})
	}
	sort.Slice(routes, func(i, j int) bool { return routes[i].PublicHost < routes[j].PublicHost })
	return tunnelPublishedConfig{
		SchemaVersion: 1,
		Domain:        s.TunnelDomain,
		Endpoint:      s.TunnelEndpoint,
		ClientID:      clientID,
		Rules:         routes,
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
	if err = validateTunnelSettings(cfg.Domain, cfg.Endpoint, cfg.ClientID); err != nil {
		return cfg, err
	}
	if cfg.ClientID == "" {
		return cfg, errors.New("tunnel snapshot missing client ID")
	}
	if cfg.Rules == nil {
		cfg.Rules = []TunnelRoute{}
	}
	seen := map[string]bool{}
	for _, r := range cfg.Rules {
		if r.PublicHost == "" || r.LocalPort < 1 || r.LocalPort > 65535 {
			return cfg, errors.New("invalid tunnel route in snapshot")
		}
		if seen[r.PublicHost] {
			return cfg, fmt.Errorf("duplicate tunnel route %s", r.PublicHost)
		}
		seen[r.PublicHost] = true
	}
	if _, err = cfg.WebSocketURL(); err != nil {
		return cfg, err
	}
	return cfg, nil
}

// TunnelConflict describes a hostname already claimed by another active client.
type TunnelConflict struct {
	Host  string `json:"host"`
	Owner string `json:"owner"`
}

// TunnelStatusSnapshot is written atomically by the background tunnel process
// so the UI and diagnostics can observe live connection state without IPC.
type TunnelStatusSnapshot struct {
	PID       int              `json:"pid"`
	Phase     string           `json:"phase"`
	Accepted  []string         `json:"accepted,omitempty"`
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

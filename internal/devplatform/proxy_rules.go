package devplatform

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
)

// ProxyRule is configuration for one local HTTP port. It has no independent
// process, identity, or lifecycle.
type ProxyRule struct {
	Domain       string `json:"domain"`
	LocalPort    int    `json:"localPort"`
	Tunnel       bool   `json:"tunnel,omitempty"`
	TunnelWorker string `json:"tunnelWorker,omitempty"`
	TunnelDomain string `json:"tunnelDomain,omitempty"`
	TunnelURL    string `json:"tunnelUrl,omitempty"`
}

// DefaultTunnelWorkerName derives the Cloudflare Worker script name for a
// local .test domain, always prefixed with "devbase-" so Devbase only manages
// Workers it owns.
func DefaultTunnelWorkerName(domain string) string {
	d := strings.ToLower(strings.TrimSpace(domain))
	d = strings.TrimSuffix(d, ".test")
	slug := strings.ReplaceAll(d, ".", "-")
	slug = strings.Trim(slug, "-")
	if slug == "" {
		return ""
	}
	name := "devbase-" + slug
	if len(name) > 63 {
		name = strings.TrimRight(name[:63], "-")
	}
	return name
}

// ResolvedTunnelWorker returns the Cloudflare Worker script name for this rule.
func (r ProxyRule) ResolvedTunnelWorker() string {
	if w := strings.ToLower(strings.TrimSpace(r.TunnelWorker)); w != "" {
		return w
	}
	return DefaultTunnelWorkerName(r.Domain)
}

// EffectiveTunnelDomain returns the normalized custom domain, or empty when
// using automatic *.workers.dev routing.
func (r ProxyRule) EffectiveTunnelDomain() string {
	return normalizeTunnelHost(r.TunnelDomain)
}

// PreviewTunnelURL returns the public HTTPS URL for a tunneled rule given the
// account's workers.dev subdomain.
func (r ProxyRule) PreviewTunnelURL(cloudflareSubdomain string) string {
	if !r.Tunnel {
		return ""
	}
	if dom := r.EffectiveTunnelDomain(); dom != "" {
		return "https://" + dom
	}
	if sub := strings.ToLower(strings.TrimSpace(cloudflareSubdomain)); sub != "" {
		if worker := r.ResolvedTunnelWorker(); worker != "" {
			return "https://" + worker + "." + sub + ".workers.dev"
		}
	}
	return strings.TrimSpace(r.TunnelURL)
}

func normalizeTunnelHost(raw string) string {
	h := strings.TrimSpace(raw)
	h = strings.TrimPrefix(h, "https://")
	h = strings.TrimPrefix(h, "http://")
	h = strings.TrimSuffix(h, "/")
	return strings.ToLower(h)
}

func validatePublicTunnelHostname(host string) error {
	if host == "" {
		return nil
	}
	if len(host) > 253 || strings.ContainsAny(host, "/: \t\r\n") {
		return errors.New("custom domain must be a hostname without scheme or path")
	}
	if !strings.Contains(host, ".") {
		return errors.New("custom domain must include a zone domain (e.g. app.example.com)")
	}
	if strings.HasSuffix(host, ".test") {
		return errors.New("custom domain cannot be a local .test domain")
	}
	if strings.HasSuffix(host, ".workers.dev") {
		return errors.New("clear Custom domain to use automatic workers.dev URLs")
	}
	for _, part := range strings.Split(host, ".") {
		if !proxyDomainPart.MatchString(part) {
			return fmt.Errorf("invalid custom domain %q", host)
		}
	}
	return nil
}

// proxyRoute is a derived transport entry, never a saved resource.
type proxyRoute struct {
	Domain    string
	LocalPort int
	Port      int
}

func (r proxyRoute) upstream() string { return fmt.Sprintf("http://127.0.0.1:%d", r.LocalPort) }
func (r proxyRoute) Validate() error {
	if r.Port < 1 || r.Port > 65535 {
		return errors.New("HTTPS port must be 1–65535")
	}
	_, err := normalizeProxyRules(r.Port, []ProxyRule{{Domain: r.Domain, LocalPort: r.LocalPort}})
	return err
}

var proxyDomainPart = regexp.MustCompile(`^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$`)

func normalizeProxyRules(listenerPort int, rules []ProxyRule) ([]ProxyRule, error) {
	if listenerPort < 1 || listenerPort > 65535 {
		return nil, errors.New("HTTPS listener port must be 1–65535")
	}
	if rules == nil {
		return nil, errors.New("proxy rules must be an array")
	}
	normalized := make([]ProxyRule, len(rules))
	seen := make(map[string]bool, len(rules))
	seenWorkers := make(map[string]bool, len(rules))
	seenTunnelDomains := make(map[string]bool, len(rules))
	for i, rule := range rules {
		rule.Domain = strings.ToLower(strings.TrimSpace(rule.Domain))
		if len(rule.Domain) > 253 || !strings.HasSuffix(rule.Domain, ".test") {
			return nil, fmt.Errorf("rule %d: use a local .test domain", i+1)
		}
		for _, part := range strings.Split(rule.Domain, ".") {
			if !proxyDomainPart.MatchString(part) {
				return nil, fmt.Errorf("rule %d: invalid domain", i+1)
			}
		}
		if rule.LocalPort < 1 || rule.LocalPort > 65535 || rule.LocalPort == listenerPort {
			return nil, fmt.Errorf("rule %d: local port must be 1–65535 and differ from HTTPS port", i+1)
		}
		if seen[rule.Domain] {
			return nil, fmt.Errorf("duplicate proxy domain %s", rule.Domain)
		}
		seen[rule.Domain] = true
		rule.TunnelWorker = strings.ToLower(strings.TrimSpace(rule.TunnelWorker))
		if rule.TunnelWorker != "" {
			if !proxyDomainPart.MatchString(rule.TunnelWorker) || !strings.HasPrefix(rule.TunnelWorker, "devbase-") || len(rule.TunnelWorker) <= len("devbase-") {
				return nil, fmt.Errorf("rule %d: worker name must start with devbase- and use valid DNS label characters", i+1)
			}
		}
		rule.TunnelDomain = normalizeTunnelHost(rule.TunnelDomain)
		if err := validatePublicTunnelHostname(rule.TunnelDomain); err != nil {
			return nil, fmt.Errorf("rule %d: %w", i+1, err)
		}
		rule.TunnelURL = strings.TrimSpace(rule.TunnelURL)
		if rule.Tunnel {
			worker := rule.ResolvedTunnelWorker()
			if !proxyDomainPart.MatchString(worker) || !strings.HasPrefix(worker, "devbase-") || len(worker) <= len("devbase-") {
				return nil, fmt.Errorf("rule %d: invalid worker name %q", i+1, worker)
			}
			if seenWorkers[worker] {
				return nil, fmt.Errorf("duplicate tunnel worker %s", worker)
			}
			seenWorkers[worker] = true
			if rule.TunnelDomain != "" {
				if seenTunnelDomains[rule.TunnelDomain] {
					return nil, fmt.Errorf("duplicate tunnel custom domain %s", rule.TunnelDomain)
				}
				seenTunnelDomains[rule.TunnelDomain] = true
			}
		} else {
			rule.TunnelURL = ""
		}
		normalized[i] = rule
	}
	return normalized, nil
}

// ReplaceProxyRules saves the entire table and HTTPS port in one state write.
func (m *Manager) ReplaceProxyRules(ctx context.Context, expectedRevision uint64, listenerPort int, rules []ProxyRule) error {
	normalized, err := normalizeProxyRules(listenerPort, rules)
	if err != nil {
		return err
	}
	return m.withState(ctx, func(s *State) error {
		if s.ProxyRulesRevision != expectedRevision {
			return errors.New("proxy configuration changed; reload before saving")
		}
		s.ProxyRules = normalized
		s.ProxyListenerPort = listenerPort
		s.ProxyRulesRevision++
		return nil
	})
}

// proxyPublishedConfig is the complete on-disk routing snapshot. Revision is
// intentionally a state-only concurrency token, not routing content.
type proxyPublishedConfig struct {
	SchemaVersion int         `json:"schemaVersion"`
	ListenerPort  int         `json:"listenerPort"`
	Rules         []ProxyRule `json:"rules"`
}

func proxyConfigFromState(s *State) proxyPublishedConfig {
	return proxyPublishedConfig{SchemaVersion: 1, ListenerPort: s.ProxyListenerPort, Rules: append([]ProxyRule{}, s.ProxyRules...)}
}
func proxyConfigFromRoutes(sites []proxyRoute) proxyPublishedConfig {
	config := proxyPublishedConfig{SchemaVersion: 1, ListenerPort: 443, Rules: []ProxyRule{}}
	if len(sites) > 0 {
		config.ListenerPort = sites[0].Port
	}
	for _, site := range sites {
		config.Rules = append(config.Rules, ProxyRule{Domain: site.Domain, LocalPort: site.LocalPort})
	}
	return config
}
func readPublishedProxyConfig(root string) (proxyPublishedConfig, error) {
	b, err := os.ReadFile(filepath.Join(root, "proxy", "rules.json"))
	if err != nil {
		return proxyPublishedConfig{}, err
	}
	var config proxyPublishedConfig
	decoder := json.NewDecoder(bytes.NewReader(b))
	decoder.DisallowUnknownFields()
	if err = decoder.Decode(&config); err != nil {
		return config, err
	}
	var extra any
	if err = decoder.Decode(&extra); err != io.EOF {
		return config, errors.New("proxy routing snapshot has trailing data")
	}
	if config.SchemaVersion != 1 {
		return config, errors.New("unsupported proxy routing schema")
	}
	rules, err := normalizeProxyRules(config.ListenerPort, config.Rules)
	if err != nil {
		return config, err
	}
	config.Rules = rules
	return config, nil
}

// ProxyPublicationState compares saved intent with the persisted routing
// snapshot. It makes no claim about a running listener or browser trust.
func (m *Manager) ProxyPublicationState() (string, error) {
	s, err := m.Read()
	if err != nil {
		return "Unknown", err
	}
	published, err := readPublishedProxyConfig(m.Root)
	if err != nil {
		return "Unknown", nil
	}
	if published.ListenerPort != s.ProxyListenerPort || !sameProxyRouting(published.Rules, s.ProxyRules) {
		return "Pending", nil
	}
	return "Published", nil
}

// sameProxyRouting compares what the HTTPS proxy routes: domain and local
// port. Tunnel settings belong to the Cloudflare tunnel, and the privileged
// socket path publishes a snapshot without them, so they must not make a
// published proxy look pending.
func sameProxyRouting(a, b []ProxyRule) bool {
	strip := func(rules []ProxyRule) []ProxyRule {
		out := make([]ProxyRule, len(rules))
		for i, r := range rules {
			out[i] = ProxyRule{Domain: r.Domain, LocalPort: r.LocalPort}
		}
		return out
	}
	return sameProxyRules(strip(a), strip(b))
}

func sameProxyRules(a, b []ProxyRule) bool {
	if len(a) != len(b) {
		return false
	}
	a = append([]ProxyRule(nil), a...)
	b = append([]ProxyRule(nil), b...)
	sort.Slice(a, func(i, j int) bool { return a[i].Domain < a[j].Domain })
	sort.Slice(b, func(i, j int) bool { return b[i].Domain < b[j].Domain })
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

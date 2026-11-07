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
	Domain    string `json:"domain"`
	LocalPort int    `json:"localPort"`
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
	if published.ListenerPort != s.ProxyListenerPort || !sameProxyRules(published.Rules, s.ProxyRules) {
		return "Pending", nil
	}
	return "Published", nil
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

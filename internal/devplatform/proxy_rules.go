package devplatform

import (
	"context"
	"errors"
	"fmt"
	"regexp"
	"strings"
)

// ProxyRule is configuration for one local HTTP port. It has no independent
// process, identity, or lifecycle.
type ProxyRule struct {
	Domain    string `json:"domain"`
	LocalPort int    `json:"localPort"`
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

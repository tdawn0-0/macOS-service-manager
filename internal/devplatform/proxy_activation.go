package devplatform

import (
	"context"
	"crypto/sha256"
	"crypto/tls"
	"crypto/x509"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"
)

// ProxyHTTPResult is a single, explicit observation; it contains no response data.
type ProxyHTTPResult struct {
	Domain      string
	Fingerprint string
	Revision    uint64
	StatusCode  int
	Duration    time.Duration
	CheckedAt   time.Time
	Failure     string // Stale, Timeout, TLS, Unavailable; never a raw error.
}

func proxyRuleFingerprint(s State, rule ProxyRule) string {
	payload := fmt.Sprintf("%d:%s:%s", s.ProxyListenerPort, rule.Domain, upstreamHostPort(rule.UpstreamHost, rule.LocalPort))
	return fmt.Sprintf("%x", sha256.Sum256([]byte(payload)))
}
func (m *Manager) ProxyRuleFingerprint(domain string) (string, uint64, error) {
	s, err := m.Read()
	if err != nil {
		return "", 0, err
	}
	domain = strings.ToLower(strings.TrimSpace(domain))
	for _, rule := range s.ProxyRules {
		if rule.Domain == domain {
			return proxyRuleFingerprint(s, rule), s.ProxyRulesRevision, nil
		}
	}
	return "", s.ProxyRulesRevision, errors.New("proxy rule is missing")
}
func (r ProxyHTTPResult) Current(now time.Time, revision uint64, fingerprint string) bool {
	return r.Revision == revision && r.Fingerprint == fingerprint && !r.CheckedAt.IsZero() && now.Sub(r.CheckedAt) <= 30*time.Second
}
func (m *Manager) CheckProxyRuleHTTP(ctx context.Context, domain string, revision uint64, fingerprint string) (ProxyHTTPResult, error) {
	return m.checkProxyRuleHTTP(ctx, domain, revision, fingerprint, nil)
}
func (m *Manager) checkProxyRuleHTTP(ctx context.Context, domain string, revision uint64, fingerprint string, roots *x509.CertPool) (ProxyHTTPResult, error) {
	result := ProxyHTTPResult{Domain: strings.ToLower(strings.TrimSpace(domain)), Fingerprint: fingerprint, Revision: revision}
	s, err := m.Read()
	if err != nil {
		return result, err
	}
	var rule *ProxyRule
	for i := range s.ProxyRules {
		if s.ProxyRules[i].Domain == result.Domain {
			rule = &s.ProxyRules[i]
			break
		}
	}
	if rule == nil || s.ProxyRulesRevision != revision || proxyRuleFingerprint(s, *rule) != fingerprint {
		result.Failure = "Stale"
		return result, errors.New("proxy rule changed; check again")
	}
	checkCtx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()
	port := s.ProxyListenerPort
	dialer := &net.Dialer{}
	transport := &http.Transport{Proxy: nil, DisableKeepAlives: true, TLSClientConfig: &tls.Config{MinVersion: tls.VersionTLS12, ServerName: result.Domain, RootCAs: roots}, DialContext: func(c context.Context, _, _ string) (net.Conn, error) {
		return dialer.DialContext(c, "tcp", net.JoinHostPort("127.0.0.1", strconv.Itoa(port)))
	}}
	defer transport.CloseIdleConnections()
	client := &http.Client{Transport: transport, CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}
	req, err := http.NewRequestWithContext(checkCtx, http.MethodGet, "https://"+result.Domain+"/", nil)
	if err != nil {
		return result, err
	}
	start := time.Now()
	resp, err := client.Do(req)
	result.Duration = time.Since(start)
	result.CheckedAt = time.Now()
	if err != nil {
		if errors.Is(err, context.DeadlineExceeded) || errors.Is(checkCtx.Err(), context.DeadlineExceeded) {
			result.Failure = "Timeout"
		} else {
			var certErr x509.UnknownAuthorityError
			var hostErr x509.HostnameError
			if errors.As(err, &certErr) || errors.As(err, &hostErr) {
				result.Failure = "TLS"
			} else {
				result.Failure = "Unavailable"
			}
		}
		return result, nil
	}
	defer resp.Body.Close()
	_, _ = io.Copy(io.Discard, io.LimitReader(resp.Body, 64<<10))
	result.StatusCode = resp.StatusCode
	latest, err := m.Read()
	if err != nil {
		return result, err
	}
	if latest.ProxyRulesRevision != revision {
		result.Failure = "Stale"
		result.StatusCode = 0
		return result, nil
	}
	for _, next := range latest.ProxyRules {
		if next.Domain == result.Domain && proxyRuleFingerprint(latest, next) == fingerprint {
			return result, nil
		}
	}
	result.Failure = "Stale"
	result.StatusCode = 0
	return result, nil
}

func resolveTunnelPublicURL(s State, rule ProxyRule) string {
	if !rule.Tunnel {
		return ""
	}
	if w, ok := s.TunnelWorkers[rule.ResolvedTunnelWorker()]; ok && w.PublicURL != "" {
		return strings.TrimSpace(w.PublicURL)
	}
	return rule.PreviewTunnelURL(s.CloudflareSubdomain)
}

func tunnelRuleFingerprint(rule ProxyRule, publicURL, authDigest string) string {
	payload := fmt.Sprintf("%s:%s:%s:%t:%s", rule.Domain, upstreamHostPort(rule.UpstreamHost, rule.LocalPort), publicURL, rule.TunnelAuth, authDigest)
	return fmt.Sprintf("%x", sha256.Sum256([]byte(payload)))
}

// TunnelRuleFingerprint identifies the current saved tunnel target for a rule.
func (m *Manager) TunnelRuleFingerprint(domain string) (string, uint64, error) {
	s, err := m.Read()
	if err != nil {
		return "", 0, err
	}
	domain = strings.ToLower(strings.TrimSpace(domain))
	for _, rule := range s.ProxyRules {
		if rule.Domain == domain {
			if !rule.Tunnel {
				return "", s.ProxyRulesRevision, errors.New("tunnel is not enabled for this rule")
			}
			publicURL := resolveTunnelPublicURL(s, rule)
			authDigest := ""
			if rule.TunnelAuth {
				if tok, tokErr := m.TunnelAccessToken(rule.Domain); tokErr == nil {
					authDigest = tunnelAuthDigest(tok)
				}
			}
			return tunnelRuleFingerprint(rule, publicURL, authDigest), s.ProxyRulesRevision, nil
		}
	}
	return "", s.ProxyRulesRevision, errors.New("proxy rule is missing")
}

// CheckTunnelRuleHTTP sends a single bounded GET / to a rule's public tunnel URL.
func (m *Manager) CheckTunnelRuleHTTP(ctx context.Context, domain string, revision uint64, fingerprint string) (ProxyHTTPResult, error) {
	result := ProxyHTTPResult{Domain: strings.ToLower(strings.TrimSpace(domain)), Fingerprint: fingerprint, Revision: revision}
	s, err := m.Read()
	if err != nil {
		return result, err
	}
	var rule *ProxyRule
	for i := range s.ProxyRules {
		if s.ProxyRules[i].Domain == result.Domain {
			rule = &s.ProxyRules[i]
			break
		}
	}
	if rule == nil || !rule.Tunnel || s.ProxyRulesRevision != revision {
		result.Failure = "Stale"
		return result, errors.New("tunnel rule changed; check again")
	}
	publicURL := resolveTunnelPublicURL(s, *rule)
	authDigest := ""
	accessToken := ""
	if rule.TunnelAuth {
		if tok, tokErr := m.TunnelAccessToken(rule.Domain); tokErr == nil && tok != "" {
			accessToken = tok
			authDigest = tunnelAuthDigest(tok)
		}
	}
	if tunnelRuleFingerprint(*rule, publicURL, authDigest) != fingerprint {
		result.Failure = "Stale"
		return result, errors.New("tunnel rule changed; check again")
	}
	if publicURL == "" {
		result.CheckedAt = time.Now()
		result.Failure = "Not deployed"
		return result, nil
	}
	u, err := url.Parse(publicURL)
	if err != nil || u.Scheme == "" || u.Host == "" {
		result.CheckedAt = time.Now()
		result.Failure = "Unavailable"
		return result, nil
	}
	u.Path = "/"
	u.RawQuery = ""
	u.Fragment = ""

	checkCtx, cancel := context.WithTimeout(ctx, 8*time.Second)
	defer cancel()
	var tlsCfg *tls.Config
	if m.tunnelTLSConfig != nil {
		tlsCfg = m.tunnelTLSConfig.Clone()
	} else {
		tlsCfg = &tls.Config{MinVersion: tls.VersionTLS12}
	}
	transport := &http.Transport{Proxy: nil, DisableKeepAlives: true, TLSClientConfig: tlsCfg}
	defer transport.CloseIdleConnections()
	client := &http.Client{Transport: transport, CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}
	req, err := http.NewRequestWithContext(checkCtx, http.MethodGet, u.String(), nil)
	if err != nil {
		return result, err
	}
	if rule.TunnelAuth && accessToken != "" {
		req.Header.Set("Authorization", "Bearer "+accessToken)
	}
	start := time.Now()
	resp, err := client.Do(req)
	result.Duration = time.Since(start)
	result.CheckedAt = time.Now()
	if err != nil {
		if errors.Is(err, context.DeadlineExceeded) || errors.Is(checkCtx.Err(), context.DeadlineExceeded) {
			result.Failure = "Timeout"
		} else {
			var certErr x509.UnknownAuthorityError
			var hostErr x509.HostnameError
			if errors.As(err, &certErr) || errors.As(err, &hostErr) {
				result.Failure = "TLS"
			} else {
				result.Failure = "Unavailable"
			}
		}
		return result, nil
	}
	defer resp.Body.Close()
	_, _ = io.Copy(io.Discard, io.LimitReader(resp.Body, 64<<10))
	result.StatusCode = resp.StatusCode

	latest, err := m.Read()
	if err != nil {
		return result, err
	}
	if latest.ProxyRulesRevision != revision {
		result.Failure = "Stale"
		result.StatusCode = 0
		return result, nil
	}
	for _, next := range latest.ProxyRules {
		if next.Domain == result.Domain && next.Tunnel {
			nextURL := resolveTunnelPublicURL(latest, next)
			nextDigest := ""
			if next.TunnelAuth {
				if tok, tokErr := m.TunnelAccessToken(next.Domain); tokErr == nil {
					nextDigest = tunnelAuthDigest(tok)
				}
			}
			if tunnelRuleFingerprint(next, nextURL, nextDigest) == fingerprint {
				return result, nil
			}
		}
	}
	result.Failure = "Stale"
	result.StatusCode = 0
	return result, nil
}

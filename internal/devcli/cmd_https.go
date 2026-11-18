package devcli

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"strconv"
	"strings"

	"devbase/internal/devplatform"
)

func runHTTPSCommand(ctx context.Context, env *Env, args []string) error {
	if hasHelpFlag(args) && len(args) == 1 {
		printTopicHelp(env.Out, "https")
		return nil
	}
	sub := "status"
	if len(args) > 0 {
		sub = args[0]
		args = args[1:]
	}
	if hasHelpFlag(args) {
		printTopicHelp(env.Out, "https")
		return nil
	}
	switch sub {
	case "status", "rules", "list":
		return runHTTPSStatus(ctx, env, sub, args)
	case "add":
		return runHTTPSAdd(ctx, env, args)
	case "remove":
		return runHTTPSRemove(ctx, env, args)
	case "set-port":
		return runHTTPSSetPort(ctx, env, args)
	case "replace":
		return runHTTPSReplace(ctx, env, args)
	case "apply", "start":
		return runHTTPSApply(ctx, env, args)
	case "stop":
		return runHTTPSStop(ctx, env, args)
	case "restart":
		return runHTTPSRestart(ctx, env, args)
	case "check":
		return runHTTPSCheck(ctx, env, args)
	default:
		return usageErrorf("unknown https subcommand %q", sub)
	}
}

func runHTTPSStatus(ctx context.Context, env *Env, sub string, args []string) error {
	if len(args) > 0 {
		return usageErrorf("unexpected argument %q for https %s", args[0], sub)
	}
	m := env.manager()
	s, err := m.Read()
	if err != nil {
		return err
	}
	snap := m.Snapshot(ctx, s)
	pub, _ := m.ProxyPublicationState()
	if pub == "" {
		pub = "Unknown"
	}
	payload := map[string]any{
		"status":           snap.States["proxy"],
		"publicationState": pub,
		"listenerPort":     s.ProxyListenerPort,
		"revision":         s.ProxyRulesRevision,
		"rules":            s.ProxyRules,
	}
	return writeResult(env, payload, func(w io.Writer) {
		fmt.Fprintf(w, "HTTPS Proxy: %s (listener :%d, revision %d, %s)\n", snap.States["proxy"], s.ProxyListenerPort, s.ProxyRulesRevision, pub)
		if len(s.ProxyRules) == 0 {
			fmt.Fprintln(w, "No HTTPS proxy rules configured.")
			return
		}
		for _, r := range s.ProxyRules {
			upstream := r.UpstreamHost
			if upstream == "" {
				upstream = "127.0.0.1"
			}
			line := fmt.Sprintf("  - https://%s -> %s:%d", r.Domain, upstream, r.LocalPort)
			if r.Tunnel {
				line += " [tunnel"
				if r.TunnelURL != "" {
					line += ": " + r.TunnelURL
				}
				line += "]"
			}
			fmt.Fprintln(w, line)
		}
	})
}

func runHTTPSAdd(ctx context.Context, env *Env, args []string) error {
	fs := newFlagSet("https add")
	var upstreamHost, tunnelWorker, tunnelDomain string
	var listenerPort int
	var tunnel, tunnelAuth, apply bool
	fs.StringVar(&upstreamHost, "upstream-host", "", "Upstream IP address or hostname (default 127.0.0.1)")
	fs.BoolVar(&tunnel, "tunnel", false, "Expose this rule via Cloudflare Tunnel")
	fs.BoolVar(&tunnelAuth, "tunnel-auth", false, "Require Devbase access token on the Cloudflare Tunnel")
	fs.StringVar(&tunnelWorker, "tunnel-worker", "", "Custom Cloudflare Worker name (devbase-*)")
	fs.StringVar(&tunnelDomain, "tunnel-domain", "", "Custom Cloudflare hostname for this rule")
	fs.IntVar(&listenerPort, "listener-port", 0, "Override HTTPS proxy listener port (1–65535)")
	fs.BoolVar(&apply, "apply", false, "Apply proxy configuration immediately after saving")
	pos, err := parseFlags(fs, args)
	if err != nil {
		return err
	}
	if len(pos) != 2 {
		return usageErrorf("usage: devbase https add <name-or-domain> <local-port> [--upstream-host <host>] [--tunnel] [--tunnel-auth] [--tunnel-worker <name>] [--tunnel-domain <host>] [--listener-port <port>] [--apply]")
	}
	domain := devplatform.LocalDomain(pos[0])
	if domain == "" {
		return usageErrorf("domain cannot be empty")
	}
	localPort, err := strconv.Atoi(pos[1])
	if err != nil || localPort < 1 || localPort > 65535 {
		return usageErrorf("local-port must be 1–65535")
	}
	if listenerPort < 0 || listenerPort > 65535 {
		return usageErrorf("--listener-port must be 1–65535")
	}

	m := env.manager()
	s, err := m.Read()
	if err != nil {
		return err
	}
	port := s.ProxyListenerPort
	if port == 0 {
		port = 443
	}
	if listenerPort > 0 {
		port = listenerPort
	}
	newRule := devplatform.ProxyRule{
		Domain:       domain,
		LocalPort:    localPort,
		UpstreamHost: strings.TrimSpace(upstreamHost),
		Tunnel:       tunnel,
		TunnelAuth:   tunnelAuth,
		TunnelWorker: strings.TrimSpace(tunnelWorker),
		TunnelDomain: strings.TrimSpace(tunnelDomain),
	}
	rules := make([]devplatform.ProxyRule, 0, len(s.ProxyRules)+1)
	replaced := false
	for _, existing := range s.ProxyRules {
		if existing.Domain == domain {
			newRule.TunnelURL = existing.TunnelURL
			rules = append(rules, newRule)
			replaced = true
			continue
		}
		rules = append(rules, existing)
	}
	if !replaced {
		rules = append(rules, newRule)
	}
	if err := m.ReplaceProxyRules(ctx, s.ProxyRulesRevision, port, rules); err != nil {
		return err
	}
	if apply {
		_ = m.EnsureLogMaintenance(ctx)
		if err := m.ApplyProxy(ctx); err != nil {
			return err
		}
	}
	sAfter, _ := m.Read()
	return writeResult(env, map[string]any{
		"rule":         newRule,
		"listenerPort": sAfter.ProxyListenerPort,
		"revision":     sAfter.ProxyRulesRevision,
		"rules":        sAfter.ProxyRules,
		"applied":      apply,
	}, func(w io.Writer) {
		fmt.Fprintf(w, "Saved HTTPS rule https://%s -> port %d (revision %d).\n", domain, localPort, sAfter.ProxyRulesRevision)
	})
}

func runHTTPSRemove(ctx context.Context, env *Env, args []string) error {
	fs := newFlagSet("https remove")
	var apply bool
	fs.BoolVar(&apply, "apply", false, "Apply proxy configuration immediately after removing")
	pos, err := parseFlags(fs, args)
	if err != nil {
		return err
	}
	if len(pos) != 1 {
		return usageErrorf("usage: devbase https remove <name-or-domain> [--apply]")
	}
	domain := devplatform.LocalDomain(pos[0])
	m := env.manager()
	s, err := m.Read()
	if err != nil {
		return err
	}
	rules := make([]devplatform.ProxyRule, 0, len(s.ProxyRules))
	found := false
	for _, r := range s.ProxyRules {
		if r.Domain == domain {
			found = true
			continue
		}
		rules = append(rules, r)
	}
	if !found {
		return fmt.Errorf("HTTPS rule %q not found", domain)
	}
	if err := m.ReplaceProxyRules(ctx, s.ProxyRulesRevision, s.ProxyListenerPort, rules); err != nil {
		return err
	}
	if apply {
		_ = m.EnsureLogMaintenance(ctx)
		if err := m.ApplyProxy(ctx); err != nil {
			return err
		}
	}
	sAfter, _ := m.Read()
	return writeResult(env, map[string]any{
		"removed":  domain,
		"revision": sAfter.ProxyRulesRevision,
		"rules":    sAfter.ProxyRules,
		"applied":  apply,
	}, func(w io.Writer) {
		fmt.Fprintf(w, "Removed HTTPS rule %s (revision %d).\n", domain, sAfter.ProxyRulesRevision)
	})
}

func runHTTPSSetPort(ctx context.Context, env *Env, args []string) error {
	fs := newFlagSet("https set-port")
	var apply bool
	fs.BoolVar(&apply, "apply", false, "Apply proxy configuration immediately after saving")
	pos, err := parseFlags(fs, args)
	if err != nil {
		return err
	}
	if len(pos) != 1 {
		return usageErrorf("usage: devbase https set-port <listener-port> [--apply]")
	}
	port, err := strconv.Atoi(pos[0])
	if err != nil || port < 1 || port > 65535 {
		return usageErrorf("listener-port must be 1–65535")
	}
	m := env.manager()
	s, err := m.Read()
	if err != nil {
		return err
	}
	if err := m.ReplaceProxyRules(ctx, s.ProxyRulesRevision, port, s.ProxyRules); err != nil {
		return err
	}
	if apply {
		_ = m.EnsureLogMaintenance(ctx)
		if err := m.ApplyProxy(ctx); err != nil {
			return err
		}
	}
	sAfter, _ := m.Read()
	return writeResult(env, map[string]any{
		"listenerPort": sAfter.ProxyListenerPort,
		"revision":     sAfter.ProxyRulesRevision,
		"applied":      apply,
	}, func(w io.Writer) {
		fmt.Fprintf(w, "HTTPS proxy listener port set to %d (revision %d).\n", port, sAfter.ProxyRulesRevision)
	})
}

func runHTTPSReplace(ctx context.Context, env *Env, args []string) error {
	fs := newFlagSet("https replace")
	var jsonStdin, apply bool
	var listenerPort int
	fs.BoolVar(&jsonStdin, "json-stdin", false, "Read replacement rules JSON ([]ProxyRule or {listenerPort, rules}) from stdin")
	fs.IntVar(&listenerPort, "listener-port", 0, "Override listener port (1–65535)")
	fs.BoolVar(&apply, "apply", false, "Apply proxy configuration immediately after replacing")
	pos, err := parseFlags(fs, args)
	if err != nil {
		return err
	}
	if len(pos) > 0 {
		return usageErrorf("unexpected argument %q for https replace", pos[0])
	}
	if !jsonStdin {
		return usageErrorf("usage: devbase https replace --json-stdin [--listener-port <port>] [--apply]")
	}
	if listenerPort < 0 || listenerPort > 65535 {
		return usageErrorf("--listener-port must be 1–65535")
	}
	raw, err := readJSONStdin(env.In, 1<<20)
	if err != nil {
		return err
	}
	m := env.manager()
	s, err := m.Read()
	if err != nil {
		return err
	}
	port := s.ProxyListenerPort
	if port == 0 {
		port = 443
	}
	var rules []devplatform.ProxyRule
	trimmed := strings.TrimSpace(string(raw))
	if strings.HasPrefix(trimmed, "[") {
		if err := json.Unmarshal(raw, &rules); err != nil {
			return usageErrorf("invalid rules JSON array: %v", err)
		}
	} else {
		var body struct {
			ListenerPort int                     `json:"listenerPort"`
			Rules        []devplatform.ProxyRule `json:"rules"`
		}
		if err := json.Unmarshal(raw, &body); err != nil {
			return usageErrorf("invalid rules JSON object: %v", err)
		}
		if body.Rules == nil {
			return usageErrorf("JSON object must include \"rules\" array")
		}
		rules = body.Rules
		if body.ListenerPort > 0 {
			port = body.ListenerPort
		}
	}
	if listenerPort > 0 {
		port = listenerPort
	}
	for i := range rules {
		rules[i].Domain = devplatform.LocalDomain(rules[i].Domain)
	}
	if err := m.ReplaceProxyRules(ctx, s.ProxyRulesRevision, port, rules); err != nil {
		return err
	}
	if apply {
		_ = m.EnsureLogMaintenance(ctx)
		if err := m.ApplyProxy(ctx); err != nil {
			return err
		}
	}
	sAfter, _ := m.Read()
	return writeResult(env, map[string]any{
		"listenerPort": sAfter.ProxyListenerPort,
		"revision":     sAfter.ProxyRulesRevision,
		"rules":        sAfter.ProxyRules,
		"applied":      apply,
	}, func(w io.Writer) {
		fmt.Fprintf(w, "Replaced HTTPS rules (%d rules, listener :%d, revision %d).\n", len(sAfter.ProxyRules), sAfter.ProxyListenerPort, sAfter.ProxyRulesRevision)
	})
}

func runHTTPSApply(ctx context.Context, env *Env, args []string) error {
	if len(args) > 0 {
		return usageErrorf("unexpected argument %q for https apply", args[0])
	}
	m := env.manager()
	_ = m.EnsureLogMaintenance(ctx)
	if err := m.ApplyProxy(ctx); err != nil {
		return err
	}
	pub, _ := m.ProxyPublicationState()
	return writeResult(env, map[string]any{"applied": true, "publicationState": pub}, func(w io.Writer) {
		fmt.Fprintf(w, "Applied HTTPS proxy configuration (%s).\n", pub)
	})
}

func runHTTPSStop(ctx context.Context, env *Env, args []string) error {
	if len(args) > 0 {
		return usageErrorf("unexpected argument %q for https stop", args[0])
	}
	if err := env.manager().StopProxy(ctx); err != nil {
		return err
	}
	return writeResult(env, map[string]any{"stopped": true}, func(w io.Writer) {
		fmt.Fprintln(w, "Stopped HTTPS proxy.")
	})
}

func runHTTPSRestart(ctx context.Context, env *Env, args []string) error {
	if len(args) > 0 {
		return usageErrorf("unexpected argument %q for https restart", args[0])
	}
	if err := env.manager().RestartProxy(ctx); err != nil {
		return err
	}
	return writeResult(env, map[string]any{"restarted": true}, func(w io.Writer) {
		fmt.Fprintln(w, "Restarted HTTPS proxy.")
	})
}

func runHTTPSCheck(ctx context.Context, env *Env, args []string) error {
	if len(args) != 1 {
		return usageErrorf("usage: devbase https check <name-or-domain>")
	}
	domain := devplatform.LocalDomain(args[0])
	m := env.manager()
	fp, rev, err := m.ProxyRuleFingerprint(domain)
	if err != nil {
		return err
	}
	res, err := m.CheckProxyRuleHTTP(ctx, domain, rev, fp)
	if err != nil {
		return err
	}
	return writeResult(env, res, func(w io.Writer) {
		if res.Failure != "" {
			fmt.Fprintf(w, "https://%s: FAILED (%s, status=%d, duration=%s)\n", res.Domain, res.Failure, res.StatusCode, res.Duration)
			return
		}
		fmt.Fprintf(w, "https://%s: HTTP %d (%s)\n", res.Domain, res.StatusCode, res.Duration)
	})
}

func runTunnelCommand(ctx context.Context, env *Env, args []string) error {
	if hasHelpFlag(args) && len(args) == 1 {
		printTopicHelp(env.Out, "tunnel")
		return nil
	}
	sub := "status"
	if len(args) > 0 {
		sub = args[0]
		args = args[1:]
	}
	if hasHelpFlag(args) {
		printTopicHelp(env.Out, "tunnel")
		return nil
	}
	switch sub {
	case "status":
		return runTunnelStatus(ctx, env, args)
	case "configure":
		return runTunnelConfigure(ctx, env, args)
	case "remove-config":
		return runTunnelRemoveConfig(ctx, env, args)
	case "discover", "refresh":
		return runTunnelDiscover(ctx, env, args)
	case "sync":
		return runTunnelSync(ctx, env, args)
	case "apply", "start":
		return runTunnelApply(ctx, env, args)
	case "stop":
		return runTunnelStop(ctx, env, args)
	case "restart":
		return runTunnelRestart(ctx, env, args)
	case "check":
		return runTunnelCheck(ctx, env, args)
	case "access-token":
		return runTunnelAccessToken(ctx, env, args)
	default:
		return usageErrorf("unknown tunnel subcommand %q", sub)
	}
}

func runTunnelStatus(ctx context.Context, env *Env, args []string) error {
	if len(args) > 0 {
		return usageErrorf("unexpected argument %q for tunnel status", args[0])
	}
	m := env.manager()
	s, err := m.Read()
	if err != nil {
		return err
	}
	snap := m.Snapshot(ctx, s)
	hasToken := m.HasCloudflareToken()
	type ruleTunnelView struct {
		Domain       string `json:"domain"`
		LocalPort    int    `json:"localPort"`
		UpstreamHost string `json:"upstreamHost,omitempty"`
		Tunnel       bool   `json:"tunnel"`
		TunnelAuth   bool   `json:"tunnelAuth,omitempty"`
		TunnelWorker string `json:"tunnelWorker"`
		TunnelDomain string `json:"tunnelDomain,omitempty"`
		TunnelURL    string `json:"tunnelUrl,omitempty"`
		WorkerState  string `json:"workerState,omitempty"`
	}
	var rules []ruleTunnelView
	for _, r := range s.ProxyRules {
		if !r.Tunnel {
			continue
		}
		worker := r.ResolvedTunnelWorker()
		rules = append(rules, ruleTunnelView{
			Domain:       r.Domain,
			LocalPort:    r.LocalPort,
			UpstreamHost: r.UpstreamHost,
			Tunnel:       r.Tunnel,
			TunnelAuth:   r.TunnelAuth,
			TunnelWorker: worker,
			TunnelDomain: r.TunnelDomain,
			TunnelURL:    r.TunnelURL,
			WorkerState:  snap.States["tunnel:worker:"+worker],
		})
	}
	payload := map[string]any{
		"status":    snap.States["tunnel"],
		"detail":    snap.States["tunnel:detail"],
		"hasToken":  hasToken,
		"accountId": s.CloudflareAccountID,
		"subdomain": s.CloudflareSubdomain,
		"clientId":  s.TunnelClientID,
		"zones":     s.CloudflareZones,
		"dns":       s.CloudflareDNS,
		"workers":   s.TunnelWorkers,
		"rules":     rules,
	}
	return writeResult(env, payload, func(w io.Writer) {
		fmt.Fprintf(w, "Cloudflare Tunnel: %s (token configured: %t)\n", snap.States["tunnel"], hasToken)
		if detail := snap.States["tunnel:detail"]; detail != "" {
			fmt.Fprintf(w, "Detail: %s\n", detail)
		}
		if s.CloudflareAccountID != "" || s.CloudflareSubdomain != "" {
			fmt.Fprintf(w, "Account: %s | workers.dev subdomain: %s\n", s.CloudflareAccountID, s.CloudflareSubdomain)
		}
		for _, r := range rules {
			fmt.Fprintf(w, "  - %s (%s) -> %s [%s]\n", r.Domain, r.TunnelWorker, r.TunnelURL, r.WorkerState)
		}
	})
}

func runTunnelConfigure(ctx context.Context, env *Env, args []string) error {
	fs := newFlagSet("tunnel configure")
	var accountID, clientID string
	var tokenStdin bool
	fs.StringVar(&accountID, "account-id", "", "Cloudflare account ID (auto-discovered when omitted)")
	fs.StringVar(&clientID, "client-id", "", "Stable tunnel client ID")
	fs.BoolVar(&tokenStdin, "token-stdin", false, "Read the Cloudflare API token from stdin")
	pos, err := parseFlags(fs, args)
	if err != nil {
		return err
	}
	if len(pos) > 0 {
		return usageErrorf("unexpected argument %q for tunnel configure", pos[0])
	}
	m := env.manager()
	token := ""
	if tokenStdin {
		token, err = readSecretStdin(env.In, "Cloudflare API token")
		if err != nil {
			return err
		}
	} else if !m.HasCloudflareToken() {
		return usageErrorf("Cloudflare API token is not saved yet; pass --token-stdin and pipe the token on stdin")
	}
	if err := m.SaveCloudflareSettings(ctx, token, strings.TrimSpace(accountID), strings.TrimSpace(clientID)); err != nil {
		return err
	}
	s, _ := m.Read()
	return writeResult(env, map[string]any{
		"configured": true,
		"hasToken":   m.HasCloudflareToken(),
		"accountId":  s.CloudflareAccountID,
		"clientId":   s.TunnelClientID,
	}, func(w io.Writer) {
		fmt.Fprintln(w, "Saved Cloudflare tunnel settings.")
	})
}

func runTunnelRemoveConfig(ctx context.Context, env *Env, args []string) error {
	fs := newFlagSet("tunnel remove-config")
	var yes, deleteWorkers bool
	fs.BoolVar(&yes, "yes", false, "Confirm removing saved Cloudflare settings and token")
	fs.BoolVar(&deleteWorkers, "delete-workers", false, "Also delete deployed devbase-* Workers on Cloudflare")
	pos, err := parseFlags(fs, args)
	if err != nil {
		return err
	}
	if len(pos) > 0 {
		return usageErrorf("unexpected argument %q for tunnel remove-config", pos[0])
	}
	if !yes {
		return usageErrorf("removing Cloudflare tunnel configuration requires --yes")
	}
	if err := env.manager().RemoveCloudflareSettings(ctx, deleteWorkers); err != nil {
		return err
	}
	return writeResult(env, map[string]any{"removed": true, "deletedWorkers": deleteWorkers}, func(w io.Writer) {
		fmt.Fprintln(w, "Removed Cloudflare tunnel settings and API token.")
	})
}

func runTunnelDiscover(ctx context.Context, env *Env, args []string) error {
	if len(args) > 0 {
		return usageErrorf("unexpected argument %q for tunnel discover", args[0])
	}
	m := env.manager()
	if err := m.RefreshCloudflareMetadata(ctx); err != nil {
		return err
	}
	s, err := m.Read()
	if err != nil {
		return err
	}
	return writeResult(env, map[string]any{
		"accountId": s.CloudflareAccountID,
		"subdomain": s.CloudflareSubdomain,
		"zones":     s.CloudflareZones,
		"dns":       s.CloudflareDNS,
	}, func(w io.Writer) {
		fmt.Fprintf(w, "Account: %s | workers.dev subdomain: %s | Zones: %d | DNS suggestions: %d\n",
			s.CloudflareAccountID, s.CloudflareSubdomain, len(s.CloudflareZones), len(s.CloudflareDNS))
	})
}

func runTunnelSync(ctx context.Context, env *Env, args []string) error {
	if len(args) > 0 {
		return usageErrorf("unexpected argument %q for tunnel sync", args[0])
	}
	m := env.manager()
	if err := m.SyncTunnels(ctx); err != nil {
		return err
	}
	s, _ := m.Read()
	return writeResult(env, map[string]any{"synced": true, "workers": s.TunnelWorkers, "rules": s.ProxyRules}, func(w io.Writer) {
		fmt.Fprintf(w, "Synchronized %d Cloudflare tunnel worker(s).\n", len(s.TunnelWorkers))
	})
}

func runTunnelApply(ctx context.Context, env *Env, args []string) error {
	if len(args) > 0 {
		return usageErrorf("unexpected argument %q for tunnel apply", args[0])
	}
	if err := env.manager().ApplyTunnel(ctx); err != nil {
		return err
	}
	return writeResult(env, map[string]any{"applied": true}, func(w io.Writer) {
		fmt.Fprintln(w, "Started Cloudflare tunnel agent.")
	})
}

func runTunnelStop(ctx context.Context, env *Env, args []string) error {
	if len(args) > 0 {
		return usageErrorf("unexpected argument %q for tunnel stop", args[0])
	}
	if err := env.manager().StopTunnel(ctx); err != nil {
		return err
	}
	return writeResult(env, map[string]any{"stopped": true}, func(w io.Writer) {
		fmt.Fprintln(w, "Stopped Cloudflare tunnel agent.")
	})
}

func runTunnelRestart(ctx context.Context, env *Env, args []string) error {
	if len(args) > 0 {
		return usageErrorf("unexpected argument %q for tunnel restart", args[0])
	}
	if err := env.manager().RestartTunnel(ctx); err != nil {
		return err
	}
	return writeResult(env, map[string]any{"restarted": true}, func(w io.Writer) {
		fmt.Fprintln(w, "Restarted Cloudflare tunnel agent.")
	})
}

func runTunnelCheck(ctx context.Context, env *Env, args []string) error {
	if len(args) != 1 {
		return usageErrorf("usage: devbase tunnel check <name-or-domain>")
	}
	domain := devplatform.LocalDomain(args[0])
	m := env.manager()
	fp, rev, err := m.TunnelRuleFingerprint(domain)
	if err != nil {
		return err
	}
	res, err := m.CheckTunnelRuleHTTP(ctx, domain, rev, fp)
	if err != nil {
		return err
	}
	return writeResult(env, res, func(w io.Writer) {
		if res.Failure != "" {
			fmt.Fprintf(w, "Tunnel %s: FAILED (%s, status=%d, duration=%s)\n", res.Domain, res.Failure, res.StatusCode, res.Duration)
			return
		}
		fmt.Fprintf(w, "Tunnel %s: HTTP %d (%s)\n", res.Domain, res.StatusCode, res.Duration)
	})
}

func runTunnelAccessToken(ctx context.Context, env *Env, args []string) error {
	fs := newFlagSet("tunnel access-token")
	var rotate bool
	fs.BoolVar(&rotate, "rotate", false, "Generate a fresh access token")
	pos, err := parseFlags(fs, args)
	if err != nil {
		return err
	}
	if len(pos) != 1 {
		return usageErrorf("usage: devbase tunnel access-token <name-or-domain> [--rotate]")
	}
	domain := devplatform.LocalDomain(pos[0])
	m := env.manager()
	var token string
	if rotate {
		token, err = m.RotateTunnelAccessToken(ctx, domain)
	} else {
		token, err = m.EnsureTunnelAccessToken(domain)
	}
	if err != nil {
		return err
	}
	return writeResult(env, map[string]any{"domain": domain, "token": token, "rotated": rotate}, func(w io.Writer) {
		fmt.Fprintln(w, token)
	})
}

func runCACommand(ctx context.Context, env *Env, args []string) error {
	if hasHelpFlag(args) && len(args) == 1 {
		printTopicHelp(env.Out, "ca")
		return nil
	}
	sub := "status"
	if len(args) > 0 {
		sub = args[0]
		args = args[1:]
	}
	if hasHelpFlag(args) {
		printTopicHelp(env.Out, "ca")
		return nil
	}
	m := env.manager()
	resolveRevision := func(rev string) (string, error) {
		if strings.TrimSpace(rev) != "" {
			return strings.TrimSpace(rev), nil
		}
		st, err := m.AuthorityStatus(ctx)
		if err != nil {
			return "", err
		}
		return st.Revision, nil
	}

	switch sub {
	case "status":
		if len(args) > 0 {
			return usageErrorf("unexpected argument %q for ca status", args[0])
		}
		st, err := m.AuthorityStatus(ctx)
		if err != nil {
			return err
		}
		return writeResult(env, st, func(w io.Writer) {
			fmt.Fprintf(w, "Active CA: %s (trust: %s, usable: %t, fp: %s, rev: %s)\n", st.Active.Status, st.Active.Trust, st.Active.Usable, st.Active.Fingerprint, st.Revision)
			if st.Active.Detail != "" {
				fmt.Fprintf(w, "  %s\n", st.Active.Detail)
			}
			if st.Prepared != nil {
				fmt.Fprintf(w, "Prepared CA: %s (trust: %s, fp: %s)\n", st.Prepared.Status, st.Prepared.Trust, st.Prepared.Fingerprint)
			}
			if st.Previous != nil {
				fmt.Fprintf(w, "Previous CA: %s (trust: %s, fp: %s)\n", st.Previous.Status, st.Previous.Trust, st.Previous.Fingerprint)
			}
			for _, arch := range st.Archives {
				fmt.Fprintf(w, "Archived %s: trust=%s, fp=%s\n", arch.ID, arch.Trust, arch.Fingerprint)
			}
		})
	case "trust":
		if len(args) > 0 {
			return usageErrorf("unexpected argument %q for ca trust", args[0])
		}
		if err := m.ConfigureTrust(ctx, false); err != nil {
			return err
		}
		st, _ := m.AuthorityStatus(ctx)
		return writeResult(env, map[string]any{"trusted": true, "authority": st}, func(w io.Writer) {
			fmt.Fprintf(w, "Local CA trust state: %s\n", st.Active.Trust)
		})
	case "untrust":
		fs := newFlagSet("ca untrust")
		var yes bool
		fs.BoolVar(&yes, "yes", false, "Confirm removing active local CA trust from the login keychain")
		pos, err := parseFlags(fs, args)
		if err != nil {
			return err
		}
		if len(pos) > 0 {
			return usageErrorf("unexpected argument %q for ca untrust", pos[0])
		}
		if !yes {
			return usageErrorf("removing local CA trust requires --yes")
		}
		if err := m.ConfigureTrust(ctx, true); err != nil {
			return err
		}
		st, _ := m.AuthorityStatus(ctx)
		return writeResult(env, map[string]any{"untrusted": true, "authority": st}, func(w io.Writer) {
			fmt.Fprintf(w, "Local CA trust state: %s\n", st.Active.Trust)
		})
	case "prepare":
		if len(args) > 0 {
			return usageErrorf("unexpected argument %q for ca prepare", args[0])
		}
		if err := m.PrepareAuthority(ctx); err != nil {
			return err
		}
		st, err := m.AuthorityStatus(ctx)
		if err != nil {
			return err
		}
		return writeResult(env, st, func(w io.Writer) {
			fp := ""
			if st.Prepared != nil {
				fp = st.Prepared.Fingerprint
			}
			fmt.Fprintf(w, "Prepared replacement CA (fingerprint: %s, revision: %s).\n", fp, st.Revision)
		})
	case "trust-prepared", "activate", "discard-prepared":
		fs := newFlagSet("ca " + sub)
		var rev string
		fs.StringVar(&rev, "revision", "", "Expected authority status revision (auto-fetched when omitted)")
		pos, err := parseFlags(fs, args)
		if err != nil {
			return err
		}
		if len(pos) > 0 {
			return usageErrorf("unexpected argument %q for ca %s", pos[0], sub)
		}
		resolvedRev, err := resolveRevision(rev)
		if err != nil {
			return err
		}
		switch sub {
		case "trust-prepared":
			err = m.TrustPreparedAuthority(ctx, resolvedRev)
		case "activate":
			err = m.ActivateAuthority(ctx, resolvedRev)
		case "discard-prepared":
			err = m.DiscardPreparedAuthority(ctx, resolvedRev)
		}
		if err != nil {
			return err
		}
		st, _ := m.AuthorityStatus(ctx)
		return writeResult(env, map[string]any{"action": sub, "revision": resolvedRev, "authority": st}, func(w io.Writer) {
			fmt.Fprintf(w, "Completed ca %s (revision %s).\n", sub, resolvedRev)
		})
	case "restore":
		fs := newFlagSet("ca restore")
		var fp, rev string
		fs.StringVar(&fp, "fingerprint", "", "Certificate SHA-1 fingerprint of saved generation (required)")
		fs.StringVar(&rev, "revision", "", "Expected authority status revision (auto-fetched when omitted)")
		pos, err := parseFlags(fs, args)
		if err != nil {
			return err
		}
		if len(pos) != 1 || strings.TrimSpace(fp) == "" {
			return usageErrorf("usage: devbase ca restore <generation-id> --fingerprint <fp> [--revision <rev>]")
		}
		resolvedRev, err := resolveRevision(rev)
		if err != nil {
			return err
		}
		if err := m.RestoreAuthority(ctx, pos[0], fp, resolvedRev); err != nil {
			return err
		}
		st, _ := m.AuthorityStatus(ctx)
		return writeResult(env, map[string]any{"restored": pos[0], "authority": st}, func(w io.Writer) {
			fmt.Fprintf(w, "Restored CA generation %s.\n", pos[0])
		})
	case "saved-trust":
		fs := newFlagSet("ca saved-trust")
		var fp string
		var remove bool
		fs.StringVar(&fp, "fingerprint", "", "Certificate SHA-1 fingerprint of saved generation (required)")
		fs.BoolVar(&remove, "remove", false, "Remove trust instead of trusting")
		pos, err := parseFlags(fs, args)
		if err != nil {
			return err
		}
		if len(pos) != 1 || strings.TrimSpace(fp) == "" {
			return usageErrorf("usage: devbase ca saved-trust <generation-id> --fingerprint <fp> [--remove]")
		}
		if err := m.ConfigureSavedAuthorityTrust(ctx, pos[0], fp, remove); err != nil {
			return err
		}
		st, _ := m.AuthorityStatus(ctx)
		return writeResult(env, map[string]any{"id": pos[0], "remove": remove, "authority": st}, func(w io.Writer) {
			fmt.Fprintf(w, "Updated trust for saved CA generation %s (remove=%t).\n", pos[0], remove)
		})
	case "remove-archived-trust":
		fs := newFlagSet("ca remove-archived-trust")
		var fp string
		var yes bool
		fs.StringVar(&fp, "fingerprint", "", "Certificate SHA-1 fingerprint of archived CA (required)")
		fs.BoolVar(&yes, "yes", false, "Confirm removing trust for archived CA")
		pos, err := parseFlags(fs, args)
		if err != nil {
			return err
		}
		if len(pos) != 1 || strings.TrimSpace(fp) == "" {
			return usageErrorf("usage: devbase ca remove-archived-trust <archive-id> --fingerprint <fp> --yes")
		}
		if !yes {
			return usageErrorf("removing archived CA trust requires --yes")
		}
		if err := m.RemoveArchivedAuthorityTrust(ctx, pos[0], fp); err != nil {
			return err
		}
		st, _ := m.AuthorityStatus(ctx)
		return writeResult(env, map[string]any{"removed": pos[0], "authority": st}, func(w io.Writer) {
			fmt.Fprintf(w, "Removed trust for archived CA %s.\n", pos[0])
		})
	default:
		return usageErrorf("unknown ca subcommand %q", sub)
	}
}

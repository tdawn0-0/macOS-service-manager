package main

import (
	"context"
	"fmt"
	"github.com/egoist/mygo/ui"
	"devbase/internal/devplatform"
	"strings"
	"time"
)

// devProxyActivation observes a saved rule without changing routing or the upstream.
func (a *app) devProxyActivation(c *ui.Context, domain string) {
	d := &a.dev
	domain = strings.ToLower(strings.TrimSpace(domain))
	if d.diagnosticReport != nil && d.diagnosticReport.ProxyRevision == d.state.ProxyRulesRevision {
		for _, check := range d.diagnosticReport.Checks {
			if check.Resource == domain && (check.Layer == "DNS" || check.Layer == "Proxy TLS" || check.Layer == "Upstream") {
				ui.Text(c, check.Layer+": "+check.Status+" · "+check.Summary).FontSize(textCaption).TextColor(c.Theme().TextMuted).Padding(0, 2)
			}
		}
	}
	if d.proxyChecking[domain] {
		ui.Text(c, "Checking HTTPS response…").FontSize(textCaption)
	} else if r, ok := d.proxyChecks[domain]; ok && r.Revision == d.state.ProxyRulesRevision {
		if !r.Current(time.Now(), d.state.ProxyRulesRevision, r.Fingerprint) {
			ui.Text(c, "HTTP observation expired; check again.").FontSize(textCaption)
		} else if r.Failure != "" {
			ui.Text(c, "HTTP check: "+r.Failure).FontSize(textCaption)
		} else {
			ui.Text(c, fmt.Sprintf("HTTP response: %d · %s ago", r.StatusCode, time.Since(r.CheckedAt).Round(time.Second))).FontSize(textCaption)
		}
	}
	if btn(c.Key("check-https-"+domain), "Check HTTPS response", "", icoStetho, btnText).AlignSelf(ui.Start).Disabled(d.busy || d.proxyDraftDirty() || d.proxyChecking[domain]).Clicked() {
		m := a.devManager()
		fp, revision, err := m.ProxyRuleFingerprint(domain)
		if err != nil {
			d.output = "The saved HTTPS rule changed. Reload before checking."
			return
		}
		// A newer check supersedes an older in-flight observation.
		d.proxyChecking = map[string]bool{domain: true}
		if d.proxyChecks == nil {
			d.proxyChecks = map[string]devplatform.ProxyHTTPResult{}
		}
		d.proxyCheckGeneration++
		generation := d.proxyCheckGeneration
		delete(d.proxyChecks, domain)
		a.spawn(func() {
			result, checkErr := m.CheckProxyRuleHTTP(context.Background(), domain, revision, fp)
			a.post(func() {
				latestFP, latestRevision, latestErr := a.devManager().ProxyRuleFingerprint(domain)
				a.dev.acceptProxyHTTPResult(domain, generation, revision, fp, result, checkErr, latestFP, latestRevision, latestErr)
			})
		})
	}
}

func (d *developmentState) acceptProxyHTTPResult(domain string, generation, revision uint64, fingerprint string, result devplatform.ProxyHTTPResult, checkErr error, latestFingerprint string, latestRevision uint64, latestErr error) {
	if generation != d.proxyCheckGeneration {
		return
	}
	delete(d.proxyChecking, domain)
	if checkErr != nil {
		d.output = "HTTPS rule changed; check again."
		return
	}
	if latestErr != nil || latestRevision != revision || latestFingerprint != fingerprint {
		return
	}
	d.proxyChecks[domain] = result
}

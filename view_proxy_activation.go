package main

import (
	"context"
	"devbase/internal/devplatform"
	"fmt"
	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/display"
	"github.com/lobehub/lobe-mygo/pkg/theme"
	"strings"
	"time"
)

// checkTag draws one observation of a rule as a small tag in its color.
func checkTag(c *ui.Context, text, status string) {
	color := display.TagColor("")
	switch status {
	case "OK":
		color = display.TagSuccess
	case "Warning":
		color = display.TagWarning
	case "Failed":
		color = display.TagError
	}
	devTag(c, text, color)
}

// devProxyActivation observes a saved rule without changing routing or the upstream.
// Each observation is one tag; only what is not OK gets a line of explanation.
func (a *app) devProxyActivation(c *ui.Context, domain string) {
	d := &a.dev
	domain = strings.ToLower(strings.TrimSpace(domain))
	tk := theme.TokensOf(c)
	var problems []devplatform.DiagnosticCheck
	var observed []devplatform.DiagnosticCheck
	if d.diagnosticReport != nil && d.diagnosticReport.ProxyRevision == d.state.ProxyRulesRevision {
		for _, check := range d.diagnosticReport.Checks {
			if check.Resource == domain && (check.Layer == "DNS" || check.Layer == "Proxy TLS" || check.Layer == "Upstream") {
				observed = append(observed, check)
				if check.Status != "OK" {
					problems = append(problems, check)
				}
			}
		}
	}
	httpText, httpStatus := "", ""
	if d.proxyChecking[domain] {
		httpText, httpStatus = "Checking HTTPS…", "Checking"
	} else if r, ok := d.proxyChecks[domain]; ok && r.Revision == d.state.ProxyRulesRevision {
		switch {
		case !r.Current(time.Now(), d.state.ProxyRulesRevision, r.Fingerprint):
			httpText, httpStatus = "HTTP result expired", "Warning"
		case r.Failure != "":
			httpText, httpStatus = "HTTP failed", "Failed"
			problems = append(problems, devplatform.DiagnosticCheck{Layer: "HTTP", Status: "Failed", Summary: r.Failure})
		default:
			httpText, httpStatus = fmt.Sprintf("HTTP %d · %s ago", r.StatusCode, time.Since(r.CheckedAt).Round(time.Second)), "OK"
		}
	}
	ui.Row(c).Wrap().Gap(6).AlignItems(ui.Center).Children(func() {
		for _, check := range observed {
			checkTag(c, check.Layer, check.Status)
		}
		if httpText != "" {
			checkTag(c, httpText, httpStatus)
		}
		ui.Spacer(c)
		if btn(c.Key("check-https-"+domain), "Check HTTPS response", icoStetho, btnText).Disabled(d.busy || d.proxyDraftDirty() || d.proxyChecking[domain]).Clicked() {
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
	})
	for _, check := range problems {
		ui.Text(c, check.Layer+" · "+check.Summary).FontSize(textCaption).TextColor(tk.TextDescription).MinWidth(0)
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

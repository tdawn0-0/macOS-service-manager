package main

import (
	"fmt"
	"strconv"

	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/action"
	"github.com/lobehub/lobe-mygo/pkg/theme"
	"macos-service-manager/internal/devplatform"
)

func (a *app) devHTTPS(c *ui.Context) {
	d := &a.dev
	if d.authorityOpen {
		a.devAuthority(c)
		return
	}
	if !d.proxyDraftLoaded {
		d.loadProxyDraft(d.state)
	}
	ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
		ui.Text(c, "HTTPS proxy rules").FontSize(18).FontWeight(650).Grow(1).MinWidth(0)
		if action.Button(c, "Local certificates", action.ButtonProps{Variant: action.Default, Size: theme.Small}).Disabled(d.busy).Clicked() {
			d.authorityOpen = true
			a.authorityTask(nil)
		}
	})
	ui.Text(c, "Each domain forwards to one local HTTP port. Save changes before Apply; upstream apps stay in your terminal or IDE.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
	state, stateErr := a.devManager().ProxyPublicationState()
	if stateErr != nil {
		state = "Unknown"
	}
	ui.Text(c, "Configuration: "+state).FontSize(textSmall).TextColor(c.Theme().TextMuted)
	switch state {
	case "Pending":
		ui.Text(c, "Saved changes are waiting for Apply proxy.").FontSize(textCaption)
	case "Unknown":
		ui.Text(c, "Publication is unknown. Review and apply the saved configuration.").FontSize(textCaption)
	case "Published":
		ui.Text(c, "Configuration published. Check DNS, certificates and upstream separately.").FontSize(textCaption)
	}
	devField(c, "HTTPS listener port", &d.proxyListenerPort, "443")
	a.devPortControls(c, &d.proxyListenerPort, "HTTPS")
	removeRule := -1
	for i := range d.proxyDraft {
		row := &d.proxyDraft[i]
		ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
			ui.TextInput(c, &row.Domain).Placeholder("app.test").Label("Domain")
			ui.TextInput(c, &row.LocalPort).Placeholder("3000").Label("Local port")
			if !d.proxyDraftDirty() && action.Button(c, "Open HTTPS URL", action.ButtonProps{Variant: action.Default, Size: theme.Small}).Clicked() {
				url := "https://" + row.Domain
				if d.state.ProxyListenerPort != 443 {
					url = fmt.Sprintf("%s:%d", url, d.state.ProxyListenerPort)
				}
				a.devTask(func(m *devplatform.Manager) (string, error) {
					_, err := m.Run(m.Context, "/usr/bin/open", url)
					return "Opened saved HTTPS rule in browser.", err
				})
			}
			if action.Button(c, "Remove rule", action.ButtonProps{Variant: action.Default, Size: theme.Small}).Clicked() {
				removeRule = i
			}
		})
		if !d.proxyDraftDirty() {
			a.devProxyActivation(c, row.Domain)
		}
	}
	if removeRule >= 0 {
		d.proxyDraft = append(d.proxyDraft[:removeRule], d.proxyDraft[removeRule+1:]...)
	}
	if action.Button(c, "Add rule", action.ButtonProps{Variant: action.Default, Size: theme.Small}).Clicked() {
		d.proxyDraft = append(d.proxyDraft, proxyRuleDraft{LocalPort: "3000"})
	}
	if action.Button(c, "Discard changes", action.ButtonProps{Variant: action.Default, Size: theme.Small}).Disabled(d.busy || !d.proxyDraftDirty()).Clicked() {
		d.loadProxyDraft(d.state)
	}
	ui.Row(c).Gap(8).Children(func() {
		if action.Button(c, "Save HTTPS rules", action.ButtonProps{Variant: action.Primary, Size: theme.Small}).Grow(1).Disabled(d.busy || !d.proxyDraftDirty()).Clicked() {
			listener, err := strconv.Atoi(d.proxyListenerPort)
			rules := make([]devplatform.ProxyRule, 0, len(d.proxyDraft))
			for _, row := range d.proxyDraft {
				port, e := strconv.Atoi(row.LocalPort)
				if e != nil {
					err = e
					break
				}
				rules = append(rules, devplatform.ProxyRule{Domain: row.Domain, LocalPort: port})
			}
			if err != nil {
				d.output = "Error: Enter valid numeric ports"
			} else {
				revision := d.proxyDraftRevision
				a.devTaskThen(func(m *devplatform.Manager) (string, error) {
					return "Saved HTTPS rules. Apply proxy to activate changes.", m.ReplaceProxyRules(m.Context, revision, listener, rules)
				}, func(err error) {
					if err == nil {
						a.dev.loadProxyDraft(a.dev.state)
					}
				})
			}
		}
		if action.Button(c, "Apply proxy", action.ButtonProps{Variant: action.Default, Size: theme.Small}).Disabled(d.busy || d.proxyDraftDirty()).Clicked() {
			a.devTask(func(m *devplatform.Manager) (string, error) {
				err := m.ApplyProxy(m.Context)
				return "Proxy configuration published. Check DNS, TLS and upstream separately.", err
			})
		}
	})
	if d.proxyDraftDirty() {
		ui.Text(c, "Save or discard changes before applying the proxy.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
	}
	devDivider(c, "Proxy")
	for _, act := range []string{"Restart HTTPS proxy", "Stop HTTPS proxy", "Configure local domains", "Trust local CA", "Remove CA trust", "Diagnose HTTPS"} {
		act := act
		if action.Button(c, act, action.ButtonProps{Variant: action.Default, Size: theme.Small}).AlignSelf(ui.Start).Disabled(d.busy || d.proxyDraftDirty()).Clicked() {
			a.devTask(func(m *devplatform.Manager) (string, error) {
				var e error
				switch act {
				case "Restart HTTPS proxy":
					e = m.RestartProxy(m.Context)
				case "Stop HTTPS proxy":
					e = m.StopProxy(m.Context)
				case "Configure local domains":
					e = m.ConfigureHosts(m.Context)
				case "Trust local CA":
					e = m.ConfigureTrust(m.Context, false)
				case "Remove CA trust":
					e = m.ConfigureTrust(m.Context, true)
				case "Diagnose HTTPS":
					report, err := m.Diagnose(m.Context)
					if err == nil {
						a.post(func() { a.showDevDiagnostics(report) })
					}
					return "Diagnostics finished. Review checks by layer.", err
				}
				return act, e
			})
		}
	}
}

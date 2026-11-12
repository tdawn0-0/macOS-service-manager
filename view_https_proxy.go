package main

import (
	"fmt"
	"strconv"

	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/display"
	"github.com/lobehub/lobe-mygo/pkg/theme"
	"devbase/internal/devplatform"
)

// devHTTPS is the single page of the HTTPS proxy: the saved table of
// Domain → Local port rules with its listener, then what publishes and
// serves it, then the local domains and trust it relies on.
func (a *app) devHTTPS(c *ui.Context) {
	d := &a.dev
	if d.authorityOpen {
		a.devAuthority(c)
		return
	}
	if !d.proxyDraftLoaded {
		d.loadProxyDraft(d.state)
	}
	tk := theme.TokensOf(c)
	dirty := d.proxyDraftDirty()
	state := d.proxyPublicationState
	if state == "" {
		state = "Unknown"
	}
	listener := d.state.ProxyListenerPort
	if listener == 0 {
		listener = 443
	}
	pageHeader(c, icoShield, ui.RGB(10, 132, 255), "HTTPS proxy rules", fmt.Sprintf("%s · listener :%d", plural(len(d.state.ProxyRules), "rule"), listener), func() {
		statusTag(c, state)
	}, func() {
		if btn(c, "Apply proxy", icoPlay, btnPrimary).Disabled(d.busy || dirty).Clicked() {
			a.devTask(func(m *devplatform.Manager) (string, error) {
				err := m.ApplyProxy(m.Context)
				return "Proxy configuration published. Check DNS, TLS and upstream separately.", err
			})
		}
	})

	devCardWith(c, icoRoute, "Rules", "", func() {
		if btn(c, "Add rule", icoPlus, btnText).Clicked() {
			d.proxyDraft = append(d.proxyDraft, proxyRuleDraft{LocalPort: "3000"})
		}
	}, func() {
		removeRule := -1
		if len(d.proxyDraft) == 0 {
			display.Empty(c, display.EmptyProps{Icon: icoRoute, Title: "No rules"})
		}
		for i := range d.proxyDraft {
			row := &d.proxyDraft[i]
			ui.Column(c).Gap(6).Children(func() {
				ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
					mono(ui.TextInput(c, &row.Domain).Placeholder("app.test").Label("Domain").Grow(1).MinWidth(0))
					// The route: the domain goes to a port of this Mac.
					ui.Row(c).Width(196).Shrink(0).Gap(8).AlignItems(ui.Center).Children(func() {
						ui.Icon(c, icoArrow).FontSize(14).TextColor(tk.TextQuaternary).Shrink(0)
						mono(ui.Text(c, "127.0.0.1:").FontSize(textSmall).TextColor(tk.TextTertiary).SingleLine().Shrink(0))
						mono(ui.TextInput(c, &row.LocalPort).Placeholder("3000").Label("Local port").Grow(1).MinWidth(0))
					})
					ui.Row(c).Width(64).Shrink(0).Gap(4).Justify(ui.End).Children(func() {
						if !dirty && iconButton(c, icoLink, "Open HTTPS URL", tk.TextSecondary).Clicked() {
							url := "https://" + row.Domain
							if d.state.ProxyListenerPort != 443 {
								url = fmt.Sprintf("%s:%d", url, d.state.ProxyListenerPort)
							}
							a.devTask(func(m *devplatform.Manager) (string, error) {
								_, err := m.Run(m.Context, "/usr/bin/open", url)
								return "Opened saved HTTPS rule in browser.", err
							})
						}
						if action := iconButton(c, icoTrash, "Remove rule", tk.TextSecondary); action.Clicked() {
							removeRule = i
						}
					})
				})
				if !dirty {
					a.devProxyActivation(c, row.Domain)
				}
			})
		}
		if removeRule >= 0 {
			d.proxyDraft = append(d.proxyDraft[:removeRule], d.proxyDraft[removeRule+1:]...)
		}
		devFieldGrid(c, func() {
			devField(c, "HTTPS listener port", &d.proxyListenerPort, "443")
		})
		a.devPortControls(c, &d.proxyListenerPort, "HTTPS")
		ui.Row(c).Wrap().Gap(8).AlignItems(ui.Center).Margin(4, 0, 0).Children(func() {
			if dirty {
				ui.Row(c).Gap(6).AlignItems(ui.Center).Grow(1).MinWidth(0).Children(func() {
					ui.Box(c).Size(6, 6).Radius(3).Background(tk.Warning).Shrink(0)
					ui.Text(c, "Unsaved changes").FontSize(textSmall).TextColor(tk.TextDescription).SingleLine()
				})
			} else {
				ui.Spacer(c)
			}
			if btn(c, "Discard changes", icoUndo, btnDefault).Disabled(d.busy || !dirty).Clicked() {
				d.loadProxyDraft(d.state)
			}
			if btn(c, "Save HTTPS rules", icoSave, btnPrimary).Disabled(d.busy || !dirty).Clicked() {
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
		})
	})

	proxyAction := func(act string) {
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
	devCardWith(c, iconServer, "Proxy", "", nil, func() {
		devButtonRow(c, func() {
			for _, act := range []struct {
				label string
				icon  *ui.SVG
			}{{"Restart HTTPS proxy", icoRotate}, {"Stop HTTPS proxy", icoStop}, {"Diagnose HTTPS", icoStetho}} {
				if btn(c, act.label, act.icon, btnDefault).Disabled(d.busy || dirty).Clicked() {
					proxyAction(act.label)
				}
			}
		})
	})
	devCardWith(c, icoKey, "Local domains & trust", "", func() {
		if btn(c, "Local certificates", icoArrow, btnText).Disabled(d.busy).Clicked() {
			d.authorityOpen = true
			a.authorityTask(nil)
		}
	}, func() {
		devButtonRow(c, func() {
			for _, act := range []struct {
				label string
				icon  *ui.SVG
				kind  btnKind
			}{{"Configure local domains", icoGlobe, btnDefault}, {"Trust local CA", icoShield, btnDefault}, {"Remove CA trust", nil, btnText}} {
				if btn(c, act.label, act.icon, act.kind).Disabled(d.busy || dirty).Clicked() {
					proxyAction(act.label)
				}
			}
		})
	})
}

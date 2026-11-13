package main

import (
	"fmt"
	"strconv"

	"devbase/internal/devplatform"
	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/choice"
	"github.com/lobehub/lobe-mygo/pkg/display"
	"github.com/lobehub/lobe-mygo/pkg/theme"
)

// devHTTPS is the single page of the HTTPS proxy: the saved table of
// Domain → Local port rules with its listener, background proxy & Cloudflare
// tunnel switches, and local domains/CA trust.
func (a *app) devHTTPS(c *ui.Context) {
	d := &a.dev
	if d.authorityOpen {
		a.devAuthority(c)
		return
	}
	if !d.proxyDraftLoaded {
		d.loadProxyDraft(d.state)
	}
	if !d.tunnelDraftLoaded {
		d.loadTunnelDraft(d.state)
	}
	tk := theme.TokensOf(c)
	dirty := d.proxyDraftDirty()
	tunnelDirty := d.tunnelDraftDirty()
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
					ui.Row(c).Width(188).Shrink(0).Gap(8).AlignItems(ui.Center).Children(func() {
						ui.Icon(c, icoArrow).FontSize(14).TextColor(tk.TextQuaternary).Shrink(0)
						mono(ui.Text(c, "127.0.0.1:").FontSize(textSmall).TextColor(tk.TextTertiary).SingleLine().Shrink(0))
						mono(ui.TextInput(c, &row.LocalPort).Placeholder("3000").Label("Local port").Grow(1).MinWidth(0))
					})
					ui.Row(c).Shrink(0).Gap(6).AlignItems(ui.Center).Children(func() {
						choice.Switch(c, &row.Tunnel, choice.SwitchProps{Size: theme.Small}).Label(fmt.Sprintf("Tunnel rule %d", i+1)).Tooltip("Expose this rule through the Cloudflare tunnel")
						ui.Text(c, "Tunnel").FontSize(textCaption).TextColor(tk.TextSecondary).SingleLine().Shrink(0)
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
				if row.Tunnel {
					subPlaceholder := devplatform.DefaultTunnelSubdomain(row.Domain)
					if subPlaceholder == "" {
						subPlaceholder = "subdomain"
					}
					ui.Row(c).Gap(8).AlignItems(ui.Center).Padding(0, 0, 0, 12).Children(func() {
						ui.Icon(c, icoGlobe).FontSize(13).TextColor(tk.TextTertiary).Shrink(0)
						mono(ui.TextInput(c, &row.TunnelSubdomain).Placeholder(subPlaceholder).Label(fmt.Sprintf("Tunnel prefix %d", i+1)).Width(160))
						if d.state.TunnelDomain != "" {
							mono(ui.Text(c, "."+d.state.TunnelDomain).FontSize(textSmall).TextColor(tk.TextTertiary).SingleLine().Grow(1).MinWidth(0))
						} else {
							ui.Text(c, "Set wildcard domain below").FontSize(textCaption).TextColor(tk.TextTertiary).SingleLine().Grow(1).MinWidth(0)
						}
						if !dirty && d.state.TunnelDomain != "" {
							publicHost := (devplatform.ProxyRule{Domain: row.Domain, Tunnel: true, TunnelSubdomain: row.TunnelSubdomain}).PublicHost(d.state.TunnelDomain)
							if publicHost != "" {
								publicURL := "https://" + publicHost
								if iconButton(c, icoCopy, "Copy tunnel URL", tk.TextSecondary).Clicked() {
									if a.copy != nil {
										a.copy(publicURL)
									}
									d.output = "Copied " + publicURL
								}
								if iconButton(c, icoLink, "Open tunnel URL", tk.TextSecondary).Clicked() {
									a.devTask(func(m *devplatform.Manager) (string, error) {
										_, err := m.Run(m.Context, "/usr/bin/open", publicURL)
										return "Opened tunnel URL in browser.", err
									})
								}
							}
						}
					})
				}
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
					rules = append(rules, devplatform.ProxyRule{
						Domain:          row.Domain,
						LocalPort:       port,
						Tunnel:          row.Tunnel,
						TunnelSubdomain: row.TunnelSubdomain,
					})
				}
				if err != nil {
					d.output = "Error: Enter valid numeric ports"
				} else {
					revision := d.proxyDraftRevision
					a.devTaskThen(func(m *devplatform.Manager) (string, error) {
						return "Saved HTTPS rules. Apply proxy or tunnel to activate changes.", m.ReplaceProxyRules(m.Context, revision, listener, rules)
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
	proxyStatus := d.statuses["proxy"]
	if proxyStatus == "" {
		proxyStatus = "Stopped"
		if len(d.state.ProxyRules) > 0 {
			proxyStatus = "Unknown"
		}
	}
	proxyOn := proxyStatus == "Running"
	devCardWith(c, iconServer, "Proxy", proxyStatus, func() {
		if choice.Switch(c, &proxyOn, choice.SwitchProps{Size: theme.Small}).Label("HTTPS proxy service").Disabled(d.busy || dirty || (len(d.state.ProxyRules) == 0 && !proxyOn)).Tooltip("Start or stop the background HTTPS proxy launchd service").Changed() {
			a.devTask(func(m *devplatform.Manager) (string, error) {
				if proxyOn {
					if err := m.EnsureLogMaintenance(m.Context); err != nil {
						return "HTTPS proxy start", err
					}
					return "Proxy configuration published. Check DNS, TLS and upstream separately.", m.ApplyProxy(m.Context)
				}
				return "Stopped HTTPS proxy", m.StopProxy(m.Context)
			})
		}
	}, func() {
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

	tunnelStatus := d.statuses["tunnel"]
	if tunnelStatus == "" {
		tunnelStatus = "Stopped"
	}
	tunnelOn := tunnelStatus == "Running"
	tunnelNote := tunnelStatus
	if detail := d.statuses["tunnel:detail"]; tunnelOn && detail != "" {
		tunnelNote = detail
	}
	devCardWith(c, icoGlobe, "Cloudflare tunnel", tunnelNote, func() {
		if choice.Switch(c, &tunnelOn, choice.SwitchProps{Size: theme.Small}).Label("Cloudflare tunnel service").Disabled(d.busy || tunnelDirty || (d.state.TunnelDomain == "" && d.state.TunnelEndpoint == "" && !tunnelOn)).Tooltip("Start or stop the background Cloudflare tunnel launchd service").Changed() {
			a.devTask(func(m *devplatform.Manager) (string, error) {
				if tunnelOn {
					return "Cloudflare tunnel started.", m.ApplyTunnel(m.Context)
				}
				return "Stopped Cloudflare tunnel", m.StopTunnel(m.Context)
			})
		}
	}, func() {
		devFieldGrid(c, func() {
			devField(c, "Wildcard domain", &d.tunnelDomain, "example.com")
			devField(c, "Client ID", &d.tunnelClientID, "mac-auto")
			devField(c, "Worker URL", &d.tunnelEndpoint, "https://devbase-gateway.example.com")
			secretPlaceholder := "Paste or generate (16+ chars)"
			if d.statuses["tunnel:secret"] == "Configured" {
				secretPlaceholder = "Saved (paste to replace)"
			}
			devField(c, "Tunnel secret", &d.tunnelSecretDraft, secretPlaceholder)
		})
		ui.Row(c).Wrap().Gap(8).AlignItems(ui.Center).Children(func() {
			if tunnelDirty {
				ui.Row(c).Gap(6).AlignItems(ui.Center).Grow(1).MinWidth(0).Children(func() {
					ui.Box(c).Size(6, 6).Radius(3).Background(tk.Warning).Shrink(0)
					ui.Text(c, "Unsaved tunnel changes").FontSize(textSmall).TextColor(tk.TextDescription).SingleLine()
				})
			} else {
				ui.Spacer(c)
			}
			if btn(c, "Discard tunnel changes", icoUndo, btnDefault).Disabled(d.busy || !tunnelDirty).Clicked() {
				d.loadTunnelDraft(d.state)
			}
			if btn(c, "Save tunnel settings", icoSave, btnPrimary).Disabled(d.busy || !tunnelDirty).Clicked() {
				domain, clientID, endpoint, secret := d.tunnelDomain, d.tunnelClientID, d.tunnelEndpoint, d.tunnelSecretDraft
				a.devTaskThen(func(m *devplatform.Manager) (string, error) {
					return "Saved Cloudflare tunnel settings.", m.SaveTunnelSettings(m.Context, domain, clientID, endpoint, secret)
				}, func(err error) {
					if err == nil {
						a.dev.loadTunnelDraft(a.dev.state)
					}
				})
			}
		})
		devButtonRow(c, func() {
			if btn(c, "Apply tunnel", icoPlay, btnDefault).Disabled(d.busy || dirty || tunnelDirty || (d.state.TunnelDomain == "" && d.state.TunnelEndpoint == "")).Clicked() {
				a.devTask(func(m *devplatform.Manager) (string, error) {
					return "Cloudflare tunnel routes published.", m.ApplyTunnel(m.Context)
				})
			}
			if btn(c, "Restart tunnel", icoRotate, btnDefault).Disabled(d.busy || !tunnelOn).Clicked() {
				a.devTask(func(m *devplatform.Manager) (string, error) {
					return "Restarted Cloudflare tunnel.", m.RestartTunnel(m.Context)
				})
			}
			if btn(c, "Generate secret", icoKey, btnDefault).Disabled(d.busy).Clicked() {
				a.devTask(func(m *devplatform.Manager) (string, error) {
					return "Generated new tunnel secret. Copy it to wrangler secret put TUNNEL_SECRET.", m.GenerateTunnelSecret(m.Context)
				})
			}
			if btn(c, "Copy secret", icoCopy, btnDefault).Disabled(d.busy || d.statuses["tunnel:secret"] != "Configured").Clicked() {
				a.devTask(func(m *devplatform.Manager) (string, error) {
					secret, err := m.ReadTunnelSecret()
					if err == nil && a.copy != nil {
						a.post(func() { a.copy(secret) })
					}
					return "Copied tunnel secret to clipboard.", err
				})
			}
			if btn(c, "Export Worker", icoDownload, btnDefault).Disabled(d.busy).Clicked() {
				a.devTask(func(m *devplatform.Manager) (string, error) {
					dir, err := m.ExportTunnelWorker(m.Context)
					if err == nil {
						_, _ = m.Run(m.Context, "/usr/bin/open", dir)
					}
					return "Exported Cloudflare Worker template to " + dir, err
				})
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

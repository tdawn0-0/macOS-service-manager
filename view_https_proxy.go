package main

import (
	"fmt"
	"sort"
	"strconv"
	"strings"

	"devbase/internal/devplatform"
	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/choice"
	"github.com/lobehub/lobe-mygo/pkg/display"
	"github.com/lobehub/lobe-mygo/pkg/theme"
)

func cloudflareHostSuggestions(s devplatform.State, localDomain string) []string {
	seen := map[string]bool{}
	var out []string
	add := func(host string) {
		h := strings.ToLower(strings.TrimSpace(host))
		if h == "" || strings.HasPrefix(h, "*.") || !strings.Contains(h, ".") || seen[h] {
			return
		}
		seen[h] = true
		out = append(out, h)
	}
	for _, rec := range s.CloudflareDNS {
		add(rec.Name)
	}
	slug := strings.TrimPrefix(devplatform.DefaultTunnelWorkerName(localDomain), "devbase-")
	for _, z := range s.CloudflareZones {
		if slug != "" {
			add(slug + "." + z.Name)
		}
		add(z.Name)
	}
	sort.Strings(out)
	return out
}

func hasAnyTunnelRule(s devplatform.State) bool {
	for _, r := range s.ProxyRules {
		if r.Tunnel {
			return true
		}
	}
	return false
}

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
	}, nil)

	devCardWith(c, icoRoute, "Rules", "", func() {
		if btn(c, "Add rule", icoPlus, btnText).Clicked() {
			d.proxyDraft = append(d.proxyDraft, proxyRuleDraft{LocalPort: "3000", TunnelMode: tunnelModeWorkersDev})
		}
	}, func() {
		removeRule := -1
		if len(d.proxyDraft) == 0 {
			display.Empty(c, display.EmptyProps{Icon: icoRoute, Title: "No rules"})
		}
		for i := range d.proxyDraft {
			row := &d.proxyDraft[i]
			devInset(c, func() {
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
					if row.TunnelMode == "" {
						row.TunnelMode = tunnelModeWorkersDev
					}
					ui.Row(c).Wrap().Gap(8).AlignItems(ui.Center).Padding(0, 0, 0, 12).Children(func() {
						ui.Icon(c, icoGlobe).FontSize(13).TextColor(tk.TextTertiary).Shrink(0)
						modes := []string{tunnelModeWorkersDev, tunnelModeCustomDomain}
						if ui.Select(c, &row.TunnelMode, modes).Label(fmt.Sprintf("Tunnel mode %d", i+1)).Width(164).Changed() && row.TunnelMode == tunnelModeWorkersDev {
							row.TunnelDomain = ""
						}
						if row.TunnelMode == tunnelModeCustomDomain {
							mono(ui.TextInput(c, &row.TunnelDomain).Placeholder("app.example.com").Label(fmt.Sprintf("Custom domain %d", i+1)).Width(210))
							if suggestions := cloudflareHostSuggestions(d.state, row.Domain); len(suggestions) > 0 {
								pick := "Existing DNS..."
								opts := append([]string{pick}, suggestions...)
								if ui.Select(c, &pick, opts).Label(fmt.Sprintf("DNS suggestion %d", i+1)).Width(164).Changed() && pick != "Existing DNS..." {
									row.TunnelDomain = pick
								}
							}
						}
						previewRule := devplatform.ProxyRule{
							Domain:       row.Domain,
							Tunnel:       true,
							TunnelWorker: row.TunnelWorker,
							TunnelDomain: effectiveDraftTunnelDomain(*row),
							TunnelURL:    row.TunnelURL,
						}
						previewURL := previewRule.PreviewTunnelURL(d.state.CloudflareSubdomain)
						previewText := previewURL
						if previewText == "" {
							if worker := previewRule.ResolvedTunnelWorker(); worker != "" {
								previewText = worker + " (.workers.dev)"
							} else {
								previewText = "Deploy to assign URL"
							}
						}
						mono(ui.Text(c, previewText).FontSize(textSmall).TextColor(tk.TextTertiary).SingleLine().Grow(1).MinWidth(140))
						if !dirty && previewURL != "" {
							if iconButton(c, icoCopy, "Copy tunnel URL", tk.TextSecondary).Clicked() {
								if a.copy != nil {
									a.copy(previewURL)
								}
								d.output = "Copied " + previewURL
							}
							if iconButton(c, icoLink, "Open tunnel URL", tk.TextSecondary).Clicked() {
								a.devTask(func(m *devplatform.Manager) (string, error) {
									_, err := m.Run(m.Context, "/usr/bin/open", previewURL)
									return "Opened tunnel URL in browser.", err
								})
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
		ui.Column(c).Width(200).Children(func() {
			devField(c, "HTTPS listener port", &d.proxyListenerPort, "443")
		})
		a.devPortControls(c, &d.proxyListenerPort, "HTTPS")
		ui.Row(c).Wrap().Gap(8).AlignItems(ui.Center).Margin(4, 0, 0).Children(func() {
			// One line says where the rules stand; the one button beside it is
			// the next step: save the edits, then apply what was saved.
			note, dot := "Applied", tk.Success
			switch {
			case dirty:
				note, dot = "Unsaved changes", tk.Warning
			case state == "Pending":
				note, dot = "Saved, not applied yet", tk.Warning
			case state != "Published":
				note, dot = "Not applied", tk.TextQuaternary
			}
			ui.Row(c).Gap(6).AlignItems(ui.Center).Grow(1).MinWidth(0).Children(func() {
				ui.Box(c).Size(6, 6).Radius(3).Background(dot).Shrink(0)
				ui.Text(c, note).FontSize(textSmall).TextColor(tk.TextDescription).SingleLine()
			})
			if dirty && btn(c, "Discard changes", icoUndo, btnDefault).Disabled(d.busy).Clicked() {
				d.loadProxyDraft(d.state)
			}
			if !dirty {
				kind := btnDefault
				if state == "Pending" {
					kind = btnPrimary
				}
				if btn(c, "Apply proxy", icoPlay, kind).Disabled(d.busy || len(d.state.ProxyRules) == 0).Clicked() {
					a.devTask(func(m *devplatform.Manager) (string, error) {
						err := m.ApplyProxy(m.Context)
						return "Proxy configuration published. Check DNS, TLS and upstream separately.", err
					})
				}
			}
			if dirty && btn(c, "Save HTTPS rules", icoSave, btnPrimary).Disabled(d.busy).Clicked() {
				listener, err := strconv.Atoi(d.proxyListenerPort)
				rules := make([]devplatform.ProxyRule, 0, len(d.proxyDraft))
				for _, row := range d.proxyDraft {
					port, e := strconv.Atoi(row.LocalPort)
					if e != nil {
						err = e
						break
					}
					rules = append(rules, devplatform.ProxyRule{
						Domain:       row.Domain,
						LocalPort:    port,
						Tunnel:       row.Tunnel,
						TunnelWorker: row.TunnelWorker,
						TunnelDomain: effectiveDraftTunnelDomain(row),
						TunnelURL:    row.TunnelURL,
					})
				}
				if err != nil {
					d.output = "Error: Enter valid numeric ports"
				} else {
					revision := d.proxyDraftRevision
					rulesSaved := false
					a.devTaskThen(func(m *devplatform.Manager) (string, error) {
						if err := m.ReplaceProxyRules(m.Context, revision, listener, rules); err != nil {
							return "", err
						}
						rulesSaved = true
						if m.HasCloudflareToken() {
							if err := m.SyncTunnels(m.Context); err != nil {
								return "", fmt.Errorf("saved HTTPS rules; Cloudflare sync failed: %w", err)
							}
							return "Saved HTTPS rules and synced Cloudflare Workers.", nil
						}
						return "Saved HTTPS rules.", nil
					}, func(err error) {
						if err == nil || rulesSaved {
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
			}{{"Restart HTTPS proxy", icoRotate}, {"Diagnose HTTPS", icoStetho}} {
				if btn(c, act.label, act.icon, btnDefault).Disabled(d.busy || dirty).Clicked() {
					proxyAction(act.label)
				}
			}
		})
	})

	hasTunnelRules := hasAnyTunnelRule(d.state)
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
		if choice.Switch(c, &tunnelOn, choice.SwitchProps{Size: theme.Small}).Label("Cloudflare tunnel service").Disabled(d.busy || tunnelDirty || (!hasTunnelRules && !tunnelOn)).Tooltip("Start or stop the background Cloudflare tunnel launchd service").Changed() {
			a.devTask(func(m *devplatform.Manager) (string, error) {
				if tunnelOn {
					return "Cloudflare tunnel started.", m.ApplyTunnel(m.Context)
				}
				return "Stopped Cloudflare tunnel", m.StopTunnel(m.Context)
			})
		}
	}, func() {
		devFieldGrid(c, func() {
			tokenPlaceholder := "Paste Cloudflare API token"
			if d.statuses["tunnel:token"] == "Configured" {
				tokenPlaceholder = "Saved (paste to replace)"
			}
			devField(c, "Cloudflare API token", &d.cloudflareTokenDraft, tokenPlaceholder)
			devField(c, "Account ID", &d.cloudflareAccountID, "Auto-detected")
			devField(c, "Client ID", &d.tunnelClientID, "mac-auto")
		})
		if d.state.CloudflareSubdomain != "" || len(d.state.CloudflareZones) > 0 {
			var info []string
			if d.state.CloudflareSubdomain != "" {
				info = append(info, "*."+d.state.CloudflareSubdomain+".workers.dev")
			}
			if len(d.state.CloudflareZones) > 0 {
				names := make([]string, 0, len(d.state.CloudflareZones))
				for _, z := range d.state.CloudflareZones {
					names = append(names, z.Name)
				}
				info = append(info, "Zones: "+strings.Join(names, ", "))
			}
			mono(ui.Text(c, strings.Join(info, " · ")).FontSize(textCaption).TextColor(tk.TextTertiary).SingleLine())
		}
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
			if btn(c, "Save Cloudflare settings", icoSave, btnPrimary).Disabled(d.busy || !tunnelDirty).Clicked() {
				token, accountID, clientID := d.cloudflareTokenDraft, d.cloudflareAccountID, d.tunnelClientID
				a.devTaskThen(func(m *devplatform.Manager) (string, error) {
					return "Saved Cloudflare settings.", m.SaveCloudflareSettings(m.Context, token, accountID, clientID)
				}, func(err error) {
					if err == nil {
						a.dev.loadTunnelDraft(a.dev.state)
					}
				})
			}
		})
		devButtonRow(c, func() {
			if btn(c, "Deploy Workers", icoGlobe, btnDefault).Disabled(d.busy || dirty || tunnelDirty || !hasTunnelRules || d.statuses["tunnel:token"] != "Configured").Clicked() {
				a.devTaskThen(func(m *devplatform.Manager) (string, error) {
					return "Deployed Cloudflare Workers.", m.SyncTunnels(m.Context)
				}, func(err error) {
					if err == nil {
						a.dev.loadProxyDraft(a.dev.state)
						a.dev.loadTunnelDraft(a.dev.state)
					}
				})
			}
			if btn(c, "Refresh zones", icoRotate, btnDefault).Disabled(d.busy || tunnelDirty || d.statuses["tunnel:token"] != "Configured").Clicked() {
				a.devTaskThen(func(m *devplatform.Manager) (string, error) {
					return "Refreshed Cloudflare zones and DNS records.", m.RefreshCloudflareMetadata(m.Context)
				}, func(err error) {
					if err == nil {
						a.dev.loadTunnelDraft(a.dev.state)
					}
				})
			}
			if btn(c, "Restart tunnel", icoPlay, btnDefault).Disabled(d.busy || !tunnelOn).Clicked() {
				a.devTask(func(m *devplatform.Manager) (string, error) {
					return "Restarted Cloudflare tunnel.", m.RestartTunnel(m.Context)
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

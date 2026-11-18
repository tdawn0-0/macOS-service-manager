package main

import (
	"context"
	"fmt"
	"sort"
	"strconv"
	"strings"
	"time"

	"devbase/internal/devplatform"
	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/action"
	"github.com/lobehub/lobe-mygo/pkg/choice"
	"github.com/lobehub/lobe-mygo/pkg/display"
	"github.com/lobehub/lobe-mygo/pkg/feedback"
	"github.com/lobehub/lobe-mygo/pkg/theme"
)

type httpsAuthorityCheckState struct {
	loading   bool
	attempted bool
	seq       uint64
}

type httpsListeningPortsState struct {
	ports       []devplatform.ListeningPort
	loaded      bool
	loading     bool
	err         string
	updatedAt   time.Time
	hoverPrimed bool
}

var (
	httpsAuthorityChecks = map[*app]*httpsAuthorityCheckState{}
	httpsListeningPorts  = map[*app]*httpsListeningPortsState{}
)

func (a *app) ensureHTTPSAuthorityStatus() {
	d := &a.dev
	if a.post == nil || d.authorityStatus != nil {
		return
	}
	st := httpsAuthorityChecks[a]
	if st == nil {
		st = &httpsAuthorityCheckState{}
		httpsAuthorityChecks[a] = st
	}
	if st.loading || (st.attempted && st.seq == d.taskSequence) {
		return
	}
	st.loading = true
	st.attempted = true
	st.seq = d.taskSequence
	m := a.devManager()
	a.spawn(func() {
		ctx, cancel := context.WithTimeout(context.Background(), 6*time.Second)
		defer cancel()
		status, err := m.AuthorityStatus(ctx)
		a.post(func() {
			if cur := httpsAuthorityChecks[a]; cur != nil {
				cur.loading = false
			}
			if err == nil {
				a.dev.authorityStatus = &status
				delete(httpsAuthorityChecks, a)
			}
		})
	})
}

func (a *app) refreshAuthorityStatusInTask(m *devplatform.Manager) {
	ctx, cancel := context.WithTimeout(context.Background(), 6*time.Second)
	defer cancel()
	if status, err := m.AuthorityStatus(ctx); err == nil {
		a.post(func() {
			a.dev.authorityStatus = &status
			delete(httpsAuthorityChecks, a)
		})
	}
}

func localCATrustLabel(status *devplatform.AuthorityStatus) string {
	if status == nil {
		return ""
	}
	if status.Active.Status == "Not created" {
		return "Not created"
	}
	if status.Active.Trust == "Trusted" {
		return "Trusted"
	}
	return "Not trusted"
}

func (a *app) listeningPortsCache() *httpsListeningPortsState {
	if a.post == nil {
		return &httpsListeningPortsState{}
	}
	st := httpsListeningPorts[a]
	if st == nil {
		st = &httpsListeningPortsState{}
		httpsListeningPorts[a] = st
	}
	return st
}

func (a *app) refreshListeningPorts(force bool) {
	if a.post == nil {
		return
	}
	st := a.listeningPortsCache()
	if st.loading {
		return
	}
	if !force && st.loaded && time.Since(st.updatedAt) < 2*time.Second {
		return
	}
	st.loading = true
	m := a.devManager()
	a.spawn(func() {
		ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
		defer cancel()
		ports, err := m.ListeningUserPorts(ctx)
		a.post(func() {
			cur := a.listeningPortsCache()
			cur.loading = false
			cur.loaded = true
			cur.updatedAt = time.Now()
			if err != nil {
				cur.err = err.Error()
				return
			}
			cur.err = ""
			cur.ports = ports
		})
	})
}

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

func buildProxyRulesFromDraft(d *developmentState) (int, []devplatform.ProxyRule, error) {
	listener, err := strconv.Atoi(d.proxyListenerPort)
	if err != nil {
		return 0, nil, err
	}
	rules := make([]devplatform.ProxyRule, 0, len(d.proxyDraft))
	for _, row := range d.proxyDraft {
		port, e := strconv.Atoi(row.LocalPort)
		if e != nil {
			return 0, nil, e
		}
		rules = append(rules, devplatform.ProxyRule{
			Domain:       devplatform.LocalDomain(row.Domain),
			LocalPort:    port,
			UpstreamHost: row.UpstreamHost,
			Tunnel:       row.Tunnel,
			TunnelAuth:   row.TunnelAuth,
			TunnelWorker: row.TunnelWorker,
			TunnelDomain: effectiveDraftTunnelDomain(row),
			TunnelURL:    row.TunnelURL,
		})
	}
	return listener, rules, nil
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
	a.ensureHTTPSAuthorityStatus()
	portsCache := a.listeningPortsCache()
	if a.post != nil && !portsCache.loaded {
		a.refreshListeningPorts(false)
	}
	anyPickerHovered := false
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
				ui.Row(c).Wrap().Gap(8).AlignItems(ui.Center).Children(func() {
					// The name is typed; the namespace after it is fixed, and macOS resolves it to this Mac.
					ui.Row(c).Grow(1).MinWidth(220).Gap(2).AlignItems(ui.Center).Children(func() {
						mono(ui.TextInput(c, &row.Domain).Placeholder("app").Label("Domain").Grow(1).MinWidth(0))
						mono(ui.Text(c, devplatform.LocalDomainSuffix).FontSize(textSmall).TextColor(tk.TextTertiary).SingleLine().Shrink(0))
					})
					// The route: the domain goes to a port of this Mac.
					// The upstream is empty for this Mac (127.0.0.1) or any IP address or host name.
					ui.Row(c).Width(348).Shrink(0).Gap(6).AlignItems(ui.Center).Children(func() {
						ui.Icon(c, icoArrow).FontSize(14).TextColor(tk.TextQuaternary).Shrink(0)
						mono(ui.TextInput(c, &row.UpstreamHost).Placeholder("127.0.0.1").Label("Upstream host").Grow(1).MinWidth(0))
						mono(ui.Text(c, ":").FontSize(textSmall).TextColor(tk.TextTertiary).SingleLine().Shrink(0))
						mono(ui.TextInput(c, &row.LocalPort).Placeholder("3000").Label("Local port").Width(72).Shrink(0))
						pickBtn := action.ActionIcon(c, icoPlug, action.ActionIconProps{
							Variant: action.ActionIconBorderless,
							Size:    theme.Small,
						}).Label(fmt.Sprintf("Pick active port %d", i+1)).Tooltip("Choose a listening local port").Disabled(d.busy)
						if pickBtn.Hovered() {
							anyPickerHovered = true
						}
						pickBtn.Menu(func(m *ui.Menu) {
							if a.post != nil && !portsCache.loading && time.Since(portsCache.updatedAt) >= 2*time.Second {
								a.refreshListeningPorts(false)
							}
							if portsCache.loading && len(portsCache.ports) == 0 {
								m.Item("Scanning listening ports…").Disabled(true)
							} else if portsCache.err != "" && len(portsCache.ports) == 0 {
								m.Item("Could not scan listening ports").Disabled(true)
							} else if len(portsCache.ports) == 0 {
								m.Item("No listening local ports").Disabled(true)
							} else {
								for _, lp := range portsCache.ports {
									label := strconv.Itoa(lp.Port)
									if lp.Process != "" {
										label = fmt.Sprintf("%d · %s", lp.Port, lp.Process)
									}
									if m.Item(label).ID(strconv.Itoa(lp.Port)).Chosen() {
										row.LocalPort = strconv.Itoa(lp.Port)
									}
								}
							}
							if a.post != nil {
								m.Separator()
								if m.Item("Refresh listening ports").Chosen() {
									a.refreshListeningPorts(true)
								}
							}
						})
					})
					ui.Row(c).Shrink(0).Gap(6).AlignItems(ui.Center).Children(func() {
						choice.Switch(c, &row.Tunnel, choice.SwitchProps{Size: theme.Small}).Label(fmt.Sprintf("Tunnel rule %d", i+1)).Tooltip("Expose this rule through the Cloudflare tunnel")
						ui.Text(c, "Tunnel").FontSize(textCaption).TextColor(tk.TextSecondary).SingleLine().Shrink(0)
					})
					ui.Row(c).Shrink(0).Gap(4).Justify(ui.End).Children(func() {
						if !dirty {
							url := "https://" + devplatform.LocalDomain(row.Domain)
							if d.state.ProxyListenerPort != 0 && d.state.ProxyListenerPort != 443 {
								url = fmt.Sprintf("%s:%d", url, d.state.ProxyListenerPort)
							}
							if iconButton(c, icoCopy, "Copy HTTPS URL", tk.TextSecondary).Clicked() {
								if a.copy != nil {
									a.copy(url)
								}
								d.output = "Copied " + url
							}
							if iconButton(c, icoLink, "Open HTTPS URL", tk.TextSecondary).Clicked() {
								a.devTask(func(m *devplatform.Manager) (string, error) {
									_, err := m.Run(m.Context, "/usr/bin/open", url)
									return "Opened saved HTTPS rule in browser.", err
								})
							}
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
							if suggestions := cloudflareHostSuggestions(d.state, devplatform.LocalDomain(row.Domain)); len(suggestions) > 0 {
								pick := "Existing DNS..."
								opts := append([]string{pick}, suggestions...)
								if ui.Select(c, &pick, opts).Label(fmt.Sprintf("DNS suggestion %d", i+1)).Width(164).Changed() && pick != "Existing DNS..." {
									row.TunnelDomain = pick
								}
							}
						}
						ui.Row(c).Shrink(0).Gap(6).AlignItems(ui.Center).Children(func() {
							choice.Switch(c, &row.TunnelAuth, choice.SwitchProps{Size: theme.Small}).Label(fmt.Sprintf("Protect tunnel %d", i+1)).Tooltip("Require an access token on public tunnel requests")
							ui.Text(c, "Protect").FontSize(textCaption).TextColor(tk.TextSecondary).SingleLine().Shrink(0)
						})
						previewRule := devplatform.ProxyRule{
							Domain:       devplatform.LocalDomain(row.Domain),
							Tunnel:       true,
							TunnelAuth:   row.TunnelAuth,
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
						if !dirty && row.TunnelAuth {
							localDom := devplatform.LocalDomain(row.Domain)
							if iconButton(c, icoKey, "Copy tunnel token", tk.TextSecondary).Disabled(d.busy).Clicked() {
								a.devTask(func(m *devplatform.Manager) (string, error) {
									tok, err := m.EnsureTunnelAccessToken(localDom)
									if err == nil {
										a.post(func() {
											if a.copy != nil {
												a.copy(tok)
											}
										})
										return "Copied tunnel access token for " + localDom, nil
									}
									return "", err
								})
							}
							if iconButton(c, icoRotate, "Rotate tunnel token", tk.TextSecondary).Disabled(d.busy).Clicked() {
								a.devTask(func(m *devplatform.Manager) (string, error) {
									if _, err := m.RotateTunnelAccessToken(m.Context, localDom); err != nil {
										return "", err
									}
									return "Rotated tunnel access token for " + localDom, nil
								})
							}
						}
					})
				}
				if !dirty {
					a.devProxyActivation(c, devplatform.LocalDomain(row.Domain))
				}
			})
		}
		if a.post != nil {
			if anyPickerHovered {
				if !portsCache.hoverPrimed {
					portsCache.hoverPrimed = true
					a.refreshListeningPorts(true)
				}
			} else {
				portsCache.hoverPrimed = false
			}
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
						a.refreshAuthorityStatusInTask(m)
						return "Proxy configuration published. Check DNS, TLS and upstream separately.", err
					})
				}
			}
			if dirty && btn(c, "Save HTTPS rules", icoSave, btnDefault).Disabled(d.busy).Clicked() {
				listener, rules, err := buildProxyRulesFromDraft(d)
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
			if dirty && btn(c, "Save & apply", icoPlay, btnPrimary).Disabled(d.busy).Clicked() {
				listener, rules, err := buildProxyRulesFromDraft(d)
				if err != nil {
					d.output = "Error: Enter valid numeric ports"
				} else {
					revision := d.proxyDraftRevision
					tunnelRunning := d.statuses["tunnel"] == "Running"
					rulesSaved := false
					a.devTaskThen(func(m *devplatform.Manager) (string, error) {
						if err := m.ReplaceProxyRules(m.Context, revision, listener, rules); err != nil {
							return "", err
						}
						rulesSaved = true
						synced := false
						if m.HasCloudflareToken() {
							if err := m.SyncTunnels(m.Context); err != nil {
								return "", fmt.Errorf("saved HTTPS rules; Cloudflare sync failed: %w", err)
							}
							synced = true
						}
						if err := m.EnsureLogMaintenance(m.Context); err != nil {
							return "", err
						}
						if err := m.ApplyProxy(m.Context); err != nil {
							return "", err
						}
						a.refreshAuthorityStatusInTask(m)
						if tunnelRunning {
							hasTunnel := false
							for _, r := range rules {
								if r.Tunnel {
									hasTunnel = true
									break
								}
							}
							if hasTunnel {
								if err := m.ApplyTunnel(m.Context); err != nil {
									return "", err
								}
							} else if err := m.StopTunnel(m.Context); err != nil {
								return "", err
							}
						}
						if synced {
							return "Saved, synced Cloudflare Workers, and applied proxy configuration.", nil
						}
						return "Saved and applied proxy configuration.", nil
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
				a.refreshAuthorityStatusInTask(m)
			case "Stop HTTPS proxy":
				e = m.StopProxy(m.Context)
			case "Trust local CA":
				e = m.ConfigureTrust(m.Context, false)
				a.refreshAuthorityStatusInTask(m)
			case "Remove CA trust":
				e = m.ConfigureTrust(m.Context, true)
				a.refreshAuthorityStatusInTask(m)
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
	devCardWith(c, iconServer, "Proxy", "", func() {
		if proxyStatus != "Running" && proxyStatus != "Stopped" {
			statusTag(c, proxyStatus)
		}
		if choice.Switch(c, &proxyOn, choice.SwitchProps{Size: theme.Small}).Label("HTTPS proxy service").Disabled(d.busy || dirty || (len(d.state.ProxyRules) == 0 && !proxyOn)).Tooltip("Start or stop the background HTTPS proxy launchd service").Changed() {
			a.devTask(func(m *devplatform.Manager) (string, error) {
				if proxyOn {
					if err := m.EnsureLogMaintenance(m.Context); err != nil {
						return "HTTPS proxy start", err
					}
					err := m.ApplyProxy(m.Context)
					a.refreshAuthorityStatusInTask(m)
					return "Proxy configuration published. Check DNS, TLS and upstream separately.", err
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
			if btn(c, "Read proxy logs", icoScroll, btnText).Disabled(d.busy).Clicked() {
				d.showResult = true
				a.devTask(func(m *devplatform.Manager) (string, error) { return m.Logs("proxy") })
			}
			if btn(c, "Open proxy logs in Console", icoLink, btnText).Disabled(d.busy).Clicked() {
				a.openJobLogs("proxy", "HTTPS proxy", nil)
			}
		})
	})

	hasTunnelRules := hasAnyTunnelRule(d.state)
	tunnelStatus := d.statuses["tunnel"]
	if tunnelStatus == "" {
		tunnelStatus = "Stopped"
	}
	tunnelOn := tunnelStatus == "Running"
	devCardWith(c, icoGlobe, "Cloudflare tunnel", "", func() {
		if tunnelStatus != "Running" && tunnelStatus != "Stopped" {
			statusTag(c, tunnelStatus)
		}
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
			ui.Field(c, "Cloudflare API token", func() {
				ui.TextInput(c, &d.cloudflareTokenDraft).Placeholder(tokenPlaceholder).Password().Tooltip("Required token permissions: Account Workers Scripts Edit, Zone Workers Routes Edit, Zone Read, DNS Read")
			})
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
					if err := m.SaveCloudflareSettings(m.Context, token, accountID, clientID); err != nil {
						return "", err
					}
					shouldDiscover := strings.TrimSpace(token) != ""
					if !shouldDiscover && m.HasCloudflareToken() {
						if s, err := m.Read(); err == nil && s.CloudflareAccountID == "" {
							shouldDiscover = true
						}
					}
					if shouldDiscover {
						if err := m.RefreshCloudflareMetadata(m.Context); err != nil {
							return "Saved Cloudflare settings (metadata check failed: " + err.Error() + ").", nil
						}
						return "Saved Cloudflare settings and refreshed zones.", nil
					}
					return "Saved Cloudflare settings.", nil
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
			if btn(c, "Remove Cloudflare token", icoTrash, btnDanger).Disabled(d.busy || d.statuses["tunnel:token"] != "Configured").Clicked() {
				a.ask = askState{
					open:    true,
					title:   "Remove Cloudflare API token?",
					message: "Stops the tunnel if running and clears saved Cloudflare account metadata from this Mac.",
					button:  "Remove Token",
					run: func() {
						a.devTaskThen(func(m *devplatform.Manager) (string, error) {
							return "Removed Cloudflare token and account metadata.", m.RemoveCloudflareSettings(m.Context, false)
						}, func(err error) {
							if err == nil {
								a.dev.loadTunnelDraft(a.dev.state)
							}
						})
					},
				}
			}
			if btn(c, "Read tunnel logs", icoScroll, btnText).Disabled(d.busy).Clicked() {
				d.showResult = true
				a.devTask(func(m *devplatform.Manager) (string, error) { return m.Logs("tunnel") })
			}
			if btn(c, "Open tunnel logs in Console", icoLink, btnText).Disabled(d.busy).Clicked() {
				a.openJobLogs("tunnel", "Cloudflare tunnel", nil)
			}
		})
	})

	devCardWith(c, icoKey, "Local certificate trust", "", func() {
		if label := localCATrustLabel(d.authorityStatus); label != "" {
			statusTag(c, label)
		}
		if btn(c, "Local certificates", icoArrow, btnText).Disabled(d.busy).Clicked() {
			d.authorityOpen = true
			a.authorityTask(nil)
		}
	}, func() {
		if proxyOn && d.authorityStatus != nil && d.authorityStatus.Active.Trust != "Trusted" {
			devHint(c, feedback.Warning, "Proxy is running, but the local CA is not trusted yet.", "")
		}
		devButtonRow(c, func() {
			for _, act := range []struct {
				label string
				icon  *ui.SVG
				kind  btnKind
			}{{"Trust local CA", icoShield, btnDefault}, {"Remove CA trust", nil, btnText}} {
				if btn(c, act.label, act.icon, act.kind).Disabled(d.busy || dirty).Clicked() {
					proxyAction(act.label)
				}
			}
		})
	})
}

package main

import (
	"devbase/internal/devplatform"
	"devbase/internal/privhelper"
	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/choice"
	"github.com/lobehub/lobe-mygo/pkg/theme"
)

const navSettings = "settings"

func (a *app) settingsView(c *ui.Context) {
	if !a.settingsLoaded && !a.settingsBusy {
		a.refreshSettings()
	}
	d := &a.dev
	if !d.loaded && !d.busy {
		a.devTask(func(m *devplatform.Manager) (string, error) { return "Ready to build locally.", nil })
	}

	tk := theme.TokensOf(c)
	g := glassOf(c.Theme())
	ui.Column(c).Grow(1).MinWidth(0).Children(func() {
		ui.Row(c).Height(barHeight).Shrink(0).Padding(0, 12, 0, 20).AlignItems(ui.Center).Gap(8).DragWindow().Children(func() {
			ui.Text(c, "Settings").FontSize(15).FontWeight(600).SingleLine().Shrink(0)
			ui.Spacer(c)
			if a.helperBusy || a.settingsBusy || d.busy {
				ui.Spinner(c).Label("Loading").Size(14, 14)
			}
			iconButton(c, icoRotate, "Refresh", c.Theme().TextMuted).Disabled(a.settingsBusy || a.helperBusy).OnClick(a.refreshSettings)
		})
		ui.Column(c).Height(1).Shrink(0).Background(hairline(c.Theme()))
		ui.Scroll(c.Key("settings")).TrackScroll(&a.settingsScroll).Grow(1).MinWidth(0).MinHeight(0).Padding(24, 28, 28).Background(g.sheet).Children(func() {
			ui.Column(c).Gap(16).MaxWidth(880).Children(func() {
				helperBadge := "Not installed"
				if a.helperStatus.State == privhelper.StateInstalled {
					helperBadge = "Installed"
				}
				helperMsg := a.helperStatus.Message
				if helperMsg == "" {
					helperMsg = "Install once to delegate local HTTPS port 443, /etc/hosts, port inspection, and system launchd jobs without password prompts."
				}
				devCardWith(c, icoShield, "Privileged helper", "System daemon · on-demand socket", func() {
					statusTag(c, helperBadge)
				}, func() {
					ui.Text(c, helperMsg).FontSize(textSmall).TextColor(tk.TextDescription)
					devKV(c, "Daemon", privhelper.PlistPath)
					devKV(c, "Socket", privhelper.SocketPath)
					devKV(c, "Helper", privhelper.HelperDir)
					devButtonRow(c, func() {
						if a.helperStatus.State != privhelper.StateInstalled {
							if btn(c, "Install helper", icoShield, btnPrimary).Disabled(a.helperBusy || d.busy).Clicked() {
								a.installHelper()
							}
						}
						if a.helperStatus.State == privhelper.StateInstalled {
							if btn(c, "Remove helper", nil, btnText).Disabled(a.helperBusy || d.busy).Clicked() {
								a.uninstallHelper()
							}
						}
						if btn(c, "Refresh status", icoRotate, btnDefault).Disabled(a.settingsBusy || a.helperBusy).Clicked() {
							a.refreshSettings()
						}
					})
				})

				fdaBadge := "Allowed"
				if !a.settingsLoaded {
					fdaBadge = "Checking"
				} else if a.fdaNeedsAccess {
					fdaBadge = "Restricted"
				}
				devCardWith(c, icoKey, "macOS permissions", "Privacy & Login Items", func() {
					statusTag(c, fdaBadge)
				}, func() {
					devSubtle(c, "Full Disk Access lets Devbase inspect protected launchd and background task items. Login Items controls apps opened at login.")
					devButtonRow(c, func() {
						if btn(c, "Open Full Disk Access…", icoLink, btnDefault).Clicked() {
							a.openFullDiskAccess()
						}
						if btn(c, "Open Login Items…", icoLink, btnDefault).Clicked() {
							a.openLoginSettings()
						}
					})
				})

				rotEnabled := !d.state.LogRotationDisabled
				rotBadge := "Enabled"
				if !rotEnabled {
					rotBadge = "Disabled"
				}
				devCardWith(c, icoSettings, "Workspace & maintenance", "Logs, storage & diagnostics", func() {
					statusTag(c, rotBadge)
				}, func() {
					rotValue := rotEnabled
					ui.Row(c).Gap(10).AlignItems(ui.Center).Children(func() {
						if choice.Switch(c, &rotValue, choice.SwitchProps{Size: theme.Small}).Label("Automatic log rotation").Disabled(d.busy).Changed() {
							a.setLogRotation(rotValue)
						}
						ui.Text(c, "Trim managed service and database logs every 60 seconds").FontSize(textSmall).TextColor(tk.TextDescription).Grow(1).MinWidth(0)
					})
					if a.workspaceRoot != "" {
						devKV(c, "Workspace", a.workspaceRoot)
					}
					devButtonRow(c, func() {
						if btn(c, "Show workspace in Finder", icoFolder, btnDefault).Disabled(a.reveal == nil || a.workspaceRoot == "").Clicked() {
							a.reveal(a.workspaceRoot)
						}
						if btn(c, "Run diagnostics", icoStetho, btnDefault).Disabled(d.busy).Clicked() {
							a.chosen.Clear()
							a.nav = navDevelopment
							d.operationsOpen = false
							a.runDevDiagnostics()
						}
					})
				})
			})
		})
	})
}

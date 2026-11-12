package main

import (
	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/display"
	"github.com/lobehub/lobe-mygo/pkg/feedback"
	"github.com/lobehub/lobe-mygo/pkg/theme"
	"devbase/internal/devplatform"
)

func (a *app) devJavaInstallations(c *ui.Context) {
	d := &a.dev
	if !d.javaLoaded && !d.busy && a.post != nil {
		a.devTask(func(*devplatform.Manager) (string, error) { return "Local Java installations refreshed", nil })
	}
	devCardWith(c, icoCoffee, "Installed Java", "", func() {
		if btn(c, "Refresh", icoRotate, btnText).Disabled(d.busy || a.packageTasksRunning()).Clicked() {
			a.devTask(func(*devplatform.Manager) (string, error) { return "Local Java installations refreshed", nil })
		}
	}, func() {
		devSubtle(c, a.devManager().JavaInstallationDirectory()).Selectable()
		if d.javaError != "" {
			devHint(c, feedback.Warning, d.javaError, "")
		}
		if d.javaLoaded && d.javaError == "" && len(d.javaInstallations) == 0 {
			devSubtle(c, "No JDK installations found in this directory.")
		}
		for _, row := range d.javaInstallations {
			row := row
			devInset(c, func() {
				ui.Row(c).Wrap().Gap(8).AlignItems(ui.Center).Children(func() {
					ui.Row(c).Gap(8).AlignItems(ui.Center).Grow(1).MinWidth(160).Children(func() {
						ui.Text(c, row.Label()).FontWeight(500).SingleLine().MinWidth(100)
						if d.state.RuntimeDefaults["java"] == row.ID || row.PackageID != "" && d.state.RuntimeDefaults["java"] == row.PackageID {
							display.Tag(c, "Default", display.TagProps{Color: display.TagGreen, Size: theme.Small})
						}
						if row.Managed {
							devTag(c, "Devbase", display.TagColor(""))
						}
					})
					ui.Row(c).Gap(6).AlignItems(ui.Center).Shrink(0).Children(func() {
						isDefault := d.state.RuntimeDefaults["java"] == row.ID || row.PackageID != "" && d.state.RuntimeDefaults["java"] == row.PackageID
						if !isDefault && btn(c.Key(row.ID+"-default"), "Set default", nil, btnDefault).Disabled(d.busy || a.packageTasksRunning() || !row.CanUse).Clicked() {
							a.devTask(func(m *devplatform.Manager) (string, error) {
								return "Java default selected; enable terminal integration to use it", m.SetJavaDefault(m.Context, row)
							})
						}
						if btn(c.Key(row.ID+"-copy"), "Copy JAVA_HOME", icoCopy, btnText).Clicked() && a.copy != nil {
							a.copy(row.Home)
						}
					})
				})
				build := "Vendor: " + row.Vendor
				if row.Build != "" {
					build += " · Build: " + row.Build
				}
				devSubtle(c, build)
				ui.Text(c, row.Path).FontSize(textSmall).Selectable()
				source := "Existing installation"
				if row.Managed {
					source = "Installed by Devbase"
				}
				if row.Symlink {
					source = "Symbolic link → " + row.Home
				}
				devSubtle(c, source)
				if row.Problem != "" {
					ui.Text(c, row.Problem).FontSize(textSmall).TextColor(c.Theme().Warning)
				}
				if row.Managed {
					if p, ok := d.state.Packages[row.PackageID]; ok {
						a.devPackageIntegrity(c, row.PackageID, p)
					}
				}
				if selected := d.javaRemove; selected != nil && selected.ID == row.ID {
					devDangerCard(c, "Remove "+selected.Label()+"?", func() {
						ui.Text(c, selected.Path).FontSize(textSmall)
						message := "Moves only this JDK to Trash. Close Java programs using it first. IDEs and tools referencing it will need another JDK."
						label := "Move JDK to Trash"
						if selected.Symlink {
							message = "Moves only this symbolic link to Trash. The target JDK is kept."
							label = "Move link to Trash"
						}
						devSubtle(c, message)
						devButtonRow(c, func() {
							if btn(c, label, icoTrash, btnDanger).Disabled(d.busy || a.packageTasksRunning()).Clicked() {
								reviewed := *selected
								a.devTaskThen(func(m *devplatform.Manager) (string, error) {
									path, err := m.RemoveJavaInstallation(m.Context, reviewed)
									if err != nil {
										return "Java removal did not complete", err
									}
									return "Moved to Trash: " + path, nil
								}, func(error) { a.dev.javaRemove = nil })
							}
							if btn(c, "Cancel Java removal", nil, btnDefault).Disabled(d.busy).Clicked() {
								d.javaRemove = nil
							}
						})
					})
				} else if btn(c.Key(row.ID+"-remove"), "Remove JDK", icoTrash, btnDanger).AlignSelf(ui.Start).Disabled(d.busy || a.packageTasksRunning()).Clicked() {
					d.javaRemove = &row
				}
			})
		}
	})
}

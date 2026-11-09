package main

import (
	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/action"
	"github.com/lobehub/lobe-mygo/pkg/display"
	"github.com/lobehub/lobe-mygo/pkg/theme"
	"macos-service-manager/internal/devplatform"
)

func (a *app) devJavaInstallations(c *ui.Context) {
	d := &a.dev
	if !d.javaLoaded && !d.busy && a.post != nil {
		a.devTask(func(*devplatform.Manager) (string, error) { return "Local Java installations refreshed", nil })
	}
	devDivider(c, "Installed Java")
	ui.Text(c, a.devManager().JavaInstallationDirectory()).FontSize(textSmall).TextColor(c.Theme().TextMuted)
	if action.Button(c, "Refresh installed Java", action.ButtonProps{Variant: action.Default, Size: theme.Small}).Disabled(d.busy || a.packageTasksRunning()).Clicked() {
		a.devTask(func(*devplatform.Manager) (string, error) { return "Local Java installations refreshed", nil })
	}
	if d.javaError != "" {
		ui.Text(c, d.javaError).FontSize(textSmall).TextColor(c.Theme().Warning)
	}
	if d.javaLoaded && d.javaError == "" && len(d.javaInstallations) == 0 {
		ui.Text(c, "No JDK installations found in this directory.").FontSize(textSmall)
	}
	for _, row := range d.javaInstallations {
		row := row
		devDivider(c, row.Label())
		ui.Text(c, "Vendor: "+row.Vendor).FontSize(textSmall)
		if row.Build != "" {
			ui.Text(c, "Build: "+row.Build).FontSize(textSmall)
		}
		ui.Text(c, row.Path).FontSize(textSmall)
		source := "Existing installation"
		if row.Managed {
			source = "Installed by Service Manager"
		}
		if row.Symlink {
			source = "Symbolic link → " + row.Home
		}
		ui.Text(c, source).FontSize(textSmall).TextColor(c.Theme().TextMuted)
		if row.Problem != "" {
			ui.Text(c, row.Problem).FontSize(textSmall).TextColor(c.Theme().Warning)
		}
		ui.Row(c).Wrap().Gap(6).Children(func() {
			if d.state.RuntimeDefaults["java"] == row.ID || row.PackageID != "" && d.state.RuntimeDefaults["java"] == row.PackageID {
				display.Tag(c, "Default", display.TagProps{Color: display.TagGreen, Size: theme.Small})
			} else if action.Button(c.Key(row.ID+"-default"), "Set default", action.ButtonProps{Variant: action.Default, Size: theme.Small}).Disabled(d.busy || a.packageTasksRunning() || !row.CanUse).Clicked() {
				a.devTask(func(m *devplatform.Manager) (string, error) {
					return "Java default selected; enable terminal integration to use it", m.SetJavaDefault(m.Context, row)
				})
			}
			if action.Button(c.Key(row.ID+"-copy"), "Copy JAVA_HOME", action.ButtonProps{Variant: action.Default, Size: theme.Small}).Clicked() && a.copy != nil {
				a.copy(row.Home)
			}
			if action.Button(c.Key(row.ID+"-remove"), "Remove JDK", action.ButtonProps{Variant: action.Default, Size: theme.Small}).Disabled(d.busy || a.packageTasksRunning()).Clicked() {
				d.javaRemove = &row
			}
		})
		if row.Managed {
			if p, ok := d.state.Packages[row.PackageID]; ok {
				a.devPackageIntegrity(c, row.PackageID, p)
			}
		}
		if selected := d.javaRemove; selected != nil && selected.ID == row.ID {
			ui.Column(c).Gap(6).Children(func() {
				ui.Text(c, "Remove "+selected.Label()+"?")
				ui.Text(c, selected.Path).FontSize(textSmall)
				message := "Moves only this JDK to Trash. Close Java programs using it first. IDEs and tools referencing it will need another JDK."
				label := "Move JDK to Trash"
				if selected.Symlink {
					message = "Moves only this symbolic link to Trash. The target JDK is kept."
					label = "Move link to Trash"
				}
				ui.Text(c, message).FontSize(textSmall)
				ui.Row(c).Wrap().Gap(6).Children(func() {
					if action.Button(c, label, action.ButtonProps{Variant: action.Primary, Size: theme.Small}).Disabled(d.busy || a.packageTasksRunning()).Clicked() {
						reviewed := *selected
						a.devTaskThen(func(m *devplatform.Manager) (string, error) {
							path, err := m.RemoveJavaInstallation(m.Context, reviewed)
							if err != nil {
								return "Java removal did not complete", err
							}
							return "Moved to Trash: " + path, nil
						}, func(error) { a.dev.javaRemove = nil })
					}
					if action.Button(c, "Cancel Java removal", action.ButtonProps{Variant: action.Default, Size: theme.Small}).Disabled(d.busy).Clicked() {
						d.javaRemove = nil
					}
				})
			})
		}
	}
}

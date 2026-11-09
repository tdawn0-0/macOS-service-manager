package main

import (
	"fmt"

	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/action"
	"github.com/lobehub/lobe-mygo/pkg/theme"
	"devbase/internal/devplatform"
)

func (a *app) storePackageCheck(id string, result devplatform.PackageIntegrityResult) {
	if a.dev.packageChecks == nil {
		a.dev.packageChecks = map[string]devplatform.PackageIntegrityResult{}
	}
	a.dev.packageChecks[id] = result
	if info := a.dev.packagePreview; info != nil && info.Package.ID() == id {
		info.Integrity = result.Status
		info.IntegrityNote = "Checked " + result.CheckedAt.Local().Format("15:04:05")
	}
}
func (a *app) devPackageIntegrity(c *ui.Context, id string, p devplatform.Package) {
	d := &a.dev
	status := "Not checked"
	if d.state.PackageIntegrity[id] == "" {
		status = "Not recorded"
	}
	if p.Formula != "" {
		status = "Homebrew managed"
	}
	result, checked := d.packageChecks[id]
	if checked && result.Baseline == d.state.PackageIntegrity[id] {
		status = result.Status
	} else {
		checked = false
	}
	color := c.Theme().TextMuted
	if status == "Verified" {
		color = c.Theme().Success
	}
	if status == "Needs repair" {
		color = c.Theme().Danger
	}
	ui.Text(c, "Installed files: "+status).FontSize(textSmall).FontWeight(600).TextColor(color)
	if checked {
		ui.Text(c, fmt.Sprintf("%d entries · checked %s", result.Files, result.CheckedAt.Local().Format("2006-01-02 15:04:05"))).FontSize(textSmall).TextColor(c.Theme().TextMuted)
		for _, issue := range result.Issues {
			ui.Text(c, issue).FontSize(textSmall).Selectable()
		}
	}
	ui.Row(c).Wrap().Gap(8).Children(func() {
		if action.Button(c, "Check files "+p.Name+" "+p.Version, action.ButtonProps{Variant: action.Default, Size: theme.Small}).AlignSelf(ui.Start).Tooltip(checkTip(p)).Disabled(d.busy || a.packageTaskRunning(id)).Clicked() {
			a.devTask(func(m *devplatform.Manager) (string, error) {
				r, e := m.VerifyPackage(m.Context, id)
				if e == nil {
					a.post(func() { a.storePackageCheck(id, r) })
				}
				return r.Text(), e
			})
		}
		if action.Button(c, "Repair "+p.Name+" "+p.Version, action.ButtonProps{Variant: action.Primary, Size: theme.Small}).AlignSelf(ui.Start).Tooltip("Restores this version from its recorded source and checksum. Stop databases and commands using this version first. Database data stays in separate folders.").Disabled(d.busy || a.packageTaskRunning(id) || p.Formula != "").Clicked() {
			a.startPackageTask(p, "repair", func(m *devplatform.Manager) (string, error) {
				if e := m.RepairPackage(m.Context, id); e != nil {
					return "Repair did not complete.", e
				}
				r, e := m.VerifyPackage(m.Context, id)
				if e == nil {
					a.post(func() { a.storePackageCheck(id, r) })
				}
				return "Restored the same exact version.\n" + r.Text(), e
			}, nil)
		}
	})
	if action.Button(c, "Reveal package folder "+p.Name+" "+p.Version, action.ButtonProps{Variant: action.Default, Size: theme.Small}).AlignSelf(ui.Start).Disabled(d.busy || a.reveal == nil).Clicked() {
		a.devTask(func(m *devplatform.Manager) (string, error) {
			path := m.PackageDirectory(p)
			a.post(func() { a.reveal(path) })
			return "Revealed package folder: " + path, nil
		})
	}
	if p.Formula != "" {
		ui.Text(c, "Shared Homebrew install · repair with brew reinstall "+p.Formula).FontSize(textSmall).TextColor(c.Theme().TextMuted).Selectable()
	}
}

func checkTip(p devplatform.Package) string {
	if p.Formula != "" {
		return "Validates the keg source and required executables, not every file or dependency."
	}
	return ""
}

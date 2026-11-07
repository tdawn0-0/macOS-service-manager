package main

import (
	"fmt"
	"github.com/egoist/mygo/ui"
	"macos-service-manager/internal/devplatform"
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
	} else if status == "Not recorded" {
		ui.Text(c, "Repair from the verified archive to record a trusted installation index.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
	}
	ui.Row(c).Wrap().Gap(8).Children(func() {
		if ui.Button(c, "Check files "+p.Name+" "+p.Version).Disabled(d.busy || a.packageTaskRunning(id)).Clicked() {
			a.devTask(func(m *devplatform.Manager) (string, error) {
				r, e := m.VerifyPackage(m.Context, id)
				if e == nil {
					a.post(func() { a.storePackageCheck(id, r) })
				}
				return r.Text(), e
			})
		}
		if ui.Button(c, "Repair "+p.Name+" "+p.Version).Disabled(d.busy || a.packageTaskRunning(id) || p.Formula != "").Clicked() {
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
	if ui.Button(c, "Reveal package folder "+p.Name+" "+p.Version).Disabled(d.busy || a.reveal == nil).Clicked() {
		a.devTask(func(m *devplatform.Manager) (string, error) {
			path := m.PackageDirectory(p)
			a.post(func() { a.reveal(path) })
			return "Revealed package folder: " + path, nil
		})
	}
	note := "Repair restores this version from its recorded source and checksum. Stop its projects, databases and CLI commands first. Database data stays in separate folders."
	if p.Formula != "" {
		note = "Check files validates the keg source and required executables, not every file or dependency. This Homebrew installation is shared: removing it here only removes workspace registration. To repair it, stop its users and run brew reinstall " + p.Formula + "."

	}
	ui.Text(c, note).FontSize(textSmall).TextColor(c.Theme().TextMuted)
}

package main

import (
	"fmt"
	"github.com/egoist/mygo/ui"
	"macos-service-manager/internal/devplatform"
	"strings"
)

func (d *developmentState) runtimeEditRevision(project, service string) uint64 {
	return d.runtimeEditRevisions[devplatform.ServiceStatusKey(project, service)]
}
func (d *developmentState) noteRuntimeEdit(project, service string) {
	if project == "" || service == "" {
		return
	}
	if d.runtimeEditRevisions == nil {
		d.runtimeEditRevisions = map[string]uint64{}
	}
	d.runtimeEditRevisions[devplatform.ServiceStatusKey(project, service)]++
}
func (d *developmentState) runtimeService(name string) string {
	match := func(service devplatform.Service) bool {
		if len(service.Command) == 0 {
			return false
		}
		switch name {
		case "node":
			return service.Command[0] == "node" || service.Command[0] == "npm" || service.Command[0] == "npx"
		case "php":
			return service.Command[0] == "php" || service.Command[0] == "php-fpm"
		case "go":
			return service.Command[0] == "go"
		}
		return false
	}
	for _, service := range d.draft.Services {
		if service.ID == d.serviceOriginalID && match(service) {
			return service.ID
		}
	}
	for _, service := range d.draft.Services {
		if match(service) {
			return service.ID
		}
	}
	return ""
}
func (d *developmentState) bindRuntime(project, service, packageID string) bool {
	if project == "" || d.id != project || service == "" {
		return false
	}
	for n := range d.draft.Services {
		if d.draft.Services[n].ID == service {
			d.draft.Services[n].Package = packageID
			if d.serviceOriginalID == service {
				d.servicePackage = packageID
			}
			return true
		}
	}
	return false
}

func (a *app) devRuntimeHints(c *ui.Context) {
	d := &a.dev
	devDivider(c, "Runtimes")
	if ui.Button(c, "Rescan project files").AlignSelf(ui.Start).Disabled(d.busy).Clicked() {
		dir, id := d.draft.Directory, d.id
		a.devTask(func(m *devplatform.Manager) (string, error) {
			inspection, e := devplatform.InspectProject(dir)
			a.post(func() {
				if a.dev.id != id || a.dev.draft.Directory != dir {
					return
				}
				a.dev.draft.RuntimeHints = inspection.RuntimeHints
				a.dev.draft.DetectionWarnings = nil
				if e != nil {
					a.dev.draft.DetectionWarnings = []string{e.Error()}
				}
			})
			return "Project declarations inspected. Existing service commands were kept.", e
		})
	}
	for _, warning := range d.draft.DetectionWarnings {
		ui.Text(c, warning).FontSize(textSmall).TextColor(c.Theme().Danger)
	}
	for _, runtime := range devplatform.RuntimeConflicts(d.draft.RuntimeHints) {
		ui.Text(c, "Exact "+softwareTitle(runtime)+" declarations disagree. Choose which version to use.").FontSize(textSmall).TextColor(c.Theme().Danger)
	}
	for _, hint := range d.draft.RuntimeHints {
		ui.Text(c, softwareTitle(hint.Name)+" "+hint.Version+" · "+hint.Source).FontSize(textSmall).SingleLine().Tooltip(hint.Source + ": " + hint.Version)
		version := hint.ExactVersion()
		for _, id := range sortedKeys(d.state.Packages) {
			pkg := d.state.Packages[id]
			if version != "" && pkg.Name == hint.Name && pkg.Version == version {
				label := "Use installed " + softwareTitle(pkg.Name) + " " + pkg.Version + " from " + hint.Source
				if ui.Button(c, label).AlignSelf(ui.Start).Disabled(d.busy).Clicked() && d.flushService() {
					d.noteRuntimeEdit(d.id, d.runtimeService(hint.Name))
					if d.bindRuntime(d.id, d.runtimeService(hint.Name), id) {
						d.output = "Runtime selected. Save the project to use it."
					} else {
						d.output = "Add a matching service in Services to use this runtime."
					}
				}
			}
		}
		if hint.Name != "node" && hint.Name != "php" {
			continue
		}
		label := "Choose exact " + softwareTitle(hint.Name) + " version from " + hint.Source
		if version != "" {
			label = "Choose " + softwareTitle(hint.Name) + " " + version + " from " + hint.Source
		}
		if ui.Button(c, label).AlignSelf(ui.Start).Disabled(d.busy).Clicked() && d.flushService() {
			d.runtimeTargetProject = d.id
			d.runtimeTargetService = d.runtimeService(hint.Name)
			d.software = hint.Name
			d.version = version
			if version == "" {
				d.version = devplatform.AvailableVersions(hint.Name)[0]
			}
			d.softwareContext = fmt.Sprintf("%s declares %s %s.", hint.Source, softwareTitle(hint.Name), hint.Version)
			d.tab = 1
			d.detailScroll[1].Y = 0
		}
	}
}

func (d *developmentState) previewMatches() bool {
	return d.packagePreview != nil && d.packagePreview.Package.Name == d.software && d.packagePreview.Package.Version == strings.TrimPrefix(strings.TrimSpace(d.version), "v")
}
func packageDetailsText(info devplatform.PackageDetails) string {
	if info.Package.Formula != "" {
		return fmt.Sprintf("Homebrew formula: %s\nVersion: %s\nSource: %s\nFormula SHA-256: %s\nArchitecture: %s\nLicense: %s\nCellar folder: %s", info.Package.Formula, info.Package.Version, info.Package.URL, info.Package.SHA256, info.Package.Arch, info.Package.License, info.Location)
	}
	minimum := "See upstream requirements"
	if info.Package.MinMacOS != "" {
		minimum = "macOS " + info.Package.MinMacOS + " or newer"
	}
	return fmt.Sprintf("Source: %s\nSHA-256: %s\nArchitecture: %s\nSystem: %s\nLicense: %s\nInstallation folder: %s", info.Package.URL, info.Package.SHA256, info.Package.Arch, minimum, info.Package.License, info.Location)
}
func (a *app) devPackageReview(c *ui.Context) {
	d := &a.dev
	if d.softwareContext != "" {
		ui.Text(c, d.softwareContext).FontSize(textSmall).TextColor(c.Theme().TextMuted)
	}
	if ui.Button(c, "Review package").AlignSelf(ui.Start).Disabled(d.busy || d.software == "" || strings.TrimSpace(d.version) == "").Clicked() {
		name, version := d.software, strings.TrimSpace(d.version)
		a.devTask(func(m *devplatform.Manager) (string, error) {
			pkg, e := m.ResolveInstalledOrCatalog(m.Context, name, version)
			if e != nil {
				return "", e
			}
			info, e := m.PackageDetails(m.Context, pkg)
			if e == nil {
				a.post(func() {
					a.dev.packagePreview = &info
					a.dev.packageDetailsText = packageDetailsText(info)
					if pkg.Formula != "" && a.dev.software == name && strings.TrimSpace(a.dev.version) == version {
						a.dev.version = pkg.Version
					}
				})
			}
			return "Package details ready.", e
		})
	}
	valid := d.previewMatches()
	if valid {
		info := d.packagePreview
		devDivider(c, "Package")
		size := "Download size is unavailable until transfer starts"
		if info.ArchiveBytes > 0 {
			size = fmt.Sprintf("Archive size: %.1f MiB", float64(info.ArchiveBytes)/(1<<20))
		}
		if info.Cached {
			size += " · verified cache"
		}
		if info.Installed && info.ArchiveBytes == 0 {
			size = "Installed version · download size unknown"
		}
		if info.Package.Formula != "" {
			size = "Homebrew downloads and verifies the bottle and its dependencies."
		}
		ui.Text(c, size).FontSize(textSmall).TextColor(c.Theme().TextMuted)
		ui.Text(c, info.Compatibility).FontSize(textSmall).TextColor(func() ui.Color {
			if !info.Compatible {
				return c.Theme().Danger
			}
			return c.Theme().TextMuted
		}())
		ui.Column(c).Padding(12).Radius(8).Background(c.Theme().Surface).MinWidth(0).Children(func() {
			ui.Text(c, d.packageDetailsText).FontSize(textSmall).Selectable().Label("Package details")
		})
		ui.Row(c).Gap(8).Children(func() {
			if ui.Button(c, "Copy package source").AlignSelf(ui.Start).Clicked() && a.copy != nil {
				a.copy(info.Package.URL)
			}
			if ui.Button(c, "Copy package checksum").AlignSelf(ui.Start).Clicked() && a.copy != nil {
				a.copy(info.Package.SHA256)
			}
		})
		for _, reference := range info.References {
			ui.Text(c, reference).FontSize(textSmall).TextColor(c.Theme().TextMuted)
		}
		if info.Installed && info.Integrity != "" {
			ui.Text(c, "Installed files: "+info.Integrity+". "+info.IntegrityNote).FontSize(textSmall).TextColor(c.Theme().TextMuted)
		}
	}
	installLabel := "Download and install"
	if valid && d.packagePreview.Package.Formula != "" {
		installLabel = "Install with Homebrew"
	}
	if valid && d.packagePreview.Installed {
		installLabel = "Use installed version"
	}
	if ui.PrimaryButton(c, installLabel).AlignSelf(ui.Start).Disabled(d.busy || !valid || !d.packagePreview.Compatible || (d.packagePreview.Integrity == "Needs repair" && d.packagePreview.Package.Formula == "") || a.packageTaskRunning(d.packagePreview.Package.ID())).Clicked() {
		pkg := d.packagePreview.Package
		project, service := d.runtimeTargetProject, d.runtimeTargetService
		d.noteRuntimeEdit(project, service)
		selectionRevision := d.runtimeEditRevision(project, service)
		previousPackage := ""
		for _, candidate := range d.draft.Services {
			if candidate.ID == service {
				previousPackage = candidate.Package
			}
		}
		a.startPackageTask(pkg, "install", func(m *devplatform.Manager) (string, error) {
			if err := m.Install(m.Context, pkg); err != nil {
				return "Install did not complete.", err
			}
			return "Installed " + softwareTitle(pkg.Name) + " " + pkg.Version, nil
		}, func(err error) string {
			if err != nil {
				return ""
			}
			unchanged := false
			if a.dev.id == project {
				for _, candidate := range a.dev.draft.Services {
					if candidate.ID == service && candidate.Package == previousPackage {
						unchanged = true
					}
				}
			}
			if a.dev.serviceOriginalID == service && a.dev.servicePackage != previousPackage {
				unchanged = false
			}
			bound := unchanged && a.dev.runtimeEditRevision(project, service) == selectionRevision && a.dev.bindRuntime(project, service, pkg.ID())
			if a.dev.packagePreview != nil && a.dev.packagePreview.Package.ID() == pkg.ID() {
				a.dev.packagePreview.Installed = true
			}
			if bound {
				return ". Return to the project and save its runtime selection."
			}
			if project != "" && service != "" {
				return ". The project selection changed; choose this installed version in the intended service."
			}
			return ""
		})
	}
	if d.runtimeTargetProject != "" && d.id == d.runtimeTargetProject {
		if ui.Button(c, "Back to project").Clicked() {
			d.tab = 0
			d.projectTab = 0
			d.importing = false
			d.detailScroll[0].Y = 0
		}
	}
}

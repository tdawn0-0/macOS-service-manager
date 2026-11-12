package main

import (
	"context"
	"fmt"
	"strings"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/display"
	"github.com/lobehub/lobe-mygo/pkg/feedback"
	"github.com/lobehub/lobe-mygo/pkg/theme"
	"devbase/internal/devplatform"
)

func softwareTitle(name string) string {
	if title, ok := map[string]string{"node": "Node.js", "java": "Java", "python": "Python (uv)", "mysql": "MySQL", "postgresql": "PostgreSQL", "redis": "Redis", "mongodb": "MongoDB"}[name]; ok {
		return title
	}
	return name
}

// softwareDefault is the version a piece of software runs by default, when
// one is chosen.
func (d *developmentState) softwareDefault(name string) string {
	id := ""
	switch name {
	case "node":
		id = d.state.NodeDefault
	case "java":
		id = d.state.RuntimeDefaults[name]
	}
	if p, ok := d.state.Packages[id]; ok {
		return p.Version
	}
	return ""
}

// softwareCount is how many versions of a piece of software are installed.
func (d *developmentState) softwareCount(name string) int {
	switch {
	case name == "python":
		return len(d.uvInstalled)
	case name == "java" && d.javaLoaded:
		return len(d.javaInstallations)
	}
	return len(d.packages(name))
}

// openSoftware goes to a piece of software's page.
func (a *app) openSoftware(name string) {
	d := &a.dev
	a.nav = navDevelopment
	d.tab = devTabSoftware
	d.detailScroll[devTabSoftware].Y = 0
	d.software = name
	d.softwareContext = ""
	d.version = a.defaultVersion(name)
}

func (a *app) devSoftware(c *ui.Context) {
	d := &a.dev
	if d.software == "" {
		d.software = "node"
		d.version = devplatform.AvailableVersions("node")[0]
	}
	icon, tint := softwareMeta(d.software)
	meta := "Not installed"
	if n := d.softwareCount(d.software); n > 0 {
		meta = plural(n, "version") + " installed"
	}
	if v := d.softwareDefault(d.software); v != "" {
		meta += " · default " + v
	}
	if d.software == "python" && d.uvPin != "" {
		meta += " · " + d.uvPin
	}
	var actions func()
	if engine := d.software; isDatabaseEngine(engine) && len(d.packages(engine)) > 0 {
		// An installed engine is a step towards an instance of it.
		actions = func() {
			if btn(c, "New instance", icoPlus, btnDefault).Disabled(d.busy).Clicked() {
				a.openNewInstance(engine)
			}
		}
	}
	pageHeader(c, icon, tint, softwareTitle(d.software), meta, nil, actions)
	if d.software == "python" {
		a.devPython(c)
		return
	}
	a.devPackageTasks(c)
	if d.software == "java" {
		a.devJavaInstallations(c)
		devCardWith(c, icoDownload, "Install a JDK", "", nil, func() {
			a.devRuntimeCatalog(c)
			a.devPackageReview(c)
		})
		devCardWith(c, icoTerminal, "Terminal", "", nil, func() { a.devAdditionalRuntimeShell(c) })
		return
	}
	if ids := d.packages(d.software); len(ids) > 0 {
		devCardWith(c, iconPackage, "Installed", "", nil, func() {
			for _, id := range ids {
				a.devInstalledPackage(c, id, d.state.Packages[id])
			}
		})
	}
	devCardWith(c, icoDownload, "Install a version", "", nil, func() {
		if d.software == "node" {
			a.loadNodeReleases()
			devFieldGrid(c, func() {
				devNodeChoice(c, &d.version, d.nodeReleases)
				devField(c, "Exact version", &d.version, "22.14.0, 22, lts or latest")
			})
			if d.nodeReleasesError != "" {
				devHint(c, feedback.Warning, d.nodeReleasesError, "")
			} else if d.nodeReleasesStale {
				devHint(c, feedback.Warning, "nodejs.org could not be reached. Showing the last release list.", "")
			}
		} else {
			devFieldGrid(c, func() {
				devChoice(c, "Version", &d.version, devplatform.AvailableVersions(d.software))
				devField(c, "Exact version", &d.version, "For example 22.14.0")
			})
		}
		a.devPackageReview(c)
	})
	if d.software == "node" {
		devCardWith(c, icoTerminal, "Terminal", "", nil, func() { a.devNodeTerminal(c) })
	}
}

// devInstalledPackage is an installed version in the list of its software:
// its name and version, whether it is the default, and what can be done
// with it, with the checks of its files under them.
func (a *app) devInstalledPackage(c *ui.Context, id string, p devplatform.Package) {
	d := &a.dev
	devInset(c, func() {
		ui.Row(c).Wrap().Gap(8).AlignItems(ui.Center).Children(func() {
			ui.Row(c).Gap(8).AlignItems(ui.Center).Grow(1).MinWidth(160).Children(func() {
				ui.Text(c, softwareTitle(p.Name)+" · "+p.Version).FontWeight(500).SingleLine().MinWidth(100)
				if p.Formula != "" {
					devTag(c, "Homebrew", display.TagColor(""))
				}
				if p.Name == "node" && id == d.state.NodeDefault {
					display.Tag(c, "Default", display.TagProps{Color: display.TagGreen, Size: theme.Small})
				}
				if p.Name == "java" && d.state.RuntimeDefaults[p.Name] == id {
					display.Tag(c, "Default", display.TagProps{Color: display.TagGreen, Size: theme.Small})
				}
			})
			ui.Row(c).Gap(6).AlignItems(ui.Center).Shrink(0).Children(func() {
				if p.Name == "node" && id != d.state.NodeDefault {
					if btn(c, "Set default", nil, btnDefault).Disabled(d.busy).Clicked() {
						a.devTask(func(m *devplatform.Manager) (string, error) {
							return "Default is Node.js " + p.Version, m.SetNodeDefault(m.Context, id)
						})
					}
				}
				if p.Name == "node" {
					a.devNodeExecutable(c, id)
				}
				if p.Name == "java" {
					a.devAdditionalRuntimeInstalled(c, id, p)
				}
			})
		})
		devSubtle(c, "License: "+p.License)
		a.devPackageIntegrity(c, id, p)
		removeText := "Uninstall"
		if p.Formula != "" {
			removeText = "Remove from workspace"
		}
		if btn(c, removeText, icoTrash, btnDanger).AlignSelf(ui.Start).Disabled(d.busy || a.packageTaskRunning(id)).Clicked() {
			a.devTask(func(m *devplatform.Manager) (string, error) {
				if p.Formula != "" {
					return "Removed workspace registration. The shared Homebrew installation is kept.", m.Uninstall(m.Context, id)
				}
				return "Uninstalled " + p.Name, m.Uninstall(m.Context, id)
			})
		}
	})
}

// defaultVersion is the version offered first: for Node.js the newest
// long-term support release once nodejs.org's list is read.
func (a *app) defaultVersion(name string) string {
	if name == "node" {
		for _, r := range a.dev.nodeReleases {
			if r.LTS != "" {
				return r.Version
			}
		}
	}
	if versions := devplatform.AvailableVersions(name); len(versions) > 0 {
		return versions[0]
	}
	return ""
}

// loadNodeReleases reads nodejs.org's release list in the background, once,
// and again when the workspace is refreshed.
func (a *app) loadNodeReleases() {
	d := &a.dev
	if d.nodeReleasesTried || a.post == nil {
		return
	}
	d.nodeReleasesTried = true
	refresh := d.nodeReleasesRefresh
	d.nodeReleasesRefresh = false
	m := a.devManager()
	a.spawn(func() {
		ctx, cancel := context.WithTimeout(context.Background(), 45*time.Second)
		defer cancel()
		releases, stale, e := m.NodeReleases(ctx, refresh)
		a.post(func() {
			a.dev.nodeReleases, a.dev.nodeReleasesStale, a.dev.nodeReleasesError = releases, stale, ""
			if e != nil {
				a.dev.nodeReleasesError = "Releases could not be read from nodejs.org: " + e.Error()
			}
		})
	})
}

// devNodeChoice picks among the newest release of each major version, with
// the long-term support lines named. Any other version goes in the field
// below it.
func devNodeChoice(c *ui.Context, version *string, releases []devplatform.NodeRelease) {
	if len(releases) == 0 {
		devChoice(c, "Version", version, devplatform.AvailableVersions("node"))
		return
	}
	var options []string
	lookup := map[string]string{}
	display := ""
	seen := map[string]bool{}
	for _, r := range releases {
		major := strings.SplitN(r.Version, ".", 2)[0]
		if seen[major] {
			continue
		}
		seen[major] = true
		label := r.Version
		if r.LTS != "" {
			label += " · LTS " + r.LTS
		}
		options = append(options, label)
		lookup[label] = r.Version
		if r.Version == *version {
			display = label
		}
	}
	if devChoice(c, "Version", &display, options) {
		*version = lookup[display]
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
	text := fmt.Sprintf("Source: %s\nSHA-256: %s\nArchitecture: %s\nSystem: %s\nLicense: %s\nInstallation folder: %s", info.Package.URL, info.Package.SHA256, info.Package.Arch, minimum, info.Package.License, info.Location)
	if info.Package.JavaProduct != "" {
		text = "Distribution: " + info.Package.JavaProduct + "\n" + text
	}
	return text
}

// devPackageReview reviews the version chosen and installs it: the review
// comes first, and Install acts on what it showed.
func (a *app) devPackageReview(c *ui.Context) {
	d := &a.dev
	if d.softwareContext != "" {
		devSubtle(c, d.softwareContext)
	}
	valid := d.previewMatches()
	if valid {
		info := d.packagePreview
		devInset(c, func() {
			ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
				ui.Text(c, softwareTitle(info.Package.Name)+" "+info.Package.Version).FontWeight(500).SingleLine().MinWidth(0)
				if info.Installed {
					statusTag(c, "Installed")
				}
				if info.Cached {
					devTag(c, "Cached", display.TagColor(""))
				}
				ui.Spacer(c)
				if btn(c, "Source", icoCopy, btnText).Clicked() && a.copy != nil {
					a.copy(info.Package.URL)
				}
				if btn(c, "Checksum", icoCopy, btnText).Clicked() && a.copy != nil {
					a.copy(info.Package.SHA256)
				}
			})
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
			devSubtle(c, size)
			ui.Text(c, info.Compatibility).FontSize(textSmall).TextColor(func() ui.Color {
				if !info.Compatible {
					return c.Theme().Danger
				}
				return c.Theme().TextMuted
			}())
			ui.Text(c, d.packageDetailsText).FontSize(textSmall).Selectable().Label("Package details")
			for _, reference := range info.References {
				devSubtle(c, reference)
			}
			if info.Installed && info.Integrity != "" {
				devSubtle(c, "Installed files: "+info.Integrity+". "+info.IntegrityNote)
			}
		})
	}
	installLabel := "Download and install"
	if valid && d.packagePreview.Package.Formula != "" {
		installLabel = "Install with Homebrew"
	}
	if valid && d.packagePreview.Installed {
		installLabel = "Already installed"
	}
	devButtonRow(c, func() {
		reviewKind := btnPrimary
		if valid {
			reviewKind = btnDefault
		}
		if btn(c, "Review package", icoFileCheck, reviewKind).Disabled(d.busy || d.software == "" || strings.TrimSpace(d.version) == "").Clicked() {
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
						if (pkg.Formula != "" || name == "node") && a.dev.software == name && strings.TrimSpace(a.dev.version) == version {
							a.dev.version = pkg.Version
						}
					})
				}
				return "Package details ready.", e
			})
		}
		installKind := btnDefault
		if valid {
			installKind = btnPrimary
		}
		if btn(c, installLabel, icoDownload, installKind).Disabled(d.busy || !valid || d.packagePreview.Installed || !d.packagePreview.Compatible || (d.packagePreview.Integrity == "Needs repair" && d.packagePreview.Package.Formula == "") || a.packageTaskRunning(d.packagePreview.Package.ID())).Clicked() {
			pkg := d.packagePreview.Package
			a.startPackageTask(pkg, "install", func(m *devplatform.Manager) (string, error) {
				if err := m.Install(m.Context, pkg); err != nil {
					return "Install did not complete.", err
				}
				return "Installed " + softwareTitle(pkg.Name) + " " + pkg.Version, nil
			}, nil)
		}
	})
}

package main

import (
	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/display"
	"github.com/lobehub/lobe-mygo/pkg/theme"
	"devbase/internal/devplatform"
)

func (a *app) devRuntimeCatalog(c *ui.Context) {
	d := &a.dev
	name := d.software
	if name == "java" {
		devSubtle(c, "JDK downloads from the JetBrains catalog used by IDEA. Choose a distribution and version; review its source and license before installing.")
	}
	devFieldGrid(c, func() {
		devChoice(c, "Version", &d.version, d.runtimeVersions[name])
		devField(c, "Exact version", &d.version, "Refresh available versions")
	})
	if btn(c, "Refresh available versions", icoRotate, btnText).AlignSelf(ui.Start).Disabled(d.busy).Clicked() {
		a.devTask(func(m *devplatform.Manager) (string, error) {
			rows, e := m.RuntimeVersions(m.Context, name)
			if e == nil {
				a.post(func() {
					if a.dev.runtimeVersions == nil {
						a.dev.runtimeVersions = map[string][]string{}
					}
					a.dev.runtimeVersions[name] = rows
					if name == "java" && a.dev.software == name && len(rows) > 0 {
						found := false
						for _, v := range rows {
							if v == a.dev.version {
								found = true
							}
						}
						if !found {
							a.dev.version = rows[0]
						}
					}
				})
			}
			return "Version list refreshed", e
		})
	}
}
func (a *app) devPython(c *ui.Context) {
	d := &a.dev
	devCardWith(c, icoTerminal, "uv", d.uvDescription, nil, func() {
		devSubtle(c, "Python uses uv’s existing installation store. Uninstall removes the selected interpreter; virtual environments that reference it will need repair.")
		devField(c, "uv executable", &d.uvPath, "/absolute/path/to/uv (optional)")
		devButtonRow(c, func() {
			if btn(c, "Use uv executable", icoCheck, btnDefault).Disabled(d.busy).Clicked() {
				path := d.uvPath
				a.devTask(func(m *devplatform.Manager) (string, error) {
					return "uv executable selected", m.SetUVPath(m.Context, path)
				})
			}
			if btn(c, "Refresh Python versions", icoRotate, btnText).Disabled(d.busy).Clicked() {
				a.devTask(func(m *devplatform.Manager) (string, error) {
					description, e := m.UVDescription(m.Context)
					if e != nil {
						return "", e
					}
					installed, e := m.UVPythons(m.Context, true)
					if e != nil {
						return "", e
					}
					pin, pinErr := m.UVGlobalPin(m.Context)
					if pinErr != nil {
						pin = "Unavailable; refresh to retry"
					} else if pin == "" {
						pin = "No global pin"
					}
					a.post(func() { a.dev.uvInstalled = installed; a.dev.uvDescription = description; a.dev.uvPin = pin })
					available, e := m.UVPythons(m.Context, false)
					if e == nil {
						a.post(func() { a.dev.uvInstalled = installed; a.dev.uvAvailable = available })
					}
					return "Python versions refreshed", e
				})
			}
		})
	})
	devCardWith(c, icoDownload, "Install a version", "", nil, func() {
		var keys []string
		for _, r := range d.uvAvailable {
			if r.OS == "macos" && r.Arch == "aarch64" {
				keys = append(keys, r.Key)
			}
		}
		devChoice(c, "Python version", &d.uvVersion, keys)
		if btn(c, "Install Python with uv", icoDownload, btnPrimary).AlignSelf(ui.Start).Disabled(d.busy || d.uvVersion == "").Clicked() {
			key := d.uvVersion
			a.devTask(func(m *devplatform.Manager) (string, error) {
				e := m.InstallUVPython(m.Context, key)
				if e == nil {
					rows, err := m.UVPythons(m.Context, true)
					e = err
					if e == nil {
						a.post(func() { a.dev.uvInstalled = rows })
					}
				}
				return "Python installed by uv", e
			})
		}
	})
	note := ""
	if d.uvPin != "" {
		note = "Global uv pin: " + d.uvPin
	}
	devCardWith(c, iconPackage, "Installed by uv", note, nil, func() {
		devSubtle(c, "Set uv default updates the global uv Python pin. Directory pins and active virtual environments still take precedence; this does not replace the shell's python command.")
		if len(d.uvInstalled) == 0 {
			devSubtle(c, "None installed")
		}
		for _, r := range d.uvInstalled {
			r := r
			devInset(c, func() {
				ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
					ui.Text(c, r.Key).FontWeight(600).SingleLine().MinWidth(0)
					if d.uvPin == r.Key {
						display.Tag(c, "Default", display.TagProps{Color: display.TagGreen, Size: theme.Small})
					}
				})
				devButtonRow(c, func() {
					if btn(c, "Copy path", icoCopy, btnText).Clicked() && a.copy != nil {
						a.copy(r.Path)
					}
					for _, act := range []string{"default", "uninstall"} {
						label, kind, icon := "Set uv default "+r.Version, btnDefault, (*ui.SVG)(nil)
						if act == "uninstall" {
							label, kind, icon = "Uninstall Python "+r.Version, btnDanger, icoTrash
						}
						if btn(c, label, icon, kind).Disabled(d.busy).Clicked() {
							a.devTask(func(m *devplatform.Manager) (string, error) {
								e := m.UVPythonAction(m.Context, r.Key, act)
								if e == nil && act == "default" {
									a.post(func() { a.dev.uvPin = r.Key })
								}
								if e == nil {
									rows, err := m.UVPythons(m.Context, true)
									e = err
									if e == nil {
										a.post(func() { a.dev.uvInstalled = rows })
									}
								}
								return "Python " + act + " completed", e
							})
						}
					}
				})
			})
		}
	})
}

func (a *app) devAdditionalRuntimeInstalled(c *ui.Context, id string, p devplatform.Package) {
	if a.dev.state.RuntimeDefaults[p.Name] != id {
		if btn(c, "Set default", nil, btnDefault).Disabled(a.dev.busy).Clicked() {
			a.devTask(func(m *devplatform.Manager) (string, error) {
				return "Default version selected; enable terminal integration to use it", m.SetRuntimeDefault(m.Context, p.Name, id)
			})
		}
	}
	if btn(c, "Copy executable path", icoCopy, btnText).Disabled(a.dev.busy).Clicked() {
		path, e := a.devManager().Binary(&a.dev.state, id, p.Name)
		if e != nil {
			a.dev.output = e.Error()
		} else if a.copy != nil {
			a.copy(path)
		}
	}
}
func (a *app) devAdditionalRuntimeShell(c *ui.Context) {
	name := a.dev.software
	d := &a.dev
	if d.runtimeShell == nil {
		d.runtimeShell = map[string]int{}
	}
	if d.runtimeShell[name] == 0 {
		d.runtimeShell[name] = 2
		if a.devManager().RuntimeShellEnabled(name) {
			d.runtimeShell[name] = 1
		}
	}
	devSubtle(c, "Detects installed shells and enables new terminal and AI sessions.")
	devButtonRow(c, func() {
		if d.runtimeShell[name] == 1 {
			ui.Row(c).Gap(8).AlignItems(ui.Center).Grow(1).MinWidth(0).Children(func() {
				statusTag(c, "Enabled")
				ui.Text(c, "Enabled for detected shells").FontSize(textSmall).TextColor(c.Theme().Success).SingleLine().MinWidth(0)
			})
		} else if btn(c, "Enable in terminal", icoTerminal, btnPrimary).Disabled(a.dev.busy).Clicked() {
			a.devTask(func(m *devplatform.Manager) (string, error) {
				e := m.ConfigureRuntimeShell(m.Context, name, true)
				a.post(func() {
					if a.dev.runtimeShell != nil {
						a.dev.runtimeShell[name] = 0
					}
				})
				return "Terminal configuration updated; open a new terminal", e
			})
		}
		if btn(c, "Disable in terminal", nil, btnDefault).Disabled(a.dev.busy).Clicked() {
			a.devTask(func(m *devplatform.Manager) (string, error) {
				e := m.ConfigureRuntimeShell(m.Context, name, false)
				a.post(func() {
					if a.dev.runtimeShell != nil {
						a.dev.runtimeShell[name] = 0
					}
				})
				return "Terminal configuration updated; open a new terminal", e
			})
		}
		a.devBashCommand(c, name)
	})
}

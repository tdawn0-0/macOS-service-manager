package main

import (
	"context"
	"time"

	"devbase/internal/devplatform"
	"github.com/egoist/mygo"
	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/display"
	"github.com/lobehub/lobe-mygo/pkg/theme"
)

func (a *app) applyRuntimeVersions(name string, rows []string, overwrite bool) {
	if a.dev.runtimeVersions == nil {
		a.dev.runtimeVersions = map[string][]string{}
	}
	if !overwrite && len(a.dev.runtimeVersions[name]) > 0 {
		return
	}
	a.dev.runtimeVersions[name] = rows
	if name == "java" && a.dev.software == name && len(rows) > 0 {
		found := false
		for _, v := range rows {
			if v == a.dev.version {
				found = true
				break
			}
		}
		if !found {
			a.dev.version = rows[0]
		}
	}
}

func (a *app) devRuntimeCatalog(c *ui.Context) {
	d := &a.dev
	name := d.software
	if name == "java" && a.post != nil && d.runtimeVersions["java"] == nil {
		if d.runtimeVersions == nil {
			d.runtimeVersions = map[string][]string{}
		}
		d.runtimeVersions["java"] = []string{}
		m := a.devManager()
		a.spawn(func() {
			ctx, cancel := context.WithTimeout(context.Background(), 45*time.Second)
			defer cancel()
			rows, e := m.RuntimeVersions(ctx, name)
			if e != nil {
				return
			}
			a.post(func() { a.applyRuntimeVersions(name, rows, false) })
		})
	}
	versionChanged := false
	devFieldGrid(c, func() {
		versionChanged = devChoice(c, "Version", &d.version, d.runtimeVersions[name])
		devField(c, "Exact version", &d.version, "Refresh available versions")
	})
	if versionChanged {
		a.previewPackageAsync(name, d.version)
	}
	if btn(c, "Refresh available versions", icoRotate, btnText).AlignSelf(ui.Start).Disabled(d.busy).Clicked() {
		a.devTask(func(m *devplatform.Manager) (string, error) {
			rows, e := m.RuntimeVersions(m.Context, name)
			if e == nil {
				a.post(func() { a.applyRuntimeVersions(name, rows, true) })
			}
			return "Version list refreshed", e
		})
	}
}

func (a *app) fetchUVPython(ctx context.Context, m *devplatform.Manager, force bool) error {
	description, e := m.UVDescription(ctx)
	if e != nil {
		return e
	}
	installed, e := m.UVPythons(ctx, true)
	if e != nil {
		return e
	}
	pin, pinErr := m.UVGlobalPin(ctx)
	if pinErr != nil {
		pin = "Unavailable; refresh to retry"
	} else if pin == "" {
		pin = "No global pin"
	}
	a.post(func() {
		if !force && (len(a.dev.uvInstalled) > 0 || len(a.dev.uvAvailable) > 0) {
			return
		}
		a.dev.uvInstalled = installed
		a.dev.uvDescription = description
		a.dev.uvPin = pin
	})
	available, e := m.UVPythons(ctx, false)
	if e == nil {
		a.post(func() {
			if !force && len(a.dev.uvAvailable) > 0 {
				return
			}
			a.dev.uvInstalled = installed
			a.dev.uvAvailable = available
			if a.dev.uvVersion == "" {
				for _, r := range available {
					if r.OS == "macos" && r.Arch == "aarch64" {
						a.dev.uvVersion = r.Key
						break
					}
				}
			}
		})
	}
	return e
}

func (a *app) devPython(c *ui.Context) {
	d := &a.dev
	if a.post != nil && d.runtimeVersions["python"] == nil && len(d.uvAvailable) == 0 && len(d.uvInstalled) == 0 {
		if d.runtimeVersions == nil {
			d.runtimeVersions = map[string][]string{}
		}
		d.runtimeVersions["python"] = []string{}
		m := a.devManager()
		a.spawn(func() {
			ctx, cancel := context.WithTimeout(context.Background(), 45*time.Second)
			defer cancel()
			_ = a.fetchUVPython(ctx, m, false)
		})
	}
	devCardWith(c, icoTerminal, "uv", d.uvDescription, nil, func() {
		devField(c, "uv executable", &d.uvPath, "/absolute/path/to/uv (optional)")
		devButtonRow(c, func() {
			if btn(c, "Choose uv executable…", icoFolder, btnDefault).Disabled(d.busy).Clicked() {
				paths, e := a.pickPaths(mygo.OpenDialogOptions{
					Title:           "Select uv executable",
					ShowHiddenFiles: true,
				})
				if e != nil {
					d.output = e.Error()
				} else if len(paths) > 0 {
					d.uvPath = paths[0]
				}
			}
			if btn(c, "Use uv executable", icoCheck, btnDefault).Disabled(d.busy).Clicked() {
				path := d.uvPath
				a.devTask(func(m *devplatform.Manager) (string, error) {
					return "uv executable selected", m.SetUVPath(m.Context, path)
				})
			}
			if btn(c, "Refresh Python versions", icoRotate, btnText).Disabled(d.busy).Clicked() {
				a.devTask(func(m *devplatform.Manager) (string, error) {
					return "Python versions refreshed", a.fetchUVPython(m.Context, m, true)
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
	devCardWith(c, iconPackage, "Installed by uv", "", nil, func() {
		if len(d.uvInstalled) == 0 {
			devSubtle(c, "None installed")
		}
		for _, r := range d.uvInstalled {
			r := r
			devInset(c, func() {
				ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
					ui.Text(c, r.Key).FontWeight(500).SingleLine().MinWidth(0)
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
	devButtonRow(c, func() {
		if d.runtimeShell[name] == 1 {
			ui.Row(c).Gap(8).AlignItems(ui.Center).Grow(1).MinWidth(0).Children(func() {
				devStatusDot(c, "Succeeded")
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

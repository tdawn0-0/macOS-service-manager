package main

import (
	"github.com/egoist/mygo/ui"
	"macos-service-manager/internal/devplatform"
)

func (a *app) devRuntimeCatalog(c *ui.Context) {
	d := &a.dev
	name := d.software
	if name == "java" {
		ui.Text(c, "JDK downloads from the JetBrains catalog used by IDEA. Choose a distribution and version; review its source and license before installing.").FontSize(textSmall)
	}
	if ui.Button(c, "Refresh available versions").Disabled(d.busy).Clicked() {
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
	devChoice(c, "Version", &d.version, d.runtimeVersions[name])
	devField(c, "Exact version", &d.version, "Refresh available versions")
}
func (a *app) devPython(c *ui.Context) {
	d := &a.dev
	ui.Text(c, "Python uses uv’s existing installation store. Uninstall removes the selected interpreter; virtual environments that reference it will need repair.").FontSize(textSmall)
	devField(c, "uv executable", &d.uvPath, "/absolute/path/to/uv (optional)")
	if ui.Button(c, "Use uv executable").Disabled(d.busy).Clicked() {
		path := d.uvPath
		a.devTask(func(m *devplatform.Manager) (string, error) {
			return "uv executable selected", m.SetUVPath(m.Context, path)
		})
	}
	if ui.Button(c, "Refresh Python versions").Disabled(d.busy).Clicked() {
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
	if d.uvDescription != "" {
		ui.Text(c, d.uvDescription).FontSize(textSmall)
	}
	var keys []string
	for _, r := range d.uvAvailable {
		if r.OS == "macos" && r.Arch == "aarch64" {
			keys = append(keys, r.Key)
		}
	}
	devChoice(c, "Python version", &d.uvVersion, keys)
	if ui.PrimaryButton(c, "Install Python with uv").Disabled(d.busy || d.uvVersion == "").Clicked() {
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
	devDivider(c, "Installed by uv")
	if d.uvPin != "" {
		ui.Text(c, "Global uv pin: "+d.uvPin).FontSize(textSmall)
	}
	ui.Text(c, "Set uv default updates the global uv Python pin. Directory pins and active virtual environments still take precedence; this does not replace the shell's python command.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
	for _, r := range d.uvInstalled {
		r := r
		ui.Column(c).Gap(6).Children(func() {
			ui.Text(c, r.Key)
			ui.Row(c).Wrap().Gap(6).Children(func() {
				if ui.Button(c, "Copy Python path "+r.Version).Clicked() && a.copy != nil {
					a.copy(r.Path)
				}
				for _, action := range []string{"default", "uninstall"} {
					label := "Set uv default " + r.Version
					if action == "uninstall" {
						label = "Uninstall Python " + r.Version
					}
					if ui.Button(c, label).Disabled(d.busy).Clicked() {
						a.devTask(func(m *devplatform.Manager) (string, error) {
							e := m.UVPythonAction(m.Context, r.Key, action)
							if e == nil && action == "default" {
								a.post(func() { a.dev.uvPin = r.Key })
							}
							if e == nil {
								rows, err := m.UVPythons(m.Context, true)
								e = err
								if e == nil {
									a.post(func() { a.dev.uvInstalled = rows })
								}
							}
							return "Python " + action + " completed", e
						})
					}
				}
			})
		})
	}
}

func (a *app) devAdditionalRuntimeInstalled(c *ui.Context, id string, p devplatform.Package) {
	if a.dev.state.RuntimeDefaults[p.Name] == id {
		ui.Text(c, "Default")
	} else if ui.Button(c, "Set default "+p.Version).Disabled(a.dev.busy).Clicked() {
		a.devTask(func(m *devplatform.Manager) (string, error) {
			return "Default version selected; enable terminal integration to use it", m.SetRuntimeDefault(m.Context, p.Name, id)
		})
	}
	if ui.Button(c, "Copy executable path").Disabled(a.dev.busy).Clicked() {
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
	ui.Text(c, "Detects installed shells and enables new terminal and AI sessions.").FontSize(textSmall)
	for _, enable := range []bool{true, false} {
		label := "Disable in terminal"
		if enable {
			label = "Enable in terminal"
		}
		if ui.Button(c, label).Disabled(a.dev.busy).Clicked() {
			a.devTask(func(m *devplatform.Manager) (string, error) {
				return "Terminal configuration updated; open a new terminal", m.ConfigureRuntimeShell(m.Context, name, enable)
			})
		}
	}
	a.devBashCommand(c, name)
}

package main

import (
	"github.com/egoist/mygo/ui"
	"macos-service-manager/internal/devplatform"
)

// devNodeTerminal offers the shell setup that makes this app replace fnm and
// nvm: each folder runs the Node.js version its files name.
func (a *app) devNodeTerminal(c *ui.Context) {
	d := &a.dev
	if d.nodeShell == 0 {
		d.nodeShell = 2
		if a.devManager().NodeShellEnabled() {
			d.nodeShell = 1
			m := a.devManager()
			a.spawn(func() { _ = m.WriteNodeShell() })
		}
	}
	devDivider(c, "Terminal")
	ui.Text(c, "Enable or disable in a new terminal. Running Node.js processes keep their version.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
	ui.Row(c).Wrap().Gap(8).AlignItems(ui.Center).Children(func() {
		if d.nodeShell == 1 {
			ui.Text(c, "Enabled in ~/.zshrc").FontSize(textSmall).TextColor(c.Theme().Success).Grow(1).MinWidth(0)
			if ui.Button(c, "Disable in terminal").Disabled(d.busy).Clicked() {
				a.devTask(func(m *devplatform.Manager) (string, error) {
					_, e := m.DisableNodeShell(m.Context)
					a.post(func() { a.dev.nodeShell = 0 })
					return "Removed the Service Manager block from ~/.zshrc", e
				})
			}
			return
		}
		if ui.PrimaryButton(c, "Enable in terminal").Tooltip("Adds one block to ~/.zshrc. Each folder then runs the Node.js its .node-version, .nvmrc or package.json engines name, else the default.").Disabled(d.busy).Clicked() {
			a.devTask(func(m *devplatform.Manager) (string, error) {
				rc, e := m.EnableNodeShell(m.Context)
				a.post(func() { a.dev.nodeShell = 0 })
				return "Added the Service Manager block to " + rc + ". Open a new terminal.", e
			})
		}
	})
}

// devNodeExecutable copies only a managed, executable Node binary path.
func (a *app) devNodeExecutable(c *ui.Context, id string) {
	if ui.Button(c, "Copy executable path").Disabled(a.dev.busy).Clicked() {
		path, err := a.devManager().Binary(&a.dev.state, id, "node")
		if err != nil {
			a.dev.output = "Node.js executable is unavailable. Repair the installation before copying its path."
			return
		}
		if a.copy != nil {
			a.copy(path)
		}
		a.dev.output = "Copied Node.js executable path."
	}
}

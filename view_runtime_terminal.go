package main

import (
	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/action"
	"github.com/lobehub/lobe-mygo/pkg/theme"
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
		}
	}
	devDivider(c, "Terminal")
	ui.Text(c, "Detects installed shells and enables new terminal and AI sessions.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
	ui.Row(c).Wrap().Gap(8).AlignItems(ui.Center).Children(func() {
		if d.nodeShell == 1 {
			ui.Text(c, "Enabled for detected shells").FontSize(textSmall).TextColor(c.Theme().Success).Grow(1).MinWidth(0)
		} else if action.Button(c, "Enable in terminal", action.ButtonProps{Variant: action.Primary, Size: theme.Small}).Tooltip("Configures installed zsh, Bash and fish, including non-interactive commands. Each folder runs its declared Node.js version, else the default.").Disabled(d.busy).Clicked() {
			a.devTask(func(m *devplatform.Manager) (string, error) {
				_, e := m.EnableNodeShell(m.Context)
				a.post(func() { a.dev.nodeShell = 0 })
				return "Enabled for installed shells. Start a new terminal or AI session.", e
			})
		}
		// Cleanup remains available for partial/older setups and uninstalled shells.
		if action.Button(c, "Disable in terminal", action.ButtonProps{Variant: action.Default, Size: theme.Small}).Disabled(d.busy).Clicked() {
			a.devTask(func(m *devplatform.Manager) (string, error) {
				_, e := m.DisableNodeShell(m.Context)
				a.post(func() { a.dev.nodeShell = 0 })
				return "Removed the Devbase blocks from all supported shells", e
			})
		}
	})
	a.devBashCommand(c, "node")
}

// devNodeExecutable copies only a managed, executable Node binary path.
func (a *app) devNodeExecutable(c *ui.Context, id string) {
	if action.Button(c, "Copy executable path", action.ButtonProps{Variant: action.Default, Size: theme.Small}).Disabled(a.dev.busy).Clicked() {
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

func (a *app) devBashCommand(c *ui.Context, name string) {
	if action.Button(c, "Copy Bash command", action.ButtonProps{Variant: action.Default, Size: theme.Small}).Tooltip("For standalone AI tasks using bash -c. Enable terminal integration first, then replace YOUR_COMMAND in the copied command.").Disabled(a.dev.busy).Clicked() {
		m := a.devManager()
		if !m.RuntimeShellEnabled(name) {
			a.dev.output = "Enable in terminal first to prepare the Bash environment."
			return
		}
		if a.copy != nil {
			a.copy(m.BashCommand())
		}
		a.dev.output = "Copied Bash command. Replace YOUR_COMMAND with the AI task's command."
	}
}

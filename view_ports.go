package main

import (
	"fmt"
	"strconv"

	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/action"
	"github.com/lobehub/lobe-mygo/pkg/theme"
	"devbase/internal/devplatform"
)

func (a *app) storePortInspection(result devplatform.PortInspection) {
	if a.dev.portChecks == nil {
		a.dev.portChecks = map[int]devplatform.PortInspection{}
	}
	a.dev.portChecks[result.Port] = result
}
func (a *app) checkDevPort(port int) {
	a.devTask(func(m *devplatform.Manager) (string, error) {
		result, err := m.InspectPort(m.Context, port)
		if err == nil {
			a.post(func() { a.storePortInspection(result) })
		}
		return fmt.Sprintf("Port %d checked.", port), err
	})
}
func (a *app) devPortResult(c *ui.Context, port int) {
	r, ok := a.dev.portChecks[port]
	if !ok {
		return
	}
	color := c.Theme().Warning
	if r.Status == "Available" {
		color = c.Theme().Success
	}
	ui.Text(c, fmt.Sprintf("Port %d · %s", port, r.Status)).FontSize(textSmall).FontWeight(600).TextColor(color)
	for _, listener := range r.Listeners {
		owner := "UID unknown"
		if listener.UID >= 0 {
			owner = fmt.Sprintf("UID %d", listener.UID)
		}
		ui.Text(c, fmt.Sprintf("PID %d · %s · %s", listener.PID, owner, listener.Name)).FontSize(textSmall)
		ui.Text(c, listener.Address).FontSize(textSmall).TextColor(c.Theme().TextMuted)
	}
	if r.Truncated {
		ui.Text(c, "Showing the first 64 listener records.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
	}
	ui.Text(c, r.Detail).FontSize(textSmall).TextColor(c.Theme().TextMuted)
}
func (a *app) devPortControls(c *ui.Context, value *string, scope string) {
	d := &a.dev
	port, err := strconv.Atoi(*value)
	ui.Row(c).Wrap().Gap(8).Children(func() {
		if action.Button(c, "Check "+scope+" port", action.ButtonProps{Variant: action.Default, Size: theme.Small}).AlignSelf(ui.Start).Disabled(d.busy || err != nil || port < 1 || port > 65535).Clicked() {
			a.checkDevPort(port)
		}
		if action.Button(c, "Find available "+scope+" port", action.ButtonProps{Variant: action.Default, Size: theme.Small}).AlignSelf(ui.Start).Disabled(d.busy).Clicked() {
			original, tab, dbID := *value, d.tab, d.dbID
			a.devTask(func(m *devplatform.Manager) (string, error) {
				suggestion, err := m.AvailableLocalPort(m.Context)
				if err == nil {
					a.post(func() {
						if a.dev.tab != tab || *value != original {
							return
						}
						if scope == "database" && a.dev.dbID != dbID {
							return
						}
						*value = strconv.Itoa(suggestion)
					})
				}
				return fmt.Sprintf("Suggested port: %d. An unchanged form is filled automatically. Review, save and retry; this does not reserve the port.", suggestion), err
			})
		}
	})
	if err == nil {
		a.devPortResult(c, port)
	}
}

func (a *app) devDatabasePort(c *ui.Context, instance devplatform.Instance) {
	d := &a.dev
	devDivider(c, "Local endpoint")
	if d.dbPort != strconv.Itoa(instance.Port) {
		ui.Text(c, fmt.Sprintf("Current configured port: %d", instance.Port)).FontSize(textSmall)
		a.devPortResult(c, instance.Port)
	}
	devField(c, "Database port", &d.dbPort, strconv.Itoa(instance.Port))
	a.devPortControls(c, &d.dbPort, "database")
	port, err := strconv.Atoi(d.dbPort)
	if action.Button(c, "Save database port", action.ButtonProps{Variant: action.Primary, Size: theme.Small}).AlignSelf(ui.Start).Tooltip("Stop this instance and disconnect its clients first. Data, users and password are kept; update client connection settings afterwards.").Disabled(d.busy || err != nil || port < 1024 || port > 65535 || port == instance.Port).Clicked() {
		a.devTask(func(m *devplatform.Manager) (string, error) {
			return "Database port update. Review connection details before restarting.", m.ChangeInstancePort(m.Context, instance, port)
		})
	}
	if action.Button(c, "Repair saved port settings", action.ButtonProps{Variant: action.Default, Size: theme.Small}).AlignSelf(ui.Start).Tooltip("Restores the saved endpoint after an interrupted change. Data and credentials stay in place.").Disabled(d.busy).Clicked() {
		a.devTask(func(m *devplatform.Manager) (string, error) {
			return "Saved port settings repair. Check the port before starting.", m.RepairInstancePort(m.Context, instance)
		})
	}
}

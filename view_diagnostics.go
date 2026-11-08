package main

import (
	"fmt"
	"github.com/egoist/mygo/ui"
	"macos-service-manager/internal/devplatform"
)

func devStatusColor(c *ui.Context, status string) ui.Color {
	switch status {
	case "Healthy":
		return c.Theme().Success
	case "Failed":
		return c.Theme().Danger
	case "Not ready", "Partly running", "Restarting", "Unknown":
		return c.Theme().Warning
	}
	return c.Theme().TextMuted
}

func (a *app) showDevDiagnostics(report devplatform.DiagnosticReport) {
	a.dev.diagnosticReport = &report
	a.dev.diagnosticsOpen = true
	a.dev.diagnosticScroll.Y = 0
	b, err := report.RedactedJSON()
	if err == nil {
		a.dev.diagnosticJSON = string(b)
	}
}

func (a *app) runDevDiagnostics() {
	a.dev.diagnosticsOpen = true
	a.devTask(func(m *devplatform.Manager) (string, error) {
		report, err := m.Diagnose(m.Context)
		if err == nil {
			a.post(func() { a.showDevDiagnostics(report) })
		}
		return "Diagnostics finished. Review checks by layer.", err
	})
}

func (a *app) devDiagnostics(c *ui.Context) {
	d := &a.dev
	ui.Column(c).Gap(10).Children(func() {
		report := d.diagnosticReport
		summary := ""
		if report != nil {
			failures, warnings := 0, 0
			for _, check := range report.Checks {
				if check.Status == "Failed" {
					failures++
				}
				if check.Status == "Warning" {
					warnings++
				}
			}
			summary = fmt.Sprintf("%d failed · %d warnings · %d checks", failures, warnings, len(report.Checks))
		}
		ui.Row(c).Wrap().Gap(8).AlignItems(ui.Center).Children(func() {
			ui.Text(c, summary).FontWeight(600).Grow(1).MinWidth(0)
			if report != nil {
				if ui.Button(c, "Copy redacted report").Tooltip("Anonymous resource names. Excludes local paths, domains, commands, environment values, logs, credentials and private keys.").Disabled(d.busy || a.copy == nil).Clicked() {
					a.copy(d.diagnosticJSON)
				}
				if ui.Button(c, "Export redacted report").Tooltip("Anonymous resource names. Excludes local paths, domains, commands, environment values, logs, credentials and private keys.").Disabled(d.busy).Clicked() {
					snapshot := *report
					a.devTask(func(m *devplatform.Manager) (string, error) {
						path, err := m.ExportDiagnostics(m.Context, snapshot)
						if err == nil && a.reveal != nil {
							a.post(func() { a.reveal(path) })
						}
						return "Redacted report saved: " + path, err
					})
				}
			}
			if ui.Button(c, "Run diagnostics again").Tooltip("Changes no hosts, certificates or services.").Disabled(d.busy).Clicked() {
				a.runDevDiagnostics()
			}
			if ui.Button(c, "Back to workspace").Clicked() {
				d.diagnosticsOpen = false
			}
		})
		if d.diagnosticPendingTarget != nil {
			ui.Text(c, "Unsaved database changes. Discard them to open the selected setting?").TextColor(c.Theme().Warning)
			ui.Row(c).Gap(8).Children(func() {
				if ui.Button(c, "Cancel navigation").Clicked() {
					d.diagnosticPendingTarget = nil
				}
				if ui.Button(c, "Discard draft and open").Clicked() {
					target := *d.diagnosticPendingTarget
					d.diagnosticPendingTarget = nil
					a.openDiagnosticTarget(target, true)
				}
			})
		}
		if report == nil {
			return
		}
		ui.Text(c, "Checked "+report.GeneratedAt.Local().Format("2006-01-02 15:04:05")).FontSize(textCaption).TextColor(c.Theme().TextMuted)
		for _, check := range report.Checks {
			color := c.Theme().TextMuted
			if check.Status == "Failed" {
				color = c.Theme().Danger
			}
			if check.Status == "Warning" {
				color = c.Theme().Warning
			}
			if check.Status == "OK" {
				color = c.Theme().Success
			}
			ui.Column(c).Padding(12).Radius(8).Background(c.Theme().Surface).Gap(5).Children(func() {
				ui.Text(c, check.Status+" · "+check.Layer).FontSize(textSmall).FontWeight(650).TextColor(color)
				ui.Text(c, check.Resource).FontWeight(600)
				ui.Text(c, check.Summary).FontSize(textSmall)
				if check.Remedy != "" {
					ui.Text(c, check.Remedy).FontSize(textSmall).TextColor(c.Theme().TextMuted)
				}
				if check.Target.Kind != "" && ui.Button(c, "Open related setting").Disabled(d.busy).Clicked() {
					a.openDiagnosticTarget(check.Target, false)
				}
			})
		}
	})
}

// openDiagnosticTarget resolves a private, typed target against current state.
func (a *app) openDiagnosticTarget(target devplatform.DiagnosticTarget, discard bool) {
	d := &a.dev
	switch target.Kind {
	case "proxy":
		if target.ID != "" {
			found := false
			for _, rule := range d.state.ProxyRules {
				if rule.Domain == target.ID {
					found = true
					break
				}
			}
			if !found {
				d.output = "That proxy rule is no longer saved. Run diagnostics again."
				return
			}
		}
		d.tab = devTabHTTPS
	case "ca":
		d.tab = devTabHTTPS
		d.authorityOpen = true
	case "ports":
		d.tab = devTabHTTPS
	case "software":
		pkg, ok := d.state.Packages[target.ID]
		if !ok {
			d.output = "That software version is no longer installed. Run diagnostics again."
			return
		}
		d.tab = devTabSoftware
		d.software = pkg.Name
		d.version = pkg.Version
	case "database":
		instance, ok := d.state.Instances[target.ID]
		if !ok {
			d.output = "That database instance is no longer saved. Run diagnostics again."
			return
		}
		if d.dbID != target.ID && !discard && (d.dbNewDraft || d.dbName != "" || d.dbUser != "" || d.dbPassword != "") {
			copy := target
			d.diagnosticPendingTarget = &copy
			return
		}
		d.tab = devTabDatabases
		if d.dbID != target.ID {
			d.dbID = instance.ID
			d.dbKind = instance.Kind
			d.dbPackage = instance.Package
			d.dbPort = fmt.Sprint(instance.Port)
			d.persistent = instance.Persistent
			d.dbName = ""
			d.dbUser = ""
			d.dbPassword = ""
			d.dbNewDraft = false
		}
	default:
		d.output = "The requested setting is unavailable."
		return
	}
	d.diagnosticsOpen = false
}

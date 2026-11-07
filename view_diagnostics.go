package main

import (
	"fmt"
	"github.com/egoist/mygo/ui"
	"macos-service-manager/internal/devplatform"
)

func devProjectStarted(status string) bool {
	switch status {
	case "Running", "Healthy", "Not ready", "Partly running", "Starting", "Restarting", "Stopping", "Failed":
		return true
	}
	return false
}
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
		devHeading(c, "Find the failing layer", "Check software, processes, database ports, DNS, CA trust and HTTPS separately.")
		ui.Row(c).Gap(8).Children(func() {
			if ui.Button(c, "Back to workspace").Clicked() {
				d.diagnosticsOpen = false
			}
			if ui.Button(c, "Run diagnostics again").Disabled(d.busy).Clicked() {
				a.runDevDiagnostics()
			}
		})
		if d.diagnosticReport == nil {
			ui.Text(c, "Run diagnostics to create a report. This does not change hosts, certificates or services.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
			return
		}
		report := d.diagnosticReport
		failures, warnings := 0, 0
		for _, check := range report.Checks {
			if check.Status == "Failed" {
				failures++
			}
			if check.Status == "Warning" {
				warnings++
			}
		}
		ui.Text(c, fmt.Sprintf("%d failed · %d warnings · %d checks", failures, warnings, len(report.Checks))).FontWeight(600)
		ui.Text(c, "Checked "+report.GeneratedAt.Local().Format("2006-01-02 15:04:05")+". Results are a snapshot; run again after a change.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
		ui.Row(c).Gap(8).Children(func() {
			if ui.Button(c, "Copy redacted report").Disabled(d.busy || a.copy == nil).Clicked() {
				a.copy(d.diagnosticJSON)
			}
			if ui.Button(c, "Export redacted report").Disabled(d.busy).Clicked() {
				snapshot := *report
				a.devTask(func(m *devplatform.Manager) (string, error) {
					path, err := m.ExportDiagnostics(m.Context, snapshot)
					if err == nil && a.reveal != nil {
						a.post(func() { a.reveal(path) })
					}
					return "Redacted report saved: " + path, err
				})
			}
		})
		ui.Text(c, "Export uses anonymous resource names. It excludes local paths, domains, commands, environment values, logs, credentials and private keys.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
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
			})
		}
	})
}

package main

import (
	"fmt"

	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/display"
	"github.com/lobehub/lobe-mygo/pkg/feedback"
	"github.com/lobehub/lobe-mygo/pkg/theme"
	"devbase/internal/devplatform"
)

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
	tk := theme.TokensOf(c)
	report := d.diagnosticReport
	summary := ""
	status := ""
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
		switch {
		case failures > 0:
			status = "Failed"
		case warnings > 0:
			status = "Not ready"
		default:
			status = "Healthy"
		}
	}
	pageHeader(c, icoStetho, tk.TextSecondary, "Diagnostics", summary, func() { statusTag(c, status) }, func() {
		if btn(c, "Run diagnostics again", icoRotate, btnPrimary).Tooltip("Changes no hosts, certificates or services.").Disabled(d.busy).Clicked() {
			a.runDevDiagnostics()
		}
	})
	devButtonRow(c, func() {
		if btn(c, "Back to workspace", icoUndo, btnText).Clicked() {
			d.diagnosticsOpen = false
		}
		if report != nil {
			ui.Spacer(c)
			if btn(c, "Copy redacted report", icoCopy, btnText).Tooltip("Anonymous resource names. Excludes local paths, domains, commands, environment values, logs, credentials and private keys.").Disabled(d.busy || a.copy == nil).Clicked() {
				a.copy(d.diagnosticJSON)
			}
			if btn(c, "Export redacted report", icoDownload, btnText).Tooltip("Anonymous resource names. Excludes local paths, domains, commands, environment values, logs, credentials and private keys.").Disabled(d.busy).Clicked() {
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
	})
	if d.diagnosticPendingTarget != nil {
		devHint(c, feedback.Warning, "Unsaved database changes. Discard them to open the selected setting?", "")
		devButtonRow(c, func() {
			if btn(c, "Cancel navigation", nil, btnDefault).Clicked() {
				d.diagnosticPendingTarget = nil
			}
			if btn(c, "Discard draft and open", nil, btnDanger).Clicked() {
				target := *d.diagnosticPendingTarget
				d.diagnosticPendingTarget = nil
				a.openDiagnosticTarget(target, true)
			}
		})
	}
	if report == nil {
		return
	}
	devCardWith(c, nil, "Checks", "", nil, func() {
		for _, check := range report.Checks {
			devInset(c, func() {
				ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
					color := display.TagColor("")
					switch check.Status {
					case "Failed":
						color = display.TagError
					case "Warning":
						color = display.TagWarning
					case "OK":
						color = display.TagSuccess
					}
					display.Tag(c, check.Status+" · "+check.Layer, display.TagProps{Color: color, Size: theme.Small})
					ui.Text(c, check.Resource).FontWeight(500).SingleLine().Grow(1).MinWidth(0)
					if check.Target.Kind != "" && btn(c, "Open related setting", icoArrow, btnText).Disabled(d.busy).Clicked() {
						a.openDiagnosticTarget(check.Target, false)
					}
				})
				ui.Text(c, check.Summary).FontSize(textSmall)
				if check.Remedy != "" {
					devSubtle(c, check.Remedy)
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

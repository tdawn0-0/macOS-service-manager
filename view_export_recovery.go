package main

import (
	"context"
	"fmt"
	"strings"
	"time"

	"github.com/egoist/mygo/ui"
	"macos-service-manager/internal/devplatform"
)

func (a *app) inspectDevExport(m *devplatform.Manager, dir string) (bool, error) {
	// Inspection remains available after the operation's context is cancelled.
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	recovery, err := m.InspectExportRecovery(ctx, dir)
	if err == nil {
		a.post(func() { a.dev.exportRecovery = recovery; a.dev.exportRecoveryDirectory = dir })
	}
	return recovery != nil, err
}

func (a *app) devExportRecovery(c *ui.Context, dir string) {
	d := &a.dev
	if ui.Button(c, "Review export recovery").Disabled(d.busy).Clicked() {
		a.devTask(func(m *devplatform.Manager) (string, error) {
			pending, err := a.inspectDevExport(m, dir)
			if pending {
				return "Interrupted export found. Review the saved operation below.", err
			}
			return "No interrupted environment export.", err
		})
	}
	r := d.exportRecovery
	if r == nil || d.exportRecoveryDirectory != dir {
		return
	}
	devDivider(c, "Interrupted environment export")
	ui.Text(c, r.Phase).FontSize(textSmall).FontWeight(600).TextColor(c.Theme().Warning)
	ui.Text(c, fmt.Sprintf("Operation %s · %s", r.OperationID, r.CreatedAt.Local().Format("2006-01-02 15:04"))).FontSize(textSmall).TextColor(c.Theme().TextMuted)
	ui.Text(c, "Prepared snapshot: "+r.ProjectName).FontSize(textSmall).FontWeight(600)
	ui.Text(c, fmt.Sprintf("Services: %d · Databases: %d · Sites: %d", r.Services, r.Databases, r.Sites)).FontSize(textSmall).TextColor(c.Theme().TextMuted)
	if len(r.PackageIDs) > 0 {
		ui.Text(c, "Locked software: "+strings.Join(r.PackageIDs, ", ")).FontSize(textSmall).TextColor(c.Theme().TextMuted)
	}
	for _, file := range r.Files {
		ui.Text(c, file.Name+" · "+file.Status).FontSize(textSmall).TextColor(c.Theme().TextMuted)
	}
	ui.Text(c, "Complete saves the prepared declaration and version lock together. Import stays blocked until this operation is resolved.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
	if !r.CanResume {
		ui.Text(c, "Files changed after export began. They will be kept. Review again, or discard the record and export again from the registered source project.").FontSize(textSmall).TextColor(c.Theme().Warning)
	}
	if ui.Button(c, "Complete interrupted export").Disabled(d.busy || !r.CanResume).Clicked() {
		fingerprint := r.Fingerprint
		a.devTask(func(m *devplatform.Manager) (string, error) {
			err := m.RecoverExport(m.Context, dir, fingerprint)
			_, inspectErr := a.inspectDevExport(m, dir)
			if err != nil {
				return "Export recovery stopped.", err
			}
			return "Environment export completed. You can import or export again.", inspectErr
		})
	}
	ui.Text(c, "Discard removes only the operation record. Both files stay as they are; a mismatched pair may still require a fresh export.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
	if ui.Button(c, "Discard export record (keep files)").Disabled(d.busy).Clicked() {
		fingerprint := r.Fingerprint
		a.devTask(func(m *devplatform.Manager) (string, error) {
			err := m.DiscardExport(m.Context, dir, fingerprint)
			_, inspectErr := a.inspectDevExport(m, dir)
			if err != nil {
				return "Export record was not discarded.", err
			}
			return "Export record discarded. Existing files were kept.", inspectErr
		})
	}
}

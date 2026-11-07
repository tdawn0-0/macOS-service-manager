package main

import (
	"context"
	"fmt"
	"time"

	"github.com/egoist/mygo/ui"
	"macos-service-manager/internal/devplatform"
)

func (a *app) deletionTask(fn func(*devplatform.Manager) error, after func(error)) {
	a.devTaskThen(func(m *devplatform.Manager) (string, error) {
		var actionErr error
		if fn != nil {
			actionErr = fn(m)
		}
		ctx, cancel := context.WithTimeout(context.Background(), 8*time.Second)
		defer cancel()
		records, e := m.PendingInstanceDeletions(ctx)
		if e == nil {
			a.post(func() { a.dev.instanceDeletions = records })
		}
		if actionErr != nil {
			return "Database deletion stopped. Review its current state before continuing.", actionErr
		}
		return "Database deletion status refreshed.", e
	}, after)
}
func (a *app) openInstanceDeletions() {
	a.dev.deletionsOpen = true
	a.dev.instanceDeletionPreview = nil
	a.dev.deleteConfirmation = ""
	a.dev.detailScroll[2].Y = 0
	a.deletionTask(nil, nil)
}
func (a *app) previewInstanceDeletion(id string) {
	a.devTask(func(m *devplatform.Manager) (string, error) {
		p, e := m.PreviewInstanceDeletion(m.Context, id)
		if e == nil {
			a.post(func() {
				if a.dev.dbID == id {
					a.dev.instanceDeletionPreview = &p
					a.dev.deleteConfirmation = ""
					a.dev.detailScroll[2].Y = 0
				}
			})
		}
		return "Review the actual data path and impact before confirming permanent deletion.", e
	})
}
func deletionPath(c *ui.Context, label, path string) {
	ui.Field(c, label, func() { value := path; ui.TextArea(c, &value).ReadOnly(true).Lines(2, 3) })
}
func (a *app) devInstanceDeletionPreview(c *ui.Context) {
	d := &a.dev
	p := d.instanceDeletionPreview
	devHeading(c, "Delete database data", "Permanent deletion is separate from removing a management record. Stop and unlink the instance first.")
	ui.Text(c, p.InstanceID+" · "+softwareTitle(p.Kind)).FontWeight(600)
	deletionPath(c, "Data path to delete", p.Path)
	ui.Text(c, fmt.Sprintf("%d entries · %.2f MiB logical file size", p.Entries, float64(p.Bytes)/(1<<20))).FontSize(textSmall)
	ui.Text(c, "Includes every database in this instance, local configuration and credentials. Source projects, installed software and backups outside this path are retained. This does not create a backup.").FontSize(textSmall)
	color := c.Theme().Warning
	if p.CanDelete {
		color = c.Theme().Danger
	}
	ui.Text(c, p.Detail).FontSize(textSmall).TextColor(color)
	for _, ref := range p.References {
		ui.Text(c, "Linked project: "+ref).FontSize(textSmall)
	}
	if ui.Button(c, "Refresh deletion preview").Disabled(d.busy).Clicked() {
		a.previewInstanceDeletion(p.InstanceID)
	}
	devField(c, "Confirm instance ID", &d.deleteConfirmation, p.InstanceID)
	ui.Text(c, "Type the exact instance ID. Once erasure begins, cancelling can leave a pending deletion; it cannot restore already erased files.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
	if ui.Button(c, "Permanently delete instance data").Disabled(d.busy || !p.CanDelete || d.deleteConfirmation != p.InstanceID).Clicked() {
		id, fingerprint, confirmation := p.InstanceID, p.Fingerprint, d.deleteConfirmation
		a.deletionTask(func(m *devplatform.Manager) error {
			return m.DeleteInstanceData(m.Context, id, fingerprint, confirmation)
		}, func(err error) {
			a.dev.deleteConfirmation = ""
			_, exists := a.dev.state.Instances[id]
			if !exists || len(a.dev.instanceDeletions) > 0 {
				a.dev.instanceDeletionPreview = nil
				a.dev.deletionsOpen = len(a.dev.instanceDeletions) > 0
				a.dev.detailScroll[2].Y = 0
				if !exists {
					a.dev.dbID = ""
				}
			}
		})
	}
	if ui.Button(c, "Cancel deletion preview").Disabled(d.busy).Clicked() {
		d.instanceDeletionPreview = nil
		d.deleteConfirmation = ""
		d.detailScroll[2].Y = 0
	}
}
func (a *app) devInstanceDeletions(c *ui.Context) {
	d := &a.dev
	devHeading(c, "Pending data deletions", "Review interrupted deletions here. No data is erased automatically when the app opens.")
	if ui.Button(c, "Back to database settings").Disabled(d.busy).Clicked() {
		d.deletionsOpen = false
		d.deleteConfirmation = ""
		d.detailScroll[2].Y = 0
	}
	if ui.Button(c, "Refresh pending deletions").Disabled(d.busy).Clicked() {
		a.deletionTask(nil, nil)
	}
	if len(d.instanceDeletions) == 0 {
		ui.Text(c, "No pending data deletions.").FontSize(textSmall)
		return
	}
	for _, r := range d.instanceDeletions {
		devDivider(c, r.InstanceID+" · "+r.Status)
		ui.Text(c, "Deletion "+r.ID).FontSize(textSmall).TextColor(c.Theme().TextMuted)
		ui.Text(c, r.Detail).FontSize(textSmall)
		deletionPath(c, "Original data path "+r.ID, r.OriginalPath)
		if r.Entries > 0 {
			deletionPath(c, "Staged data path "+r.ID, r.StagedPath)
			ui.Text(c, fmt.Sprintf("Remaining: %d entries · %.2f MiB", r.Entries, float64(r.Bytes)/(1<<20))).FontSize(textSmall)
		}
		if r.Busy {
			ui.Text(c, "Another caller still owns this deletion. Wait or cancel it there, then refresh.").FontSize(textSmall).TextColor(c.Theme().Warning)
		}
		prefix := r.ID[:min(8, len(r.ID))]
		if ui.Button(c, "Restore original data "+prefix).Disabled(d.busy || r.Busy || !r.CanRestore).Clicked() {
			a.deletionTask(func(m *devplatform.Manager) error { return m.RestoreInstanceDeletion(m.Context, r.ID, r.Revision) }, nil)
		}
		ui.Text(c, "Before removal commits, restore keeps the original data and clears the pending record. After commit, finish erases the staged data.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
		devField(c, "Confirm deletion of "+r.InstanceID, &d.deleteConfirmation, r.InstanceID)
		if ui.Button(c, "Finish permanent deletion "+prefix).Disabled(d.busy || r.Busy || !r.CanErase || d.deleteConfirmation != r.InstanceID).Clicked() {
			confirmation := d.deleteConfirmation
			a.deletionTask(func(m *devplatform.Manager) error {
				return m.FinishInstanceDeletion(m.Context, r.ID, r.Revision, confirmation)
			}, func(error) { a.dev.deleteConfirmation = "" })
		}
	}
}

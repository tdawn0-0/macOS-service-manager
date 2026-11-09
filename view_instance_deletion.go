package main

import (
	"context"
	"fmt"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/action"
	"github.com/lobehub/lobe-mygo/pkg/theme"
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
	a.dev.detailScroll[devTabDatabases].Y = 0
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
					a.dev.detailScroll[devTabDatabases].Y = 0
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
	devPaneHeader(c, "Delete "+p.InstanceID, func() {
		if action.Button(c, "Refresh deletion preview", action.ButtonProps{Variant: action.Default, Size: theme.Small}).Disabled(d.busy).Clicked() {
			a.previewInstanceDeletion(p.InstanceID)
		}
		if action.Button(c, "Cancel deletion preview", action.ButtonProps{Variant: action.Default, Size: theme.Small}).AlignSelf(ui.Start).Disabled(d.busy).Clicked() {
			d.instanceDeletionPreview = nil
			d.deleteConfirmation = ""
			d.detailScroll[devTabDatabases].Y = 0
		}
	})
	ui.Text(c, softwareTitle(p.Kind)).FontSize(textSmall).TextColor(c.Theme().TextMuted)
	deletionPath(c, "Data path to delete", p.Path)
	ui.Text(c, fmt.Sprintf("%d entries · %.2f MiB logical file size", p.Entries, float64(p.Bytes)/(1<<20))).FontSize(textSmall)
	ui.Text(c, "Covers every database, configuration and credential in this instance. No backup is made; backups elsewhere are kept.").FontSize(textSmall)
	color := c.Theme().Warning
	if p.CanDelete {
		color = c.Theme().Danger
	}
	ui.Text(c, p.Detail).FontSize(textSmall).TextColor(color)
	for _, ref := range p.References {
		ui.Text(c, "Linked project: "+ref).FontSize(textSmall)
	}
	devField(c, "Confirm instance ID", &d.deleteConfirmation, p.InstanceID)
	if action.Button(c, "Permanently delete instance data", action.ButtonProps{Variant: action.Primary, Size: theme.Small}).AlignSelf(ui.Start).Tooltip("Once erasure begins, cancelling can leave a pending deletion. Erased files cannot be restored.").Disabled(d.busy || !p.CanDelete || d.deleteConfirmation != p.InstanceID).Clicked() {
		id, fingerprint, confirmation := p.InstanceID, p.Fingerprint, d.deleteConfirmation
		a.deletionTask(func(m *devplatform.Manager) error {
			return m.DeleteInstanceData(m.Context, id, fingerprint, confirmation)
		}, func(err error) {
			a.dev.deleteConfirmation = ""
			_, exists := a.dev.state.Instances[id]
			if !exists || len(a.dev.instanceDeletions) > 0 {
				a.dev.instanceDeletionPreview = nil
				a.dev.deletionsOpen = len(a.dev.instanceDeletions) > 0
				a.dev.detailScroll[devTabDatabases].Y = 0
				if !exists {
					a.dev.dbID = ""
				}
			}
		})
	}
}
func (a *app) devInstanceDeletions(c *ui.Context) {
	d := &a.dev
	devPaneHeader(c, "Pending data deletions", func() {
		if action.Button(c, "Refresh pending deletions", action.ButtonProps{Variant: action.Default, Size: theme.Small}).Disabled(d.busy).Clicked() {
			a.deletionTask(nil, nil)
		}
		if action.Button(c, "Back to database settings", action.ButtonProps{Variant: action.Default, Size: theme.Small}).Disabled(d.busy).Clicked() {
			d.deletionsOpen = false
			d.deleteConfirmation = ""
			d.detailScroll[devTabDatabases].Y = 0
		}
	})
	if len(d.instanceDeletions) == 0 {
		ui.Text(c, "None").FontSize(textSmall).TextColor(c.Theme().TextMuted)
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
			ui.Text(c, "Another caller owns this deletion.").FontSize(textSmall).TextColor(c.Theme().Warning)
		}
		prefix := r.ID[:min(8, len(r.ID))]
		if action.Button(c, "Restore original data "+prefix, action.ButtonProps{Variant: action.Default, Size: theme.Small}).AlignSelf(ui.Start).Tooltip("Before removal commits, keeps the original data and clears the pending record. After commit, finish erases the staged data.").Disabled(d.busy || r.Busy || !r.CanRestore).Clicked() {
			a.deletionTask(func(m *devplatform.Manager) error { return m.RestoreInstanceDeletion(m.Context, r.ID, r.Revision) }, nil)
		}
		devField(c, "Confirm deletion of "+r.InstanceID, &d.deleteConfirmation, r.InstanceID)
		if action.Button(c, "Finish permanent deletion "+prefix, action.ButtonProps{Variant: action.Primary, Size: theme.Small}).AlignSelf(ui.Start).Disabled(d.busy || r.Busy || !r.CanErase || d.deleteConfirmation != r.InstanceID).Clicked() {
			confirmation := d.deleteConfirmation
			a.deletionTask(func(m *devplatform.Manager) error {
				return m.FinishInstanceDeletion(m.Context, r.ID, r.Revision, confirmation)
			}, func(error) { a.dev.deleteConfirmation = "" })
		}
	}
}

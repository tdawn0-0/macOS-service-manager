package main

import (
	"context"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/action"
	"github.com/lobehub/lobe-mygo/pkg/theme"
	"devbase/internal/devplatform"
)

func (a *app) databaseWorkTask(fn func(*devplatform.Manager) error) {
	a.devTask(func(m *devplatform.Manager) (string, error) {
		var actionErr error
		if fn != nil {
			actionErr = fn(m)
		}
		ctx, cancel := context.WithTimeout(context.Background(), 8*time.Second)
		defer cancel()
		records, e := m.PendingDatabaseWork(ctx)
		if e == nil {
			a.post(func() { a.dev.databaseWork = records })
		}
		if actionErr != nil {
			return "Database task needs review. Files are retained.", actionErr
		}
		return "Database task status refreshed.", e
	})
}
func (a *app) openDatabaseWork() {
	a.dev.databaseWorkOpen = true
	a.dev.initializationsOpen = false
	a.dev.deletionsOpen = false
	a.dev.instanceDeletionPreview = nil
	a.dev.deleteConfirmation = ""
	a.dev.detailScroll[devTabDatabases].Y = 0
	a.databaseWorkTask(nil)
}
func (a *app) devDatabaseWork(c *ui.Context) {
	d := &a.dev
	devPaneHeader(c, "Unfinished database tasks", func() {
		if btn(c, "Refresh database tasks", icoRotate, btnText).Disabled(d.busy).Clicked() {
			a.databaseWorkTask(nil)
		}
		if btn(c, "Back to database settings", icoUndo, btnDefault).Disabled(d.busy).Clicked() {
			d.databaseWorkOpen = false
			d.detailScroll[devTabDatabases].Y = 0
		}
	})
	if len(d.databaseWork) == 0 {
		devEmpty(c, "None")
		return
	}
	for _, r := range d.databaseWork {
		devDivider(c, r.InstanceID+" · "+r.Kind)
		color := c.Theme().Warning
		if r.Status == "Succeeded" {
			color = c.Theme().Success
		}
		ui.Text(c, r.Status).FontWeight(600).TextColor(color)
		ui.Text(c, r.Detail).FontSize(textSmall)
		ui.Text(c, "Task "+r.ID+" · Target "+r.Database).FontSize(textSmall).TextColor(c.Theme().TextMuted)
		if r.Output != "" {
			deletionPath(c, "Output "+r.ID, r.Output)
		}
		prefix := r.ID[:min(8, len(r.ID))]
		label := "Finish database task " + prefix
		if r.Running {
			label = "Stop database task " + prefix
		}
		if action.Button(c, label, action.ButtonProps{Variant: action.Primary, Size: theme.Small}).AlignSelf(ui.Start).Tooltip("Does not undo changes already written. Database files and backup output are kept.").Disabled(d.busy || !r.CanResolve).Clicked() {
			a.databaseWorkTask(func(m *devplatform.Manager) error { return m.ResolveDatabaseWork(m.Context, r.ID, r.Revision) })
		}
	}
}

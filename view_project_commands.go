package main

import (
	"context"
	"github.com/egoist/mygo/ui"
	"macos-service-manager/internal/devplatform"
	"time"
)

func (a *app) projectCommandTask(fn func(*devplatform.Manager) error) {
	a.devTask(func(m *devplatform.Manager) (string, error) {
		var actionErr error
		if fn != nil {
			actionErr = fn(m)
		}
		ctx, cancel := context.WithTimeout(context.Background(), 8*time.Second)
		defer cancel()
		rows, e := m.PendingProjectCommands(ctx)
		if e == nil {
			a.post(func() { a.dev.commandWork = rows })
		}
		if actionErr != nil {
			return "Project command needs review. Source and dependency files are retained.", actionErr
		}
		return "Project command status refreshed.", e
	})
}
func (a *app) openProjectCommands() {
	a.dev.commandWorkOpen = true
	a.dev.detailScroll[0].Y = 0
	a.projectCommandTask(nil)
}
func (a *app) devProjectCommands(c *ui.Context) {
	d := &a.dev
	devPaneHeader(c, "Unfinished project commands", func() {
		if ui.Button(c, "Refresh project commands").Disabled(d.busy).Clicked() {
			a.projectCommandTask(nil)
		}
		if ui.Button(c, "Back to project settings").Disabled(d.busy).Clicked() {
			d.commandWorkOpen = false
			d.detailScroll[0].Y = 0
		}
	})
	if len(d.commandWork) == 0 {
		ui.Text(c, "None").FontSize(textSmall).TextColor(c.Theme().TextMuted)
		return
	}
	for _, r := range d.commandWork {
		devDivider(c, r.ProjectID)
		color := c.Theme().Warning
		if r.Status == "Succeeded" {
			color = c.Theme().Success
		}
		ui.Text(c, r.Status).FontWeight(600).TextColor(color)
		ui.Text(c, r.Detail).FontSize(textSmall)
		ui.Text(c, "Task "+r.ID).FontSize(textSmall).TextColor(c.Theme().TextMuted)
		prefix := r.ID[:min(8, len(r.ID))]
		label := "Finish project command " + prefix
		if r.Running {
			label = "Stop project command " + prefix
		}
		if ui.Button(c, label).AlignSelf(ui.Start).Tooltip("Clears the private task files. Source, dependencies and files the command wrote are kept; nothing is undone.").Disabled(d.busy || !r.CanResolve).Clicked() {
			a.projectCommandTask(func(m *devplatform.Manager) error { return m.ResolveProjectCommand(m.Context, r.ID, r.Revision) })
		}
		if ui.Button(c, "Reveal private command files "+prefix).AlignSelf(ui.Start).Tooltip("Inputs and output may contain secrets.").Disabled(d.busy || a.reveal == nil).Clicked() {
			a.devTask(func(m *devplatform.Manager) (string, error) {
				path, e := m.ProjectCommandFiles(r.ID)
				if e == nil {
					a.post(func() { a.reveal(path) })
				}
				return "Private task folder revealed. Treat its contents as sensitive.", e
			})
		}
	}
}

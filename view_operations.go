package main

import (
	"path/filepath"
	"strconv"
	"strings"

	"github.com/egoist/mygo/ui"
	"macos-service-manager/internal/devplatform"
)

func (a *app) openOperations() {
	a.dev.operationsOpen = true
	a.dev.diagnosticsOpen = false
	a.devTask(func(m *devplatform.Manager) (string, error) { return "Task history refreshed.", nil })
}
func (a *app) reviewOperation(r devplatform.OperationRecord) {
	d := &a.dev
	d.commandWorkOpen = false
	req := r.Request
	switch req.Kind {
	case "reconcile-project-command":
		d.tab = 0
		a.openProjectCommands()
	case "exec":
		d.tab = 0
		if p, ok := d.state.Projects[req.Resource]; ok {
			d.selectProject(p)
		}
		if r.Status != "Succeeded" {
			a.openProjectCommands()
		}
	case "install":
		d.tab = 1
		d.software = req.Name
		d.version = req.Version
		d.packagePreview = nil
	case "repair":
		d.tab = 1
		if p, ok := d.state.Packages[req.Resource]; ok {
			d.software = p.Name
			d.version = p.Version
		}
	case "delete-instance", "finish-deletion", "restore-deletion":
		d.tab = 2
		a.openInstanceDeletions()
	case "reconcile-database-work":
		d.tab = 2
		a.openDatabaseWork()
	case "reconcile-initialization":
		d.tab = 2
		a.openInitializations()
	case "initialize":
		d.tab = 2
		d.dbNewDraft = true
		d.dbID = ""
		d.dbKind = req.Name
		d.dbPackage = req.Version
		d.dbPort = ""
		d.initializationsOpen = true
		d.detailScroll[2].Y = 0
		a.initializationTaskThen(nil, func(err error) {
			if err == nil && len(a.dev.initializations) == 0 {
				a.dev.initializationsOpen = false
			}
		})
	case "up", "down", "restart", "remove-project":
		d.tab = 0
		if p, ok := d.state.Projects[req.Resource]; ok && d.id != req.Resource {
			d.selectProject(p)
		}
	default:
		d.tab = 2
		d.dbID = req.Resource
		if i, ok := d.state.Instances[req.Resource]; ok {
			d.dbNewDraft = false
			d.dbKind = i.Kind
			d.dbPackage = i.Package
			d.dbPort = strconv.Itoa(i.Port)
		}
		d.dbName = req.Database
		if r.Status == "Interrupted" && (req.Kind == "backup" || req.Kind == "restore" || req.Kind == "create-user" || req.Kind == "create-database") {
			a.openDatabaseWork()
		}
		if req.Kind == "restore" {
			d.dbName = ""
			d.backupPath = req.SourcePath
			d.output = "Choose a new restore target. A partial target is retained and is never overwritten by retry."
		}
	}
	d.operationsOpen = false
	d.detailScroll[d.tab].Y = 0
}
func (a *app) devOperations(c *ui.Context) {
	d := &a.dev
	ui.Row(c).Grow(1).MinHeight(0).AlignItems(ui.Stretch).Children(func() {
		if d.operationID == "" && len(d.operations) > 0 {
			d.operationID = d.operations[0].ID
		}
		ui.Column(c).Width(224).Shrink(0).Background(c.Theme().Surface).Children(func() {
			ui.Scroll(c.Key("operations-list")).Grow(1).MinHeight(0).Padding(10).Children(func() {
				ui.Column(c).Gap(4).Children(func() {
					for _, r := range d.operations {
						trailing := r.Status
						if r.Status == "Succeeded" || r.Status == "Running" {
							trailing = ""
						}
						if devListEntry(c, "Task "+r.ID, r.Request.Kind+" · "+r.Request.Resource, r.Status, trailing, d.operationID == r.ID) {
							d.operationID = r.ID
							d.operationScroll.Y = 0
						}
					}
				})
			})
		})
		ui.Column(c).Width(1).Background(hairline(c.Theme()))
		ui.Scroll(c.Key("operation-detail")).TrackScroll(&d.operationScroll).Grow(1).MinWidth(0).MinHeight(0).Padding(20, 24).Children(func() {
			ui.Column(c).Gap(10).Children(func() {
				if ui.Button(c, "Back to workspace").Clicked() {
					d.operationsOpen = false
				}
				if d.operationsError != "" {
					ui.Text(c, d.operationsError).FontSize(textSmall).TextColor(c.Theme().Warning)
				}
				var selected *devplatform.OperationRecord
				for n := range d.operations {
					if d.operations[n].ID == d.operationID {
						selected = &d.operations[n]
						break
					}
				}
				if selected == nil {
					ui.Text(c, "No tasks").FontSize(textSmall).TextColor(c.Theme().TextMuted)
					return
				}
				r := *selected
				color := c.Theme().Warning
				if r.Status == "Succeeded" {
					color = c.Theme().Success
				}
				if r.Status == "Running" {
					color = c.Theme().Accent
				}
				ui.Text(c, r.Status+" · "+r.Phase).FontWeight(600).TextColor(color)
				ui.Text(c, "Operation "+r.ID).FontSize(textSmall)
				ui.Text(c, "Started "+r.StartedAt.Local().Format("2006-01-02 15:04:05")+"\nUpdated "+r.UpdatedAt.Local().Format("2006-01-02 15:04:05")).FontSize(textSmall).TextColor(c.Theme().TextMuted)
				ui.Text(c, r.Result).FontSize(textSmall)
				if r.Busy {
					ui.Text(c, "Another caller owns this task.").FontSize(textSmall)
				}
				if r.Followup != "" {
					ui.Text(c, "Follow-up operation: "+r.Followup).FontSize(textSmall)
				}
				if r.RetryOf != "" {
					ui.Text(c, "Retry of: "+r.RetryOf).FontSize(textSmall)
				}
				if r.Request.Database != "" {
					ui.Text(c, "Database: "+r.Request.Database).FontSize(textSmall)
				}
				if r.Artifact != "" && r.Request.Kind == "backup" {
					if strings.HasSuffix(r.Artifact, ".dump") || strings.HasSuffix(r.Artifact, ".partial") {
						ui.Text(c, "Output: "+filepath.Base(r.Artifact)).FontSize(textSmall)
					}
					if r.Status != "Succeeded" {
						ui.Text(c, "This output may be incomplete. Do not use it as a verified backup.").FontSize(textSmall).TextColor(c.Theme().Warning)
					}
				}
				if ui.Button(c, "Retry operation").AlignSelf(ui.Start).Tooltip("Rechecks saved settings and live launchd jobs. Ownership is never assumed from a historical PID.").Disabled(d.busy || !devplatform.OperationCanRetry(r)).Clicked() {
					a.devTask(func(m *devplatform.Manager) (string, error) {
						return "Follow-up task finished. Review its result.", m.RetryOperation(m.Context, r.ID, r.Revision)
					})
				}
				if ui.Button(c, "Review resource settings").AlignSelf(ui.Start).Tooltip("Initialization, restore and commands are never replayed. Existing data is kept.").Disabled(d.busy || r.Busy).Clicked() {
					a.reviewOperation(r)
				}
				if ui.Button(c, "Reveal task files").AlignSelf(ui.Start).Disabled(d.busy || a.reveal == nil).Clicked() {
					a.devTask(func(m *devplatform.Manager) (string, error) {
						path := filepath.Join(m.Root, "operations", r.ID)
						a.post(func() { a.reveal(path) })
						return "Task folder revealed.", nil
					})
				}
				if ui.Button(c, "Dismiss record (keep all files)").AlignSelf(ui.Start).Disabled(d.busy || r.Busy).Clicked() {
					a.devTask(func(m *devplatform.Manager) (string, error) {
						return "Task record dismissed; resource and output files retained.", m.DismissOperation(m.Context, r.ID, r.Revision)
					})
				}
			})
		})
	})
}

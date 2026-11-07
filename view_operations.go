package main

import (
	"fmt"
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
		ui.Column(c).Width(238).Shrink(0).Background(c.Theme().Surface).Children(func() {
			ui.Scroll(c.Key("operations-list")).Grow(1).MinHeight(0).Padding(12).Children(func() {
				ui.Column(c).Gap(8).Children(func() {
					ui.Text(c, fmt.Sprintf("%d LOCAL TASKS", len(d.operations))).FontSize(textCaption).TextColor(c.Theme().TextMuted)
					for _, r := range d.operations {
						if devListEntry(c, "Task "+r.ID, r.Request.Kind+" · "+r.Request.Resource, r.Status+" · "+r.Phase, d.operationID == r.ID) {
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
				devHeading(c, "Task history", "App and CLI operations share this private history. Interrupted work is never reported as successful.")
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
					ui.Text(c, "Choose a task to inspect its phase, result and recovery options.").FontSize(textSmall)
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
					ui.Text(c, "A caller still owns this operation. Cancel it in that caller or wait, then refresh.").FontSize(textSmall)
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
					} else {
						ui.Text(c, "Review unfinished database tasks for this worker’s current output and result.").FontSize(textSmall)
					}
					if r.Status != "Succeeded" {
						ui.Text(c, "This output may be incomplete. Do not use it as a verified backup.").FontSize(textSmall).TextColor(c.Theme().Warning)
					}
				}
				ui.Text(c, "Retry rechecks current saved settings and live launchd jobs. It does not assume ownership from a historical PID.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
				if ui.Button(c, "Retry operation").Disabled(d.busy || !devplatform.OperationCanRetry(r)).Clicked() {
					a.devTask(func(m *devplatform.Manager) (string, error) {
						return "Follow-up task finished. Review its result.", m.RetryOperation(m.Context, r.ID, r.Revision)
					})
				}
				if ui.Button(c, "Review resource settings").Disabled(d.busy || r.Busy).Clicked() {
					a.reviewOperation(r)
				}
				if r.Request.Kind == "exec" {
					ui.Text(c, "Command inputs and output stay in private task files while pending; they are excluded from history and never replayed. Review unfinished project commands after a caller exits.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
				}
				if r.Request.Kind == "initialize" || r.Request.Kind == "restore" {
					ui.Text(c, "Initialization and restore require review and a new target. Existing data is retained; these operations are never replayed automatically.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
				}
				if ui.Button(c, "Reveal task files").Disabled(d.busy || a.reveal == nil).Clicked() {
					a.devTask(func(m *devplatform.Manager) (string, error) {
						path := filepath.Join(m.Root, "operations", r.ID)
						a.post(func() { a.reveal(path) })
						return "Task folder revealed.", nil
					})
				}
				if ui.Button(c, "Dismiss record (keep all files)").Disabled(d.busy || r.Busy).Clicked() {
					a.devTask(func(m *devplatform.Manager) (string, error) {
						return "Task record dismissed; resource and output files retained.", m.DismissOperation(m.Context, r.ID, r.Revision)
					})
				}
				ui.Text(c, strings.Join([]string{"Dismissing removes only this history record.", "Package, instance, backup and project files are kept."}, " ")).FontSize(textSmall).TextColor(c.Theme().TextMuted)
			})
		})
	})
}

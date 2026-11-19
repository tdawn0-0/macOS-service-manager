package main

import (
	"path/filepath"
	"strconv"
	"strings"

	"devbase/internal/devplatform"
	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/display"
	"github.com/lobehub/lobe-mygo/pkg/feedback"
	"github.com/lobehub/lobe-mygo/pkg/theme"
)

func (a *app) openOperations() {
	a.dev.operationsOpen = true
	a.dev.diagnosticsOpen = false
	a.devTask(func(m *devplatform.Manager) (string, error) { return "Task history refreshed.", nil })
}
func (a *app) reviewOperation(r devplatform.OperationRecord) {
	d := &a.dev
	req := r.Request
	switch req.Kind {
	case "install":
		d.tab = devTabSoftware
		d.software = req.Name
		d.version = req.Version
		d.packagePreview = nil
	case "repair":
		d.tab = devTabSoftware
		if p, ok := d.state.Packages[req.Resource]; ok {
			d.software = p.Name
			d.version = p.Version
		}
	case "delete-instance", "finish-deletion", "restore-deletion":
		d.tab = devTabDatabases
		a.openInstanceDeletions()
	case "reconcile-database-work":
		d.tab = devTabDatabases
		a.openDatabaseWork()
	case "reconcile-initialization":
		d.tab = devTabDatabases
		a.openInitializations()
	case "initialize":
		d.tab = devTabDatabases
		d.dbNewDraft = true
		d.dbID = ""
		d.dbKind = req.Name
		d.dbPackage = req.Version
		d.dbPort = ""
		d.dbDataDirectory = ""
		d.initializationsOpen = true
		d.detailScroll[devTabDatabases].Y = 0
		a.initializationTaskThen(nil, func(err error) {
			if err == nil && len(a.dev.initializations) == 0 {
				a.dev.initializationsOpen = false
			}
		})
	default:
		d.tab = devTabDatabases
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
	tk := theme.TokensOf(c)
	ui.Row(c).Grow(1).MinHeight(0).AlignItems(ui.Stretch).Children(func() {
		if d.operationID == "" && len(d.operations) > 0 {
			d.operationID = d.operations[0].ID
		}
		ui.Column(c).Width(228).Shrink(0).Children(func() {
			ui.Scroll(c.Key("operations-list")).Grow(1).MinHeight(0).Padding(4, 8, 12).Children(func() {
				ui.Column(c).Gap(2).Children(func() {
					masterGroup(c, "Tasks", nil)
					if len(d.operations) == 0 {
						ui.Text(c, "No tasks").FontSize(textSmall).TextColor(tk.TextTertiary).Padding(6, 10)
					}
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
		ui.Scroll(c.Key("operation-detail")).TrackScroll(&d.operationScroll).Grow(1).MinWidth(0).MinHeight(0).Padding(22, 28, 28).Background(glassOf(c.Theme()).sheet).Children(func() {
			ui.Column(c).Gap(14).MaxWidth(880).Children(func() {
				if btn(c, "Back to workspace", icoUndo, btnText).AlignSelf(ui.Start).Clicked() {
					d.operationsOpen = false
				}
				if d.operationsError != "" {
					devHint(c, feedback.Warning, d.operationsError, "")
				}
				var selected *devplatform.OperationRecord
				for n := range d.operations {
					if d.operations[n].ID == d.operationID {
						selected = &d.operations[n]
						break
					}
				}
				if selected == nil {
					display.Empty(c, display.EmptyProps{Icon: icoHistory, Title: "No tasks"})
					return
				}
				r := *selected
				pageHeader(c, icoHistory, tk.TextSecondary, r.Request.Kind+" · "+r.Request.Resource, "Operation "+r.ID, func() {
					statusTag(c, r.Status)
				}, nil)
				devCardWith(c, nil, r.Status+" · "+r.Phase, "", nil, func() {
					devKV(c, "Started", r.StartedAt.Local().Format("2006-01-02 15:04:05"))
					devKV(c, "Updated", r.UpdatedAt.Local().Format("2006-01-02 15:04:05"))
					if r.Request.Database != "" {
						devKV(c, "Database", r.Request.Database)
					}
					if r.Followup != "" {
						devKV(c, "Follow-up", r.Followup)
					}
					if r.RetryOf != "" {
						devKV(c, "Retry of", r.RetryOf)
					}
					if r.Artifact != "" && r.Request.Kind == "backup" && (strings.HasSuffix(r.Artifact, ".dump") || strings.HasSuffix(r.Artifact, ".partial")) {
						devKV(c, "Output", filepath.Base(r.Artifact))
					}
					if r.Result != "" {
						ui.Text(c, r.Result).FontSize(textSmall).Selectable()
					}
					if r.Busy {
						devHint(c, feedback.Info, "Another caller owns this task.", "")
					}
					if r.Artifact != "" && r.Request.Kind == "backup" && r.Status != "Succeeded" {
						devHint(c, feedback.Warning, "This output may be incomplete. Do not use it as a verified backup.", "")
					}
				})
				devButtonRow(c, func() {
					if btn(c, "Retry operation", icoRotate, btnPrimary).Tooltip("Rechecks saved settings and live launchd jobs. Ownership is never assumed from a historical PID.").Disabled(d.busy || !devplatform.OperationCanRetry(r)).Clicked() {
						a.devTask(func(m *devplatform.Manager) (string, error) {
							return "Follow-up task finished. Review its result.", m.RetryOperation(m.Context, r.ID, r.Revision)
						})
					}
					if btn(c, "Review resource settings", icoArrow, btnDefault).Tooltip("Initialization, restore and commands are never replayed. Existing data is kept.").Disabled(d.busy || r.Busy).Clicked() {
						a.reviewOperation(r)
					}
					if btn(c, "Reveal task files", icoFolder, btnText).Disabled(d.busy || a.reveal == nil).Clicked() {
						a.devTask(func(m *devplatform.Manager) (string, error) {
							path := filepath.Join(m.Root, "operations", r.ID)
							a.post(func() { a.reveal(path) })
							return "Task folder revealed.", nil
						})
					}
					if btn(c, "Dismiss record (keep all files)", nil, btnText).Disabled(d.busy || r.Busy).Clicked() {
						a.devTask(func(m *devplatform.Manager) (string, error) {
							return "Task record dismissed; resource and output files retained.", m.DismissOperation(m.Context, r.ID, r.Revision)
						})
					}
				})
			})
		})
	})
}

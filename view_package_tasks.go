package main

import (
	"context"
	"errors"
	"fmt"
	"strconv"
	"strings"

	"devbase/internal/devplatform"
	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/action"
	"github.com/lobehub/lobe-mygo/pkg/theme"
)

type packageTask struct {
	afterSync                           func(error) string
	err                                 error
	syncPending                         bool
	Package                             devplatform.Package
	Kind, Progress, Result, OperationID string
	Running                             bool
	cancel                              context.CancelFunc
}

func (a *app) packageTasksRunning() bool {
	for _, task := range a.dev.packageTasks {
		if task.Running {
			return true
		}
	}
	return false
}
func (a *app) packageTaskRunning(id string) bool {
	task := a.dev.packageTasks[id]
	return task != nil && task.Running
}
func (a *app) cancelPackageTasks() {
	for _, task := range a.dev.packageTasks {
		if task.Running && task.cancel != nil {
			task.cancel()
		}
	}
}
func (a *app) startPackageTask(pkg devplatform.Package, kind string, work func(*devplatform.Manager) (string, error), after func(error) string) {
	if a.dev.busy || a.packageTaskRunning(pkg.ID()) || a.quit.open || a.quit.allow {
		return
	}
	if a.dev.packageTasks == nil {
		a.dev.packageTasks = map[string]*packageTask{}
	}
	ctx, cancel := context.WithCancel(context.Background())
	task := &packageTask{Package: pkg, Kind: kind, Progress: "Preparing " + pkg.ID(), Running: true, cancel: cancel, afterSync: after}
	a.dev.packageTasks[pkg.ID()] = task
	if r := a.dev.packageContention; r != nil && r.PackageID == pkg.ID() {
		a.dev.packageContention = nil
	}
	factory := a.dev.managerFactory
	a.spawn(func() {
		defer cancel()
		m := devplatform.New(devplatform.DefaultRoot())
		if factory != nil {
			m = factory()
		}
		m.Context = ctx
		observer := m.Event
		m.Event = func(message string) {
			if observer != nil {
				observer(message)
			}
			a.post(func() {
				if a.dev.packageTasks[pkg.ID()] != task || !task.Running {
					return
				}
				task.Progress = message
				if strings.HasPrefix(message, "Operation ") {
					fields := strings.Fields(message)
					if len(fields) > 1 {
						task.OperationID = fields[1]
					}
				}
			})
		}
		result, err := work(m)
		a.post(func() {
			if a.dev.packageTasks[pkg.ID()] != task {
				return
			}
			task.Running = false
			task.err = err
			task.syncPending = true
			task.cancel = nil
			task.Result = result
			if err != nil {
				task.Result += "\nError: " + err.Error()
			}
			var contention *devplatform.PackageBusyError
			if errors.As(err, &contention) {
				a.dev.packageContention = contention
			}
			a.dev.packageNeedsRefresh = true
			a.dev.lastPackageTask = task
			a.refreshFinishedPackages()
		})
	})
}
func (a *app) refreshFinishedPackages() {
	d := &a.dev
	if (!d.packageNeedsRefresh && !d.databaseNeedsRefresh) || d.busy {
		return
	}
	d.packageNeedsRefresh = false
	d.databaseNeedsRefresh = false
	last, lastDatabase, previousOutput := d.lastPackageTask, d.lastDatabaseTask, d.output
	var databases []*databaseTask
	for _, task := range d.databaseTasks {
		if !task.Running && task.syncPending {
			databases = append(databases, task)
			task.syncPending = false
		}
	}
	var completed []*packageTask
	for _, task := range d.packageTasks {
		if !task.Running && task.syncPending {
			completed = append(completed, task)
			task.syncPending = false
		}
	}
	a.devTaskKeepingDraft(func(m *devplatform.Manager) (string, error) { return "Software task status refreshed.", nil }, func(err error) {
		if err != nil {
			return
		}
		for _, task := range completed {
			if a.dev.packageTasks[task.Package.ID()] == task && task.afterSync != nil {
				task.Result += task.afterSync(task.err)
			}
		}
		for _, task := range databases {
			if a.dev.databaseTasks[task.Instance.ID] != task || task.err != nil || task.Kind != "" && task.Kind != "initialize" {
				continue
			}
			i := task.Instance
			if a.dev.dbNewDraft && a.dev.dbID == i.ID && a.dev.dbKind == i.Kind && a.dev.dbPackage == i.Package && a.dev.dbPort == strconv.Itoa(i.Port) && a.dev.persistent == i.Persistent {
				a.dev.dbNewDraft = false
			}
		}
		if a.dev.tab == devTabDatabases && lastDatabase != nil && a.dev.databaseTasks[lastDatabase.Instance.ID] == lastDatabase && a.dev.dbID == lastDatabase.Instance.ID {
			a.dev.output = lastDatabase.Result
		} else if last != nil && a.dev.packageTasks[last.Package.ID()] == last && a.dev.tab == devTabSoftware && a.dev.software == last.Package.Name && strings.TrimPrefix(strings.TrimSpace(a.dev.version), "v") == last.Package.Version {
			a.dev.output = last.Result
		} else {
			a.dev.output = previousOutput
		}
	}, true)
}
func (a *app) showPackageTasks() {
	d := &a.dev
	d.tab = devTabSoftware
	d.diagnosticsOpen = false
	d.operationsOpen = false
	d.detailScroll[devTabSoftware].Y = 0
	if last := d.lastPackageTask; last != nil && d.packageTasks[last.Package.ID()] == last {
		d.software = last.Package.Name
		d.version = last.Package.Version
	}
	for _, id := range sortedKeys(d.packageTasks) {
		task := d.packageTasks[id]
		if task.Running {
			d.software = task.Package.Name
			d.version = task.Package.Version
			break
		}
	}
}
func (a *app) devPackageTasks(c *ui.Context) {
	d := &a.dev
	for _, id := range sortedKeys(d.packageTasks) {
		task := d.packageTasks[id]
		if task.Package.Name != d.software {
			continue
		}
		devDivider(c, strings.ToUpper(task.Kind[:1])+task.Kind[1:]+" · "+task.Package.Version)
		if task.Running {
			ui.Text(c, task.Progress).FontSize(textSmall).Selectable()
			if action.Button(c, "Cancel package "+id, action.ButtonProps{Variant: action.Default, Size: theme.Small}).AlignSelf(ui.Start).Clicked() && task.cancel != nil {
				task.cancel()
			}
		} else {
			ui.Text(c, task.Result).FontSize(textSmall)
			if action.Button(c, "Dismiss package result "+id, action.ButtonProps{Variant: action.Default, Size: theme.Small}).AlignSelf(ui.Start).Clicked() {
				delete(d.packageTasks, id)
			}
		}
	}
	if r := d.packageContention; r != nil && strings.HasPrefix(r.PackageID, d.software+"@") {
		ui.Text(c, fmt.Sprintf("%s is busy in another caller. Finish or cancel that task there, then retry.", r.PackageID)).FontSize(textSmall).TextColor(c.Theme().Warning)
		if action.Button(c, "Review package tasks", action.ButtonProps{Variant: action.Primary, Size: theme.Small}).AlignSelf(ui.Start).Disabled(d.busy).Clicked() {
			a.openOperations()
		}
	}
}

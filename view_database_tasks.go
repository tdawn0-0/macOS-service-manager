package main

import (
	"context"
	"errors"
	"strconv"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/action"
	"github.com/lobehub/lobe-mygo/pkg/theme"
	"macos-service-manager/internal/devplatform"
)

type databaseTask struct {
	Instance               devplatform.Instance
	Kind, Progress, Result string
	Running, syncPending   bool
	err                    error
	cancel                 context.CancelFunc
}

func (a *app) databaseTaskRunning(id string) bool {
	task := a.dev.databaseTasks[id]
	return task != nil && task.Running
}

func (a *app) applyDatabaseTaskPhases() {
	for id, task := range a.dev.databaseTasks {
		if !task.Running {
			continue
		}
		phase := ""
		switch task.Kind {
		case "start":
			phase = "Starting"
		case "stop", "remove":
			phase = "Stopping"
		case "restart":
			phase = "Restarting"
		}
		if phase != "" {
			if a.dev.statuses == nil {
				a.dev.statuses = map[string]string{}
			}
			a.dev.statuses["instance:"+id] = phase
		}
	}
}

func (a *app) databaseTasksRunning() bool {
	for _, task := range a.dev.databaseTasks {
		if task.Running {
			return true
		}
	}
	return false
}
func (a *app) backgroundTasksRunning() bool {
	return a.packageTasksRunning() || a.databaseTasksRunning()
}
func (a *app) cancelBackgroundTasks() {
	a.cancelPackageTasks()
	for _, task := range a.dev.databaseTasks {
		if task.Running && task.cancel != nil {
			task.cancel()
		}
	}
}
func (a *app) startDatabaseTask(i devplatform.Instance) {
	a.startDatabaseOperation(i, "initialize", func(m *devplatform.Manager) (string, error) {
		return "Created " + i.ID, m.CreateInstance(m.Context, i)
	})
}

func (a *app) startDatabaseOperation(i devplatform.Instance, kind string, work func(*devplatform.Manager) (string, error)) {
	a.startDatabaseOperationWithOffer(i, kind, work, nil)
}

type connectionOperationOffer struct {
	InstanceID, Kind, Database, Username, MongoAuthSource string
	OperationID                                           uint64
}

func (a *app) startDatabaseOperationWithOffer(i devplatform.Instance, kind string, work func(*devplatform.Manager) (string, error), offer *connectionOperationOffer) {
	d := &a.dev
	if d.busy || a.quit.open || a.quit.allow {
		return
	}
	if task := d.databaseTasks[i.ID]; task != nil && task.Running {
		return
	}
	if d.databaseTasks == nil {
		d.databaseTasks = map[string]*databaseTask{}
	}
	ctx, cancel := context.WithCancel(context.Background())
	task := &databaseTask{Instance: i, Kind: kind, Progress: "Preparing " + i.ID, Running: true, cancel: cancel}
	d.databaseTasks[i.ID] = task
	d.taskSequence++
	if offer != nil {
		copy := *offer
		copy.OperationID = d.taskSequence
		offer = &copy
		d.profileOffer = nil
	}
	a.applyDatabaseTaskPhases()
	factory := d.managerFactory
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
				if a.dev.databaseTasks[i.ID] == task && task.Running {
					task.Progress = message
				}
			})
		}
		result, err := work(m)
		completed := err == nil && ctx.Err() == nil
		var inspection *devplatform.PortInspection
		var portError *devplatform.PortUnavailableError
		if errors.As(err, &portError) {
			if observed, inspectErr := m.InspectPort(ctx, portError.Port); inspectErr == nil {
				inspection = &observed
			}
		}
		a.post(func() {
			if a.dev.databaseTasks[i.ID] != task {
				return
			}
			if inspection != nil {
				a.storePortInspection(*inspection)
			}
			task.Running = false
			a.dev.taskSequence++
			task.cancel = nil
			task.syncPending = true
			task.err = err
			task.Result = result
			if completed && offer != nil {
				if current, exists := a.dev.state.Instances[offer.InstanceID]; exists && current.Kind == offer.Kind {
					copy := *offer
					a.dev.profileOffer = &copy
				}
			}
			if err != nil {
				label := "Database operation"
				if kind == "initialize" {
					label = "Initialization"
				}
				task.Result = label + " did not complete.\nError: " + err.Error()
			}
			a.dev.databaseNeedsRefresh = true
			a.dev.lastDatabaseTask = task
			a.refreshFinishedPackages()
		})
	})
}
func (a *app) showDatabaseTasks() {
	d := &a.dev
	d.tab = devTabDatabases
	d.diagnosticsOpen = false
	d.operationsOpen = false
	d.databaseWorkOpen = false
	d.initializationsOpen = false
	d.instanceDeletionPreview = nil
	d.deletionsOpen = false
	d.detailScroll[devTabDatabases].Y = 0
}
func (a *app) devDatabaseTasks(c *ui.Context) {
	d := &a.dev
	for _, id := range sortedKeys(d.databaseTasks) {
		task := d.databaseTasks[id]
		label, cancelLabel, dismissLabel := "Initialization", "Cancel initialization ", "Dismiss initialization result "
		if task.Kind != "" && task.Kind != "initialize" {
			label, cancelLabel, dismissLabel = "Database · "+task.Kind, "Cancel database operation ", "Dismiss database result "
		}
		devDivider(c, label+" · "+id)
		if task.Running {
			ui.Text(c, task.Progress).FontSize(textSmall)
			if action.Button(c, cancelLabel+id, action.ButtonProps{Variant: action.Default, Size: theme.Small}).AlignSelf(ui.Start).Tooltip("Canceling does not undo data already written. Review the target before retrying.").Clicked() && task.cancel != nil {
				task.cancel()
			}
		} else {
			ui.Text(c, task.Result).FontSize(textSmall)
			if action.Button(c, dismissLabel+id, action.ButtonProps{Variant: action.Default, Size: theme.Small}).AlignSelf(ui.Start).Clicked() {
				delete(d.databaseTasks, id)
			}
		}
	}
}
func (a *app) initializationTask(fn func(*devplatform.Manager) (string, error)) {
	a.initializationTaskThen(fn, nil)
}
func (a *app) initializationTaskThen(fn func(*devplatform.Manager) (string, error), after func(error)) {
	a.devTaskThen(func(m *devplatform.Manager) (string, error) {
		out := "Initialization status refreshed."
		var actionErr error
		if fn != nil {
			out, actionErr = fn(m)
		}
		ctx, cancel := context.WithTimeout(context.Background(), 8*time.Second)
		defer cancel()
		records, e := m.PendingInstanceInitializations(ctx)
		if e == nil {
			a.post(func() { a.dev.initializations = records })
		}
		if actionErr != nil {
			return out, actionErr
		}
		return out, e
	}, after)
}
func (a *app) openInitializations() {
	a.dev.initializationsOpen = true
	a.dev.detailScroll[devTabDatabases].Y = 0
	a.initializationTask(nil)
}
func (a *app) devInitializations(c *ui.Context) {
	d := &a.dev
	devPaneHeader(c, "Unfinished initializations", func() {
		if action.Button(c, "Refresh initializations", action.ButtonProps{Variant: action.Default, Size: theme.Small}).Disabled(d.busy).Clicked() {
			a.initializationTask(nil)
		}
		if action.Button(c, "Back to database settings", action.ButtonProps{Variant: action.Default, Size: theme.Small}).Disabled(d.busy).Clicked() {
			d.databaseWorkOpen = false
			d.initializationsOpen = false
			d.detailScroll[devTabDatabases].Y = 0
		}
	})
	if len(d.initializations) == 0 {
		ui.Text(c, "None").FontSize(textSmall).TextColor(c.Theme().TextMuted)
		return
	}
	for _, r := range d.initializations {
		devDivider(c, r.InstanceID+" · "+r.Status)
		ui.Text(c, softwareTitle(r.Kind)+" · port "+strconv.Itoa(r.Port)).FontSize(textSmall)
		ui.Text(c, r.Detail).FontSize(textSmall)
		deletionPath(c, "Prepared data "+r.ID, r.Path)
		label := "Stop and archive data " + r.ID[:min(8, len(r.ID))]
		if r.Status == "Published" {
			label = "Finish registration " + r.ID[:min(8, len(r.ID))]
		} else if r.Status == "Archived" {
			label = "Finish archive " + r.ID[:min(8, len(r.ID))]
		}
		if action.Button(c, label, action.ButtonProps{Variant: action.Primary, Size: theme.Small}).AlignSelf(ui.Start).Disabled(d.busy || r.Busy || !r.CanResolve).Clicked() {
			a.initializationTask(func(m *devplatform.Manager) (string, error) {
				archive, e := m.ResolveInstanceInitialization(m.Context, r.ID, r.Revision)
				if archive != "" {
					return "Initialization archived; files retained at " + archive, e
				}
				return "Initialization record reconciled. Registered data was kept.", e
			})
		}
	}
}

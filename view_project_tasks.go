package main

import (
	"context"
	"errors"
	"github.com/egoist/mygo/ui"
	"macos-service-manager/internal/devplatform"
	"unicode/utf8"
)

type projectTask struct {
	Project                devplatform.Project
	Kind, Progress, Result string
	Running                bool
	err                    error
	cancel                 context.CancelFunc
}

func (a *app) projectTaskRunning(id string) bool {
	task := a.dev.projectTasks[id]
	return task != nil && task.Running
}
func (a *app) projectTasksRunning() bool {
	for _, task := range a.dev.projectTasks {
		if task.Running {
			return true
		}
	}
	return false
}
func (a *app) applyProjectTaskPhases() {
	for id, task := range a.dev.projectTasks {
		if !task.Running {
			continue
		}
		phase := ""
		switch task.Kind {
		case "up":
			phase = "Starting"
		case "down", "remove":
			phase = "Stopping"
		case "restart":
			phase = "Restarting"
		}
		if phase != "" {
			if a.dev.statuses == nil {
				a.dev.statuses = map[string]string{}
			}
			a.dev.statuses["project:"+id] = phase
		}
	}
}
func (a *app) startProjectOperation(p devplatform.Project, kind string, work func(*devplatform.Manager) (string, error)) {
	d := &a.dev
	if d.busy || a.quit.open || a.quit.allow || a.projectTaskRunning(p.ID) {
		return
	}
	if d.projectTasks == nil {
		d.projectTasks = map[string]*projectTask{}
	}
	ctx, cancel := context.WithCancel(context.Background())
	task := &projectTask{Project: p.Clone(), Kind: kind, Progress: "Preparing " + p.ID, Running: true, cancel: cancel}
	if kind == "dependencies" {
		task.Progress = "Installing project dependencies for " + p.Name
	}
	d.projectTasks[p.ID] = task
	d.taskSequence++
	a.applyProjectTaskPhases()
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
				if a.dev.projectTasks[p.ID] == task && task.Running {
					task.Progress = message
				}
			})
		}
		result, err := work(m)
		var inspection *devplatform.PortInspection
		var portError *devplatform.PortUnavailableError
		if errors.As(err, &portError) {
			if r, e := m.InspectPort(ctx, portError.Port); e == nil {
				inspection = &r
			}
		}
		a.post(func() {
			if a.dev.projectTasks[p.ID] != task {
				return
			}
			task.Running = false
			task.cancel = nil
			task.err = err
			if kind == "dependencies" {
				result = projectCommandPreview(result)
			}
			task.Result = result
			if err != nil {
				task.Result = "Project operation did not complete.\nError: " + err.Error()
				if kind == "dependencies" && result != "" {
					task.Result += "\n" + result
				}
			}
			if inspection != nil {
				a.storePortInspection(*inspection)
			}
			var contention *devplatform.PackageBusyError
			if errors.As(err, &contention) {
				a.dev.packageContention = contention
			}
			a.dev.taskSequence++
			a.dev.projectNeedsRefresh = true
			a.dev.lastProjectTask = task
			a.refreshFinishedPackages()
		})
	})
}
func (a *app) runProjectAction(p devplatform.Project, kind string) {
	a.startProjectOperation(p, kind, func(m *devplatform.Manager) (string, error) {
		switch kind {
		case "up":
			return "Started project " + p.ID, m.Up(m.Context, p.ID)
		case "down":
			return "Stopped project " + p.ID, m.Down(m.Context, p.ID)
		case "restart":
			return "Restarted project " + p.ID, m.RestartProject(m.Context, p.ID)
		case "remove":
			return "Removed project " + p.ID, m.RemoveProject(m.Context, p.ID)
		case "prepare":
			return "Environment prepared. Review services and start the project when ready.", m.PrepareEnvironment(m.Context, p.ID)
		}
		return "", errors.New("unknown project action")
	})
}
func (a *app) showProjectTasks() {
	d := &a.dev
	d.tab = 0
	d.commandWorkOpen = false
	d.operationsOpen = false
	d.diagnosticsOpen = false
	d.detailScroll[0].Y = 0
}
func (a *app) devProjectTasks(c *ui.Context) {
	for _, id := range sortedKeys(a.dev.projectTasks) {
		task := a.dev.projectTasks[id]
		label := map[string]string{"up": "Start", "down": "Stop", "restart": "Restart", "prepare": "Prepare environment", "remove": "Remove", "dependencies": "Install dependencies"}[task.Kind]
		if label == "" {
			label = "Operation"
		}
		devDivider(c, "Project · "+id+" · "+label)
		if task.Running {
			ui.Text(c, task.Progress).FontSize(textSmall)
			if ui.Button(c, "Cancel project operation "+id).AlignSelf(ui.Start).Clicked() && task.cancel != nil {
				task.cancel()
			}
		} else {
			if task.Kind == "dependencies" {
				value := task.Result
				ui.TextArea(c, &value).ReadOnly(true).Lines(3, 10)
			} else {
				ui.Text(c, task.Result).FontSize(textSmall)
			}
			if ui.Button(c, "Dismiss project result "+id).AlignSelf(ui.Start).Clicked() {
				delete(a.dev.projectTasks, id)
			}
		}
	}
}

func projectCommandPreview(output string) string {
	const limit = 16 << 10
	if len(output) <= limit {
		return output
	}
	cut := len(output) - limit
	for cut < len(output) && !utf8.RuneStart(output[cut]) {
		cut++
	}
	return "Showing the last 16 KiB of command output.\n" + output[cut:]
}

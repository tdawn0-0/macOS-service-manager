package main

import (
	"context"
	"fmt"
	"github.com/egoist/mygo/ui"
	"macos-service-manager/internal/devplatform"
	"time"
)

type quitState struct {
	open, allow, loading, stopping bool
	stopProxy                      bool
	sequence                       uint64
	plan                           *devplatform.ShutdownPlan
	err                            string
}

func (a *app) requestQuit() {
	if a.showQuit != nil {
		a.showQuit()
	}
	if a.quit.open {
		return
	}
	a.quit.sequence++
	a.quit.loading = false
	a.quit.open = true
	a.quit.stopProxy = true
	a.quit.err = ""
	a.quit.plan = nil
	a.loadQuitPreview()
}
func (a *app) loadQuitPreview() {
	if a.quit.loading || a.dev.busy || a.backgroundTasksRunning() {
		return
	}
	a.quit.loading = true
	sequence, factory := a.quit.sequence, a.dev.managerFactory
	a.spawn(func() {
		m := devplatform.New(devplatform.DefaultRoot())
		if factory != nil {
			m = factory()
		}
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		plan, err := m.PreviewShutdown(ctx)
		a.post(func() {
			if a.quit.sequence != sequence {
				return
			}
			a.quit.loading = false
			if !a.quit.open {
				return
			}
			if err != nil {
				a.quit.err = "Could not read the quit scope: " + err.Error()
				return
			}
			a.quit.plan = &plan
		})
	})
}
func (a *app) finishQuit() {
	a.quit.open = false
	a.quit.allow = true
	if a.quitApp != nil {
		a.quitApp()
	}
}
func (a *app) stopForQuit() {
	if a.quitOperationBusy() || a.quit.plan == nil || a.quit.stopping {
		return
	}
	plan, options := *a.quit.plan, devplatform.ShutdownOptions{StopProxy: a.quit.stopProxy}
	a.quit.stopping = true
	a.quit.err = ""
	a.devTaskThen(func(m *devplatform.Manager) (string, error) {
		return "Managed services stopped.", m.Shutdown(m.Context, plan, options)
	}, func(err error) {
		a.quit.stopping = false
		if err == nil {
			a.finishQuit()
			return
		}
		a.quit.err = a.dev.output + "\nSome services may already have stopped. Review the scope to retry, or quit keeping what remains."
		a.quit.plan = nil
	})
}

func (a *app) quitOperationBusy() bool {
	if a.dev.busy || a.backgroundTasksRunning() || a.editor.saving {
		return true
	}
	for _, busy := range a.busy {
		if busy {
			return true
		}
	}
	return false
}
func (a *app) quitDialog(c *ui.Context) {
	q := &a.quit
	if !q.open {
		return
	}
	if q.plan == nil && !q.loading && q.err == "" && !a.dev.busy && !a.backgroundTasksRunning() {
		a.loadQuitPreview()
	}
	width, height := c.Size()
	if ui.Modal(c, &q.open, func() {
		ui.Column(c).Width(min(float32(620), width-48)).Gap(12).Children(func() {
			ui.Text(c, "Quit Service Manager?").FontSize(18).FontWeight(650)
			ui.Scroll(c).MaxHeight(height - 200).Children(func() {
				ui.Column(c).Gap(8).Children(func() {
					if a.dev.proxyDraftDirty() || a.editor.open {
						ui.Text(c, "Unsaved edits will be discarded when the app exits.").FontSize(textSmall).TextColor(c.Theme().Warning)
					}
					if q.loading {
						ui.Text(c, "Reading managed service scope…").FontSize(textSmall)
					}
					if a.quitOperationBusy() && !q.stopping {
						ui.Text(c, "An operation is running. Finish it before quitting.").FontSize(textSmall)
						if a.backgroundTasksRunning() && ui.Button(c, "Cancel background tasks").Clicked() {
							a.cancelBackgroundTasks()
						}
						if a.dev.cancel != nil && ui.Button(c, "Cancel current operation").Clicked() {
							a.dev.cancel()
						}
					}
					if q.plan != nil {
						ui.Text(c, fmt.Sprintf("Stop scope: %d managed databases", len(q.plan.Instances))).FontSize(textSmall).FontWeight(600)
						for _, i := range q.plan.Instances {
							ui.Text(c, "Database · "+i.ID+" · "+softwareTitle(i.Name)).FontSize(textSmall)
						}
						ui.Checkbox(c, &q.stopProxy, "Stop local HTTPS proxy").Disabled(q.stopping)
						if q.plan.PrivilegedProxy && q.stopProxy {
							ui.Text(c, "Stopping the system HTTPS listener may require macOS administrator authorization.").FontSize(textSmall).TextColor(c.Theme().Warning)
						}
					}
					if q.stopping {
						ui.Text(c, a.dev.output).FontSize(textSmall)
						if ui.Button(c, "Cancel stopping").Clicked() && a.dev.cancel != nil {
							a.dev.cancel()
						}
					}
					if q.err != "" {
						ui.Text(c, q.err).FontSize(textSmall).TextColor(c.Theme().Danger)
						if ui.Button(c, "Review updated quit scope").Disabled(a.dev.busy).Clicked() {
							q.err = ""
							a.loadQuitPreview()
						}
					}
				})
			})
			ui.Row(c).Gap(8).Children(func() {
				if ui.Button(c, "Cancel quit").Disabled(q.stopping).Clicked() {
					q.open = false
					q.sequence++
				}
				ui.Spacer(c)
				if ui.Button(c, "Keep services and quit").Disabled(a.quitOperationBusy()).Clicked() {
					a.finishQuit()
				}
				if ui.PrimaryButton(c, "Stop managed databases and quit").Tooltip("Stops the selected app-owned resources. If HTTPS is kept, its log maintenance continues. Data, settings, CA trust and domain mappings are kept.").Disabled(a.quitOperationBusy() || q.loading || q.plan == nil).Clicked() {
					a.stopForQuit()
				}
			})
		})
	}).Dismissed() && q.stopping {
		q.open = true
	}
}

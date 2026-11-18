package main

import (
	"context"
	"devbase/internal/devplatform"
	"fmt"
	"strings"
	"sync"
	"testing"
	"time"
)

func TestUIBackgroundInstanceStartupKeepsPhaseConfigurationAndIndependentInstance(t *testing.T) {
	for _, cancelled := range []bool{false, true} {
		t.Run(fmt.Sprintf("cancel-%t", cancelled), func(t *testing.T) {
			h, m, _, id, redis := newUIDeletionInstance(t, cancelled)
			h.click("Stop")
			entered, release := make(chan struct{}), make(chan struct{})
			var signal, once sync.Once
			unblock := func() { once.Do(func() { close(release) }) }
			base := h.a.dev.managerFactory
			h.a.dev.managerFactory = func() *devplatform.Manager {
				worker := base()
				observer := worker.Event
				worker.Event = func(s string) {
					if observer != nil {
						observer(s)
					}
					if s == "Starting database "+id {
						signal.Do(func() { close(entered) })
						select {
						case <-release:
						case <-worker.Context.Done():
						}
					}
				}
				return worker
			}
			t.Cleanup(func() {
				unblock()
				h.a.cancelBackgroundTasks()
				h.foregroundOnly = false
				h.waitIdle()
				_ = m.InstanceAction(context.Background(), id, "remove")
				_ = m.InstanceAction(context.Background(), "independent-start", "remove")
			})
			h.foregroundOnly = true
			h.click("Start")
			select {
			case <-entered:
			case <-time.After(5 * time.Second):
				t.Fatal("startup did not pause")
			}
			if h.a.dev.busy || h.a.dev.statuses["instance:"+id] != "Starting" {
				t.Fatal("startup occupied foreground or hid its phase")
			}
			h.a.dev.statusLast = time.Now().Add(-6 * time.Second)
			h.ui.Frame()
			waitUIPackageCondition(t, h, func() bool { return !h.a.dev.statusPolling })
			if h.a.dev.statuses["instance:"+id] != "Starting" {
				t.Fatal("poll replaced the active startup phase")
			}
			configureUIInitialization(t, h, "independent-start", freeUIPort(t))
			h.click("Create instance")
			waitUIPackageCondition(t, h, func() bool {
				return !h.a.databaseTaskRunning("independent-start") && !h.a.dev.busy && !h.a.dev.databaseNeedsRefresh
			})
			h.click("Start")
			waitUIPackageCondition(t, h, func() bool {
				return !h.a.databaseTaskRunning("independent-start") && !h.a.dev.busy && !h.a.dev.databaseNeedsRefresh
			})
			if !h.a.databaseTaskRunning(id) || h.a.dev.statuses["instance:"+id] != "Starting" {
				t.Fatal("independent completion replaced the active startup")
			}
			h.click("HTTPS")
			h.fill("HTTPS listener port", fmt.Sprint(freeUIPort(t)))
			h.click("Add rule")
			h.fill("Domain", "during-startup.devbase.localhost")
			h.fill("Local port", "3000")
			timer := time.AfterFunc(4*time.Second, unblock)
			h.click("Save HTTPS rules")
			timer.Stop()
			state, e := m.Read()
			if e != nil || len(state.ProxyRules) != 1 || !h.a.databaseTaskRunning(id) {
				t.Fatal("startup blocked configuration", e)
			}
			if h.a.dev.statuses["instance:"+id] != "Starting" {
				t.Fatal("configuration refresh replaced startup phase")
			}
			h.fill("Domain", "unsaved-startup.devbase.localhost")
			if cancelled {
				h.click("Database tasks")
				h.click("Cancel database operation " + id)
			} else {
				unblock()
			}
			h.foregroundOnly = false
			h.waitIdle()
			if h.a.dev.proxyDraft[0].Domain != "unsaved-startup.devbase.localhost" {
				t.Fatal("startup completion replaced draft")
			}
			h.click("Databases")
			h.click("Instance " + id)
			if cancelled {
				if !strings.Contains(h.a.dev.databaseTasks[id].Result, "did not complete") || h.a.dev.statuses["instance:"+id] != "Stopped" {
					t.Fatal("cancelled startup was reported as running", h.a.dev.statuses)
				}
				unblock()
				h.click("Start")
			}
			if h.a.dev.databaseTasks[id].err != nil || h.a.dev.statuses["instance:"+id] != "Running" {
				t.Fatal("retry did not become ready", h.a.dev.databaseTasks[id].Result, h.a.dev.statuses["instance:"+id])
			}
			if redis("GET", "value") != "42" {
				t.Fatal("lifecycle lost existing database contents")
			}
			h.click("Stop")
			h.click("Instance independent-start")
			h.click("Restart")
			h.click("Stop")
		})
	}
}

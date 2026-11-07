package main

import (
	"context"
	"macos-service-manager/internal/devplatform"
	"time"
)

// Status observations never replace the form or the saved configuration. An
// operation begun after this poll owns the next state, even if its result
// arrives before an older, slower observation.
func (a *app) pollDevStatus() {
	d := &a.dev
	if !d.loaded {
		return
	}
	if d.statusLast.IsZero() {
		d.statusLast = time.Now()
		return
	}
	if d.busy || d.statusPolling || time.Since(d.statusLast) < 5*time.Second {
		return
	}
	d.statusPolling = true
	s, sequence, factory := d.state, d.taskSequence, d.managerFactory
	a.spawn(func() {
		m := devplatform.New(devplatform.DefaultRoot())
		if factory != nil {
			m = factory()
		}
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancel()
		result := m.Snapshot(ctx, s)
		a.post(func() {
			a.dev.statusPolling = false
			a.dev.statusLast = time.Now()
			if a.dev.busy || a.dev.taskSequence != sequence {
				return
			}
			a.dev.statuses = result.States
			a.applyDatabaseTaskPhases()
			a.applyProjectTaskPhases()
			a.dev.serviceStatuses = result.Services
			a.dev.folderErrors = result.FolderErrors
		})
	})
}

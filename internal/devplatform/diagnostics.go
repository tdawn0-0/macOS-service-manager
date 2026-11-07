package devplatform

import (
	"context"
	"errors"
	"fmt"
	"time"
)

func (m *Manager) Doctor(ctx context.Context) (string, error) {
	report, err := m.Diagnose(ctx)
	if err != nil {
		return "", err
	}
	return report.Text(), nil
}

type ServiceStatus struct {
	Status       string `json:"status"`
	PID          int    `json:"pid,omitempty"`
	Launches     int    `json:"launches,omitempty"`
	LastExitCode *int   `json:"lastExitCode,omitempty"`
	LastSignal   string `json:"lastSignal,omitempty"`
	Message      string `json:"message,omitempty"`
}
type RuntimeSnapshot struct {
	FolderErrors map[string]string        `json:"folderErrors,omitempty"`
	States       map[string]string        `json:"states"`
	Services     map[string]ServiceStatus `json:"services"`
}

func ServiceStatusKey(project, service string) string { return "service:" + project + ":" + service }

func (m *Manager) observeService(ctx context.Context, job string, svc Service) ServiceStatus {
	state, err := m.readJobState(ctx, job)
	result := ServiceStatus{Status: "Stopped", PID: state.PID, Launches: state.Runs, LastExitCode: state.LastExitCode, LastSignal: state.LastSignal}
	if ctx.Err() != nil || errors.Is(err, context.Canceled) || errors.Is(err, context.DeadlineExceeded) {
		result.Status = "Unknown"
		return result
	}
	if err != nil {
		return result
	}
	if !state.Running {
		result.Status = "Failed"
		if svc.Restart {
			result.Status = "Restarting"
			result.Message = fmt.Sprintf("Waiting for launchd; restart throttle %d seconds.", svc.restartThrottle())
		}
		if state.LastExitCode != nil {
			result.Message += fmt.Sprintf(" Last exit code: %d.", *state.LastExitCode)
		}
		if state.LastSignal != "" {
			result.Message += " Last terminating signal: " + state.LastSignal + "."
		}
		if state.Runs >= 3 {
			result.Message += " Repeated launches observed; inspect the logs if exits continue."
		}
		return result
	}
	result.Status = "Running"
	if svc.Health == "" {
		result.Message = "Process is running. Configure a readiness check to verify readiness."
		return result
	}
	check, done := context.WithTimeout(ctx, time.Second)
	err = probeHealth(check, svc.Health)
	done()
	if ctx.Err() != nil {
		result.Status = "Unknown"
		return result
	}
	if err != nil {
		result.Status = "Not ready"
		result.Message = "Process is running but the readiness check failed."
		return result
	}
	// Recheck ownership after the network probe. A listening external process
	// must not hide a managed process which exited during the probe.
	after, err := m.readJobState(ctx, job)
	if err != nil || !after.Running || (state.PID > 0 && after.PID != state.PID) {
		result.Status = "Not ready"
		result.Message = "Process changed or exited during the readiness check."
		return result
	}
	result.Status = "Healthy"
	result.Message = "Configured readiness check passed."
	if svc.Health == "process" {
		result.Message = "Process check passed; application ports were not checked."
	}
	return result
}

// Snapshot is a bounded observation, never an inferred success from registration.
// Unknown means the observation budget expired, not that a service stopped.
func (m *Manager) Snapshot(ctx context.Context, s State) RuntimeSnapshot {
	ctx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()
	out := RuntimeSnapshot{States: map[string]string{}, Services: map[string]ServiceStatus{}, FolderErrors: map[string]string{}}
	instanceRunning := map[string]bool{}
	for _, id := range diagnosticKeys(s.Instances) {
		job, err := m.readJobState(ctx, "db-"+id)
		status := "Stopped"
		if err == nil {
			status = "Failed"
		}
		if err == nil && job.Running {
			status = "Running"
			instanceRunning[id] = true
		}
		if ctx.Err() != nil || errors.Is(err, context.Canceled) || errors.Is(err, context.DeadlineExceeded) {
			status = "Unknown"
		}
		out.States["instance:"+id] = status
	}
	for _, id := range diagnosticKeys(s.Projects) {
		p := s.Projects[id]
		if err := projectFolderError(p.Directory); err != nil {
			out.FolderErrors[id] = err.Error()
		}
		marked := m.loaded(ctx, "project-"+id)
		owns := marked
		running, healthy, unready, restarting, failed, unknown := 0, 0, 0, 0, 0, 0
		for _, svc := range p.Services {
			state := m.observeService(ctx, "p-"+id+"-"+svc.ID, svc)
			key := ServiceStatusKey(id, svc.ID)
			out.Services[key] = state
			out.States[key] = state.Status
			if state.Status != "Stopped" && state.Status != "Unknown" {
				owns = true
			}
			switch state.Status {
			case "Healthy":
				running++
				healthy++
			case "Running":
				running++
			case "Not ready":
				running++
				unready++
			case "Restarting":
				restarting++
			case "Failed":
				failed++
			case "Unknown":
				unknown++
			}
		}
		for _, ref := range p.Instances {
			if instanceRunning[ref] {
				running++
			} else if marked {
				failed++
			}
			if out.States["instance:"+ref] == "Unknown" {
				unknown++
			}
		}
		status := "Stopped"
		total := len(p.Services) + len(p.Instances)
		switch {
		case ctx.Err() != nil || unknown > 0:
			status = "Unknown"
		case total == 0 && marked:
			status = "Failed"
			if proxy, err := m.readJobState(ctx, "proxy"); (err == nil && proxy.Running) || m.socketLoaded(ctx) {
				status = "Running"
			}
		case !owns: // A standalone database is not a project owner.
		case running > 0 && running < total:
			status = "Partly running"
		case failed > 0 || (marked && running == 0 && restarting == 0):
			status = "Failed"
		case restarting > 0:
			status = "Restarting"
		case unready > 0:
			status = "Not ready"
		case running == total && total > 0:
			status = "Running"
			if len(p.Services) > 0 && healthy == len(p.Services) {
				status = "Healthy"
			}
		}
		out.States["project:"+id] = status
	}
	return out
}
func (m *Manager) Statuses(ctx context.Context, s State) map[string]string {
	return m.Snapshot(ctx, s).States
}

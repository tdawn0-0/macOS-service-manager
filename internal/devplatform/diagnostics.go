package devplatform

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"
)

type RuntimeSnapshot struct {
	States map[string]string `json:"states"`
}

// Snapshot is a bounded observation, never an inferred success from registration.
// Unknown means the observation budget expired, not that a service stopped.
func (m *Manager) Snapshot(ctx context.Context, s State) RuntimeSnapshot {
	ctx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()
	out := RuntimeSnapshot{States: map[string]string{}}
	if m.HasCloudflareToken() {
		out.States["tunnel:token"] = "Configured"
	}
	type observation struct {
		key, status string
	}
	type task struct {
		key string
		run func(context.Context) observation
	}
	var tasks []task
	for _, id := range diagnosticKeys(s.Instances) {
		key := "instance:" + id
		tasks = append(tasks, task{key, func(check context.Context) observation {
			job, err := m.readJobState(check, "db-"+id)
			status := "Stopped"
			if err == nil {
				status = "Failed"
				if job.Running {
					status = "Running"
				}
			}
			if check.Err() != nil || errors.Is(err, context.Canceled) || errors.Is(err, context.DeadlineExceeded) {
				status = "Unknown"
			}
			return observation{key: key, status: status}
		}})
	}
	// Observe an app-owned proxy even after the last rule is removed: an
	// unapplied configuration can leave the previous listener running.
	_, proxyJobError := os.Stat(filepath.Join(m.Root, "jobs", m.label("proxy")+".plist"))
	_, socketJobError := os.Stat(m.socketJobPath())
	observeProxy := len(s.ProxyRules) > 0 || proxyJobError == nil || socketJobError == nil
	if observeProxy {
		tasks = append(tasks, task{"proxy", func(check context.Context) observation {
			job, err := m.readJobState(check, "proxy")
			status := "Unknown"
			if err == nil {
				status = "Failed"
			} else if strings.Contains(err.Error(), "Could not find service") || strings.Contains(err.Error(), "Could not find specified service") {
				status = "Stopped"
			}
			if (err == nil && job.Running) || m.socketLoaded(check) {
				status = "Running"
			}
			if check.Err() != nil || errors.Is(err, context.Canceled) || errors.Is(err, context.DeadlineExceeded) {
				status = "Unknown"
			}
			return observation{key: "proxy", status: status}
		}})
	}
	_, tunnelJobError := os.Stat(filepath.Join(m.Root, "jobs", m.label("tunnel")+".plist"))
	anyTunnelRule := false
	for _, rule := range s.ProxyRules {
		if rule.Tunnel {
			anyTunnelRule = true
			break
		}
	}
	observeTunnel := m.HasCloudflareToken() || s.CloudflareAccountID != "" || len(s.TunnelWorkers) > 0 || anyTunnelRule || tunnelJobError == nil
	if observeTunnel {
		tasks = append(tasks, task{"tunnel", func(check context.Context) observation {
			job, err := m.readJobState(check, "tunnel")
			status := "Unknown"
			if err == nil {
				status = "Failed"
				if job.Running {
					status = "Running"
				}
			} else if strings.Contains(err.Error(), "Could not find service") || strings.Contains(err.Error(), "Could not find specified service") {
				status = "Stopped"
			}
			if check.Err() != nil || errors.Is(err, context.Canceled) || errors.Is(err, context.DeadlineExceeded) {
				status = "Unknown"
			}
			return observation{key: "tunnel", status: status}
		}})
	}
	// A fixed worker pool bounds launchctl/probe concurrency. Divide the available
	// time across worst-case queue batches, reserving 10% for collection. Every
	// queued item gets a budget, rather than later items inheriting an empty one.
	results := make(chan observation, len(tasks))
	queue := make(chan task, len(tasks))
	for _, work := range tasks {
		queue <- work
	}
	close(queue)
	deadline, _ := ctx.Deadline()
	batches := max(1, (len(tasks)+3)/4)
	budget := time.Until(deadline) * 9 / 10 / time.Duration(batches)
	var workers sync.WaitGroup
	for n := 0; n < min(4, len(tasks)); n++ {
		workers.Add(1)
		go func() {
			defer workers.Done()
			for work := range queue {
				r := observation{key: work.key, status: "Unknown"}
				if ctx.Err() == nil && budget > 0 {
					check, done := context.WithTimeout(ctx, budget)
					r = work.run(check)
					done()
				}
				results <- r
			}
		}()
	}
	workers.Wait()
	close(results)
	observed := make(map[string]observation, len(tasks))
	for r := range results {
		observed[r.key] = r
	}
	for _, id := range diagnosticKeys(s.Instances) {
		out.States["instance:"+id] = observed["instance:"+id].status
	}
	if observeProxy {
		out.States["proxy"] = observed["proxy"].status
	}
	if observeTunnel {
		st := observed["tunnel"].status
		out.States["tunnel"] = st
		if st == "Running" {
			if ts, ok := m.readTunnelStatus(); ok {
				if detail := formatTunnelDetailFrom(ts); detail != "" {
					out.States["tunnel:detail"] = detail
				}
				for name, ws := range ts.WorkerStates {
					if name != "" && ws != "" {
						out.States["tunnel:worker:"+name] = ws
					}
				}
			}
		}
	}
	return out
}

func (m *Manager) formatTunnelDetail() string {
	st, ok := m.readTunnelStatus()
	if !ok {
		return ""
	}
	return formatTunnelDetailFrom(st)
}

func formatTunnelDetailFrom(st TunnelStatusSnapshot) string {
	switch st.Phase {
	case "Connected":
		if len(st.Conflicts) > 0 {
			if len(st.Conflicts) == 1 {
				return "Conflict: 1 worker"
			}
			return fmt.Sprintf("Conflict: %d workers", len(st.Conflicts))
		}
		if st.Total > 0 {
			if st.Connected == 1 && st.Total == 1 {
				return "Connected · 1 worker"
			}
			return fmt.Sprintf("Connected · %d/%d workers", st.Connected, st.Total)
		}
		return "Connected"
	case "AuthFailed":
		return "Auth failed"
	case "Reconnecting", "Connecting":
		return st.Phase
	}
	return ""
}

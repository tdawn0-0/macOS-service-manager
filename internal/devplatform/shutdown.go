package devplatform

import (
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"
)

type ShutdownResource struct{ ID, Name string }
type ShutdownPlan struct {
	Projects        []ShutdownResource
	Instances       []ShutdownResource
	Fingerprint     string
	PrivilegedProxy bool
}
type ShutdownOptions struct{ StopProxy bool }

func (m *Manager) shutdownFingerprint(s State) string {
	b, _ := json.Marshal(s)
	return fmt.Sprintf("%x", sha256.Sum256(append([]byte(filepath.Clean(m.Root)+"\x00"), b...)))
}

// Preview includes all configured resources, including currently stopped ones:
// another client may start one before shutdown acquires the state lock. It does
// not inspect or modify keychain trust, hosts, or any external service.
func (m *Manager) PreviewShutdown(ctx context.Context) (ShutdownPlan, error) {
	if err := ctx.Err(); err != nil {
		return ShutdownPlan{}, err
	}
	s, err := m.Read()
	if err != nil {
		return ShutdownPlan{}, err
	}
	p := ShutdownPlan{Fingerprint: m.shutdownFingerprint(s), Projects: []ShutdownResource{}, Instances: []ShutdownResource{}, PrivilegedProxy: privilegedPorts(allSites(&s))}
	for _, id := range diagnosticKeys(s.Projects) {
		p.Projects = append(p.Projects, ShutdownResource{id, s.Projects[id].Name})
	}
	for _, id := range diagnosticKeys(s.Instances) {
		p.Instances = append(p.Instances, ShutdownResource{id, s.Instances[id].Kind})
	}
	if _, err = os.Stat(m.socketJobPath()); err == nil {
		p.PrivilegedProxy = true
	} else if !errors.Is(err, os.ErrNotExist) {
		return ShutdownPlan{}, err
	}
	return p, nil
}

// Shutdown stops only jobs derived from this storage root's managed records.
// A failed/cancelled stop leaves the caller open to report or retry. Data,
// configuration, login preferences and CA/hosts state remain available.
func (m *Manager) Shutdown(ctx context.Context, plan ShutdownPlan, options ShutdownOptions) error {
	return m.withState(ctx, func(s *State) error {
		if plan.Fingerprint == "" || plan.Fingerprint != m.shutdownFingerprint(*s) {
			return errors.New("workspace changed after the quit preview; review the updated scope and retry")
		}
		ids := diagnosticKeys(s.Instances)
		for _, p := range s.Projects {
			ids = append(ids, p.Instances...)
		}
		keys := append(projectResourceKeys(diagnosticKeys(s.Projects)...), instanceResourceKeys(ids)...)
		release, e := m.resourceLeases(ctx, keys...)
		if e != nil {
			return e
		}
		defer release()
		// A finite login launcher must not start a project after this shutdown.
		for _, id := range diagnosticKeys(s.Projects) {
			if err := m.stopJob(ctx, "login-"+id); err != nil {
				return fmt.Errorf("stop login launcher for %s: %w", id, err)
			}
		}
		for _, id := range diagnosticKeys(s.Projects) {
			m.emit("Stopping project " + s.Projects[id].Name)
			if err := m.downProject(ctx, s, id); err != nil {
				return fmt.Errorf("stop project %s: %w", id, err)
			}
		}
		// downProject preserves a shared database until its last project stops.
		// Also stop standalone managed instances after all projects are down.
		for _, id := range diagnosticKeys(s.Instances) {
			m.emit("Stopping database " + id)
			if err := m.stopJob(ctx, "db-"+id); err != nil {
				return fmt.Errorf("stop database %s: %w", id, err)
			}
		}
		if options.StopProxy {
			m.emit("Stopping local HTTPS proxy")
			if err := m.stopProxy(ctx); err != nil {
				return fmt.Errorf("stop HTTPS proxy: %w", err)
			}
		}
		m.emit("Stopping scheduled log maintenance")
		if err := m.stopLogMaintenance(ctx); err != nil {
			return err
		}
		// A failed launchctl read is not proof that a job is absent. Verify the
		// full chosen scope before allowing the GUI to finish quitting.
		var targets []string
		for _, id := range diagnosticKeys(s.Projects) {
			targets = append(targets, domain()+"/"+m.label("project-"+id), domain()+"/"+m.label("login-"+id))
			for _, svc := range s.Projects[id].Services {
				targets = append(targets, domain()+"/"+m.label("p-"+id+"-"+svc.ID))
			}
		}
		for _, id := range diagnosticKeys(s.Instances) {
			targets = append(targets, domain()+"/"+m.label("db-"+id))
		}
		targets = append(targets, domain()+"/"+m.label("maintenance"))
		if options.StopProxy {
			targets = append(targets, domain()+"/"+m.label("proxy"), m.socketTarget())
		}
		for _, target := range targets {
			if err := m.verifyShutdownTarget(ctx, target); err != nil {
				return err
			}
		}
		return nil
	})
}

func (m *Manager) verifyShutdownTarget(ctx context.Context, target string) error {
	ctx, cancel := context.WithTimeout(ctx, time.Second)
	defer cancel()
	_, err := m.Run(ctx, "launchctl", "print", target)
	if ctx.Err() != nil {
		return fmt.Errorf("could not verify stopped service: %w", ctx.Err())
	}
	if err == nil {
		return fmt.Errorf("managed service is still registered: %s", target)
	}
	if strings.Contains(err.Error(), "Could not find service") || strings.Contains(err.Error(), "Could not find specified service") {
		return nil
	}
	return fmt.Errorf("could not verify stopped service %s: %w", target, err)
}

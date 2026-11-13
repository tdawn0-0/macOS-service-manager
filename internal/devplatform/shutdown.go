package devplatform

import (
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"fmt"
	"path/filepath"
	"strings"
	"time"
)

type ShutdownResource struct{ ID, Name string }
type ShutdownPlan struct {
	Instances   []ShutdownResource
	Fingerprint string
}
type ShutdownOptions struct{}

func (m *Manager) shutdownFingerprint(s State) string {
	b, _ := json.Marshal(s)
	return fmt.Sprintf("%x", sha256.Sum256(append([]byte(filepath.Clean(m.Root)+"\x00"), b...)))
}

// Preview includes all configured database resources, including currently stopped ones:
// another client may start one before shutdown acquires the state lock. Background HTTPS
// proxy and tunnel services are managed independently by launchd and are not stopped on quit.
func (m *Manager) PreviewShutdown(ctx context.Context) (ShutdownPlan, error) {
	if err := ctx.Err(); err != nil {
		return ShutdownPlan{}, err
	}
	s, err := m.Read()
	if err != nil {
		return ShutdownPlan{}, err
	}
	p := ShutdownPlan{Fingerprint: m.shutdownFingerprint(s), Instances: []ShutdownResource{}}
	for _, id := range diagnosticKeys(s.Instances) {
		p.Instances = append(p.Instances, ShutdownResource{id, s.Instances[id].Kind})
	}
	return p, nil
}

// Shutdown stops only managed database jobs derived from this storage root's
// records. Background HTTPS proxy, Cloudflare tunnel, and log maintenance jobs
// remain running under launchd.
func (m *Manager) Shutdown(ctx context.Context, plan ShutdownPlan, options ShutdownOptions) error {
	return m.withState(ctx, func(s *State) error {
		if plan.Fingerprint == "" || plan.Fingerprint != m.shutdownFingerprint(*s) {
			return errors.New("workspace changed after the quit preview; review the updated scope and retry")
		}
		keys := instanceResourceKeys(diagnosticKeys(s.Instances))
		release, e := m.resourceLeases(ctx, keys...)
		if e != nil {
			return e
		}
		defer release()
		for _, id := range diagnosticKeys(s.Instances) {
			m.emit("Stopping database " + id)
			if err := m.stopJob(ctx, "db-"+id); err != nil {
				return fmt.Errorf("stop database %s: %w", id, err)
			}
		}
		// A failed launchctl read is not proof that a job is absent. Verify the
		// full chosen scope before allowing the GUI to finish quitting.
		var targets []string
		for _, id := range diagnosticKeys(s.Instances) {
			targets = append(targets, domain()+"/"+m.label("db-"+id))
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

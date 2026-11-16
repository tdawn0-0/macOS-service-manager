package devplatform

import (
	"context"
	"devbase/internal/logs"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
)

// PatchSeries splits an exact x.y.z version into its series "x.y" and patch
// number. ok is false for anything else.
func PatchSeries(version string) (series string, patch int, ok bool) {
	if !exactRuntimeVersion.MatchString(version) {
		return "", 0, false
	}
	parts := strings.Split(version, ".")
	n, err := strconv.Atoi(parts[2])
	if err != nil {
		return "", 0, false
	}
	return parts[0] + "." + parts[1], n, true
}

func upgradableKind(kind string) bool { return kind == "mysql" || kind == "mongodb" }

// UpgradeTargets lists the installed packages an instance could move to:
// same engine, same architecture, same series, higher patch. Newest first.
// It is pure: it reads only s.
func UpgradeTargets(s State, i Instance) []string {
	if !upgradableKind(i.Kind) {
		return nil
	}
	current, ok := s.Packages[i.Package]
	if !ok {
		return nil
	}
	series, patch, ok := PatchSeries(current.Version)
	if !ok {
		return nil
	}
	type candidate struct {
		id    string
		patch int
	}
	var found []candidate
	for id, p := range s.Packages {
		if p.Name != i.Kind || p.Arch != current.Arch {
			continue
		}
		candidateSeries, candidatePatch, ok := PatchSeries(p.Version)
		if !ok || candidateSeries != series || candidatePatch <= patch {
			continue
		}
		found = append(found, candidate{id, candidatePatch})
	}
	sort.Slice(found, func(a, b int) bool { return found[a].patch > found[b].patch })
	out := make([]string, 0, len(found))
	for _, c := range found {
		out = append(out, c.id)
	}
	return out
}

// NewerListedPatch returns the newest version in listed that is in the same
// series as installed and has a higher patch, or "".
func NewerListedPatch(installed string, listed []string) string {
	series, patch, ok := PatchSeries(installed)
	if !ok {
		return ""
	}
	best, bestPatch := "", patch
	for _, v := range listed {
		s, p, ok := PatchSeries(v)
		if ok && s == series && p > bestPatch {
			best, bestPatch = v, p
		}
	}
	return best
}

// mysqlStoppedCleanly reports whether the instance's last recorded server
// event is a completed shutdown.
func (m *Manager) mysqlStoppedCleanly(id string) bool {
	snapshot, err := logs.Tail(filepath.Join(m.Root, "logs", "db-"+id+".err.log"), 256<<10, 2000)
	if err != nil {
		return false
	}
	for n := len(snapshot.Lines) - 1; n >= 0; n-- {
		line := snapshot.Lines[n]
		if strings.Contains(line, "Shutdown complete") {
			return true
		}
		if strings.Contains(line, "ready for connections") {
			return false
		}
	}
	return false
}

// UpgradeInstancePackage moves a stopped instance to target, a newer
// installed patch release of the same engine and series. It changes the saved
// package and discards the instance's generated job file, which the next start
// writes again; data, port and credentials are untouched.
func (m *Manager) UpgradeInstancePackage(ctx context.Context, expected Instance, target string) error {
	if err := validID(expected.ID); err != nil {
		return err
	}
	if !upgradableKind(expected.Kind) {
		return errors.New("patch upgrades are available for MySQL and MongoDB instances only")
	}
	unlock, err := m.resourceLeases(ctx, resourceKey{"database instance", expected.ID})
	if err != nil {
		return err
	}
	defer unlock()
	records, err := m.databaseWorkRecords()
	if err != nil {
		return err
	}
	for _, r := range records {
		if r.Instance.ID == expected.ID {
			return errors.New("finish or resolve this instance's unfinished database task before upgrading")
		}
	}
	return m.withStateTransaction(ctx, func(s *State) (func() error, func(), error) {
		i, ok := s.Instances[expected.ID]
		if !ok {
			return nil, nil, errors.New("instance not found")
		}
		if e := m.verifyDataLocation(i); e != nil {
			return nil, nil, e
		}
		if i != expected {
			return nil, nil, errors.New("database settings changed; select the instance again before upgrading")
		}
		allowed := false
		for _, t := range UpgradeTargets(*s, i) {
			if t == target {
				allowed = true
				break
			}
		}
		if !allowed {
			return nil, nil, errors.New("choose a newer installed patch release of the same series")
		}
		server := "mysqld"
		if i.Kind == "mongodb" {
			server = "mongod"
		}
		if _, e := m.Binary(s, target, server); e != nil {
			return nil, nil, e
		}
		if e := m.verifyShutdownTarget(ctx, domain()+"/"+m.label("db-"+i.ID)); e != nil {
			return nil, nil, fmt.Errorf("stop the database before upgrading: %w", e)
		}
		if i.Kind == "mysql" && !m.mysqlStoppedCleanly(i.ID) {
			return nil, nil, errors.New("could not confirm that MySQL shut down cleanly; start the instance, stop it, then upgrade")
		}
		i.Package = target
		s.Instances[i.ID] = i
		m.emit("Moved " + i.ID + " to " + target)
		job := filepath.Join(m.Root, "jobs", m.label("db-"+i.ID)+".plist")
		commit := func() {
			if e := os.Remove(job); e != nil && !errors.Is(e, os.ErrNotExist) {
				m.emit("Could not remove the old job file for " + expected.ID + "; the next start replaces it")
			}
		}
		return nil, commit, nil
	})
}

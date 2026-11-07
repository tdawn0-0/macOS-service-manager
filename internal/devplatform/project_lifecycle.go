package devplatform

import (
	"context"
	"errors"
	"fmt"
	"reflect"
	"sort"
)

func projectResourceKeys(ids ...string) []resourceKey {
	var keys []resourceKey
	for _, id := range ids {
		if id != "" {
			keys = append(keys, resourceKey{"project", id})
		}
	}
	return keys
}

// Hold the project, its instances, and (when executing) packages; the workspace
// lock is reserved for short configuration/proxy publication, never readiness.
func (m *Manager) withProjectLifecycle(ctx context.Context, id string, executing bool, fn func(*State) error) error {
	if e := validID(id); e != nil {
		return e
	}
	if e := ctx.Err(); e != nil {
		return e
	}
	if e := m.init(); e != nil {
		return e
	}
	release, e := m.resourceLeases(ctx, projectResourceKeys(id)...)
	if e != nil {
		return e
	}
	defer release()
	before, e := m.Read()
	if e != nil {
		return e
	}
	p, ok := before.Projects[id]
	if !ok {
		return errors.New("project not found")
	}
	if p.ID != id {
		return errors.New("project identity mismatch")
	}
	releaseInstances, e := m.resourceLeases(ctx, instanceResourceKeys(p.Instances)...)
	if e != nil {
		return e
	}
	defer releaseInstances()
	if executing {
		packages := map[string]bool{}
		for _, svc := range p.Services {
			if svc.Package != "" {
				packages[svc.Package] = true
			}
		}
		for _, iid := range p.Instances {
			if i, ok := before.Instances[iid]; ok {
				packages[i.Package] = true
			}
		}
		ids := make([]string, 0, len(packages))
		for pkg := range packages {
			ids = append(ids, pkg)
		}
		sort.Strings(ids)
		var releases []func()
		defer func() {
			for n := len(releases) - 1; n >= 0; n-- {
				releases[n]()
			}
		}()
		for _, pkg := range ids {
			release, e := m.packageLease(ctx, pkg, false)
			if e != nil {
				return e
			}
			releases = append(releases, release)
		}
	}
	latest, e := m.Read()
	if e != nil {
		return e
	}
	if !reflect.DeepEqual(latest.Projects[id], p) {
		return errors.New("project changed before its operation; refresh and retry")
	}
	for _, iid := range p.Instances {
		if latest.Instances[iid] != before.Instances[iid] {
			return errors.New("project database settings changed; refresh and retry")
		}
	}
	if e = ctx.Err(); e != nil {
		return e
	}
	return fn(&latest)
}

func (m *Manager) projectActiveKnown(ctx context.Context, p Project) (bool, error) {
	jobs := []string{"project-" + p.ID}
	for _, svc := range p.Services {
		jobs = append(jobs, "p-"+p.ID+"-"+svc.ID)
	}
	for _, job := range jobs {
		loaded, e := m.packageJobLoaded(ctx, m.label(job))
		if e != nil {
			return false, fmt.Errorf("cannot confirm project %s state: %w", p.ID, e)
		}
		if loaded {
			return true, nil
		}
	}
	return false, nil
}

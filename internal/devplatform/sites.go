package devplatform

import (
	"context"
	"errors"
)

// EditSite changes only routing declarations. Applying them is a separate
// explicit action, allowing routes to change without restarting project jobs.
func (m *Manager) EditSite(ctx context.Context, oldOwner, oldID, owner string, site Site) error {
	if e := site.Validate(); e != nil {
		return e
	}
	release, e := m.resourceLeases(ctx, projectResourceKeys(oldOwner, owner)...)
	if e != nil {
		return e
	}
	defer release()
	return m.withState(ctx, func(s *State) error {
		if oldID != "" && !removeSite(s, oldOwner, oldID) {
			return errors.New("site changed or was removed; select it again")
		}
		if owner == "" {
			if _, exists := s.Sites[site.ID]; exists {
				return errors.New("a site with this name already exists")
			}
			s.Sites[site.ID] = site
		} else {
			p, ok := s.Projects[owner]
			if !ok {
				return errors.New("site project not found")
			}
			for _, existing := range p.Sites {
				if existing.ID == site.ID {
					return errors.New("a site with this name already exists in the project")
				}
			}
			p.Sites = append(p.Sites, site)
			s.Projects[owner] = p
		}
		return validateSites(allSites(s))
	})
}

func removeSite(s *State, owner, id string) bool {
	if owner == "" {
		if _, exists := s.Sites[id]; !exists {
			return false
		}
		delete(s.Sites, id)
		return true
	}
	p, ok := s.Projects[owner]
	if !ok {
		return false
	}
	for n, site := range p.Sites {
		if site.ID == id {
			p.Sites = append(p.Sites[:n], p.Sites[n+1:]...)
			s.Projects[owner] = p
			return true
		}
	}
	return false
}

func (m *Manager) RemoveSite(ctx context.Context, owner, id string) error {
	release, e := m.resourceLeases(ctx, projectResourceKeys(owner)...)
	if e != nil {
		return e
	}
	defer release()
	return m.withState(ctx, func(s *State) error {
		if !removeSite(s, owner, id) {
			return errors.New("site not found")
		}
		return nil
	})
}

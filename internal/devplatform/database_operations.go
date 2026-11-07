package devplatform

import (
	"context"
	"errors"
)

func instanceResourceKeys(ids []string) []resourceKey {
	keys := make([]resourceKey, 0, len(ids))
	for _, id := range ids {
		keys = append(keys, resourceKey{"database instance", id})
	}
	return keys
}

// Data tools use an immutable configuration snapshot, never a state transaction.
// Every lifecycle or data-directory mutation must respect the same instance lease.
func (m *Manager) withDatabaseOperation(ctx context.Context, id string, fn func(*State) error) error {
	if e := validID(id); e != nil {
		return e
	}
	if e := m.init(); e != nil {
		return e
	}
	release, e := m.resourceLeases(ctx, instanceResourceKeys([]string{id})...)
	if e != nil {
		return e
	}
	defer release()
	s, e := m.Read()
	if e != nil {
		return e
	}
	i, ok := s.Instances[id]
	if !ok {
		return errors.New("database instance not found")
	}
	packageRelease, e := m.packageLease(ctx, i.Package, false)
	if e != nil {
		return e
	}
	defer packageRelease()
	// The package may have finished a repair while its lease was being acquired.
	s, e = m.Read()
	if e != nil {
		return e
	}
	if current, ok := s.Instances[id]; !ok || current != i {
		return errors.New("database settings changed; retry with the current instance")
	}
	if m.databaseWork != nil && i != m.databaseWork.record.Instance {
		return errors.New("database task settings changed; review the unfinished task")
	}
	m.emit("Using database " + id)
	if e = ctx.Err(); e != nil {
		return e
	}
	return fn(&s)
}

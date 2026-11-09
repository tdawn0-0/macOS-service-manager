package devplatform

import (
	"context"
	"fmt"
)

func (m *Manager) RuntimeVersions(ctx context.Context, name string) ([]string, error) {
	var versions []string
	switch name {
	case "java":
		rows, err := javaCatalog(ctx)
		if err != nil {
			return nil, err
		}
		for _, p := range rows {
			versions = append(versions, p.Version)
		}
	default:
		return nil, fmt.Errorf("unsupported runtime %s", name)
	}
	return versions, nil
}

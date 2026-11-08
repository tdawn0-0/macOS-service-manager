package devplatform

import (
	"context"
	"fmt"
	"strings"
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
	case "go":
		var rows []struct {
			Version string
			Stable  bool
			Files   []struct{ OS, Arch, Kind string }
		}
		if err := fetch(ctx, "https://go.dev/dl/?mode=json&include=all", &rows); err != nil {
			return nil, err
		}
		for _, r := range rows {
			if !r.Stable {
				continue
			}
			for _, f := range r.Files {
				if f.OS == "darwin" && f.Arch == "arm64" && f.Kind == "archive" {
					versions = append(versions, strings.TrimPrefix(r.Version, "go"))
					break
				}
			}
		}
	default:
		return nil, fmt.Errorf("unsupported runtime %s", name)
	}
	return versions, nil
}

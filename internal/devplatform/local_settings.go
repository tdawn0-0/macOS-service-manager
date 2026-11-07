package devplatform

import (
	"context"
	"errors"
	"fmt"
	"path/filepath"
	"strings"
)

type LocalArgument struct {
	Prefix string `json:"prefix,omitempty"`
	Index  int    `json:"index"`
	Kind   string `json:"kind"` // executable, path, secret, text
	Label  string `json:"label"`
	Value  string `json:"value,omitempty"`
}

func LocalArgumentPlaceholder(index int) string { return fmt.Sprintf("<local argument %d>", index+1) }

// These values live only in private application state. A shared declaration
// contains definitions and placeholders, never the local values.
func (p Project) MissingLocalSettings() []string {
	var missing []string
	for _, svc := range p.Services {
		for _, arg := range svc.LocalArguments {
			if arg.Value == "" {
				missing = append(missing, svc.ID+": "+arg.Label)
			}
		}
		if svc.NeedsEnvironmentFile && svc.EnvFile == "" {
			missing = append(missing, svc.ID+": choose a local environment file")
		}
		if svc.NeedsHealthReview && svc.Health == "" {
			missing = append(missing, svc.ID+": configure a local readiness check")
		}
		for _, key := range svc.RequiredEnvironment {
			if _, ok := svc.Environment[key]; !ok && svc.EnvFile == "" {
				missing = append(missing, svc.ID+": environment variable "+key)
			}
		}
	}
	for _, site := range p.Sites {
		if site.NeedsReview && !site.Disabled {
			missing = append(missing, "Review upstream for "+site.Domain+" in HTTPS")
		}
	}
	return missing
}
func (p Project) requireLocalSettings(ctx context.Context) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	if missing := p.MissingLocalSettings(); len(missing) > 0 {
		return errors.New("complete local settings before starting: " + strings.Join(missing, "; "))
	}
	for _, svc := range p.Services {
		env := map[string]string{}
		if len(svc.RequiredEnvironment) > 0 && svc.EnvFile != "" {
			path := svc.EnvFile
			if !filepath.IsAbs(path) {
				path = filepath.Join(p.Directory, path)
			}
			values, err := readEnvironment(path)
			if err != nil {
				return fmt.Errorf("read local environment for %s: %w", svc.ID, err)
			}
			env = values
		}
		for key, value := range svc.Environment {
			env[key] = value
		}
		for _, key := range svc.RequiredEnvironment {
			if _, ok := env[key]; !ok {
				return fmt.Errorf("complete local environment variable %s for service %s", key, svc.ID)
			}
		}
	}
	return nil
}

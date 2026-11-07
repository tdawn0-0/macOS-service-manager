package devplatform

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"syscall"
)

type FolderPathChange struct{ Service, Field, From, To string }
type ProjectFolderPreview struct {
	ProjectID, ProjectName string
	From, To               string
	Changes                []FolderPathChange
	Warnings               []string
	Fingerprint            string
}

func projectFolderError(dir string) error {
	info, err := os.Stat(dir)
	if err != nil || !info.IsDir() {
		return errors.New("project folder is unavailable; choose its current location in project settings")
	}
	return nil
}

// Use lexical project-relative paths: a project may already have moved, and
// symlinks inside it should retain their relative meaning at the new location.
func relocatedPath(from, to, value string) (string, bool) {
	if !filepath.IsAbs(value) {
		return value, false
	}
	rel, err := filepath.Rel(filepath.Clean(from), filepath.Clean(value))
	if err != nil || rel != "." && !safeRelative(rel) {
		return value, false
	}
	return filepath.Join(to, rel), true
}
func folderCandidate(root string, old, draft Project) (Project, ProjectFolderPreview, error) {
	to, err := canonicalProjectDirectory(draft.Directory)
	if err != nil {
		return Project{}, ProjectFolderPreview{}, fmt.Errorf("choose an existing working folder: %w", err)
	}
	p := draft.Clone()
	p.Directory = to
	preview := ProjectFolderPreview{ProjectID: p.ID, ProjectName: p.Name, From: old.Directory, To: to}
	change := func(service, field, value string) string {
		updated, ok := relocatedPath(old.Directory, to, value)
		if ok && updated != value {
			preview.Changes = append(preview.Changes, FolderPathChange{service, field, value, updated})
		}
		return updated
	}
	for n := range p.Services {
		svc := &p.Services[n]
		svc.EnvFile = change(svc.ID, "Environment file", svc.EnvFile)
		local := map[int]LocalArgument{}
		for j := range svc.LocalArguments {
			arg := &svc.LocalArguments[j]
			if arg.Kind == "path" || arg.Kind == "executable" {
				arg.Value = change(svc.ID, fmt.Sprintf("Local argument %d", arg.Index+1), arg.Value)
				if arg.Value != "" {
					svc.Command[arg.Index] = arg.Prefix + arg.Value
				}
			}
			local[arg.Index] = *arg
		}
		for j, value := range svc.Command {
			if _, ok := local[j]; ok {
				continue
			}
			if j > 0 && sensitiveFlag(svc.Command[j-1]) {
				continue
			}
			if strings.HasPrefix(value, "-") && sensitiveFlag(value) {
				continue
			}
			prefix, path := "", value
			if key, tail, ok := strings.Cut(value, "="); ok && strings.HasPrefix(key, "-") {
				prefix, path = key+"=", tail
			}
			svc.Command[j] = prefix + change(svc.ID, fmt.Sprintf("Command item %d", j+1), path)
		}
		for j, value := range svc.Command {
			if value == draft.Services[n].Command[j] && strings.Contains(value, old.Directory) && old.Directory != to {
				preview.Warnings = append(preview.Warnings, fmt.Sprintf("%s: review command item %d in Services; its value still refers to the previous folder", svc.ID, j+1))
			}
		}
		for _, key := range diagnosticKeys(svc.Environment) {
			if strings.Contains(svc.Environment[key], old.Directory) {
				preview.Warnings = append(preview.Warnings, svc.ID+": review environment variable "+key+"; its value still refers to the previous folder")
			}
		}
		if svc.Directory != "" {
			if err := projectFolderError(filepath.Join(to, svc.Directory)); err != nil {
				preview.Warnings = append(preview.Warnings, svc.ID+": service subfolder is unavailable at the new location")
			}
		}
	}
	if err = p.Validate(); err != nil {
		return Project{}, preview, err
	}
	info, err := os.Stat(to)
	if err != nil {
		return Project{}, preview, err
	}
	identity := ""
	if stat, ok := info.Sys().(*syscall.Stat_t); ok {
		identity = fmt.Sprintf("%d:%d", stat.Dev, stat.Ino)
	}
	b, err := json.Marshal(struct {
		Old, Next Project
		Root      string
		Identity  string
	}{old, p, filepath.Clean(root), identity})
	if err != nil {
		return Project{}, preview, err
	}
	preview.Fingerprint = digestBytes(b)
	return p, preview, nil
}

func (m *Manager) requireProjectStopped(ctx context.Context, p Project) error {
	targets := []string{"project-" + p.ID}
	for _, svc := range p.Services {
		targets = append(targets, "p-"+p.ID+"-"+svc.ID)
	}
	for _, target := range targets {
		if err := m.verifyShutdownTarget(ctx, domain()+"/"+m.label(target)); err != nil {
			return fmt.Errorf("stop the project before changing its folder; could not verify its jobs are stopped: %w", err)
		}
	}
	return nil
}
func (m *Manager) PreviewProjectFolder(ctx context.Context, draft Project) (ProjectFolderPreview, error) {
	if err := draft.Validate(); err != nil {
		return ProjectFolderPreview{}, err
	}
	s, err := m.Read()
	if err != nil {
		return ProjectFolderPreview{}, err
	}
	old, ok := s.Projects[draft.ID]
	if !ok {
		return ProjectFolderPreview{}, errors.New("project not found")
	}
	if err = m.requireProjectStopped(ctx, old); err != nil {
		return ProjectFolderPreview{}, err
	}
	_, preview, err := folderCandidate(m.Root, old, draft)
	return preview, err
}
func (m *Manager) ApplyProjectFolder(ctx context.Context, draft Project, fingerprint string) error {
	if err := draft.Validate(); err != nil {
		return err
	}
	keys := append(projectResourceKeys(draft.ID), instanceResourceKeys(draft.Instances)...)
	release, err := m.resourceLeases(ctx, keys...)
	if err != nil {
		return err
	}
	defer release()
	return m.withStateTransaction(ctx, func(s *State) (func() error, func(), error) {
		old, ok := s.Projects[draft.ID]
		if !ok {
			return nil, nil, errors.New("project not found")
		}
		p, preview, err := folderCandidate(m.Root, old, draft)
		if err != nil {
			return nil, nil, err
		}
		if fingerprint == "" || preview.Fingerprint != fingerprint {
			return nil, nil, errors.New("project or destination folder changed; review the folder change again")
		}
		if err = m.requireProjectStopped(ctx, old); err != nil {
			return nil, nil, err
		}
		s.Projects[p.ID] = p
		if err = validateSites(allSites(s)); err != nil {
			return nil, nil, err
		}
		// The login plist and state must agree if cancellation or saving fails.
		undo, err := m.snapshotLoginConfiguration(p.ID)
		if err != nil {
			return nil, nil, err
		}
		err = m.configureLogin(ctx, p)
		return undo, nil, err
	})
}

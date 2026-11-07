package devcli

import (
	"bytes"
	"context"
	"path/filepath"
	"strings"
	"testing"

	"macos-service-manager/internal/devplatform"
)

func TestProjectFolderCLIUsesPreviewAndRetainsIdentity(t *testing.T) {
	t.Setenv("MSM_DEV_HOME", t.TempDir())
	m := devplatform.New(devplatform.DefaultRoot())
	p := devplatform.Project{SchemaVersion: 1, ID: "cli-folder", Name: "CLI folder", Directory: t.TempDir()}
	if err := m.SaveProject(context.Background(), p); err != nil {
		t.Fatal(err)
	}
	to := t.TempDir()
	var out bytes.Buffer
	if err := Run(context.Background(), []string{"project-folder", p.ID, to}, &out); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "Fingerprint:") {
		t.Fatal("CLI omitted confirmation token", out.String())
	}
	if err := Run(context.Background(), []string{"project-folder-apply", p.ID, to, "old"}, &out); err == nil {
		t.Fatal("CLI accepted invalid preview")
	}
	draft := p.Clone()
	draft.Directory = to
	preview, err := m.PreviewProjectFolder(context.Background(), draft)
	if err != nil {
		t.Fatal(err)
	}
	if err = Run(context.Background(), []string{"project-folder-apply", p.ID, to, preview.Fingerprint}, &out); err != nil {
		t.Fatal(err)
	}
	s, err := m.Read()
	if err != nil {
		t.Fatal(err)
	}
	expected, err := filepath.EvalSymlinks(to)
	if err != nil {
		t.Fatal(err)
	}
	if s.Projects[p.ID].Directory != expected || len(s.Projects) != 1 {
		t.Fatal("CLI relocation imported a new project", s.Projects)
	}
}

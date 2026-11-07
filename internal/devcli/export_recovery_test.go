package devcli

import (
	"bytes"
	"context"
	"errors"
	"strings"
	"testing"

	"macos-service-manager/internal/devplatform"
)

func TestExportRecoveryCLIRequiresReviewedFingerprint(t *testing.T) {
	root := t.TempDir()
	t.Setenv("MSM_DEV_HOME", root)
	m := devplatform.New(root)
	p := devplatform.Project{SchemaVersion: 1, ID: "cli-export", Name: "CLI recovery", Directory: t.TempDir()}
	if err := m.SaveProject(context.Background(), p); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	m.Event = func(message string) {
		if strings.HasSuffix(message, "Prepared") {
			cancel()
		}
	}
	if err := m.ExportProject(ctx, p.ID); !errors.Is(err, context.Canceled) {
		t.Fatal(err)
	}
	var out bytes.Buffer
	if err := Run(context.Background(), []string{"export-recovery", p.Directory}, &out); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "Fingerprint:") || !strings.Contains(out.String(), "Can resume: true") {
		t.Fatal("CLI omitted review details", out.String())
	}
	if err := Run(context.Background(), []string{"export-recover", p.Directory, "stale"}, &out); err == nil {
		t.Fatal("stale CLI review accepted")
	}
	preview, err := m.InspectExportRecovery(context.Background(), p.Directory)
	if err != nil {
		t.Fatal(err)
	}
	if err = Run(context.Background(), []string{"export-recover", p.Directory, preview.Fingerprint}, &out); err != nil {
		t.Fatal(err)
	}
	if _, err = m.Import(context.Background(), p.Directory); err != nil {
		t.Fatal("CLI recovery did not publish importable environment", err)
	}
}

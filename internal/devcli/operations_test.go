package devcli

import (
	"bytes"
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"macos-service-manager/internal/devplatform"
)

func TestCLITasksExposeReviewedRevisionAndDismissKeepsResources(t *testing.T) {
	root := filepath.Join(t.TempDir(), "new-root")
	t.Setenv("MSM_DEV_HOME", root)
	ctx := context.Background()
	var out bytes.Buffer
	if e := Run(ctx, []string{"tasks"}, &out); e != nil {
		t.Fatal(e)
	}
	if _, e := os.Stat(root); !os.IsNotExist(e) {
		t.Fatal("read-only task inspection created storage")
	}
	m := devplatform.New(root)
	if e := m.Up(ctx, "missing"); e == nil {
		t.Fatal("expected missing project")
	}
	out.Reset()
	if e := Run(ctx, []string{"tasks"}, &out); e != nil {
		t.Fatal(e)
	}
	var records []devplatform.OperationRecord
	if e := json.Unmarshal(out.Bytes(), &records); e != nil || len(records) != 1 || records[0].Revision == "" {
		t.Fatal(out.String(), e)
	}
	r := records[0]
	if e := Run(ctx, []string{"task-dismiss", r.ID, "stale"}, &out); e == nil {
		t.Fatal("accepted stale task preview")
	}
	sentinel := filepath.Join(root, "instances", "keep")
	if e := os.MkdirAll(sentinel, 0700); e != nil {
		t.Fatal(e)
	}
	if e := Run(ctx, []string{"task-dismiss", r.ID, r.Revision}, &out); e != nil {
		t.Fatal(e)
	}
	if _, e := os.Stat(sentinel); e != nil {
		t.Fatal("dismiss removed resource", e)
	}
}

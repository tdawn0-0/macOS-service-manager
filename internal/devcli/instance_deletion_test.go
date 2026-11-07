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

func TestCLIInstanceDeletionRequiresReviewedPathAndExactConfirmation(t *testing.T) {
	root := t.TempDir()
	t.Setenv("MSM_DEV_HOME", root)
	id := "cli-delete"
	dir := filepath.Join(root, "instances", id)
	if e := os.MkdirAll(dir, 0700); e != nil {
		t.Fatal(e)
	}
	os.WriteFile(filepath.Join(dir, "value"), []byte("42"), 0600)
	s := devplatform.State{SchemaVersion: devplatform.Schema, Projects: map[string]devplatform.Project{}, Packages: map[string]devplatform.Package{}, Sites: map[string]devplatform.Site{}, Instances: map[string]devplatform.Instance{id: {ID: id, Kind: "redis", Package: "redis@test-universal", Port: 56379}}}
	b, _ := json.Marshal(s)
	if e := os.WriteFile(filepath.Join(root, "state.json"), b, 0600); e != nil {
		t.Fatal(e)
	}
	var out bytes.Buffer
	ctx := context.Background()
	if e := Run(ctx, []string{"db-delete-preview", id}, &out); e != nil {
		t.Fatal(e)
	}
	var p devplatform.InstanceDeletionPreview
	if e := json.Unmarshal(out.Bytes(), &p); e != nil || !p.CanDelete || p.Path != dir {
		t.Fatal(out.String(), e)
	}
	if e := Run(ctx, []string{"db-delete-data", id, p.Fingerprint, "wrong"}, &out); e == nil {
		t.Fatal("accepted wrong confirmation")
	}
	if e := Run(ctx, []string{"db-delete-data", id, p.Fingerprint, id}, &out); e != nil {
		t.Fatal(e)
	}
	if _, e := os.Stat(dir); !os.IsNotExist(e) {
		t.Fatal(e)
	}
	out.Reset()
	if e := Run(ctx, []string{"db-deletions"}, &out); e != nil {
		t.Fatal(e)
	}
	var pending []devplatform.InstanceDeletionInfo
	if e := json.Unmarshal(out.Bytes(), &pending); e != nil || len(pending) > 0 {
		t.Fatal(out.String(), e)
	}
}

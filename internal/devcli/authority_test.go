package devcli

import (
	"bytes"
	"context"
	"encoding/json"
	"strings"
	"testing"

	"macos-service-manager/internal/devplatform"
)

func TestAuthorityCLIStatusPreparationAndDiscardDoNotTrustOrActivate(t *testing.T) {
	t.Setenv("MSM_DEV_HOME", t.TempDir())
	var out bytes.Buffer
	if err := Run(context.Background(), []string{"ca-info"}, &out); err != nil {
		t.Fatal(err)
	}
	var status devplatform.AuthorityStatus
	if err := json.Unmarshal(out.Bytes(), &status); err != nil || status.Active.Status != "Not created" {
		t.Fatal("CLI inspection created a CA", err, out.String())
	}
	if err := Run(context.Background(), []string{"ca-prepare"}, &out); err != nil {
		t.Fatal(err)
	}
	out.Reset()
	if err := Run(context.Background(), []string{"ca-info"}, &out); err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(out.Bytes(), &status); err != nil || status.Prepared == nil || status.Active.Status != "Not created" || strings.Contains(out.String(), "PRIVATE KEY") {
		t.Fatal("unsafe CLI preparation output", err)
	}
	if err := Run(context.Background(), []string{"ca-discard", "stale"}, &out); err == nil {
		t.Fatal("CLI discarded an unreviewed authority")
	}
	if err := Run(context.Background(), []string{"ca-discard", status.Revision}, &out); err != nil {
		t.Fatal(err)
	}
	m := devplatform.New(devplatform.DefaultRoot())
	fresh, err := m.AuthorityStatus(context.Background())
	if err != nil || fresh.Prepared != nil || fresh.Active.Status != "Not created" {
		t.Fatal("discard changed active authority", fresh, err)
	}
}

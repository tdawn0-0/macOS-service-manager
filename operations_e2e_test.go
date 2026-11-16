package main

import (
	"context"
	"testing"

	"devbase/internal/devplatform"
)

func TestUITaskHistoryInitializationRequiresNewTargetReview(t *testing.T) {
	h := newDevUIHarnessSize(t, 900, 520, true)
	m := h.a.dev.managerFactory()
	if e := m.CreateInstance(context.Background(), devplatform.Instance{ID: "old-target", Kind: "redis", Package: "redis@8.2.11-arm64", Port: 0}); e == nil {
		t.Fatal("expected fixture initialization validation failure")
	}
	h.click("Tasks")
	records, e := m.Operations(context.Background())
	if e != nil || len(records) != 1 {
		t.Fatal(records, e)
	}
	h.click("Task " + records[0].ID)
	h.click("Retry operation")
	again, _ := m.Operations(context.Background())
	if len(again) != 1 {
		t.Fatal("initialization replayed automatically")
	}
	h.click("Review resource settings")
	if h.a.dev.operationsOpen || h.a.dev.tab != devTabDatabases || h.a.dev.dbID != "" || h.a.dev.dbKind != "redis" {
		t.Fatal("new instance review missing", h.a.dev.dbID, h.a.dev.dbKind)
	}
}

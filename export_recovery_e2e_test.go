package main

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"macos-service-manager/internal/devplatform"
)

func interruptUIExportFixture(t *testing.T, m *devplatform.Manager, id string) {
	t.Helper()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	m.Event = func(message string) {
		if strings.HasSuffix(message, "Saved devmanager.lock.json") {
			cancel()
		}
	}
	err := m.ExportProject(ctx, id)
	m.Event = func(string) {}
	if !errors.Is(err, context.Canceled) {
		t.Fatal("did not interrupt real export", err)
	}
}

func TestUIExportRecoveryCompletesOrKeepsChangedFiles(t *testing.T) {
	for _, dark := range []bool{false, true} {
		t.Run(fmt.Sprintf("minimum-dark-%t", dark), func(t *testing.T) {
			h := newDevUIHarnessSize(t, 900, 520, dark)
			dir := t.TempDir()
			h.fill("Project folder", dir)
			h.click("Import project")
			sourceID := h.a.dev.id
			h.fill("Project name", "Previous UI environment")
			h.click("Save project")
			h.click("Export environment")
			if strings.Contains(h.a.dev.output, "Error:") {
				t.Fatal(h.a.dev.output)
			}
			h.fill("Project name", "Prepared UI environment")
			h.click("Save project")
			m := h.a.dev.managerFactory()
			interruptUIExportFixture(t, m, sourceID)
			// Reopen via the UI rather than relying on the exporting call's state.
			h.click("Import a project")
			h.fill("Project folder", dir)
			h.click("Import project")
			if !strings.Contains(h.a.dev.output, "unfinished") || h.a.dev.exportRecovery == nil {
				t.Fatal("UI did not surface pending export", h.a.dev.output)
			}
			operation := h.a.dev.exportRecovery.OperationID
			h.click("Complete interrupted export")
			if h.a.dev.exportRecovery != nil || strings.Contains(h.a.dev.output, "Error:") {
				t.Fatal("UI recovery failed", operation, h.a.dev.output)
			}
			h.click("Import project")
			if h.a.dev.importing || h.a.dev.id == sourceID || h.a.dev.draft.Name != "Prepared UI environment" {
				t.Fatal("could not import recovered snapshot", h.a.dev.output)
			}
			// Another interrupted export, followed by a real outside file change.
			state, err := m.Read()
			if err != nil {
				t.Fatal(err)
			}
			p := state.Projects[sourceID]
			p.Name = "Conflict prepared"
			if err = m.SaveProject(context.Background(), p); err != nil {
				t.Fatal(err)
			}
			interruptUIExportFixture(t, m, sourceID)
			path := filepath.Join(dir, "devmanager.json")
			before, err := os.ReadFile(path)
			if err != nil {
				t.Fatal(err)
			}
			changed := append(append([]byte{}, before...), []byte("\n\n")...)
			if err = os.WriteFile(path, changed, 0600); err != nil {
				t.Fatal(err)
			}
			h.click("Import a project")
			h.fill("Project folder", dir)
			h.click("Import project")
			if h.a.dev.exportRecovery == nil || h.a.dev.exportRecovery.CanResume {
				t.Fatal("changed files not protected", h.a.dev.output)
			}
			h.click("Complete interrupted export")
			if h.a.dev.exportRecovery == nil {
				t.Fatal("disabled completion discarded record")
			}
			h.click("Discard export record (keep files)")
			after, err := os.ReadFile(path)
			if err != nil || !bytes.Equal(after, changed) {
				t.Fatal("UI discard changed file", err)
			}
			if h.a.dev.exportRecovery != nil {
				t.Fatal("UI did not refresh after discard")
			}
			h.click("Project Conflict prepared")
			h.click("Export environment")
			if strings.Contains(h.a.dev.output, "Error:") {
				t.Fatal("UI could not retry export", h.a.dev.output)
			}
			if _, err = devplatform.New(h.root).Import(context.Background(), dir); err != nil {
				t.Fatal("retry did not publish a valid pair", err)
			}
		})
	}
}

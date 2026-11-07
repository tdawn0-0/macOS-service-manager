package devplatform

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func putProjectFile(t *testing.T, dir, name, value string) {
	t.Helper()
	if e := os.MkdirAll(filepath.Dir(filepath.Join(dir, name)), 0700); e != nil {
		t.Fatal(e)
	}
	if e := os.WriteFile(filepath.Join(dir, name), []byte(value), 0600); e != nil {
		t.Fatal(e)
	}
}

func TestProjectVersionDeclarationsRetainSourcesAndConflicts(t *testing.T) {
	dir := t.TempDir()
	putProjectFile(t, dir, ".nvmrc", "# exact version\nv22.14.0\n")
	putProjectFile(t, dir, ".node-version", "20.19.0\n")
	putProjectFile(t, dir, "package.json", `{"scripts":{"dev":"touch SHOULD_NOT_RUN"},"engines":{"node":">=20"},"volta":{"node":"22.14.0"}}`)
	inspection, e := InspectProject(dir)
	if e != nil {
		t.Fatal(e)
	}
	if len(inspection.RuntimeHints) != 4 || inspection.RuntimeHints[0].Source != ".nvmrc" || inspection.RuntimeHints[0].ExactVersion() != "22.14.0" || inspection.RuntimeHints[3].ExactVersion() != "" {
		t.Fatal(inspection)
	}
	conflicts := RuntimeConflicts(inspection.RuntimeHints)
	if len(conflicts) != 1 || conflicts[0] != "node" {
		t.Fatal("exact version conflict was hidden", conflicts)
	}
	m := testManager(t)
	m.Run = func(context.Context, string, ...string) (string, error) {
		t.Fatal("import executed a command")
		return "", nil
	}
	p, e := m.Import(context.Background(), dir)
	if e != nil || len(p.RuntimeHints) != 4 || len(p.Services) != 1 || p.Services[0].Package != "" || strings.Join(p.Services[0].Command, " ") != "npm run dev" {
		t.Fatal("import chose a version or lost the declarations", p, e)
	}
	if _, e = os.Stat(filepath.Join(dir, "SHOULD_NOT_RUN")); !os.IsNotExist(e) {
		t.Fatal("project scripts ran during inspection")
	}
	if e = m.ExportProject(context.Background(), p.ID); e != nil {
		t.Fatal(e)
	}
	b, _ := os.ReadFile(filepath.Join(dir, "devmanager.json"))
	if strings.Contains(string(b), "runtimeHints") || strings.Contains(string(b), "detectionWarnings") {
		t.Fatal("local inspection metadata entered the shared declaration")
	}
	// A shared declaration has priority over newly discovered project scripts.
	p.Services = []Service{{ID: "chosen", Command: []string{"/usr/bin/true"}}}
	p.Directory = "."
	if e = writeJSON(filepath.Join(dir, "devmanager.json"), p); e != nil {
		t.Fatal(e)
	}
	imported, e := m.Import(context.Background(), dir)
	if e != nil || imported.Services[0].ID != "chosen" || len(imported.RuntimeHints) != 4 {
		t.Fatal("import overwrote an explicit service", imported, e)
	}
}

func TestPHPInspectionUsesPublicRootAndDoesNotRunComposerScripts(t *testing.T) {
	dir := t.TempDir()
	putProjectFile(t, dir, ".php-version", "8.5.12")
	putProjectFile(t, dir, "composer.json", `{"require":{"php":"^8.2"},"scripts":{"post-install-cmd":"touch SHOULD_NOT_RUN"}}`)
	putProjectFile(t, dir, "public/index.php", "<?php echo 'fixture';")
	// Frontend metadata without a dev/start script is not a fabricated npm service.
	putProjectFile(t, dir, "package.json", `{"name":"frontend-assets"}`)
	inspection, e := InspectProject(dir)
	if e != nil || len(inspection.Services) != 1 || strings.Join(inspection.Services[0].Command, " ") != "php -S 127.0.0.1:8000 -t public" || len(inspection.RuntimeHints) != 2 {
		t.Fatal(inspection, e)
	}
	if len(RuntimeConflicts(inspection.RuntimeHints)) != 0 {
		t.Fatal("a range was incorrectly treated as an exact conflict")
	}
	if _, e = os.Stat(filepath.Join(dir, "SHOULD_NOT_RUN")); !os.IsNotExist(e) {
		t.Fatal("composer scripts ran")
	}
}

func TestInspectionLimitsFilesAndNeverEvaluatesVersionExpressions(t *testing.T) {
	dir := t.TempDir()
	putProjectFile(t, dir, ".nvmrc", "$(touch SHOULD_NOT_RUN)")
	putProjectFile(t, dir, "package.json", `{"name":"library","engines":{"node":">=20"}}`)
	inspection, e := InspectProject(dir)
	if e != nil || len(inspection.Services) != 0 || inspection.RuntimeHints[0].ExactVersion() != "" {
		t.Fatal(inspection, e)
	}
	if _, e = os.Stat(filepath.Join(dir, "SHOULD_NOT_RUN")); !os.IsNotExist(e) {
		t.Fatal("evaluated a version expression")
	}
	putProjectFile(t, dir, ".nvmrc", strings.Repeat("2", 1025))
	if _, e = InspectProject(dir); e == nil {
		t.Fatal("read an oversized declaration")
	}
	if e = os.Remove(filepath.Join(dir, ".nvmrc")); e != nil {
		t.Fatal(e)
	}
	outside := filepath.Join(t.TempDir(), "private-version")
	putProjectFile(t, filepath.Dir(outside), filepath.Base(outside), "22.14.0")
	if e = os.Symlink(outside, filepath.Join(dir, ".nvmrc")); e != nil {
		t.Fatal(e)
	}
	if _, e = InspectProject(dir); e == nil || !strings.Contains(e.Error(), "outside") {
		t.Fatal("followed a declaration outside the project", e)
	}
	m := testManager(t)
	if _, e = m.Import(context.Background(), ""); e == nil {
		t.Fatal("empty folder silently imported the app working directory")
	}
}

func TestNodeAliasesAndRangesRequireAnExplicitExactVersion(t *testing.T) {
	for _, version := range []string{"22", "latest", "lts/*", ">=20"} {
		if _, e := Resolve(context.Background(), "node", version); e == nil || !strings.Contains(e.Error(), "exact") {
			t.Fatal("catalog implicitly resolved an alias or range", version, e)
		}
	}
}

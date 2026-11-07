package devplatform

import (
	"context"
	"os"
	"path/filepath"
	"testing"
)

func TestRuntimeTerminalSelection(t *testing.T) {
	m := New(t.TempDir())
	installedNodeFixture(t, m, "22.14.0", "20.19.0")
	if _, err := m.SelectNode(t.TempDir(), ""); err != ErrNoNodeDefault {
		t.Fatalf("missing default: %v", err)
	}
	if err := m.SetNodeDefault(context.Background(), "node@22.14.0-arm64"); err != nil {
		t.Fatal(err)
	}
	dir := filepath.Join(t.TempDir(), "app", "sub")
	if err := os.MkdirAll(dir, 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(filepath.Dir(dir), ".nvmrc"), []byte("20\n"), 0600); err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct{ override, want string }{{"", "20.19.0"}, {"22", "22.14.0"}} {
		got, err := m.SelectNode(dir, tc.override)
		if err != nil || got.Version != tc.want {
			t.Fatalf("override %q: %+v %v", tc.override, got, err)
		}
	}
	if err := os.WriteFile(filepath.Join(dir, ".node-version"), []byte("system\n"), 0600); err != nil {
		t.Fatal(err)
	}
	got, err := m.SelectNode(dir, "")
	if err != nil || !got.System {
		t.Fatalf("system selection: %+v %v", got, err)
	}
	if err := m.SetNodeDefault(context.Background(), "mysql@8.4.4-arm64"); err == nil {
		t.Fatal("wrong software accepted")
	}
	state, err := m.Read()
	if err != nil || state.NodeDefault != "node@22.14.0-arm64" {
		t.Fatalf("default changed: %+v %v", state, err)
	}
}

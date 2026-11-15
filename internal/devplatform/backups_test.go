package devplatform

import (
	"context"
	"os"
	"path/filepath"
	"testing"
	"time"
)

func backupFixture(t *testing.T) (*Manager, string) {
	t.Helper()
	m := testManager(t)
	err := m.withState(context.Background(), func(s *State) error {
		s.Instances["app"] = Instance{ID: "app", Kind: "redis", Port: 6401}
		s.Instances["app-db"] = Instance{ID: "app-db", Kind: "redis", Port: 6402}
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	dir := filepath.Join(m.Root, "backups")
	if err := os.MkdirAll(dir, 0700); err != nil {
		t.Fatal(err)
	}
	return m, dir
}

func writeBackup(t *testing.T, dir, name string) {
	t.Helper()
	if err := os.WriteFile(filepath.Join(dir, name), []byte("data"), 0600); err != nil {
		t.Fatal(err)
	}
}

func TestBackupsListsOnlyFinishedFilesOfTheInstance(t *testing.T) {
	m, dir := backupFixture(t)
	for _, n := range []string{"app-main-0123456789abcdef.dump", "app-db-main-0123456789abcdef.dump", "app-main-0123456789abcdef.dump.partial", "app-main-XYZ.dump", "other-main-0123456789abcdef.dump"} {
		writeBackup(t, dir, n)
	}
	target := filepath.Join(t.TempDir(), "t")
	if err := os.WriteFile(target, []byte("x"), 0600); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(target, filepath.Join(dir, "app-link-0123456789abcdef.dump")); err != nil {
		t.Fatal(err)
	}
	for id, want := range map[string]string{"app": "app-main-0123456789abcdef.dump", "app-db": "app-db-main-0123456789abcdef.dump"} {
		got, err := m.Backups(context.Background(), id)
		if err != nil {
			t.Fatal(err)
		}
		if len(got) != 1 || got[0].Name != want || got[0].Database != "main" {
			t.Fatalf("%s: %+v", id, got)
		}
	}
}

func TestBackupsNewestFirst(t *testing.T) {
	m, dir := backupFixture(t)
	a, b := "app-a-0123456789abcdef.dump", "app-b-0123456789abcdef.dump"
	writeBackup(t, dir, a)
	writeBackup(t, dir, b)
	now := time.Now()
	os.Chtimes(filepath.Join(dir, a), now, now.Add(-time.Hour))
	os.Chtimes(filepath.Join(dir, b), now, now)
	got, err := m.Backups(context.Background(), "app")
	if err != nil || len(got) != 2 || got[0].Name != b {
		t.Fatalf("%+v %v", got, err)
	}
}

func TestBackupsUnknownInstance(t *testing.T) {
	m, _ := backupFixture(t)
	if _, err := m.Backups(context.Background(), "nope"); err == nil {
		t.Fatal("expected error")
	}
}

func TestDeleteBackupRemovesOnlyTheNamedFile(t *testing.T) {
	m, dir := backupFixture(t)
	a, b := "app-a-0123456789abcdef.dump", "app-b-0123456789abcdef.dump"
	writeBackup(t, dir, a)
	writeBackup(t, dir, b)
	if err := m.DeleteBackup(context.Background(), "app", a); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Lstat(filepath.Join(dir, a)); !os.IsNotExist(err) {
		t.Fatal("not removed")
	}
	if _, err := os.Lstat(filepath.Join(dir, b)); err != nil {
		t.Fatal(err)
	}
	if got, _ := m.Backups(context.Background(), "app"); len(got) != 1 {
		t.Fatalf("%+v", got)
	}
}

func TestDeleteBackupRejectsForeignAndUnsafeNames(t *testing.T) {
	m, dir := backupFixture(t)
	files := []string{"other-main-0123456789abcdef.dump", "app-db-main-0123456789abcdef.dump", "app-main-0123456789abcdef.dump.partial"}
	for _, n := range files {
		writeBackup(t, dir, n)
	}
	target := filepath.Join(t.TempDir(), "t")
	os.WriteFile(target, []byte("x"), 0600)
	link := "app-link-0123456789abcdef.dump"
	if err := os.Symlink(target, filepath.Join(dir, link)); err != nil {
		t.Fatal(err)
	}
	files = append(files, link)
	state := filepath.Join(m.Root, "state.json")
	if _, err := os.Lstat(state); err != nil {
		t.Fatal(err)
	}
	for _, n := range append([]string{"../state.json", "/etc/hosts"}, files...) {
		if err := m.DeleteBackup(context.Background(), "app", n); err == nil {
			t.Fatalf("%q was accepted", n)
		}
	}
	for _, n := range files {
		if _, err := os.Lstat(filepath.Join(dir, n)); err != nil {
			t.Fatalf("%s: %v", n, err)
		}
	}
	if _, err := os.Lstat(state); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Lstat(target); err != nil {
		t.Fatal(err)
	}
}

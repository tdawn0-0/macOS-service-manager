package logs

import (
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
)

func write(t *testing.T, content string) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), "x.log")
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
	return path
}

func TestTailLastLines(t *testing.T) {
	snap, err := Tail(write(t, "a\nb\r\nc\nd\n"), 1<<20, 3)
	if err != nil {
		t.Fatal(err)
	}
	if !slices.Equal(snap.Lines, []string{"b", "c", "d"}) || !snap.Truncated {
		t.Errorf("lines = %q truncated = %v", snap.Lines, snap.Truncated)
	}
}

func TestTailDropsCutFirstLine(t *testing.T) {
	content := strings.Repeat("x", 50) + "\nlast one\nlast two\n"
	snap, err := Tail(write(t, content), 25, 100)
	if err != nil {
		t.Fatal(err)
	}
	if !slices.Equal(snap.Lines, []string{"last one", "last two"}) || !snap.Truncated {
		t.Errorf("lines = %q truncated = %v", snap.Lines, snap.Truncated)
	}
}

func TestTailEmptyAndMissing(t *testing.T) {
	snap, err := Tail(write(t, ""), 100, 10)
	if err != nil || len(snap.Lines) != 0 {
		t.Errorf("empty: %v %q", err, snap.Lines)
	}
	if _, err := Tail(filepath.Join(t.TempDir(), "none"), 100, 10); err == nil {
		t.Error("a missing file should fail")
	}
	size, _, err := Stat(write(t, "abc"))
	if err != nil || size != 3 {
		t.Errorf("Stat = %d, %v", size, err)
	}
}

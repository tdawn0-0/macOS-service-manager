package devplatform

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestManagedLogsAreBoundedAndRejectTraversal(t *testing.T) {
	m := New(t.TempDir())
	if e := m.init(); e != nil {
		t.Fatal(e)
	}
	id := "p-1234567890-" + strings.Repeat("x", 80)
	if e := os.WriteFile(filepath.Join(m.Root, "logs", id+".log"), []byte("old-line\n"+strings.Repeat("line\n", 100000)+"last-line\n"), 0600); e != nil {
		t.Fatal(e)
	}
	text, e := m.Logs(id)
	if e != nil || !strings.Contains(text, "last-line") || strings.Contains(text, "old-line") || len(text) > 66<<10 {
		t.Fatalf("tail failed: %v (%d bytes)", e, len(text))
	}
	if _, e = m.Logs("../state"); e == nil {
		t.Fatal("accepted traversal")
	}
}

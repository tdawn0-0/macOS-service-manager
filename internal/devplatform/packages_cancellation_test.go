package devplatform

import (
	"archive/tar"
	"compress/gzip"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

func TestCancelledExtractionDoesNotCommitAndCanRetry(t *testing.T) {
	m := testManager(t)
	source := filepath.Join(t.TempDir(), "package.tar.gz")
	f, e := os.Create(source)
	if e != nil {
		t.Fatal(e)
	}
	gz := gzip.NewWriter(f)
	tw := tar.NewWriter(gz)
	data := []byte("binary fixture")
	if e = tw.WriteHeader(&tar.Header{Name: "bin/worker", Mode: 0755, Size: int64(len(data))}); e != nil {
		t.Fatal(e)
	}
	if _, e = tw.Write(data); e != nil {
		t.Fatal(e)
	}
	if e = tw.Close(); e != nil {
		t.Fatal(e)
	}
	if e = gz.Close(); e != nil {
		t.Fatal(e)
	}
	if e = f.Close(); e != nil {
		t.Fatal(e)
	}
	b, e := os.ReadFile(source)
	if e != nil {
		t.Fatal(e)
	}
	sum := sha256.Sum256(b)
	pkg := Package{Name: "fixture", Version: "1", Arch: runtime.GOARCH, URL: "https://unused.invalid/package", SHA256: hex.EncodeToString(sum[:]), Format: "tar.gz", Binaries: map[string]string{"worker": "bin/worker"}}
	if e = os.WriteFile(filepath.Join(m.Root, "downloads", pkg.SHA256), b, 0600); e != nil {
		t.Fatal(e)
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	m.Event = func(message string) {
		if strings.Contains(message, "extracting") {
			cancel()
		}
	}
	if e = m.Install(ctx, pkg); !errors.Is(e, context.Canceled) {
		t.Fatal("extract cancellation was ignored", e)
	}
	s, _ := m.Read()
	entries, _ := os.ReadDir(filepath.Join(m.Root, "packages"))
	if len(s.Packages) != 0 || len(entries) != 0 {
		t.Fatal("cancelled install left a registered or partially extracted package")
	}
	m.Event = nil
	if e = m.Install(context.Background(), pkg); e != nil {
		t.Fatal("cancelled install could not be retried", e)
	}
}

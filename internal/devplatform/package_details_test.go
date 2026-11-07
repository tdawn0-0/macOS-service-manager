package devplatform

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"sync/atomic"
	"testing"
)

func TestPackagePreviewUsesVerifiedCacheAndChecksSystemBeforeInstall(t *testing.T) {
	m := testManager(t)
	data := []byte("verified archive fixture")
	sum := sha256.Sum256(data)
	p := Package{Name: "fixture", Version: "1.0.0", Arch: runtime.GOARCH, URL: "https://unused.invalid/package", SHA256: hex.EncodeToString(sum[:]), Format: "zip", MinMacOS: "15.0", Binaries: map[string]string{"fixture": "bin/fixture"}}
	if e := os.WriteFile(filepath.Join(m.Root, "downloads", p.SHA256), data, 0600); e != nil {
		t.Fatal(e)
	}
	m.Run = func(context.Context, string, ...string) (string, error) { return "14.5", nil }
	info, e := m.PackageDetails(context.Background(), p)
	if e != nil || !info.Cached || info.ArchiveBytes != int64(len(data)) || info.Compatible || !strings.Contains(info.Compatibility, "15.0") {
		t.Fatal(info, e)
	}
	entries, _ := os.ReadDir(filepath.Join(m.Root, "packages"))
	if len(entries) > 0 {
		t.Fatal("preview installed or ran a package")
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if _, e = m.PackageDetails(ctx, p); !errors.Is(e, context.Canceled) {
		t.Fatal("metadata cancellation was ignored", e)
	}
	if e = m.withState(context.Background(), func(s *State) error { s.Packages[p.ID()] = p; return nil }); e != nil {
		t.Fatal(e)
	}
	if e = m.Install(context.Background(), p); e == nil || !strings.Contains(e.Error(), "incomplete") {
		t.Fatal("missing installed binaries were reported as installed", e)
	}
}

func TestArchiveSizeFallbackAndMetadataRejectHTTPDowngrade(t *testing.T) {
	var head, get atomic.Int32
	server := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == "HEAD" {
			head.Add(1)
			w.WriteHeader(405)
			return
		}
		get.Add(1)
		if r.Header.Get("Range") != "bytes=0-0" {
			t.Error("metadata fetched the entire archive")
		}
		w.Header().Set("Content-Range", "bytes 0-0/12345")
		w.WriteHeader(206)
		w.Write([]byte("x"))
	}))
	defer server.Close()
	bytes, e := packageArchiveSize(context.Background(), Package{URL: server.URL}, server.Client())
	if e != nil || bytes != 12345 || head.Load() != 1 || get.Load() != 1 {
		t.Fatal(bytes, e)
	}
	var insecureRequests atomic.Int32
	var secureRedirects atomic.Int32
	insecure := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { insecureRequests.Add(1) }))
	defer insecure.Close()
	redirect := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		secureRedirects.Add(1)
		http.Redirect(w, r, insecure.URL, 302)
	}))
	defer redirect.Close()
	client := redirect.Client()
	client.CheckRedirect = metadataClient().CheckRedirect
	if _, e = packageArchiveSize(context.Background(), Package{URL: redirect.URL}, client); e == nil || !strings.Contains(e.Error(), "HTTPS") || secureRedirects.Load() != 1 || insecureRequests.Load() != 0 {
		t.Fatal("metadata followed an insecure redirect", e)
	}
}

func TestInstalledPackageResolutionAndPreviewAreOffline(t *testing.T) {
	m := testManager(t)
	p := Package{Name: "node", Version: "22.14.0", Arch: runtime.GOARCH, URL: "https://offline.invalid/package", SHA256: strings.Repeat("a", 64), Format: "zip", Binaries: map[string]string{"node": "bin/node"}}
	if e := os.MkdirAll(filepath.Join(m.packageDir(p), "bin"), 0700); e != nil {
		t.Fatal(e)
	}
	if e := os.WriteFile(filepath.Join(m.packageDir(p), "bin/node"), []byte("fixture"), 0700); e != nil {
		t.Fatal(e)
	}
	if e := m.withState(context.Background(), func(s *State) error { s.Packages[p.ID()] = p; return nil }); e != nil {
		t.Fatal(e)
	}
	resolved, e := m.ResolveInstalledOrCatalog(context.Background(), "node", "v22.14.0")
	if e != nil || resolved.SHA256 != p.SHA256 {
		t.Fatal(resolved, e)
	}
	info, e := m.PackageDetails(context.Background(), resolved)
	if e != nil || !info.Installed || !info.Compatible {
		t.Fatal(info, e)
	}
}

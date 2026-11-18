package devplatform

import (
	"archive/zip"
	"bytes"
	"context"
	"crypto/sha256"
	"devbase/internal/launchd"
	"encoding/hex"
	"errors"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"syscall"
	"testing"
	"time"
)

func integrityFixture(t *testing.T) (*Manager, Package) {
	t.Helper()
	m := testManager(t)
	var archive bytes.Buffer
	z := zip.NewWriter(&archive)
	for _, item := range [][2]string{{"bin/fixture", "#!/bin/sh\nif [ \"$1\" = hold ]; then /usr/bin/touch \"$2\"; exec /bin/sleep 10; fi\nprintf fixture-ok\n"}, {"lib/data", "trusted library data"}} {
		h := &zip.FileHeader{Name: item[0], Method: zip.Deflate}
		h.SetMode(0755)
		w, err := z.CreateHeader(h)
		if err != nil {
			t.Fatal(err)
		}
		if _, err = w.Write([]byte(item[1])); err != nil {
			t.Fatal(err)
		}
	}
	h := &zip.FileHeader{Name: "lib/current", Method: zip.Deflate}
	h.SetMode(os.ModeSymlink | 0777)
	w, err := z.CreateHeader(h)
	if err != nil {
		t.Fatal(err)
	}
	w.Write([]byte("data"))
	if err = z.Close(); err != nil {
		t.Fatal(err)
	}
	sum := sha256.Sum256(archive.Bytes())
	p := Package{Name: "fixture", Version: "1.0.0", Arch: runtime.GOARCH, URL: "https://unused.invalid/fixture.zip", SHA256: hex.EncodeToString(sum[:]), Format: "zip", Binaries: map[string]string{"fixture": "bin/fixture"}}
	if err = os.WriteFile(filepath.Join(m.Root, "downloads", p.SHA256), archive.Bytes(), 0600); err != nil {
		t.Fatal(err)
	}
	if err = m.Install(context.Background(), p); err != nil {
		t.Fatal(err)
	}
	return m, p
}

func assertIntegrity(t *testing.T, m *Manager, p Package, status string) PackageIntegrityResult {
	t.Helper()
	r, err := m.VerifyPackage(context.Background(), p.ID())
	if err != nil || r.Status != status {
		t.Fatal("incorrect integrity result", status, r, err)
	}
	return r
}

func TestPackageIntegrityDetectsContentPermissionsLinksAndIndexTampering(t *testing.T) {
	m, p := integrityFixture(t)
	ctx := context.Background()
	if r := assertIntegrity(t, m, p, "Verified"); r.Files < 5 {
		t.Fatal("did not index the full tree", r)
	}
	data := filepath.Join(m.packageDir(p), "lib/data")
	if err := os.WriteFile(data, []byte("changed library data"), 0755); err != nil {
		t.Fatal(err)
	}
	r := assertIntegrity(t, m, p, "Needs repair")
	if !strings.Contains(r.Text(), "lib/data") {
		t.Fatal(r)
	}
	if err := m.Install(ctx, p); err == nil {
		t.Fatal("existing installation bypassed content verification")
	}
	if err := m.RepairPackage(ctx, p.ID()); err != nil {
		t.Fatal(err)
	}
	assertIntegrity(t, m, p, "Verified")
	if err := os.Remove(data); err != nil {
		t.Fatal(err)
	}
	if r := assertIntegrity(t, m, p, "Needs repair"); !strings.Contains(r.Text(), "Missing: lib/data") {
		t.Fatal(r)
	}
	if err := m.RepairPackage(ctx, p.ID()); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(m.packageDir(p), "unexpected"), []byte("extra"), 0600); err != nil {
		t.Fatal(err)
	}
	if r := assertIntegrity(t, m, p, "Needs repair"); !strings.Contains(r.Text(), "Unexpected: unexpected") {
		t.Fatal(r)
	}
	if err := m.RepairPackage(ctx, p.ID()); err != nil {
		t.Fatal(err)
	}
	if err := os.Chmod(filepath.Join(m.packageDir(p), "bin/fixture"), 0600); err != nil {
		t.Fatal(err)
	}
	assertIntegrity(t, m, p, "Needs repair")
	if err := m.RepairPackage(ctx, p.ID()); err != nil {
		t.Fatal(err)
	}
	outside := filepath.Join(t.TempDir(), "secret")
	if err := os.WriteFile(outside, []byte("NEVER-READ-THIS"), 0600); err != nil {
		t.Fatal(err)
	}
	link := filepath.Join(m.packageDir(p), "lib/current")
	os.Remove(link)
	if err := os.Symlink(outside, link); err != nil {
		t.Fatal(err)
	}
	r = assertIntegrity(t, m, p, "Needs repair")
	if strings.Contains(r.Text(), "NEVER-READ-THIS") {
		t.Fatal("followed a changed symlink", r)
	}
	if err := m.RepairPackage(ctx, p.ID()); err != nil {
		t.Fatal(err)
	}
	index := m.integrityPath(p.ID())
	if err := os.WriteFile(index, []byte(`{"schema":1,"files":{}}`), 0600); err != nil {
		t.Fatal(err)
	}
	assertIntegrity(t, m, p, "Needs repair")
	if err := m.RepairPackage(ctx, p.ID()); err != nil {
		t.Fatal(err)
	}
	assertIntegrity(t, m, p, "Verified")
}

func TestPackageRepairRollsBackCancellationAndStateCommitFailure(t *testing.T) {
	for _, failCommit := range []bool{false, true} {
		t.Run(map[bool]string{false: "cancel-after-swap", true: "failed-state-write"}[failCommit], func(t *testing.T) {
			m, p := integrityFixture(t)
			file := filepath.Join(m.packageDir(p), "lib/data")
			if err := os.WriteFile(file, []byte("old damaged bytes"), 0755); err != nil {
				t.Fatal(err)
			}
			indexBefore, err := os.ReadFile(m.integrityPath(p.ID()))
			if err != nil {
				t.Fatal(err)
			}
			sentinel := filepath.Join(m.Root, "instances", "untouched", "data")
			if err = atomicWrite(sentinel, []byte("database data"), 0600); err != nil {
				t.Fatal(err)
			}
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			m.Event = func(message string) {
				if !strings.HasPrefix(message, "Committing package files") {
					return
				}
				if failCommit {
					if err := os.Rename(filepath.Join(m.Root, "state.json"), filepath.Join(m.Root, "state.saved")); err != nil {
						t.Fatal(err)
					}
					if err := os.Mkdir(filepath.Join(m.Root, "state.json"), 0700); err != nil {
						t.Fatal(err)
					}
				} else {
					cancel()
				}
			}
			if err = m.RepairPackage(ctx, p.ID()); err == nil {
				t.Fatal("failed transaction reported repair success")
			}
			if failCommit {
				os.Remove(filepath.Join(m.Root, "state.json"))
				if err = os.Rename(filepath.Join(m.Root, "state.saved"), filepath.Join(m.Root, "state.json")); err != nil {
					t.Fatal(err)
				}
			}
			if b, err := os.ReadFile(file); err != nil || string(b) != "old damaged bytes" {
				t.Fatal("did not restore original installation", string(b), err)
			}
			if b, err := os.ReadFile(m.integrityPath(p.ID())); err != nil || !bytes.Equal(b, indexBefore) {
				t.Fatal("did not restore original index", err)
			}
			if b, err := os.ReadFile(sentinel); err != nil || string(b) != "database data" {
				t.Fatal("repair touched database data", err)
			}
			assertIntegrity(t, m, p, "Needs repair")
		})
	}
}

func TestRepairDownloadFailureKeepsOriginalAndCanRetry(t *testing.T) {
	m, p := integrityFixture(t)
	cache := filepath.Join(m.Root, "downloads", p.SHA256)
	archive, err := os.ReadFile(cache)
	if err != nil {
		t.Fatal(err)
	}
	var response []byte
	server := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if response == nil {
			w.WriteHeader(503)
			return
		}
		w.Write(response)
	}))
	defer server.Close()
	previous := http.DefaultTransport
	http.DefaultTransport = server.Client().Transport
	t.Cleanup(func() { http.DefaultTransport = previous })
	if err = m.withState(context.Background(), func(s *State) error { p.URL = server.URL; s.Packages[p.ID()] = p; return nil }); err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(cache, []byte("damaged cache"), 0600); err != nil {
		t.Fatal(err)
	}
	file := filepath.Join(m.packageDir(p), "lib/data")
	if err = os.WriteFile(file, []byte("old damaged bytes"), 0755); err != nil {
		t.Fatal(err)
	}
	if err = m.RepairPackage(context.Background(), p.ID()); err == nil {
		t.Fatal("unavailable archive reported repaired")
	}
	response = []byte("wrong archive bytes")
	if err = m.RepairPackage(context.Background(), p.ID()); err == nil {
		t.Fatal("incorrect checksum reported repaired")
	}
	if b, err := os.ReadFile(file); err != nil || string(b) != "old damaged bytes" {
		t.Fatal("download failure replaced the original tree", err)
	}
	response = archive
	if err = m.RepairPackage(context.Background(), p.ID()); err != nil {
		t.Fatal("failed download could not be retried", err)
	}
	assertIntegrity(t, m, p, "Verified")
}

func TestRepairAndUninstallProtectOrphanJobsWithEscapedPaths(t *testing.T) {
	m, p := integrityFixture(t)
	newRoot := filepath.Join(t.TempDir(), "workspace & packages")
	if err := os.Rename(m.Root, newRoot); err != nil {
		t.Fatal(err)
	}
	m.Root = newRoot
	label := m.label("orphan")
	if _, err := launchd.Save(filepath.Join(m.Root, "jobs"), launchd.Spec{Label: label, Args: []string{filepath.Join(m.packageDir(p), "bin/fixture")}}, ""); err != nil {
		t.Fatal(err)
	}
	queryError := error(nil)
	m.Run = func(context.Context, string, ...string) (string, error) { return "state = running", queryError }
	if err := m.RepairPackage(context.Background(), p.ID()); err == nil || !strings.Contains(err.Error(), "launchd job") {
		t.Fatal("replaced a package used by an orphan job", err)
	}
	if err := m.Uninstall(context.Background(), p.ID()); err == nil {
		t.Fatal("XML escaping hid an orphan package reference")
	}
	queryError = errors.New("launchd query unavailable")
	if err := m.RepairPackage(context.Background(), p.ID()); err == nil || !strings.Contains(err.Error(), "could not verify") {
		t.Fatal("treated failed process observation as idle", err)
	}
	queryError = errors.New("Could not find service")
	if err := m.RepairPackage(context.Background(), p.ID()); err != nil {
		t.Fatal("a stopped job unnecessarily blocked repair", err)
	}
	assertIntegrity(t, m, p, "Verified")
}

func TestIntegrityIndexSpecialFileDoesNotBlockVerification(t *testing.T) {
	m, p := integrityFixture(t)
	index := m.integrityPath(p.ID())
	if err := os.Remove(index); err != nil {
		t.Fatal(err)
	}
	if err := syscall.Mkfifo(index, 0600); err != nil {
		t.Fatal(err)
	}
	ch := make(chan error, 1)
	go func() {
		r, err := m.VerifyPackage(context.Background(), p.ID())
		if err == nil && r.Status != "Needs repair" {
			err = errors.New("special index was accepted")
		}
		ch <- err
	}()
	select {
	case err := <-ch:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(time.Second):
		t.Fatal("verification blocked on a special file")
	}
}

package devcli

import (
	"bytes"
	"context"
	"encoding/json"
	"net"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"macos-service-manager/internal/devplatform"
)

func TestCLIInitializationReviewAndArchivePreservesPreparedFiles(t *testing.T) {
	root := filepath.Join(t.TempDir(), "workspace")
	t.Setenv("MSM_DEV_HOME", root)
	ctx := context.Background()
	var out bytes.Buffer
	if e := Run(ctx, []string{"db-initializations"}, &out); e != nil {
		t.Fatal(e)
	}
	if _, e := os.Stat(root); !os.IsNotExist(e) {
		t.Fatal("inspection created storage", e)
	}
	p := devplatform.Package{Name: "redis", Version: "fixture", Arch: "universal", URL: "https://unused.invalid/fixture.zip", SHA256: strings.Repeat("0", 64), Format: "zip", Binaries: map[string]string{"redis-server": "missing"}}
	s := devplatform.State{SchemaVersion: devplatform.Schema, ProxyRules: []devplatform.ProxyRule{}, ProxyListenerPort: 443, Packages: map[string]devplatform.Package{p.ID(): p}, Instances: map[string]devplatform.Instance{}}
	if e := os.MkdirAll(root, 0700); e != nil {
		t.Fatal(e)
	}
	b, _ := json.Marshal(s)
	if e := os.WriteFile(filepath.Join(root, "state.json"), b, 0600); e != nil {
		t.Fatal(e)
	}
	listener, e := net.Listen("tcp4", "127.0.0.1:0")
	if e != nil {
		t.Fatal(e)
	}
	port := listener.Addr().(*net.TCPAddr).Port
	listener.Close()
	m := devplatform.New(root)
	if e = m.CreateInstance(ctx, devplatform.Instance{ID: "incomplete", Kind: "redis", Package: p.ID(), Port: port}); e == nil {
		t.Fatal("expected missing executable")
	}
	out.Reset()
	if e = Run(ctx, []string{"db-initializations"}, &out); e != nil {
		t.Fatal(e)
	}
	var records []devplatform.InstanceInitializationInfo
	if e = json.Unmarshal(out.Bytes(), &records); e != nil || len(records) != 1 || !records[0].CanResolve {
		t.Fatal(out.String(), e)
	}
	r := records[0]
	password, e := os.ReadFile(filepath.Join(r.Path, "password"))
	if e != nil {
		t.Fatal(e)
	}
	if e = Run(ctx, []string{"db-initialization-resolve", r.ID, "stale"}, &out); e == nil {
		t.Fatal("accepted stale review")
	}
	out.Reset()
	if e = Run(ctx, []string{"db-initialization-resolve", r.ID, r.Revision}, &out); e != nil {
		t.Fatal(e)
	}
	lines := strings.Split(strings.TrimSpace(out.String()), "\n")
	archive := lines[len(lines)-1]
	if archive != filepath.Join(root, "retained-initializations", r.ID) {
		t.Fatal("unexpected archive path", out.String())
	}
	after, e := os.ReadFile(filepath.Join(archive, "password"))
	if e != nil || !bytes.Equal(after, password) {
		t.Fatal("archive changed prepared credentials", e)
	}
	out.Reset()
	if e = Run(ctx, []string{"db-initializations"}, &out); e != nil {
		t.Fatal(e)
	}
	if e = json.Unmarshal(out.Bytes(), &records); e != nil || len(records) != 0 {
		t.Fatal(out.String(), e)
	}
}

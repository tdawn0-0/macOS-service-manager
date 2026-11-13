package devplatform

import (
	"context"
	"devbase/internal/privhelper"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
)

func setTestProxyRules(t *testing.T, m *Manager, rules []ProxyRule) {
	t.Helper()
	if err := m.withState(context.Background(), func(s *State) error {
		s.ProxyListenerPort = 443
		s.ProxyRules = rules
		return nil
	}); err != nil {
		t.Fatal(err)
	}
}

func TestHostsSyncRequiresPrivilegedHelper(t *testing.T) {
	m := testManager(t)
	hostsPath := filepath.Join(t.TempDir(), "hosts")
	if err := os.WriteFile(hostsPath, []byte("127.0.0.1 localhost\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	m.HostsPath = hostsPath
	setTestProxyRules(t, m, []ProxyRule{{Domain: "app.test", LocalPort: 3000}})

	err := m.ConfigureHosts(context.Background())
	if !errors.Is(err, privhelper.ErrNotInstalled) {
		t.Fatalf("expected ErrNotInstalled without helper, got %v", err)
	}
}

func TestHostsSyncPreservesPermissionsFlushesDNSAndDetectsConflicts(t *testing.T) {
	m := testManager(t)
	hostsPath := filepath.Join(t.TempDir(), "hosts")
	initial := "127.0.0.1 localhost\n# quoted ' 字段\n"
	if err := os.WriteFile(hostsPath, []byte(initial), 0o640); err != nil {
		t.Fatal(err)
	}
	m.HostsPath = hostsPath
	setTestProxyRules(t, m, []ProxyRule{{Domain: "app.test", LocalPort: 3000}})

	var ran [][]string
	client := privhelper.InstallTestHelper(t, privhelper.ServerConfig{
		HostsPath: hostsPath,
		Run: func(_ context.Context, name string, args ...string) ([]byte, error) {
			ran = append(ran, append([]string{name}, args...))
			return nil, nil
		},
	})

	if err := m.ConfigureHosts(context.Background()); err != nil {
		t.Fatalf("ConfigureHosts: %v", err)
	}
	got, err := os.ReadFile(hostsPath)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(string(got), "127.0.0.1 app.test\n") || !strings.Contains(string(got), "# quoted ' 字段") {
		t.Fatalf("unexpected hosts content: %q", string(got))
	}
	st, err := os.Stat(hostsPath)
	if err != nil || st.Mode().Perm() != 0o640 {
		t.Fatalf("expected 0640 mode preserved, got %v (err=%v)", st.Mode().Perm(), err)
	}
	backup, err := os.ReadFile(filepath.Join(m.Root, "hosts.backup"))
	if err != nil || string(backup) != initial {
		t.Fatalf("expected hosts.backup %q, got %q (err=%v)", initial, string(backup), err)
	}
	wantFlush := []string{"/usr/bin/dscacheutil", "-flushcache"}
	if !slices.ContainsFunc(ran, func(cmd []string) bool { return slices.Equal(cmd, wantFlush) }) {
		t.Fatalf("expected dscacheutil flush in %v", ran)
	}

	// Reject stale ExpectedOld without overwriting foreign content.
	foreign := "# separately edited\n"
	if err := os.WriteFile(hostsPath, []byte(foreign), 0o640); err != nil {
		t.Fatal(err)
	}
	if err := client.SyncHosts(context.Background(), privhelper.HostsSyncRequest{
		ExpectedOld: initial,
		NewContent:  "127.0.0.1 overwrite.test\n",
	}); err == nil {
		t.Fatal("expected conflict error when hosts file changed concurrently")
	}
	afterConflict, err := os.ReadFile(hostsPath)
	if err != nil || string(afterConflict) != foreign {
		t.Fatalf("foreign hosts content overwritten: %q, err=%v", string(afterConflict), err)
	}
}

func TestHostsSyncRejectsOversize(t *testing.T) {
	m := testManager(t)
	hostsPath := filepath.Join(t.TempDir(), "hosts")
	if err := os.WriteFile(hostsPath, []byte("127.0.0.1 localhost\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	m.HostsPath = hostsPath
	var rules []ProxyRule
	for i := 0; i < 1600; i++ {
		rules = append(rules, ProxyRule{
			Domain:    fmt.Sprintf("very-long-development-service-domain-%s-%04d.test", strings.Repeat("x", 20), i),
			LocalPort: 3000,
		})
	}
	setTestProxyRules(t, m, rules)
	if err := m.ConfigureHosts(context.Background()); err == nil || !strings.Contains(err.Error(), "size limit") {
		t.Fatalf("expected size limit error, got %v", err)
	}
}

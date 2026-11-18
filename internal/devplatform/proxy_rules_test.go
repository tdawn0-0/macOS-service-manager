package devplatform

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestProxyRulesAtomicSave(t *testing.T) {
	m := New(t.TempDir())
	if err := m.ReplaceProxyRules(context.Background(), 0, 443, []ProxyRule{{Domain: " App.DEVBASE.LOCALHOST ", LocalPort: 3000}}); err != nil {
		t.Fatal(err)
	}
	s, err := m.Read()
	if err != nil || s.ProxyRulesRevision != 1 || s.ProxyRules[0].Domain != "app.devbase.localhost" {
		t.Fatal(s, err)
	}
	for _, tc := range []struct {
		port  int
		rules []ProxyRule
	}{
		{8443, []ProxyRule{{Domain: "loop.devbase.localhost", LocalPort: 8443}}},
		{0, []ProxyRule{}},
		{8443, []ProxyRule{{Domain: "a.devbase.localhost", LocalPort: 3000}, {Domain: " A.DEVBASE.LOCALHOST ", LocalPort: 4000}}},
		{8443, []ProxyRule{{Domain: "https://a.devbase.localhost", LocalPort: 3000}}},
	} {
		if err := m.ReplaceProxyRules(context.Background(), 1, tc.port, tc.rules); err == nil {
			t.Fatal("invalid combined save accepted", tc)
		}
		s, err = m.Read()
		if err != nil || s.ProxyListenerPort != 443 || s.ProxyRulesRevision != 1 || s.ProxyRules[0].LocalPort != 3000 {
			t.Fatal("partial save", s, err)
		}
	}
	if err := m.ReplaceProxyRules(context.Background(), 0, 8443, []ProxyRule{}); err == nil {
		t.Fatal("stale revision accepted")
	}
	if err := m.ReplaceProxyRules(context.Background(), 1, 8443, []ProxyRule{}); err != nil {
		t.Fatal(err)
	}
	s, err = m.Read()
	if err != nil || s.ProxyListenerPort != 8443 || s.ProxyRulesRevision != 2 || len(s.ProxyRules) != 0 {
		t.Fatal(s, err)
	}
}

func TestProxyRules(t *testing.T) {
	t.Run("publication", func(t *testing.T) {
		m := New(t.TempDir())
		if err := m.ReplaceProxyRules(context.Background(), 0, 8443, []ProxyRule{{Domain: "b.devbase.localhost", LocalPort: 3001}, {Domain: "a.devbase.localhost", LocalPort: 3000}}); err != nil {
			t.Fatal(err)
		}
		want := func(expected string) {
			t.Helper()
			got, err := m.ProxyPublicationState()
			if err != nil || got != expected {
				t.Fatal(got, err, "want", expected)
			}
		}
		want("Unknown")
		s, _ := m.Read()
		config := proxyConfigFromState(&s)
		config.Rules[0], config.Rules[1] = config.Rules[1], config.Rules[0]
		if err := writeJSON(filepath.Join(m.Root, "proxy", "rules.json"), config); err != nil {
			t.Fatal(err)
		}
		want("Published")
		if err := m.ReplaceProxyRules(context.Background(), 1, 9443, append([]ProxyRule{}, s.ProxyRules...)); err != nil {
			t.Fatal(err)
		}
		want("Pending")
		if err := os.WriteFile(filepath.Join(m.Root, "proxy", "rules.json"), []byte(`{"schemaVersion":1,"listenerPort":9443,"rules":[],"site":"obsolete"}`), 0600); err != nil {
			t.Fatal(err)
		}
		want("Unknown")
	})
	t.Run("empty-apply", func(t *testing.T) {
		m := New(t.TempDir())
		m.Run = func(_ context.Context, command string, args ...string) (string, error) {
			if command == "launchctl" && len(args) > 0 && args[0] == "print" {
				return "", errors.New("Could not find service")
			}
			return "", nil
		}
		if err := m.ApplyProxy(context.Background()); err != nil {
			t.Fatal(err)
		}
		config, err := readPublishedProxyConfig(m.Root)
		if err != nil || config.ListenerPort != 443 || config.Rules == nil || len(config.Rules) != 0 {
			t.Fatal(config, err)
		}
		got, err := m.ProxyPublicationState()
		if err != nil || got != "Published" {
			t.Fatal(got, err)
		}
		b, err := os.ReadFile(filepath.Join(m.Root, "proxy", "rules.json"))
		if err != nil || !strings.Contains(string(b), `"rules": []`) {
			t.Fatal(string(b), err)
		}
	})
	t.Run("failed-empty-publication", func(t *testing.T) {
		m := New(t.TempDir())
		m.Run = func(_ context.Context, command string, args ...string) (string, error) {
			if command == "launchctl" && len(args) > 0 && args[0] == "print" {
				return "", errors.New("Could not find service")
			}
			return "", nil
		}
		if err := os.MkdirAll(filepath.Join(m.Root, "proxy", "rules.json"), 0700); err != nil {
			t.Fatal(err)
		}
		if err := m.ApplyProxy(context.Background()); err == nil {
			t.Fatal("reported success with unwritable publication target")
		}
		got, err := m.ProxyPublicationState()
		if err != nil || got == "Published" {
			t.Fatal(got, err)
		}
	})
	t.Run("failed-stop-keeps-publication", func(t *testing.T) {
		m := New(t.TempDir())
		path := filepath.Join(m.Root, "proxy", "rules.json")
		old := proxyPublishedConfig{SchemaVersion: 1, ListenerPort: 443, Rules: []ProxyRule{{Domain: "old.devbase.localhost", LocalPort: 3000}}}
		if err := writeJSON(path, old); err != nil {
			t.Fatal(err)
		}
		m.Run = func(_ context.Context, command string, args ...string) (string, error) {
			if command == "launchctl" && len(args) > 0 && args[0] == "print" {
				return "pid = 0", nil
			}
			if command == "launchctl" && len(args) > 0 && args[0] == "bootout" {
				return "", errors.New("bootout rejected")
			}
			return "", nil
		}
		if err := m.ApplyProxy(context.Background()); err == nil {
			t.Fatal("stop failure ignored")
		}
		config, err := readPublishedProxyConfig(m.Root)
		if err != nil || !sameProxyRules(config.Rules, old.Rules) {
			t.Fatal(config, err)
		}
	})
}

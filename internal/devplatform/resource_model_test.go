package devplatform

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestResourceOnlyCore(t *testing.T) {
	t.Run("reject PHP without invoking a package runner", func(t *testing.T) {
		m := New(t.TempDir())
		called := false
		m.Run = func(context.Context, string, ...string) (string, error) {
			called = true
			return "", nil
		}
		if _, err := m.ResolvePackage(context.Background(), "php", "8.5"); err == nil {
			t.Fatal("PHP resolver accepted a removed software type")
		}
		p := Package{Name: "php", Version: "8.5.12", Arch: "arm64", Format: "homebrew", Formula: "shivammathur/php/php@8.5", SHA256: strings.Repeat("a", 64), URL: "https://example.test/php", Binaries: map[string]string{"php": "bin/php"}}
		if err := p.Validate(); err == nil {
			t.Fatal("PHP manifest accepted")
		}
		if err := m.Install(context.Background(), p); err == nil {
			t.Fatal("PHP install accepted")
		}
		if called {
			t.Fatal("PHP rejection ran an external command")
		}
	})
	t.Run("atomic proxy configuration", func(t *testing.T) {
		m := New(t.TempDir())
		if err := m.ReplaceProxyRules(context.Background(), 0, 443, []ProxyRule{{Domain: "app.test", LocalPort: 3000}}); err != nil {
			t.Fatal(err)
		}
		if err := m.ReplaceProxyRules(context.Background(), 1, 8443, []ProxyRule{{Domain: "app.test", LocalPort: 8443}}); err == nil {
			t.Fatal("accepted self-referencing proxy rule")
		}
		s, err := m.Read()
		if err != nil || s.ProxyListenerPort != 443 || s.ProxyRulesRevision != 1 || s.ProxyRules[0].LocalPort != 3000 {
			t.Fatalf("failed update changed state: %+v, %v", s, err)
		}
	})
	t.Run("new schema round trip and old state preservation", func(t *testing.T) {
		m := New(t.TempDir())
		initial, e := m.Read()
		if e != nil || initial.SchemaVersion != 2 || initial.Packages == nil || initial.Instances == nil || initial.ProxyRules == nil || initial.ProxyListenerPort != 443 {
			t.Fatal(initial, e)
		}
		if e = m.ReplaceProxyRules(context.Background(), 0, 8443, []ProxyRule{{Domain: "  APP.TEST  ", LocalPort: 3000}}); e != nil {
			t.Fatal(e)
		}
		got, e := m.Read()
		if e != nil || got.SchemaVersion != 2 || got.ProxyRulesRevision != 1 || len(got.ProxyRules) != 1 || got.ProxyRules[0].Domain != "app.test" {
			t.Fatal(got, e)
		}
		path := filepath.Join(m.Root, "state.json")
		old := []byte(`{"schemaVersion":1,"packages":{},"projects":{},"instances":{},"sites":{}}`)
		if e = os.WriteFile(path, old, 0600); e != nil {
			t.Fatal(e)
		}
		if _, e = m.Read(); e == nil || !strings.Contains(e.Error(), "unsupported format 1") || strings.Contains(e.Error(), "projects") {
			t.Fatal("old schema must report its format, not removed fields", e)
		}
		if e = m.ReplaceProxyRules(context.Background(), 0, 443, []ProxyRule{}); e == nil {
			t.Fatal("old state overwritten")
		}
		after, e := os.ReadFile(path)
		if e != nil || !bytes.Equal(after, old) {
			t.Fatal("old state bytes changed", e)
		}
		unknown := []byte(`{"schemaVersion":2,"packages":{},"instances":{},"proxyRules":[],"proxyListenerPort":443,"unexpected":true}`)
		if e = os.WriteFile(path, unknown, 0600); e != nil {
			t.Fatal(e)
		}
		if _, e = m.Read(); e == nil || !strings.Contains(e.Error(), "unknown field") {
			t.Fatal("current schema must still reject unknown fields", e)
		}
		for _, raw := range [][]byte{[]byte(`{"schemaVersion":2,"packages":{},"instances":{},"proxyListenerPort":443}`), []byte(`{"schemaVersion":2,"packages":{},"instances":{},"proxyRules":[],"proxyListenerPort":0}`)} {
			if e = os.WriteFile(path, raw, 0600); e != nil {
				t.Fatal(e)
			}
			if _, e = m.Read(); e == nil {
				t.Fatal("incomplete state accepted", string(raw))
			}
		}
	})
	t.Run("five software and independent node version file", func(t *testing.T) {
		for _, name := range []string{"node", "mysql", "postgresql", "redis", "mongodb"} {
			if len(AvailableVersions(name)) == 0 {
				t.Fatal("missing software", name)
			}
		}
		if len(AvailableVersions("php")) != 0 {
			t.Fatal("PHP catalog remains")
		}
		dir := t.TempDir()
		if e := os.WriteFile(filepath.Join(dir, ".nvmrc"), []byte("22.14.0\n"), 0600); e != nil {
			t.Fatal(e)
		}
		spec, source := NodeVersionFile(dir)
		if spec != "22.14.0" || source == "" {
			t.Fatal(spec, source)
		}
	})
	t.Run("rules validate together and save one revision", func(t *testing.T) {
		m := New(t.TempDir())
		ctx := context.Background()
		if e := m.ReplaceProxyRules(ctx, 0, 443, []ProxyRule{{Domain: "a.test", LocalPort: 3000}}); e != nil {
			t.Fatal(e)
		}
		for _, rules := range [][]ProxyRule{nil, {{Domain: "A.TEST", LocalPort: 3000}, {Domain: "a.test", LocalPort: 4000}}, {{Domain: "public.com", LocalPort: 3000}}, {{Domain: "a.test", LocalPort: 8443}}} {
			if e := m.ReplaceProxyRules(ctx, 1, 8443, rules); e == nil {
				t.Fatal("invalid rules accepted", rules)
			}
		}
		if e := m.ReplaceProxyRules(ctx, 0, 8443, []ProxyRule{}); e == nil {
			t.Fatal("stale revision accepted")
		}
		state, e := m.Read()
		if e != nil || state.ProxyListenerPort != 443 || state.ProxyRulesRevision != 1 || state.ProxyRules[0].Domain != "a.test" {
			t.Fatal(state, e)
		}
		if e = m.ReplaceProxyRules(ctx, 1, 8443, []ProxyRule{{Domain: "b.test", LocalPort: 4000}}); e != nil {
			t.Fatal(e)
		}
		state, e = m.Read()
		if e != nil || state.ProxyListenerPort != 8443 || state.ProxyRulesRevision != 2 || len(state.ProxyRules) != 1 || state.ProxyRules[0].Domain != "b.test" {
			t.Fatal(state, e)
		}
		if e = m.ReplaceProxyRules(ctx, 2, 8443, []ProxyRule{}); e != nil {
			t.Fatal(e)
		}
		state, e = m.Read()
		if e != nil || state.ProxyRules == nil || len(state.ProxyRules) != 0 || state.ProxyRulesRevision != 3 {
			t.Fatal(state, e)
		}
	})

}

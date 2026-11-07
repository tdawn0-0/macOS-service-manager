package devplatform

import (
	"context"
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
}

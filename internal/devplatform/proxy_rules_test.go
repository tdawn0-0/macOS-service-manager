package devplatform

import (
	"context"
	"testing"
)

func TestProxyRulesAtomicSave(t *testing.T) {
	m := New(t.TempDir())
	first := []ProxyRule{{Domain: " App.TEST ", LocalPort: 3000}}
	if err := m.ReplaceProxyRules(context.Background(), 0, 443, first); err != nil {
		t.Fatal(err)
	}
	s, err := m.Read()
	if err != nil || s.ProxyListenerPort != 443 || s.ProxyRulesRevision != 1 || len(s.ProxyRules) != 1 || s.ProxyRules[0].Domain != "app.test" {
		t.Fatalf("first save: %+v, %v", s, err)
	}
	if err := m.ReplaceProxyRules(context.Background(), 1, 8443, []ProxyRule{{Domain: "app.test", LocalPort: 8443}}); err == nil {
		t.Fatal("accepted self proxy loop")
	}
	s, err = m.Read()
	if err != nil || s.ProxyListenerPort != 443 || s.ProxyRulesRevision != 1 || s.ProxyRules[0].LocalPort != 3000 {
		t.Fatalf("invalid combined save changed state: %+v, %v", s, err)
	}
	if err := m.ReplaceProxyRules(context.Background(), 0, 8443, []ProxyRule{}); err == nil {
		t.Fatal("accepted stale revision")
	}
	if err := m.ReplaceProxyRules(context.Background(), 1, 8443, []ProxyRule{}); err != nil {
		t.Fatal(err)
	}
	s, err = m.Read()
	if err != nil || s.ProxyListenerPort != 8443 || s.ProxyRulesRevision != 2 || len(s.ProxyRules) != 0 {
		t.Fatalf("valid combined save: %+v, %v", s, err)
	}
}

package devplatform

import (
	"bytes"
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestStateSchema(t *testing.T) {
	m := New(t.TempDir())
	initial, e := m.Read()
	if e != nil || initial.SchemaVersion != 2 || initial.Packages == nil || initial.Instances == nil || initial.ProxyRules == nil || initial.ProxyListenerPort != 443 {
		t.Fatal("a new workspace does not start from complete defaults", initial, e)
	}
	path := filepath.Join(m.Root, "state.json")
	old := []byte(`{"schemaVersion":1,"packages":{},"instances":{}}`)
	if e = os.WriteFile(path, old, 0600); e != nil {
		t.Fatal(e)
	}
	if _, e = m.Read(); e == nil || !strings.Contains(e.Error(), "unsupported format 1") {
		t.Fatal("an older format must be reported as such", e)
	}
	if e = m.ReplaceProxyRules(context.Background(), 0, 443, []ProxyRule{}); e == nil {
		t.Fatal("older state overwritten")
	}
	if after, e := os.ReadFile(path); e != nil || !bytes.Equal(after, old) {
		t.Fatal("older state bytes changed", e)
	}
	for _, raw := range []string{
		`{"schemaVersion":2,"packages":{},"instances":{},"proxyRules":[],"proxyListenerPort":443,"unexpected":true}`,
		`{"schemaVersion":2,"packages":{},"instances":{},"proxyListenerPort":443}`,
		`{"schemaVersion":2,"packages":{},"instances":{},"proxyRules":[],"proxyListenerPort":0}`,
	} {
		if e = os.WriteFile(path, []byte(raw), 0600); e != nil {
			t.Fatal(e)
		}
		if _, e = m.Read(); e == nil {
			t.Fatal("unknown or incomplete state accepted", raw)
		}
	}
}

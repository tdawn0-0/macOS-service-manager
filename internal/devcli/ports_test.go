package devcli

import (
	"bytes"
	"context"
	"encoding/json"
	"net"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"

	"devbase/internal/devplatform"
)

func TestPortCLIInspectsListenerAndSuggestsWithoutChangingData(t *testing.T) {
	t.Setenv("MSM_DEV_HOME", t.TempDir())
	l, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer l.Close()
	var out bytes.Buffer
	if err = Run(context.Background(), []string{"port", strconv.Itoa(l.Addr().(*net.TCPAddr).Port)}, &out); err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(out.String(), "In use") || !strings.Contains(out.String(), "PID "+strconv.Itoa(os.Getpid())) {
		t.Fatal("CLI lost listener metadata", out.String())
	}
	out.Reset()
	if err = Run(context.Background(), []string{"available-port"}, &out); err != nil {
		t.Fatal(err)
	}
	port, err := strconv.Atoi(strings.TrimSpace(out.String()))
	if err != nil || port < 1024 {
		t.Fatal("invalid CLI suggestion", out.String())
	}
	if err = Run(context.Background(), []string{"port", "bad-port"}, &out); err == nil {
		t.Fatal("invalid port accepted")
	}
	// SQL ports are passed at startup; changing metadata must retain the record.
	m := devplatform.New(devplatform.DefaultRoot())
	instance := devplatform.Instance{ID: "cli-port", Kind: "postgresql", Port: 54321}
	s := devplatform.State{SchemaVersion: devplatform.Schema, ProxyRules: []devplatform.ProxyRule{}, ProxyListenerPort: 443, Packages: map[string]devplatform.Package{}, Instances: map[string]devplatform.Instance{instance.ID: instance}}
	// Persist an isolated, stopped record; no database binary is executed.
	if err := os.MkdirAll(m.Root, 0700); err != nil {
		t.Fatal(err)
	}
	b, err := json.Marshal(s)
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(filepath.Join(m.Root, "state.json"), b, 0600); err != nil {
		t.Fatal(err)
	}
	if err = Run(context.Background(), []string{"db-port", instance.ID, strconv.Itoa(port)}, &out); err != nil {
		t.Fatal(err)
	}
	if err = Run(context.Background(), []string{"db-port-repair", instance.ID}, &out); err != nil {
		t.Fatal(err)
	}
	state, err := m.Read()
	if err != nil || state.Instances[instance.ID].Port != port || len(state.Instances) != 1 {
		t.Fatal("CLI lost instance identity", err)
	}
}

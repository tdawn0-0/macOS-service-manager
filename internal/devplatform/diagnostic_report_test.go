package devplatform

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func findDiagnostic(t *testing.T, r DiagnosticReport, layer, resource string) DiagnosticCheck {
	t.Helper()
	for _, check := range r.Checks {
		if check.Layer == layer && (resource == "" || check.Resource == resource) {
			return check
		}
	}
	t.Fatalf("missing diagnostic %s %s: %s", layer, resource, r.Text())
	return DiagnosticCheck{}
}

func TestDiagnosticDNSRejectsForeignAndWrongLoopbackAddresses(t *testing.T) {
	for _, addresses := range [][]string{nil, {"127.0.0.1"}, {"127.0.0.1", "127.0.0.1"}, {"203.0.113.5"}, {"127.0.0.2"}, {"::1"}, {"127.0.0.1", "203.0.113.5"}, {"not-an-ip"}} {
		t.Run(fmt.Sprint(addresses), func(t *testing.T) {
			m := testManager(t)
			s := emptyState()
			s.ProxyRules = []ProxyRule{{Domain: "dns.test", LocalPort: 1}}
			s.ProxyListenerPort = 65432
			r, err := m.diagnose(context.Background(), s, diagnosticProbes{
				lookup: func(context.Context, string) ([]string, error) { return addresses, nil },
				dial:   func(context.Context, string, string) (net.Conn, error) { return nil, errors.New("unavailable") },
				tls:    func(context.Context, string, string) error { return errors.New("unavailable") },
			})
			if err != nil {
				t.Fatal(err)
			}
			check := findDiagnostic(t, r, "DNS", "dns.test")
			want := "Failed"
			if len(addresses) > 0 {
				want = "OK"
				for _, ip := range addresses {
					if ip != "127.0.0.1" {
						want = "Failed"
					}
				}
			}
			if check.Status != want || (want == "Failed" && check.Remedy == "") {
				t.Fatal(check, want)
			}
		})
	}
}

func TestDiagnosticRedactionAndUnavailableLocalPort(t *testing.T) {
	m := testManager(t)
	s := emptyState()
	s.ProxyRules = []ProxyRule{{Domain: "sensitive.test", LocalPort: 1}}
	s.ProxyListenerPort = integrationPort(t)
	if err := writeJSON(filepath.Join(m.Root, "state.json"), s); err != nil {
		t.Fatal(err)
	}
	r, err := m.Diagnose(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if c := findDiagnostic(t, r, "Upstream", "sensitive.test"); c.Status != "Failed" {
		t.Fatal(c)
	}
	b, err := r.RedactedJSON()
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(b), "sensitive.test") || strings.Contains(string(b), m.Root) {
		t.Fatal("redaction leaked private data")
	}
	path, err := m.ExportDiagnostics(context.Background(), r)
	if err != nil {
		t.Fatal(err)
	}
	actual, err := os.ReadFile(path)
	if err != nil || string(actual) != string(b) {
		t.Fatal(err)
	}
	info, err := os.Stat(path)
	if err != nil || info.Mode().Perm() != 0600 {
		t.Fatal("report not private", err)
	}
	var decoded DiagnosticReport
	if err := json.Unmarshal(actual, &decoded); err != nil || !decoded.Redacted {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if _, err := m.ExportDiagnostics(ctx, r); !errors.Is(err, context.Canceled) {
		t.Fatal("cancelled export succeeded", err)
	}
}

func TestDiagnosticToleratesSlowLocalResolverAndReportsTimeoutSeparately(t *testing.T) {
	for _, timeout := range []bool{false, true} {
		t.Run(fmt.Sprint(timeout), func(t *testing.T) {
			m := testManager(t)
			s := emptyState()
			s.ProxyRules = []ProxyRule{{Domain: "slow.test", LocalPort: 1}}
			s.ProxyListenerPort = 65432
			r, err := m.diagnose(context.Background(), s, diagnosticProbes{
				lookup: func(ctx context.Context, name string) ([]string, error) {
					if timeout {
						return nil, context.DeadlineExceeded
					}
					select {
					case <-time.After(2100 * time.Millisecond):
						return []string{"127.0.0.1"}, nil
					case <-ctx.Done():
						return nil, ctx.Err()
					}
				},
				dial: func(context.Context, string, string) (net.Conn, error) { return nil, errors.New("unavailable") },
				tls:  func(context.Context, string, string) error { return errors.New("unavailable") },
			})
			if err != nil {
				t.Fatal(err)
			}
			check := findDiagnostic(t, r, "DNS", "slow.test")
			want := "OK"
			if timeout {
				want = "Warning"
			}
			if check.Status != want {
				t.Fatal("slow or incomplete resolution was called a configuration failure", check)
			}
		})
	}
}

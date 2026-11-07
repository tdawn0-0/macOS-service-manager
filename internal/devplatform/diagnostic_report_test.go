package devplatform

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"net/http/httptest"
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
			s.Sites["site"] = Site{ID: "site", Domain: "dns.test", Upstream: "http://127.0.0.1:1", Port: 65432}
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

func TestDiagnosticSeparatesRuntimeTLSAndUnavailableUpstream(t *testing.T) {
	m := testManager(t)
	const secret = "private-token-SHOULD-NOT-BE-EXPORTED"
	m.Run = func(ctx context.Context, name string, args ...string) (string, error) {
		if name == "launchctl" {
			return "state = running\nenvironment = " + secret, nil
		}
		return secret, errors.New(secret)
	}
	if _, _, err := m.ca(); err != nil {
		t.Fatal(err)
	}
	upstream := httptest.NewUnstartedServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		t.Error("diagnostics should not request an application path")
	}))
	upstream.Config.ErrorLog = log.New(io.Discard, "", 0)
	upstream.StartTLS()
	defer upstream.Close()
	s := emptyState()
	s.Projects["app"] = Project{ID: "app", Name: secret, Directory: t.TempDir(), Services: []Service{{ID: "web", Command: []string{secret}, Environment: map[string]string{"TOKEN": secret}}}}
	s.Sites["tls"] = Site{ID: "tls", Domain: "sensitive.test", Upstream: upstream.URL + "/private?token=" + secret, Port: integrationPort(t)}
	if err := writeJSON(filepath.Join(m.Root, "state.json"), s); err != nil {
		t.Fatal(err)
	}
	r, err := m.Diagnose(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if c := findDiagnostic(t, r, "Readiness", ""); c.Status != "Warning" {
		t.Fatal("running without a probe falsely reported healthy", c)
	}
	if c := findDiagnostic(t, r, "Upstream TLS", "sensitive.test"); c.Status != "Failed" {
		t.Fatal("accepted an untrusted TLS upstream", c)
	}
	if c := findDiagnostic(t, r, "Proxy TLS", "sensitive.test"); c.Status != "Failed" {
		t.Fatal("accepted a missing local listener", c)
	}
	if c := findDiagnostic(t, r, "Trust", ""); c.Status != "Failed" {
		t.Fatal(c)
	}
	b, err := r.RedactedJSON()
	if err != nil {
		t.Fatal(err)
	}
	for _, forbidden := range []string{secret, "sensitive.test", upstream.URL, m.Root, "\"TOKEN\"", "BEGIN PRIVATE KEY"} {
		if strings.Contains(string(b), forbidden) {
			t.Fatal("export leaked local data", forbidden, string(b))
		}
	}
	if !strings.Contains(r.Text(), secret) {
		t.Fatal("export unexpectedly modified the local report")
	}
	path, err := m.ExportDiagnostics(context.Background(), r)
	if err != nil {
		t.Fatal(err)
	}
	actual, err := os.ReadFile(path)
	if err != nil || string(actual) != string(b) {
		t.Fatal(err, string(actual))
	}
	info, err := os.Stat(path)
	if err != nil || info.Mode().Perm() != 0600 {
		t.Fatal("report is not private", err)
	}
	var decoded DiagnosticReport
	if err = json.Unmarshal(actual, &decoded); err != nil || !decoded.Redacted {
		t.Fatal(err, decoded)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if _, err = m.ExportDiagnostics(ctx, r); !errors.Is(err, context.Canceled) {
		t.Fatal("cancelled export succeeded", err)
	}
}

func TestDiagnosticHealthyProcessAndDisabledSite(t *testing.T) {
	m := testManager(t)
	m.Run = func(context.Context, string, ...string) (string, error) { return "state = running", nil }
	healthy := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { w.WriteHeader(204) }))
	defer healthy.Close()
	s := emptyState()
	s.Projects["healthy"] = Project{ID: "healthy", Name: "App", Services: []Service{{ID: "api", Command: []string{"api"}, Health: healthy.URL}}}
	s.Sites["disabled"] = Site{ID: "disabled", Domain: "disabled.test", Disabled: true}
	r, err := m.diagnose(context.Background(), s, diagnosticProbes{
		lookup: func(context.Context, string) ([]string, error) { t.Fatal("probed disabled DNS"); return nil, nil },
		dial: func(context.Context, string, string) (net.Conn, error) {
			t.Fatal("probed disabled upstream")
			return nil, nil
		},
		tls: func(context.Context, string, string) error { t.Fatal("probed disabled TLS"); return nil },
	})
	if err != nil {
		t.Fatal(err)
	}
	if c := findDiagnostic(t, r, "Readiness", ""); c.Status != "OK" {
		t.Fatal(c)
	}
	if c := findDiagnostic(t, r, "Site", "disabled.test"); c.Status != "Disabled" {
		t.Fatal(c)
	}
}

func TestDiagnosticToleratesSlowLocalResolverAndReportsTimeoutSeparately(t *testing.T) {
	for _, timeout := range []bool{false, true} {
		t.Run(fmt.Sprint(timeout), func(t *testing.T) {
			m := testManager(t)
			s := emptyState()
			s.Sites["slow"] = Site{ID: "slow", Domain: "slow.test", Upstream: "http://127.0.0.1:1", Port: 65432}
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

package main

import (
	"context"
	"crypto/tls"
	"crypto/x509"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"
)

func TestUIHTTPSPortConflictKeepsLastGoodRulesAndRepairsByChangingPort(t *testing.T) {
	for _, dark := range []bool{false, true} {
		t.Run(fmt.Sprintf("minimum-dark-%t", dark), func(t *testing.T) {
			h := newDevUIHarnessSize(t, 900, 520, dark)
			m := h.a.dev.managerFactory()
			defer m.StopProxy(context.Background())
			upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { io.WriteString(w, "managed HTTPS after port repair") }))
			defer upstream.Close()
			h.click("HTTPS")
			goodPort := freeUIPort(t)
			h.fill("HTTPS listener port", fmt.Sprint(goodPort))
			h.click("Add rule")
			h.fill("Domain", "good-port.test")
			h.fill("Local port", fmt.Sprint(upstream.Listener.Addr().(*net.TCPAddr).Port))
			h.click("Save HTTPS rules")
			h.click("Apply proxy")
			if strings.Contains(h.a.dev.output, "Error:") {
				t.Fatal(h.a.dev.output)
			}
			ca, err := os.ReadFile(filepath.Join(h.root, "proxy", "ca.pem"))
			if err != nil {
				t.Fatal(err)
			}
			pool := x509.NewCertPool()
			pool.AppendCertsFromPEM(ca)
			check := func(domain string, port int) {
				t.Helper()
				transport := &http.Transport{TLSClientConfig: &tls.Config{RootCAs: pool, ServerName: domain}}
				defer transport.CloseIdleConnections()
				client := &http.Client{Transport: transport, Timeout: 3 * time.Second}
				req, _ := http.NewRequest("GET", fmt.Sprintf("https://127.0.0.1:%d/", port), nil)
				req.Host = domain
				res, err := client.Do(req)
				if err != nil {
					t.Fatal(err)
				}
				body, _ := io.ReadAll(res.Body)
				res.Body.Close()
				if res.StatusCode != 200 || string(body) != "managed HTTPS after port repair" {
					t.Fatal("wrong HTTPS response", string(body))
				}
			}
			check("good-port.test", goodPort)
			before, err := m.InspectPort(context.Background(), goodPort)
			if err != nil || len(before.Listeners) == 0 {
				t.Fatal("could not observe initial proxy", err)
			}
			occupied, err := net.Listen("tcp4", "127.0.0.1:0")
			if err != nil {
				t.Fatal(err)
			}
			defer occupied.Close()
			oldPort := occupied.Addr().(*net.TCPAddr).Port
			h.fill("HTTPS listener port", fmt.Sprint(oldPort))
			h.click("Save HTTPS rules")
			h.click("Apply proxy")
			if !strings.Contains(h.a.dev.output, "unavailable") || h.a.dev.portChecks[oldPort].Status != "In use" {
				t.Fatal("port failure not shown", h.a.dev.output)
			}
			check("good-port.test", goodPort)
			after, err := m.InspectPort(context.Background(), goodPort)
			if err != nil || len(after.Listeners) == 0 || after.Listeners[0].PID != before.Listeners[0].PID {
				t.Fatal("failed preflight restarted the last good proxy", err)
			}
			h.click("Check HTTPS port")
			if !h.ui.HasText(fmt.Sprintf("PID %d", os.Getpid())) {
				t.Fatal("UI did not show the actual occupying process", h.ui.Texts())
			}
			h.click("Find available HTTPS port")
			newPort, err := strconv.Atoi(h.a.dev.proxyListenerPort)
			if err != nil || newPort == oldPort || newPort < 1024 {
				t.Fatal("invalid port suggestion", h.a.dev.proxyListenerPort)
			}
			h.click("Save HTTPS rules")
			h.click("Apply proxy")
			if strings.Contains(h.a.dev.output, "Error:") {
				t.Fatal("GUI port repair failed", h.a.dev.output)
			}
			check("good-port.test", newPort)
			conn, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", oldPort), time.Second)
			if err != nil {
				t.Fatal("port repair stopped an external listener", err)
			}
			conn.Close()
		})
	}
}

package main

import (
	"context"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/sha256"
	"crypto/tls"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/hex"
	"encoding/pem"
	"errors"
	"fmt"
	"io"
	"math/big"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"

	"macos-service-manager/internal/devplatform"
)

type uiAuthorityTrust struct {
	mu             sync.Mutex
	denied         bool
	missingRemoval bool
	deleteCount    int
	trusted        map[string]bool
}

func (f *uiAuthorityTrust) deny(value bool) { f.mu.Lock(); defer f.mu.Unlock(); f.denied = value }
func uiCertificateFingerprint(path string) (string, error) {
	b, err := os.ReadFile(path)
	if err != nil {
		return "", err
	}
	block, _ := pem.Decode(b)
	if block == nil {
		return "", errors.New("missing test certificate")
	}
	cert, err := x509.ParseCertificate(block.Bytes)
	if err != nil {
		return "", err
	}
	sum := sha256.Sum256(cert.Raw)
	return strings.ToUpper(hex.EncodeToString(sum[:])), nil
}
func fakeUIAuthorityTrust(h *devUIHarness) *uiAuthorityTrust {
	f := &uiAuthorityTrust{trusted: map[string]bool{}}
	base := h.a.dev.managerFactory
	h.a.dev.managerFactory = func() *devplatform.Manager {
		m := base()
		run := m.Run
		m.Run = func(ctx context.Context, name string, args ...string) (string, error) {
			if name != "/usr/bin/security" {
				return run(ctx, name, args...)
			}
			f.mu.Lock()
			defer f.mu.Unlock()
			if args[0] == "default-keychain" {
				return `"/tmp/ui-only-keychain"`, nil
			}
			if args[0] == "delete-certificate" {
				f.deleteCount++
				return "", nil
			}
			path := args[len(args)-1]
			if args[0] == "verify-cert" {
				path = args[2]
			}
			if !strings.HasPrefix(path, filepath.Clean(h.root)+string(os.PathSeparator)) {
				return "", errors.New("test blocked access outside isolated CA storage")
			}
			fingerprint, err := uiCertificateFingerprint(path)
			if err != nil {
				return "", err
			}
			switch args[0] {
			case "verify-cert":
				if f.trusted[fingerprint] {
					return "", nil
				}
				return "", errors.New("not trusted")
			case "add-trusted-cert":
				if f.denied {
					return "", errors.New("authorization cancelled in test")
				}
				f.trusted[fingerprint] = true
				return "", nil
			case "remove-trusted-cert":
				delete(f.trusted, fingerprint)
				if f.missingRemoval {
					return "", errors.New("SecTrustSettingsRemoveTrustSettings: The specified item could not be found in the keychain.")
				}
				return "", nil
			}
			return "", errors.New("test blocked unexpected keychain mutation")
		}
		return m
	}
	return f
}
func installExpiredUIAuthority(t *testing.T, root string) string {
	t.Helper()
	dir := filepath.Join(root, "proxy")
	if err := os.MkdirAll(dir, 0700); err != nil {
		t.Fatal(err)
	}
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	cert := &x509.Certificate{SerialNumber: big.NewInt(1), Subject: pkix.Name{CommonName: "Devbase Local Development CA"}, IsCA: true, BasicConstraintsValid: true, KeyUsage: x509.KeyUsageCertSign, NotBefore: time.Now().Add(-48 * time.Hour), NotAfter: time.Now().Add(-time.Hour)}
	der, err := x509.CreateCertificate(rand.Reader, cert, cert, &key.PublicKey, key)
	if err != nil {
		t.Fatal(err)
	}
	encoded, err := x509.MarshalECPrivateKey(key)
	if err != nil {
		t.Fatal(err)
	}
	for name, b := range map[string][]byte{"ca.pem": pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der}), "ca-key.pem": pem.EncodeToMemory(&pem.Block{Type: "EC PRIVATE KEY", Bytes: encoded})} {
		if err = os.WriteFile(filepath.Join(dir, name), b, 0600); err != nil {
			t.Fatal(err)
		}
	}
	fingerprint, err := uiCertificateFingerprint(filepath.Join(dir, "ca.pem"))
	if err != nil {
		t.Fatal(err)
	}
	return fingerprint
}

func TestUIAuthorityRepairTrustCancellationActivationRollbackAndTLS(t *testing.T) {
	for _, dark := range []bool{false, true} {
		t.Run(fmt.Sprintf("minimum-dark-%t", dark), func(t *testing.T) {
			h := newDevUIHarnessSize(t, 900, 520, dark)
			old := installExpiredUIAuthority(t, h.root)
			trust := fakeUIAuthorityTrust(h)
			upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { io.WriteString(w, "HTTPS after CA repair") }))
			defer upstream.Close()
			m := h.a.dev.managerFactory()
			defer m.StopProxy(context.Background())
			h.click("HTTPS")
			h.click("Local certificates")
			if h.a.dev.authorityStatus == nil || h.a.dev.authorityStatus.Active.Status != "Expired" || h.a.dev.authorityStatus.Active.Fingerprint != old {
				t.Fatal("UI did not identify expired authority", h.a.dev.output)
			}
			h.click("Copy active CA fingerprint")
			if len(h.copied) != 1 || h.copied[0] != old {
				t.Fatal("copied fingerprint is not the displayed authority")
			}
			h.click("Prepare replacement CA")
			r := h.a.dev.authorityStatus
			if r.Prepared == nil || r.Active.Fingerprint != old || r.Prepared.Fingerprint == old {
				t.Fatal("preparation replaced the active authority", h.a.dev.output)
			}
			repairedFingerprint := r.Prepared.Fingerprint
			h.click("Activate prepared CA")
			if h.a.dev.authorityStatus.Active.Fingerprint != old {
				t.Fatal("untrusted CA activated")
			}
			trust.deny(true)
			h.click("Trust prepared CA")
			if !strings.Contains(h.a.dev.output, "authorization cancelled") || h.a.dev.authorityStatus.Active.Fingerprint != old {
				t.Fatal("authorization cancellation did not preserve old authority", h.a.dev.output)
			}
			trust.deny(false)
			h.click("Trust prepared CA")
			h.click("Activate prepared CA")
			r = h.a.dev.authorityStatus
			if r.Active.Fingerprint != repairedFingerprint || !r.Active.Usable || r.Prepared != nil {
				t.Fatal("replacement not activated", h.a.dev.output)
			}
			h.click("Back to HTTPS rules")
			port := freeUIPort(t)
			h.fill("HTTPS listener port", fmt.Sprint(port))
			h.click("Add rule")
			h.fill("Domain", "ca-repair.test")
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
			check := func() {
				t.Helper()
				transport := &http.Transport{TLSClientConfig: &tls.Config{RootCAs: pool, ServerName: "ca-repair.test"}}
				defer transport.CloseIdleConnections()
				client := &http.Client{Transport: transport, Timeout: 3 * time.Second}
				req, _ := http.NewRequest("GET", fmt.Sprintf("https://127.0.0.1:%d", port), nil)
				req.Host = "ca-repair.test"
				res, err := client.Do(req)
				if err != nil {
					t.Fatal(err)
				}
				body, _ := io.ReadAll(res.Body)
				res.Body.Close()
				if res.StatusCode != 200 || string(body) != "HTTPS after CA repair" {
					t.Fatal("new CA did not serve the configured site", string(body))
				}
			}
			check()
			h.click("Local certificates")
			h.click("Prepare replacement CA")
			h.click("Trust prepared CA")
			h.click("Activate prepared CA")
			if !strings.Contains(h.a.dev.output, "stop the HTTPS proxy") || h.a.dev.authorityStatus.Active.Fingerprint != repairedFingerprint {
				t.Fatal("running proxy changed authority", h.a.dev.output)
			}
			h.click("Stop HTTPS proxy")
			h.click("Activate prepared CA")
			r = h.a.dev.authorityStatus
			if r.Previous == nil || r.Previous.Fingerprint != repairedFingerprint {
				t.Fatal("valid previous authority not retained")
			}
			h.click("Use saved CA " + r.Previous.ID[:8])
			r = h.a.dev.authorityStatus
			if r.Active.Fingerprint != repairedFingerprint {
				t.Fatal("GUI rollback did not restore the selected authority", h.a.dev.output)
			}
			h.click("Remove saved CA trust " + r.Previous.ID[:8])
			h.click("Apply proxy")
			check()
			if strings.Contains(h.a.dev.output, "Error:") {
				t.Fatal(h.a.dev.output)
			}
		})
	}
}

func TestUIAuthorityRemovalFinishesAfterTrustSettingsAlreadyRemoved(t *testing.T) {
	for _, dark := range []bool{false, true} {
		t.Run(fmt.Sprintf("minimum-dark-%t", dark), func(t *testing.T) {
			h := newDevUIHarnessSize(t, 900, 520, dark)
			trust := fakeUIAuthorityTrust(h)
			h.click("HTTPS")
			h.click("Local certificates")
			h.click("Prepare replacement CA")
			h.click("Trust prepared CA")
			h.click("Activate prepared CA")
			if h.a.dev.authorityStatus.Active.Trust != "Trusted" {
				t.Fatal(h.a.dev.output)
			}
			trust.mu.Lock()
			trust.missingRemoval = true
			trust.mu.Unlock()
			h.click("Remove CA trust")
			if h.a.dev.authorityStatus.Active.Trust != "Not trusted" || strings.Contains(h.a.dev.output, "Error:") {
				t.Fatal("partial cleanup did not finish in UI", h.a.dev.output)
			}
			trust.mu.Lock()
			deleted := trust.deleteCount
			trust.mu.Unlock()
			if deleted != 1 {
				t.Fatal("cleanup did not reach exact certificate deletion", deleted)
			}
		})
	}
}

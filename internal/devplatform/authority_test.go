package devplatform

import (
	"context"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/sha1"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/hex"
	"encoding/json"
	"encoding/pem"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"
)

type authorityTrustFixture struct {
	trusted       map[string]bool
	deny, running bool
	removed       []string
}

func certificateAt(t *testing.T, path string) *x509.Certificate {
	t.Helper()
	b, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	block, _ := pem.Decode(b)
	if block == nil {
		t.Fatal("certificate missing")
	}
	cert, err := x509.ParseCertificate(block.Bytes)
	if err != nil {
		t.Fatal(err)
	}
	return cert
}
func fakeAuthorityTrust(t *testing.T, m *Manager) *authorityTrustFixture {
	t.Helper()
	f := &authorityTrustFixture{trusted: map[string]bool{}}
	m.Run = func(_ context.Context, name string, args ...string) (string, error) {
		if filepath.Base(name) == "launchctl" {
			if args[0] != "print" {
				t.Fatal("authority switched a process", args)
			}
			if f.running {
				return "state = running", nil
			}
			return "", errors.New("Could not find service")
		}
		if name != "/usr/bin/security" {
			t.Fatal("unexpected authority command", name, args)
		}
		switch args[0] {
		case "verify-cert":
			cert := certificateAt(t, args[2])
			now := time.Now()
			if f.trusted[digestBytes(cert.Raw)] && !now.Before(cert.NotBefore) && now.Before(cert.NotAfter) {
				return "", nil
			}
			return "", errors.New("not trusted")
		case "default-keychain":
			return `"/tmp/service-manager-test-login.keychain-db"`, nil
		case "add-trusted-cert":
			if f.deny {
				return "", errors.New("authorization cancelled")
			}
			cert := certificateAt(t, args[len(args)-1])
			f.trusted[digestBytes(cert.Raw)] = true
			return "", nil
		case "remove-trusted-cert":
			cert := certificateAt(t, args[1])
			delete(f.trusted, digestBytes(cert.Raw))
			return "", nil
		case "delete-certificate":
			f.removed = append(f.removed, args[2])
			return "", nil
		}
		t.Fatal("unexpected security operation", args)
		return "", nil
	}
	return f
}
func legacyAuthorityFixture(t *testing.T, expires time.Time, names ...string) (*Manager, *authorityTrustFixture, string) {
	t.Helper()
	m := testManager(t)
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	name := authorityName
	if len(names) > 0 {
		name = names[0]
	}
	template := &x509.Certificate{SerialNumber: serial(), Subject: pkix.Name{CommonName: name}, NotBefore: time.Now().Add(-48 * time.Hour), NotAfter: expires, IsCA: true, BasicConstraintsValid: true, KeyUsage: x509.KeyUsageCertSign}
	der, err := x509.CreateCertificate(rand.Reader, template, template, &key.PublicKey, key)
	if err != nil {
		t.Fatal(err)
	}
	encoded, err := x509.MarshalECPrivateKey(key)
	if err != nil {
		t.Fatal(err)
	}
	if err = atomicWrite(filepath.Join(m.authorityRoot(), "ca.pem"), pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der}), 0600); err != nil {
		t.Fatal(err)
	}
	if err = atomicWrite(filepath.Join(m.authorityRoot(), "ca-key.pem"), pem.EncodeToMemory(&pem.Block{Type: "EC PRIVATE KEY", Bytes: encoded}), 0600); err != nil {
		t.Fatal(err)
	}
	f := fakeAuthorityTrust(t, m)
	f.trusted[digestBytes(der)] = true
	return m, f, strings.ToUpper(digestBytes(der))
}
func TestAuthorityStatusDoesNotCreateKeysOrRevealPrivateMaterial(t *testing.T) {
	m := New(filepath.Join(t.TempDir(), "absent"))
	f := fakeAuthorityTrust(t, m)
	_ = f
	r, err := m.AuthorityStatus(context.Background())
	if err != nil || r.Active.Status != "Not created" {
		t.Fatal(r, err)
	}
	if _, err = os.Stat(m.Root); !errors.Is(err, os.ErrNotExist) {
		t.Fatal("inspection created storage")
	}
	if _, _, err = m.ca(); err != nil {
		t.Fatal(err)
	}
	r, err = m.AuthorityStatus(context.Background())
	if err != nil || !r.Active.Usable || r.Active.Trust != "Not trusted" {
		t.Fatal(r, err)
	}
	b, err := json.Marshal(r)
	if err != nil || strings.Contains(string(b), "PRIVATE KEY") {
		t.Fatal("status exported a private key", err)
	}
	for _, name := range []string{"ca.pem", "ca-key.pem"} {
		info, err := os.Stat(filepath.Join(m.authorityRoot(), name))
		if err != nil || info.Mode().Perm() != 0600 {
			t.Fatal("authority permissions", name, err)
		}
	}
}
func TestAuthorityReplacementRequiresTrustStoppedProxyAndCurrentRevision(t *testing.T) {
	m, f, old := legacyAuthorityFixture(t, time.Now().Add(90*24*time.Hour))
	if err := m.PrepareAuthority(context.Background()); err != nil {
		t.Fatal(err)
	}
	r, err := m.AuthorityStatus(context.Background())
	if err != nil || r.Active.Fingerprint != old || r.Prepared == nil || r.Prepared.Fingerprint == old {
		t.Fatal(r, err)
	}
	if err = m.ActivateAuthority(context.Background(), r.Revision); err == nil {
		t.Fatal("untrusted replacement activated")
	}
	f.deny = true
	if err = m.TrustPreparedAuthority(context.Background(), r.Revision); err == nil {
		t.Fatal("cancelled authorization succeeded")
	}
	current, _ := m.AuthorityStatus(context.Background())
	if current.Active.Fingerprint != old {
		t.Fatal("trust failure replaced the active authority")
	}
	f.deny = false
	if err = m.TrustPreparedAuthority(context.Background(), r.Revision); err != nil {
		t.Fatal(err)
	}
	f.running = true
	if err = m.ActivateAuthority(context.Background(), r.Revision); err == nil {
		t.Fatal("running proxy changed authority")
	}
	f.running = false
	if err = m.ActivateAuthority(context.Background(), "stale"); err == nil {
		t.Fatal("stale authority preview accepted")
	}
	if err = m.ActivateAuthority(context.Background(), r.Revision); err != nil {
		t.Fatal(err)
	}
	current, err = m.AuthorityStatus(context.Background())
	if err != nil || current.Active.Fingerprint != r.Prepared.Fingerprint || current.Prepared != nil || current.Previous == nil || current.Previous.Fingerprint != old {
		t.Fatal(current, err)
	}
	if err = m.RestoreAuthority(context.Background(), current.Previous.ID, old, current.Revision); err != nil {
		t.Fatal(err)
	}
	restored, err := m.AuthorityStatus(context.Background())
	if err != nil || restored.Active.Fingerprint != old {
		t.Fatal(restored, err)
	}
	previous := restored.Previous
	if err = m.RemoveArchivedAuthorityTrust(context.Background(), previous.ID, previous.Fingerprint); err != nil {
		t.Fatal(err)
	}
	cert := certificateAt(t, previous.CertificatePath)
	sum := sha1.Sum(cert.Raw)
	if len(f.removed) != 1 || f.removed[0] != strings.ToUpper(hex.EncodeToString(sum[:])) {
		t.Fatal("retirement was not pinned to the archived certificate", f.removed)
	}
	if !f.trusted[strings.ToLower(old)] {
		t.Fatal("retiring an archive removed active trust")
	}
}
func TestExpiredAuthorityRepairAndLeafLifetime(t *testing.T) {
	m, _, old := legacyAuthorityFixture(t, time.Now().Add(-time.Hour))
	r, err := m.AuthorityStatus(context.Background())
	if err != nil || r.Active.Status != "Expired" {
		t.Fatal(r, err)
	}
	if _, _, err = m.ca(); err == nil {
		t.Fatal("expired CA used for new leaf")
	}
	if err = m.PrepareAuthority(context.Background()); err != nil {
		t.Fatal(err)
	}
	r, _ = m.AuthorityStatus(context.Background())
	if err = m.TrustPreparedAuthority(context.Background(), r.Revision); err != nil {
		t.Fatal(err)
	}
	if err = m.ActivateAuthority(context.Background(), r.Revision); err != nil {
		t.Fatal(err)
	}
	r, err = m.AuthorityStatus(context.Background())
	if err != nil || !r.Active.Usable || r.Active.Fingerprint == old {
		t.Fatal(r, err)
	}
	if err = m.RestoreAuthority(context.Background(), r.Previous.ID, old, r.Revision); err == nil {
		t.Fatal("expired archive restored")
	}
	near, _, _ := legacyAuthorityFixture(t, time.Now().Add(7*24*time.Hour))
	leaf, err := near.leaf("near-expiry.devbase.localhost")
	if err != nil {
		t.Fatal(err)
	}
	ca, _, err := near.ca()
	if err != nil {
		t.Fatal(err)
	}
	if leaf.Leaf.NotAfter.After(ca.NotAfter) {
		t.Fatal("leaf lifetime exceeds authority lifetime")
	}
}
func TestAuthorityCorruptKeyAndRecordCanBeReplacedThroughController(t *testing.T) {
	for _, name := range []string{"ca-key.pem", "metadata.json"} {
		t.Run(name, func(t *testing.T) {
			m := testManager(t)
			fakeAuthorityTrust(t, m)
			if _, _, err := m.ca(); err != nil {
				t.Fatal(err)
			}
			id, err := m.readAuthorityPointer(authorityCurrent)
			if err != nil {
				t.Fatal(err)
			}
			if err = os.WriteFile(filepath.Join(m.authorityDir(id), name), []byte("damaged"), 0600); err != nil {
				t.Fatal(err)
			}
			r, err := m.AuthorityStatus(context.Background())
			if err != nil || r.Active.Usable {
				t.Fatal("damaged material reported usable", r, err)
			}
			if err = m.PrepareAuthority(context.Background()); err != nil {
				t.Fatal(err)
			}
			r, _ = m.AuthorityStatus(context.Background())
			if err = m.TrustPreparedAuthority(context.Background(), r.Revision); err != nil {
				t.Fatal(err)
			}
			if err = m.ActivateAuthority(context.Background(), r.Revision); err != nil {
				t.Fatal(err)
			}
			if _, _, err = m.ca(); err != nil {
				t.Fatal("controller did not repair damaged authority", err)
			}
		})
	}
}
func TestAuthorityInitialCreationConcurrentReadersUseOneGeneration(t *testing.T) {
	m := testManager(t)
	var wg sync.WaitGroup
	values := make(chan string, 16)
	errs := make(chan error, 16)
	for range 16 {
		wg.Go(func() {
			cert, _, err := m.ca()
			if err != nil {
				errs <- err
				return
			}
			values <- digestBytes(cert.Raw)
		})
	}
	wg.Wait()
	close(values)
	close(errs)
	for err := range errs {
		t.Error(err)
	}
	var expected string
	for value := range values {
		if expected == "" {
			expected = value
		}
		if expected != value {
			t.Fatal("concurrent setup created different active authorities")
		}
	}
}
func TestAuthorityActivationCrashChild(t *testing.T) {
	root := os.Getenv("MSM_CA_CRASH_ROOT")
	if root == "" {
		return
	}
	m := New(root)
	fakeAuthorityTrust(t, m)
	r, err := m.AuthorityStatus(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if err = m.TrustPreparedAuthority(context.Background(), r.Revision); err != nil {
		t.Fatal(err)
	}
	m.Event = func(message string) {
		if message == "Authority activation committed" {
			os.Exit(23)
		}
	}
	if err = m.ActivateAuthority(context.Background(), r.Revision); err != nil {
		t.Fatal(err)
	}
	t.Fatal("activation crash point not reached")
}
func TestAuthorityActivationCanFinishAfterProcessExit(t *testing.T) {
	m, _, _ := legacyAuthorityFixture(t, time.Now().Add(-time.Hour))
	if err := m.PrepareAuthority(context.Background()); err != nil {
		t.Fatal(err)
	}
	c := exec.Command(os.Args[0], "-test.run=^TestAuthorityActivationCrashChild$")
	c.Env = append(os.Environ(), "MSM_CA_CRASH_ROOT="+m.Root)
	out, err := c.CombinedOutput()
	var exited *exec.ExitError
	if !errors.As(err, &exited) || exited.ExitCode() != 23 {
		t.Fatalf("crash helper: %v %s", err, out)
	}
	m = New(m.Root)
	fakeAuthorityTrust(t, m)
	r, err := m.AuthorityStatus(context.Background())
	if err != nil || !r.PreparedIsActive || r.Prepared == nil {
		t.Fatal("committed activation was lost", r, err)
	}
	if err = m.ActivateAuthority(context.Background(), r.Revision); err != nil {
		t.Fatal(err)
	}
	r, err = m.AuthorityStatus(context.Background())
	if err != nil || r.Prepared != nil || !r.Active.Usable {
		t.Fatal(r, err)
	}
}

func TestAuthorityMissingSelectionRestoresSavedGenerationWithoutNewKeys(t *testing.T) {
	m := testManager(t)
	trust := fakeAuthorityTrust(t, m)
	cert, _, err := m.ca()
	if err != nil {
		t.Fatal(err)
	}
	trust.trusted[digestBytes(cert.Raw)] = true
	id, err := m.readAuthorityPointer(authorityCurrent)
	if err != nil {
		t.Fatal(err)
	}
	if err = os.Remove(filepath.Join(m.authorityRoot(), authorityCurrent)); err != nil {
		t.Fatal(err)
	}
	r, err := m.AuthorityStatus(context.Background())
	if err != nil || r.Active.Status != "Not created" || len(r.Archives) != 1 {
		t.Fatal(r, err)
	}
	if _, _, err = m.ca(); err == nil {
		t.Fatal("missing selection silently minted a different CA")
	}
	if err = m.RestoreAuthority(context.Background(), id, strings.ToUpper(digestBytes(cert.Raw)), r.Revision); err != nil {
		t.Fatal(err)
	}
	restored, _, err := m.ca()
	if err != nil || !restored.Equal(cert) {
		t.Fatal("saved authority not restored", err)
	}
}
func TestAuthorityMissingExportResumesInitialSetupAndOrphanKeyIsKept(t *testing.T) {
	m := testManager(t)
	cert, _, err := m.ca()
	if err != nil {
		t.Fatal(err)
	}
	if err = os.Remove(filepath.Join(m.authorityRoot(), "ca-key.pem")); err != nil {
		t.Fatal(err)
	}
	resumed, _, err := m.ca()
	if err != nil || !resumed.Equal(cert) {
		t.Fatal("initial export repair changed authority", err)
	}
	orphan := testManager(t)
	path := filepath.Join(orphan.authorityRoot(), "ca-key.pem")
	if err = os.WriteFile(path, []byte("preserve orphaned key"), 0600); err != nil {
		t.Fatal(err)
	}
	if _, _, err = orphan.ca(); err == nil {
		t.Fatal("orphaned key overwritten during automatic setup")
	}
	b, err := os.ReadFile(path)
	if err != nil || string(b) != "preserve orphaned key" {
		t.Fatal("orphan key lost", err)
	}
}
func TestAuthorityRefusesExternalGenerationStore(t *testing.T) {
	m := testManager(t)
	outside := t.TempDir()
	if err := os.Symlink(outside, filepath.Join(m.authorityRoot(), "ca-generations")); err != nil {
		t.Fatal(err)
	}
	if _, _, err := m.ca(); err == nil {
		t.Fatal("external generation store accepted")
	}
	entries, err := os.ReadDir(outside)
	if err != nil || len(entries) != 0 {
		t.Fatal("authority material written outside managed storage", err)
	}
}

func TestAuthorityForeignCertificateIsNotTrustedOrRemoved(t *testing.T) {
	m, _, _ := legacyAuthorityFixture(t, time.Now().Add(90*24*time.Hour), "Another application authority")
	called := false
	m.Run = func(context.Context, string, ...string) (string, error) { called = true; return "", nil }
	status, err := m.AuthorityStatus(context.Background())
	if err != nil || status.Active.Usable {
		t.Fatal(status, err)
	}
	if err = m.ConfigureTrust(context.Background(), false); err == nil {
		t.Fatal("foreign authority trusted")
	}
	if err = m.ConfigureTrust(context.Background(), true); err == nil {
		t.Fatal("foreign authority trust removed")
	}
	if called {
		t.Fatal("foreign certificate invoked keychain operations")
	}
}

func TestAuthorityRemovalResumesAfterTrustSettingsWereAlreadyRemoved(t *testing.T) {
	for _, certificateMissing := range []bool{false, true} {
		t.Run(fmt.Sprint(certificateMissing), func(t *testing.T) {
			m, _, _ := legacyAuthorityFixture(t, time.Now().AddDate(1, 0, 0))
			active, e := m.readActiveAuthority()
			if e != nil {
				t.Fatal(e)
			}
			sum := sha1.Sum(active.cert.Raw)
			wanted := strings.ToUpper(hex.EncodeToString(sum[:]))
			deleted := false
			m.Run = func(_ context.Context, name string, args ...string) (string, error) {
				if name != "/usr/bin/security" {
					t.Fatal(name, args)
				}
				switch args[0] {
				case "default-keychain":
					return `"/tmp/owned-login.keychain-db"`, nil
				case "remove-trusted-cert":
					if args[1] != active.info.CertificatePath {
						t.Fatal("wrong authority", args)
					}
					return "", errors.New("SecTrustSettingsRemoveTrustSettings: The specified item could not be found in the keychain.")
				case "delete-certificate":
					if len(args) != 4 || args[1] != "-Z" || args[2] != wanted || args[3] != "/tmp/owned-login.keychain-db" {
						t.Fatal("not an exact certificate deletion", args)
					}
					deleted = true
					if certificateMissing {
						return "", errors.New("SecKeychainSearchCopyNext: The specified item could not be found in the keychain.")
					}
					return "", nil
				default:
					t.Fatal(name, args)
					return "", nil
				}
			}
			if e = m.ConfigureTrust(context.Background(), true); e != nil || !deleted {
				t.Fatal("partial cleanup could not finish", e, deleted)
			}
		})
	}
}
func TestAuthorityRemovalDoesNotDeleteCertificateAfterOtherTrustErrors(t *testing.T) {
	m, _, _ := legacyAuthorityFixture(t, time.Now().AddDate(1, 0, 0))
	m.Run = func(_ context.Context, _ string, args ...string) (string, error) {
		switch args[0] {
		case "default-keychain":
			return `"/tmp/owned-login.keychain-db"`, nil
		case "remove-trusted-cert":
			return "", errors.New("authorization cancelled")
		case "delete-certificate":
			t.Fatal("deleted after denied trust operation")
		}
		return "", nil
	}
	if e := m.ConfigureTrust(context.Background(), true); e == nil {
		t.Fatal("authorization failure hidden")
	}
}

func TestAuthorityStatusOrdersPreviousFirstThenNewestArchive(t *testing.T) {
	m := testManager(t)
	fakeAuthorityTrust(t, m)
	now := time.Now().UTC().Truncate(time.Second)

	makeGen := func(id string, notBefore, notAfter, createdAt time.Time) {
		dir := filepath.Join(m.authorityRoot(), "ca-generations", id)
		if err := os.MkdirAll(dir, 0700); err != nil {
			t.Fatal(err)
		}
		key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
		if err != nil {
			t.Fatal(err)
		}
		template := &x509.Certificate{
			SerialNumber:          serial(),
			Subject:               pkix.Name{CommonName: authorityName},
			NotBefore:             notBefore,
			NotAfter:              notAfter,
			IsCA:                  true,
			BasicConstraintsValid: true,
			KeyUsage:              x509.KeyUsageCertSign,
		}
		der, err := x509.CreateCertificate(rand.Reader, template, template, &key.PublicKey, key)
		if err != nil {
			t.Fatal(err)
		}
		encodedKey, err := x509.MarshalECPrivateKey(key)
		if err != nil {
			t.Fatal(err)
		}
		certPEM := pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der})
		keyPEM := pem.EncodeToMemory(&pem.Block{Type: "EC PRIVATE KEY", Bytes: encodedKey})
		if err := os.WriteFile(filepath.Join(dir, "ca.pem"), certPEM, 0600); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(dir, "ca-key.pem"), keyPEM, 0600); err != nil {
			t.Fatal(err)
		}
		record := authorityRecord{
			Schema:     1,
			ID:         id,
			Kind:       "legacy",
			CreatedAt:  createdAt,
			PairDigest: authorityPairDigest(certPEM, keyPEM),
		}
		if err := writeJSON(filepath.Join(dir, "metadata.json"), record); err != nil {
			t.Fatal(err)
		}
	}

	// 1. Expired authority with lexicographically smallest ID and oldest CreatedAt
	makeGen("0000000000000001", now.Add(-48*time.Hour), now.Add(-24*time.Hour), now.Add(-48*time.Hour))
	// 2. Middle archived generation with newer CreatedAt than 0000000000000001
	makeGen("4444444444444444", now.Add(-24*time.Hour), now.Add(24*time.Hour), now.Add(-12*time.Hour))
	// 3. Active authority
	makeGen("8888888888888888", now.Add(-12*time.Hour), now.Add(48*time.Hour), now.Add(-6*time.Hour))
	// 4. Valid previous authority with lexicographically largest ID
	makeGen("ffffffffffffffff", now.Add(-36*time.Hour), now.Add(36*time.Hour), now.Add(-24*time.Hour))

	if err := m.writeAuthorityPointer(authorityCurrent, "8888888888888888"); err != nil {
		t.Fatal(err)
	}
	if err := m.writeAuthorityPointer(authorityPrevious, "ffffffffffffffff"); err != nil {
		t.Fatal(err)
	}

	status, err := m.AuthorityStatus(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	if len(status.Archives) != 3 {
		t.Fatalf("expected 3 archives, got %d", len(status.Archives))
	}
	if status.Archives[0].ID != "ffffffffffffffff" {
		t.Errorf("expected archive[0] to be previous ('ffffffffffffffff'), got %q", status.Archives[0].ID)
	}
	if status.Archives[1].ID != "4444444444444444" {
		t.Errorf("expected archive[1] to be newest non-previous ('4444444444444444'), got %q", status.Archives[1].ID)
	}
	if status.Archives[2].ID != "0000000000000001" {
		t.Errorf("expected archive[2] to be oldest non-previous ('0000000000000001'), got %q", status.Archives[2].ID)
	}
}

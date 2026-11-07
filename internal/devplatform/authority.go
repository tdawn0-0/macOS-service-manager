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
	"path/filepath"
	"strings"
	"syscall"
	"time"
)

const authorityCurrent = "ca-current"
const authorityPrepared = "ca-prepared"
const authorityPrevious = "ca-previous"
const authorityName = "Service Manager Local Development CA"

type AuthorityInfo struct {
	ID, Name, Status, Detail, Fingerprint, Trust, CertificatePath string
	NotBefore, NotAfter                                           time.Time
	Usable                                                        bool
}
type AuthorityStatus struct {
	CheckedAt        time.Time
	Active           AuthorityInfo
	Prepared         *AuthorityInfo
	Previous         *AuthorityInfo
	Archives         []AuthorityInfo
	PreparedIsActive bool
	Revision         string
}
type authorityRecord struct {
	Schema     int       `json:"schema"`
	ID         string    `json:"id"`
	Kind       string    `json:"kind"`
	CreatedAt  time.Time `json:"createdAt"`
	PairDigest string    `json:"pairDigest"`
	BaseDigest string    `json:"baseDigest,omitempty"`
}
type authorityMaterial struct {
	info            AuthorityInfo
	cert            *x509.Certificate
	key             *ecdsa.PrivateKey
	certPEM, keyPEM []byte
	digest          string
	record          authorityRecord
}

func (m *Manager) authorityRoot() string { return filepath.Join(m.Root, "proxy") }
func (m *Manager) authorityDir(id string) string {
	return filepath.Join(m.authorityRoot(), "ca-generations", id)
}
func (m *Manager) validateAuthorityStore() error {
	info, err := os.Lstat(filepath.Join(m.authorityRoot(), "ca-generations"))
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	if err != nil {
		return err
	}
	if !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
		return errors.New("managed authority store must be a directory, not a link or special file")
	}
	return nil
}
func authorityPairDigest(cert, key []byte) string {
	return digestBytes([]byte(digestBytes(cert) + ":" + digestBytes(key)))
}
func parseAuthority(certPEM, keyPEM []byte, id, path string, now time.Time) authorityMaterial {
	a := authorityMaterial{info: AuthorityInfo{ID: id, Status: "Invalid", Trust: "Not checked", CertificatePath: path}, certPEM: certPEM, keyPEM: keyPEM, digest: authorityPairDigest(certPEM, keyPEM)}
	if len(certPEM) == 0 && len(keyPEM) == 0 {
		a.info.Status = "Not created"
		a.info.Detail = "No local authority is stored."
		return a
	}
	block, _ := pem.Decode(certPEM)
	if block == nil {
		a.info.Detail = "The CA certificate is missing or invalid. Prepare a replacement."
		return a
	}
	cert, err := x509.ParseCertificate(block.Bytes)
	if err != nil {
		a.info.Detail = "The CA certificate cannot be parsed."
		return a
	}
	a.cert = cert
	a.info.Name = visiblePortText(cert.Subject.CommonName)
	a.info.Fingerprint = strings.ToUpper(digestBytes(cert.Raw))
	a.info.NotBefore = cert.NotBefore
	a.info.NotAfter = cert.NotAfter
	if cert.Subject.CommonName != authorityName {
		a.info.Detail = "The stored certificate is not this app's development authority. Prepare a replacement."
		return a
	}
	if !cert.IsCA || cert.CheckSignatureFrom(cert) != nil {
		a.info.Detail = "The stored certificate is not a valid self-signed authority."
		return a
	}
	block, _ = pem.Decode(keyPEM)
	if block == nil {
		a.info.Detail = "The CA private key is missing or invalid. Prepare a replacement."
		return a
	}
	a.key, err = x509.ParseECPrivateKey(block.Bytes)
	if err != nil {
		a.info.Detail = "The CA private key cannot be parsed."
		return a
	}
	public, ok := cert.PublicKey.(*ecdsa.PublicKey)
	if !ok || !public.Equal(&a.key.PublicKey) {
		a.info.Detail = "The stored certificate and private key do not match."
		return a
	}
	if now.Before(cert.NotBefore) {
		a.info.Status = "Not yet valid"
		a.info.Detail = "Check the system clock before replacing the authority."
		return a
	}
	if !now.Before(cert.NotAfter) {
		a.info.Status = "Expired"
		a.info.Detail = "Prepare, trust and activate a replacement authority."
		return a
	}
	a.info.Usable = true
	a.info.Status = "Ready"
	a.info.Detail = "Certificate and key match and are within their validity period."
	if cert.NotAfter.Sub(now) < 30*24*time.Hour {
		a.info.Status = "Expiring soon"
		a.info.Detail = "The authority expires within 30 days. Prepare its replacement before expiry."
	}
	return a
}
func newAuthorityBytes(now time.Time) ([]byte, []byte, error) {
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		return nil, nil, err
	}
	template := &x509.Certificate{SerialNumber: serial(), Subject: pkix.Name{CommonName: authorityName}, NotBefore: now.Add(-time.Hour), NotAfter: now.AddDate(5, 0, 0), IsCA: true, BasicConstraintsValid: true, KeyUsage: x509.KeyUsageCertSign | x509.KeyUsageCRLSign}
	der, err := x509.CreateCertificate(rand.Reader, template, template, &key.PublicKey, key)
	if err != nil {
		return nil, nil, err
	}
	encoded, err := x509.MarshalECPrivateKey(key)
	if err != nil {
		return nil, nil, err
	}
	return pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der}), pem.EncodeToMemory(&pem.Block{Type: "EC PRIVATE KEY", Bytes: encoded}), nil
}
func (m *Manager) readAuthorityPointer(name string) (string, error) {
	path := filepath.Join(m.authorityRoot(), name)
	info, err := os.Lstat(path)
	if errors.Is(err, os.ErrNotExist) {
		return "", nil
	}
	if err != nil {
		return "", err
	}
	if info.Mode()&os.ModeSymlink == 0 {
		return "", fmt.Errorf("%s must be a managed authority link", name)
	}
	target, err := os.Readlink(path)
	if err != nil {
		return "", err
	}
	id := filepath.Base(target)
	if validID(id) != nil || target != filepath.Join("ca-generations", id) {
		return "", fmt.Errorf("%s points outside the managed authority store", name)
	}
	if err = m.validateAuthorityStore(); err != nil {
		return "", err
	}
	info, err = os.Lstat(m.authorityDir(id))
	if errors.Is(err, os.ErrNotExist) {
		return id, nil
	}
	if err != nil {
		return "", err
	}
	if !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
		return "", errors.New("authority generation must be a directory")
	}
	return id, nil
}
func (m *Manager) readAuthorityGeneration(id string) (authorityMaterial, error) {
	if err := validID(id); err != nil {
		return authorityMaterial{}, err
	}
	if err := m.validateAuthorityStore(); err != nil {
		return authorityMaterial{}, err
	}
	dir := m.authorityDir(id)
	info, err := os.Lstat(dir)
	if errors.Is(err, os.ErrNotExist) {
		return authorityMaterial{info: AuthorityInfo{ID: id, Status: "Invalid", Trust: "Not checked", CertificatePath: filepath.Join(dir, "ca.pem"), Detail: "Stored authority files are missing. Choose a saved authority or prepare a replacement."}, digest: digestBytes([]byte("missing:" + id))}, nil
	}
	if err != nil {
		return authorityMaterial{}, err
	}
	if !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
		return authorityMaterial{}, errors.New("authority generation must be a directory")
	}
	read := func(name string) ([]byte, error) {
		b, err := readExportFile(dir, name, 1<<20)
		if errors.Is(err, os.ErrNotExist) {
			return nil, nil
		}
		return b, err
	}
	cert, err := read("ca.pem")
	if err != nil {
		return authorityMaterial{}, err
	}
	key, err := read("ca-key.pem")
	if err != nil {
		return authorityMaterial{}, err
	}
	a := parseAuthority(cert, key, id, filepath.Join(dir, "ca.pem"), time.Now())
	metadata, err := readExportFile(dir, "metadata.json", 1<<20)
	if err != nil || json.Unmarshal(metadata, &a.record) != nil || a.record.Schema != 1 || a.record.ID != id || a.record.PairDigest != a.digest {
		a.info.Status = "Invalid"
		a.info.Usable = false
		a.info.Detail = "Authority material or its generation record changed. Prepare a replacement."
	}
	return a, nil
}
func (m *Manager) readActiveAuthority() (authorityMaterial, error) {
	id, err := m.readAuthorityPointer(authorityCurrent)
	if err != nil {
		return authorityMaterial{}, err
	}
	if id != "" {
		return m.readAuthorityGeneration(id)
	}
	read := func(name string) ([]byte, error) {
		path := filepath.Join(m.authorityRoot(), name)
		if target, err := os.Readlink(path); err == nil && target == filepath.Join(authorityCurrent, name) {
			return nil, nil
		}
		b, err := readExportFile(m.authorityRoot(), name, 1<<20)
		if errors.Is(err, os.ErrNotExist) {
			return nil, nil
		}
		return b, err
	}
	cert, err := read("ca.pem")
	if err != nil {
		return authorityMaterial{}, err
	}
	key, err := read("ca-key.pem")
	if err != nil {
		return authorityMaterial{}, err
	}
	return parseAuthority(cert, key, "", filepath.Join(m.authorityRoot(), "ca.pem"), time.Now()), nil
}
func (m *Manager) authorityAliasesValid() error {
	for _, name := range []string{"ca.pem", "ca-key.pem"} {
		target, err := os.Readlink(filepath.Join(m.authorityRoot(), name))
		if err != nil || target != filepath.Join(authorityCurrent, name) {
			return errors.New("public CA files do not match the managed authority links; repair or complete initial setup")
		}
	}
	return nil
}
func (m *Manager) withAuthorityLock(ctx context.Context, fn func() error) error {
	if err := m.init(); err != nil {
		return err
	}
	fd, err := syscall.Open(filepath.Join(m.authorityRoot(), "ca.lock"), syscall.O_CREAT|syscall.O_RDWR|syscall.O_NOFOLLOW|syscall.O_NONBLOCK, 0600)
	if err != nil {
		return err
	}
	f := os.NewFile(uintptr(fd), "authority lock")
	defer f.Close()
	info, err := f.Stat()
	if err != nil || !info.Mode().IsRegular() {
		return errors.New("invalid authority lock")
	}
	for {
		if err = ctx.Err(); err != nil {
			return err
		}
		err = syscall.Flock(fd, syscall.LOCK_EX|syscall.LOCK_NB)
		if err == nil {
			break
		}
		if err != syscall.EWOULDBLOCK {
			return err
		}
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(20 * time.Millisecond):
		}
	}
	defer syscall.Flock(fd, syscall.LOCK_UN)
	return fn()
}
func (m *Manager) writeAuthorityPointer(name, id string) error {
	if validID(id) != nil {
		return errors.New("invalid authority ID")
	}
	path := filepath.Join(m.authorityRoot(), name)
	if info, err := os.Lstat(path); err == nil && info.Mode()&os.ModeSymlink == 0 {
		return errors.New("refusing to replace an unmanaged authority pointer")
	} else if err != nil && !errors.Is(err, os.ErrNotExist) {
		return err
	}
	tmp := filepath.Join(m.authorityRoot(), ".ca-link-"+randomID())
	defer os.Remove(tmp)
	if err := os.Symlink(filepath.Join("ca-generations", id), tmp); err != nil {
		return err
	}
	if err := os.Rename(tmp, path); err != nil {
		return err
	}
	return syncExportDirectory(m.authorityRoot())
}
func (m *Manager) ensureAuthorityAliases(replace bool) error {
	for _, name := range []string{"ca.pem", "ca-key.pem"} {
		path := filepath.Join(m.authorityRoot(), name)
		target := filepath.Join(authorityCurrent, name)
		info, err := os.Lstat(path)
		if err == nil && info.Mode()&os.ModeSymlink != 0 {
			actual, _ := os.Readlink(path)
			if actual == target {
				continue
			}
		}
		if errors.Is(err, os.ErrNotExist) {
			if err = os.Symlink(target, path); err != nil {
				return err
			}
			continue
		}
		if err != nil {
			return err
		}
		if !replace || !info.Mode().IsRegular() && info.Mode()&os.ModeSymlink == 0 {
			return errors.New("CA export path is not a replaceable managed file")
		}
		tmp := filepath.Join(m.authorityRoot(), ".ca-alias-"+randomID())
		if err = os.Symlink(target, tmp); err != nil {
			return err
		}
		err = os.Rename(tmp, path)
		_ = os.Remove(tmp)
		if err != nil {
			return err
		}
	}
	return syncExportDirectory(m.authorityRoot())
}
func (m *Manager) createAuthorityGeneration(cert, key []byte, kind, base string) (string, error) {
	id := randomID()
	parent := filepath.Join(m.authorityRoot(), "ca-generations")
	if err := os.MkdirAll(parent, 0700); err != nil {
		return "", err
	}
	if err := m.validateAuthorityStore(); err != nil {
		return "", err
	}
	stage, err := os.MkdirTemp(parent, ".prepare-")
	if err != nil {
		return "", err
	}
	defer os.RemoveAll(stage)
	for name, data := range map[string][]byte{"ca.pem": cert, "ca-key.pem": key} {
		if len(data) > 0 {
			if err = atomicWrite(filepath.Join(stage, name), data, 0600); err != nil {
				return "", err
			}
		}
	}
	record := authorityRecord{Schema: 1, ID: id, Kind: kind, CreatedAt: time.Now().UTC(), PairDigest: authorityPairDigest(cert, key), BaseDigest: base}
	if err = writeJSON(filepath.Join(stage, "metadata.json"), record); err != nil {
		return "", err
	}
	if err = syncExportDirectory(stage); err != nil {
		return "", err
	}
	if err = os.Rename(stage, m.authorityDir(id)); err != nil {
		return "", err
	}
	return id, syncExportDirectory(parent)
}
func (m *Manager) loadOrCreateAuthority() (*x509.Certificate, *ecdsa.PrivateKey, error) {
	a, err := m.readActiveAuthority()
	if err == nil && a.info.Status != "Not created" {
		if !a.info.Usable {
			return nil, nil, errors.New(a.info.Detail)
		}
		if a.info.ID == "" || m.authorityAliasesValid() == nil {
			return a.cert, a.key, nil
		}
	}
	var result authorityMaterial
	err = m.withAuthorityLock(m.Context, func() error {
		current, err := m.readActiveAuthority()
		if err != nil {
			return err
		}
		if current.info.Status == "Not created" {
			if err := m.validateAuthorityStore(); err != nil {
				return err
			}
			entries, readErr := os.ReadDir(filepath.Join(m.authorityRoot(), "ca-generations"))
			if readErr != nil && !errors.Is(readErr, os.ErrNotExist) {
				return readErr
			}
			for _, entry := range entries {
				if entry.IsDir() && validID(entry.Name()) == nil {
					return errors.New("stored authorities exist without an active selection; use Local certificates to restore one or prepare a replacement")
				}
			}
			if prepared, err := m.readAuthorityPointer(authorityPrepared); err != nil {
				return err
			} else if prepared != "" {
				return errors.New("complete the prepared authority replacement before applying HTTPS")
			}
			cert, key, err := newAuthorityBytes(time.Now())
			if err != nil {
				return err
			}
			id, err := m.createAuthorityGeneration(cert, key, "Initial", "")
			if err != nil {
				return err
			}
			if err = m.writeAuthorityPointer(authorityCurrent, id); err != nil {
				return err
			}
			current, err = m.readAuthorityGeneration(id)
			if err != nil {
				return err
			}
		}
		if !current.info.Usable {
			return errors.New(current.info.Detail)
		}
		if current.info.ID != "" {
			if err = m.ensureAuthorityAliases(false); err != nil {
				return err
			}
		}
		result = current
		return nil
	})
	return result.cert, result.key, err
}

func (m *Manager) authorityRevision(active authorityMaterial, preparedID string, prepared authorityMaterial) string {
	return digestBytes([]byte(filepath.Clean(m.Root) + "\x00" + active.digest + "\x00" + preparedID + "\x00" + prepared.digest + "\x00" + prepared.record.BaseDigest))
}
func (m *Manager) verifyAuthorityTrust(ctx context.Context, path string) string {
	query, cancel := context.WithTimeout(ctx, 2*time.Second)
	defer cancel()
	_, err := m.Run(query, "/usr/bin/security", "verify-cert", "-c", path, "-p", "basic", "-l")
	if query.Err() != nil {
		return "Unknown"
	}
	if err != nil {
		return "Not trusted"
	}
	return "Trusted"
}
func (m *Manager) AuthorityStatus(ctx context.Context) (AuthorityStatus, error) {
	if err := ctx.Err(); err != nil {
		return AuthorityStatus{}, err
	}
	active, err := m.readActiveAuthority()
	if err != nil {
		return AuthorityStatus{}, err
	}
	status := AuthorityStatus{CheckedAt: time.Now().UTC(), Active: active.info}
	if active.info.ID != "" {
		if err = m.authorityAliasesValid(); err != nil {
			status.Active.Status = "Invalid"
			status.Active.Usable = false
			status.Active.Detail = err.Error()
		}
	}
	if active.cert != nil && active.cert.Subject.CommonName == authorityName && active.cert.IsCA {
		status.Active.Trust = m.verifyAuthorityTrust(ctx, active.info.CertificatePath)
	}
	preparedID, err := m.readAuthorityPointer(authorityPrepared)
	if err != nil {
		return status, err
	}
	var prepared authorityMaterial
	if preparedID != "" {
		prepared, err = m.readAuthorityGeneration(preparedID)
		if err != nil {
			return status, err
		}
		info := prepared.info
		info.Trust = m.verifyAuthorityTrust(ctx, info.CertificatePath)
		status.Prepared = &info
		status.PreparedIsActive = preparedID == active.info.ID
	}
	status.Revision = m.authorityRevision(active, preparedID, prepared)
	previousID, err := m.readAuthorityPointer(authorityPrevious)
	if err != nil {
		return status, err
	}
	if err = m.validateAuthorityStore(); err != nil {
		return status, err
	}
	entries, err := os.ReadDir(filepath.Join(m.authorityRoot(), "ca-generations"))
	if err != nil && !errors.Is(err, os.ErrNotExist) {
		return status, err
	}
	for _, entry := range entries {
		if !entry.IsDir() || validID(entry.Name()) != nil || entry.Name() == active.info.ID || entry.Name() == preparedID {
			continue
		}
		item, itemErr := m.readAuthorityGeneration(entry.Name())
		if itemErr != nil {
			continue
		}
		status.Archives = append(status.Archives, item.info)
		if entry.Name() == previousID {
			info := item.info
			status.Previous = &info
		}
	}
	return status, ctx.Err()
}
func (m *Manager) PrepareAuthority(ctx context.Context) error {
	return m.withState(ctx, func(*State) error {
		return m.withAuthorityLock(ctx, func() error {
			id, err := m.readAuthorityPointer(authorityPrepared)
			if err != nil {
				return err
			}
			if id != "" {
				return errors.New("review or discard the already prepared authority first")
			}
			active, err := m.readActiveAuthority()
			if err != nil {
				return err
			}
			cert, key, err := newAuthorityBytes(time.Now())
			if err != nil {
				return err
			}
			id, err = m.createAuthorityGeneration(cert, key, "Replacement", active.digest)
			if err != nil {
				return err
			}
			if err = ctx.Err(); err != nil {
				return err
			}
			return m.writeAuthorityPointer(authorityPrepared, id)
		})
	})
}
func (m *Manager) checkedAuthorityRevision(revision string) (authorityMaterial, authorityMaterial, error) {
	active, err := m.readActiveAuthority()
	if err != nil {
		return active, authorityMaterial{}, err
	}
	id, err := m.readAuthorityPointer(authorityPrepared)
	if err != nil {
		return active, authorityMaterial{}, err
	}
	var prepared authorityMaterial
	if id != "" {
		prepared, err = m.readAuthorityGeneration(id)
		if err != nil {
			return active, prepared, err
		}
	}
	if revision == "" || revision != m.authorityRevision(active, id, prepared) {
		return active, prepared, errors.New("authority material changed; refresh and review its fingerprints again")
	}
	return active, prepared, nil
}
func (m *Manager) TrustPreparedAuthority(ctx context.Context, revision string) error {
	return m.withState(ctx, func(*State) error {
		return m.withAuthorityLock(ctx, func() error {
			active, prepared, err := m.checkedAuthorityRevision(revision)
			if err != nil {
				return err
			}
			if prepared.info.ID == "" || !prepared.info.Usable {
				return errors.New("prepare a valid replacement authority first")
			}
			if prepared.info.ID != active.info.ID && prepared.record.BaseDigest != active.digest {
				return errors.New("active authority changed since preparation; prepare a new replacement")
			}
			return m.configureAuthorityTrust(ctx, prepared, false)
		})
	})
}
func (m *Manager) requireProxyStopped(ctx context.Context) error {
	for _, target := range []string{domain() + "/" + m.label("proxy"), m.socketTarget()} {
		if err := m.verifyShutdownTarget(ctx, target); err != nil {
			return fmt.Errorf("stop the HTTPS proxy before switching authorities: %w", err)
		}
	}
	return nil
}
func (m *Manager) ActivateAuthority(ctx context.Context, revision string) error {
	return m.withState(ctx, func(*State) error {
		return m.withAuthorityLock(ctx, func() error {
			active, prepared, err := m.checkedAuthorityRevision(revision)
			if err != nil {
				return err
			}
			if prepared.info.ID == "" || !prepared.info.Usable {
				return errors.New("prepare a valid replacement authority first")
			}
			if prepared.info.ID == active.info.ID {
				if err = os.Remove(filepath.Join(m.authorityRoot(), authorityPrepared)); err != nil {
					return err
				}
				return syncExportDirectory(m.authorityRoot())
			}
			if prepared.record.BaseDigest != active.digest {
				return errors.New("active authority changed since preparation; prepare a new replacement")
			}
			if m.verifyAuthorityTrust(ctx, prepared.info.CertificatePath) != "Trusted" {
				return errors.New("trust the prepared CA and verify macOS trust before activation")
			}
			if err = m.requireProxyStopped(ctx); err != nil {
				return err
			}
			previous := active.info.ID
			if previous == "" {
				previous, err = m.createAuthorityGeneration(active.certPEM, active.keyPEM, "Archived", "")
				if err != nil {
					return err
				}
				if err = m.writeAuthorityPointer(authorityCurrent, previous); err != nil {
					return err
				}
			}
			if err = m.ensureAuthorityAliases(true); err != nil {
				return err
			}
			if err = m.writeAuthorityPointer(authorityPrevious, previous); err != nil {
				return err
			}
			if err = ctx.Err(); err != nil {
				return err
			}
			if err = m.writeAuthorityPointer(authorityCurrent, prepared.info.ID); err != nil {
				return err
			}
			m.emit("Authority activation committed")
			if err = os.Remove(filepath.Join(m.authorityRoot(), authorityPrepared)); err != nil {
				return fmt.Errorf("authority activated; refresh and finish activation: %w", err)
			}
			return syncExportDirectory(m.authorityRoot())
		})
	})
}
func (m *Manager) DiscardPreparedAuthority(ctx context.Context, revision string) error {
	return m.withState(ctx, func(*State) error {
		return m.withAuthorityLock(ctx, func() error {
			_, prepared, err := m.checkedAuthorityRevision(revision)
			if err != nil {
				return err
			}
			if prepared.info.ID == "" {
				return errors.New("no prepared authority")
			}
			if err = ctx.Err(); err != nil {
				return err
			}
			if err = os.Remove(filepath.Join(m.authorityRoot(), authorityPrepared)); err != nil {
				return err
			}
			return syncExportDirectory(m.authorityRoot())
		})
	})
}
func (m *Manager) RestoreAuthority(ctx context.Context, id, fingerprint, revision string) error {
	return m.withState(ctx, func(*State) error {
		return m.withAuthorityLock(ctx, func() error {
			active, prepared, err := m.checkedAuthorityRevision(revision)
			if err != nil {
				return err
			}
			if prepared.info.ID != "" {
				return errors.New("finish or discard the prepared replacement before restoring another authority")
			}
			previous, err := m.readAuthorityGeneration(id)
			if err != nil {
				return err
			}
			if !previous.info.Usable || previous.info.Fingerprint != fingerprint {
				return errors.New("selected authority is invalid, expired or changed")
			}
			if m.verifyAuthorityTrust(ctx, previous.info.CertificatePath) != "Trusted" {
				return errors.New("selected authority is not currently trusted by macOS")
			}
			if err = m.requireProxyStopped(ctx); err != nil {
				return err
			}
			if active.info.ID == "" {
				oldID, err := m.createAuthorityGeneration(active.certPEM, active.keyPEM, "Archived", "")
				if err != nil {
					return err
				}
				if err = m.writeAuthorityPointer(authorityCurrent, oldID); err != nil {
					return err
				}
				active.info.ID = oldID
			}
			if err = m.ensureAuthorityAliases(true); err != nil {
				return err
			}
			if err = m.writeAuthorityPointer(authorityPrevious, active.info.ID); err != nil {
				return err
			}
			if err = ctx.Err(); err != nil {
				return err
			}
			return m.writeAuthorityPointer(authorityCurrent, id)
		})
	})
}
func (m *Manager) RemoveArchivedAuthorityTrust(ctx context.Context, id, fingerprint string) error {
	return m.ConfigureSavedAuthorityTrust(ctx, id, fingerprint, true)
}
func (m *Manager) ConfigureSavedAuthorityTrust(ctx context.Context, id, fingerprint string, remove bool) error {
	return m.withState(ctx, func(*State) error {
		return m.withAuthorityLock(ctx, func() error {
			active, err := m.readAuthorityPointer(authorityCurrent)
			if err != nil {
				return err
			}
			prepared, err := m.readAuthorityPointer(authorityPrepared)
			if err != nil {
				return err
			}
			if id == active || id == prepared {
				return errors.New("use the active or prepared authority controls for this certificate")
			}
			archived, err := m.readAuthorityGeneration(id)
			if err != nil {
				return err
			}
			if archived.cert == nil || archived.info.Fingerprint != fingerprint {
				return errors.New("archived certificate changed; refresh before removing trust")
			}
			return m.configureAuthorityTrust(ctx, archived, remove)
		})
	})
}
func (m *Manager) configureAuthorityTrust(ctx context.Context, a authorityMaterial, remove bool) error {
	if a.cert == nil {
		return errors.New("a readable CA certificate is required")
	}
	if a.cert.Subject.CommonName != authorityName || !a.cert.IsCA || a.cert.CheckSignatureFrom(a.cert) != nil {
		return errors.New("only this app's stored self-signed authority can be managed here")
	}
	if !remove {
		if !a.info.Usable {
			return errors.New("repair the authority before trusting it")
		}
		if m.verifyAuthorityTrust(ctx, a.info.CertificatePath) == "Trusted" {
			return nil
		}
	}
	keychain, err := m.Run(ctx, "/usr/bin/security", "default-keychain", "-d", "user")
	if err != nil {
		return err
	}
	keychain = strings.Trim(strings.TrimSpace(keychain), "\"")
	if !filepath.IsAbs(keychain) {
		return errors.New("cannot locate the user keychain")
	}
	if remove {
		if _, err = m.Run(ctx, "/usr/bin/security", "remove-trusted-cert", a.info.CertificatePath); err != nil && !keychainItemMissing(err) {
			return err
		}
		sum := sha1.Sum(a.cert.Raw)
		_, err = m.Run(ctx, "/usr/bin/security", "delete-certificate", "-Z", strings.ToUpper(hex.EncodeToString(sum[:])), keychain)
		if keychainItemMissing(err) {
			return nil
		}
		return err
	}
	if _, err = m.Run(ctx, "/usr/bin/security", "add-trusted-cert", "-r", "trustRoot", "-k", keychain, a.info.CertificatePath); err != nil {
		return err
	}
	if m.verifyAuthorityTrust(ctx, a.info.CertificatePath) != "Trusted" {
		return errors.New("macOS did not verify the CA after the trust request; the active authority was kept")
	}
	return nil
}

// A previous attempt may have removed trust before its caller exited. Continue
// with the exact certificate cleanup only for macOS's explicit not-found error.
func keychainItemMissing(err error) bool {
	return err != nil && strings.Contains(strings.ToLower(err.Error()), "the specified item could not be found in the keychain")
}

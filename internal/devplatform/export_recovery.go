package devplatform

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"syscall"
	"time"
)

const exportRecordName = ".devmanager-export.json"
const exportFileLimit = 8 << 20

var exportFileNames = [2]string{"devmanager.lock.json", "devmanager.json"}

// The journal contains only the sanitized new export and hashes of prior files.
// Never store old declarations here: legacy files may contain local secrets.
type exportEntry struct {
	Name     string `json:"name"`
	Previous string `json:"previous"`
	Next     []byte `json:"next"`
}
type exportRecord struct {
	Schema    int           `json:"schema"`
	ID        string        `json:"id"`
	CreatedAt time.Time     `json:"createdAt"`
	Files     []exportEntry `json:"files"`
}
type ExportFileState struct {
	Name   string
	Status string
}
type ExportRecovery struct {
	OperationID string
	ProjectName string
	PackageIDs  []string
	Services    int
	Databases   int
	Sites       int
	Directory   string
	CreatedAt   time.Time
	Phase       string
	Files       []ExportFileState
	CanResume   bool
	Fingerprint string
}

func digestBytes(b []byte) string { sum := sha256.Sum256(b); return hex.EncodeToString(sum[:]) }
func validDigest(s string) bool   { b, err := hex.DecodeString(s); return err == nil && len(b) == 32 }
func exportFileDigest(dir, name string) (string, error) {
	b, err := readExportFile(dir, name, exportFileLimit)
	if errors.Is(err, os.ErrNotExist) {
		return "missing", nil
	}
	if err != nil {
		return "", err
	}
	return digestBytes(b), nil
}
func canonicalProjectDirectory(dir string) (string, error) {
	if strings.TrimSpace(dir) == "" {
		return "", errors.New("choose an existing project folder")
	}
	p, err := filepath.Abs(dir)
	if err != nil {
		return "", err
	}
	p, err = filepath.EvalSymlinks(p)
	if err != nil {
		return "", err
	}
	info, err := os.Stat(p)
	if err != nil {
		return "", err
	}
	if !info.IsDir() {
		return "", errors.New("choose an existing project folder")
	}
	return p, nil
}

// Coordinate readers/writers even when two app storage roots share a checkout.
// The lock lives in the user's cache; importing adds no repository lock file.
func lockEnvironmentFiles(ctx context.Context, dir string) (func(), error) {
	info, err := os.Stat(dir)
	if err != nil {
		return nil, err
	}
	identity := dir
	if stat, ok := info.Sys().(*syscall.Stat_t); ok {
		identity = fmt.Sprintf("%d:%d", stat.Dev, stat.Ino)
	}
	cache, err := os.UserCacheDir()
	if err != nil {
		return nil, err
	}
	cache = filepath.Join(cache, "ServiceManager", "project-export-locks")
	if err = os.MkdirAll(cache, 0700); err != nil {
		return nil, err
	}
	fd, err := syscall.Open(filepath.Join(cache, digestBytes([]byte(identity))+".lock"), syscall.O_RDWR|syscall.O_CREAT|syscall.O_NOFOLLOW|syscall.O_NONBLOCK, 0600)
	if err != nil {
		return nil, err
	}
	f := os.NewFile(uintptr(fd), "project export lock")
	info, err = f.Stat()
	if err != nil || !info.Mode().IsRegular() {
		f.Close()
		return nil, errors.New("invalid project export lock file")
	}
	for {
		if err = ctx.Err(); err != nil {
			f.Close()
			return nil, err
		}
		err = syscall.Flock(fd, syscall.LOCK_EX|syscall.LOCK_NB)
		if err == nil {
			return func() { _ = syscall.Flock(fd, syscall.LOCK_UN); _ = f.Close() }, nil
		}
		if err != syscall.EWOULDBLOCK {
			f.Close()
			return nil, err
		}
		select {
		case <-ctx.Done():
			f.Close()
			return nil, ctx.Err()
		case <-time.After(25 * time.Millisecond):
		}
	}
}
func writeExportFile(dir, name string, b []byte) error {
	if _, err := exportFileDigest(dir, name); err != nil {
		return err
	}
	if err := atomicWrite(filepath.Join(dir, name), b, 0600); err != nil {
		return err
	}
	return syncExportDirectory(dir)
}
func readExportRecord(dir string) (*exportRecord, []byte, error) {
	b, err := readExportFile(dir, exportRecordName, 24<<20)
	if errors.Is(err, os.ErrNotExist) {
		return nil, nil, nil
	}
	if err != nil {
		return nil, nil, err
	}
	var r exportRecord
	if err = json.Unmarshal(b, &r); err != nil {
		return nil, nil, fmt.Errorf("read export recovery record: %w", err)
	}
	if r.Schema != 1 || validID(r.ID) != nil || r.CreatedAt.IsZero() || len(r.Files) != 2 {
		return nil, nil, errors.New("invalid export recovery record")
	}
	for n, entry := range r.Files {
		if entry.Name != exportFileNames[n] || len(entry.Next) > exportFileLimit || !json.Valid(entry.Next) || entry.Previous != "missing" && !validDigest(entry.Previous) {
			return nil, nil, errors.New("invalid export recovery entry")
		}
	}
	var shared sharedProject
	if err = json.Unmarshal(r.Files[1].Next, &shared); err != nil {
		return nil, nil, err
	}
	if shared.SchemaVersion != 2 || shared.LockSHA256 != digestBytes(r.Files[0].Next) {
		return nil, nil, errors.New("export recovery payload does not contain a matching environment pair")
	}
	// Validate the lock structure without using repository URLs or running code.
	if _, err = parseEnvironmentLock(r.Files[0].Next); err != nil {
		return nil, nil, err
	}
	return &r, b, nil
}
func inspectExportRecoveryLocked(dir string) (*ExportRecovery, error) {
	r, b, err := readExportRecord(dir)
	if err != nil || r == nil {
		return nil, err
	}
	preview := &ExportRecovery{OperationID: r.ID, Directory: dir, CreatedAt: r.CreatedAt, CanResume: true, Phase: "Prepared"}
	var shared sharedProject
	if err := json.Unmarshal(r.Files[1].Next, &shared); err != nil {
		return nil, err
	}
	lock, err := parseEnvironmentLock(r.Files[0].Next)
	if err != nil {
		return nil, err
	}
	preview.ProjectName = shared.Name
	preview.Services = len(shared.Services)
	preview.Databases = len(shared.Instances)
	preview.Sites = len(shared.Sites)
	for id := range lock.Packages {
		preview.PackageIDs = append(preview.PackageIDs, id)
	}
	sort.Strings(preview.PackageIDs)
	fingerprint := append([]byte(dir+"\x00"), b...)
	applied := 0
	for _, entry := range r.Files {
		current, err := exportFileDigest(dir, entry.Name)
		status := "Previous file"
		if err != nil {
			status = "Cannot inspect file: " + err.Error()
			preview.CanResume = false
		} else if current == digestBytes(entry.Next) {
			status = "New file saved"
			applied++
		} else if current != entry.Previous {
			status = "Changed after export began"
			preview.CanResume = false
		}
		fingerprint = append(fingerprint, []byte(entry.Name+":"+current+":"+status)...)
		preview.Files = append(preview.Files, ExportFileState{Name: entry.Name, Status: status})
	}
	if applied > 0 {
		preview.Phase = "Partially saved"
	}
	if applied == len(r.Files) {
		preview.Phase = "Files saved; commit pending"
	}
	if !preview.CanResume {
		preview.Phase = "Review file changes"
	}
	preview.Fingerprint = digestBytes(fingerprint)
	return preview, nil
}

func (m *Manager) InspectExportRecovery(ctx context.Context, dir string) (*ExportRecovery, error) {
	dir, err := canonicalProjectDirectory(dir)
	if err != nil {
		return nil, err
	}
	unlock, err := lockEnvironmentFiles(ctx, dir)
	if err != nil {
		return nil, err
	}
	defer unlock()
	return inspectExportRecoveryLocked(dir)
}
func (m *Manager) RecoverExport(ctx context.Context, dir, fingerprint string) error {
	dir, err := canonicalProjectDirectory(dir)
	if err != nil {
		return err
	}
	unlock, err := lockEnvironmentFiles(ctx, dir)
	if err != nil {
		return err
	}
	defer unlock()
	preview, err := inspectExportRecoveryLocked(dir)
	if err != nil {
		return err
	}
	if preview == nil {
		return errors.New("there is no interrupted export")
	}
	if preview.Fingerprint != fingerprint {
		return errors.New("export files changed; review recovery again")
	}
	if !preview.CanResume {
		return errors.New("export files changed outside this operation; keep the files and review before retrying")
	}
	r, _, err := readExportRecord(dir)
	if err != nil {
		return err
	}
	return m.completeExportLocked(ctx, dir, r)
}

// Discard removes only the reviewed operation record, never either user file.
func (m *Manager) DiscardExport(ctx context.Context, dir, fingerprint string) error {
	dir, err := canonicalProjectDirectory(dir)
	if err != nil {
		return err
	}
	unlock, err := lockEnvironmentFiles(ctx, dir)
	if err != nil {
		return err
	}
	defer unlock()
	preview, err := inspectExportRecoveryLocked(dir)
	if err != nil {
		return err
	}
	if preview == nil {
		return errors.New("there is no interrupted export")
	}
	if fingerprint != preview.Fingerprint {
		return errors.New("export files changed; review recovery again")
	}
	if err = ctx.Err(); err != nil {
		return err
	}
	if err = os.Remove(filepath.Join(dir, exportRecordName)); err != nil {
		return err
	}
	return syncExportDirectory(dir)
}
func (m *Manager) completeExportLocked(ctx context.Context, dir string, r *exportRecord) error {
	for _, entry := range r.Files {
		if err := ctx.Err(); err != nil {
			return err
		}
		current, err := exportFileDigest(dir, entry.Name)
		if err != nil {
			return err
		}
		if current != entry.Previous && current != digestBytes(entry.Next) {
			return errors.New("export target changed; review recovery before retrying")
		}
		if current != digestBytes(entry.Next) {
			if err = writeExportFile(dir, entry.Name, entry.Next); err != nil {
				return err
			}
		}
		m.emit("Export " + r.ID + " · Saved " + entry.Name)
	}
	if err := ctx.Err(); err != nil {
		return err
	}
	for _, entry := range r.Files {
		current, err := exportFileDigest(dir, entry.Name)
		if err != nil {
			return err
		}
		if current != digestBytes(entry.Next) {
			return errors.New("export file changed before commit; review recovery again")
		}
	}
	if err := os.Remove(filepath.Join(dir, exportRecordName)); err != nil {
		return err
	}
	return syncExportDirectory(dir)
}
func (m *Manager) publishEnvironmentLocked(ctx context.Context, dir string, shared sharedProject, lock EnvironmentLock) error {
	if r, _, err := readExportRecord(dir); err != nil {
		return err
	} else if r != nil {
		return fmt.Errorf("export %s is unfinished; review export recovery first", r.ID)
	}
	lockBytes, err := exportJSON(lock)
	if err != nil {
		return err
	}
	shared.LockSHA256 = digestBytes(lockBytes)
	declaration, err := exportJSON(shared)
	if err != nil {
		return err
	}
	r := exportRecord{Schema: 1, ID: randomID(), CreatedAt: time.Now().UTC()}
	for n, b := range [][]byte{lockBytes, declaration} {
		if len(b) > exportFileLimit {
			return errors.New("environment export exceeds its size limit")
		}
		hash, err := exportFileDigest(dir, exportFileNames[n])
		if err != nil {
			return err
		}
		r.Files = append(r.Files, exportEntry{Name: exportFileNames[n], Previous: hash, Next: b})
	}
	if err = ctx.Err(); err != nil {
		return err
	}
	if err = writeJSON(filepath.Join(dir, exportRecordName), r); err != nil {
		return err
	}
	if err = syncExportDirectory(dir); err != nil {
		return err
	}
	m.emit("Export " + r.ID + " · Prepared")
	if err = m.completeExportLocked(ctx, dir, &r); err != nil {
		return fmt.Errorf("export %s interrupted; review export recovery: %w", r.ID, err)
	}
	return nil
}
func verifySharedEnvironmentPair(dir string, shared sharedProject) error {
	// Initial schema 2 exports had no digest. Read them for compatibility.
	if shared.LockSHA256 == "" {
		return nil
	}
	if !validDigest(shared.LockSHA256) {
		return errors.New("invalid environment lock digest")
	}
	b, err := readExportFile(dir, "devmanager.lock.json", exportFileLimit)
	if err != nil {
		return err
	}
	if digestBytes(b) != shared.LockSHA256 {
		return errors.New("environment declaration and lock do not match; complete export recovery or export again from the source project")
	}
	return nil
}

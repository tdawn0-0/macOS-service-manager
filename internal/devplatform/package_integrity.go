package devplatform

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"sort"
	"syscall"
	"time"
)

type packageFile struct {
	Kind   string `json:"kind"`
	Mode   uint32 `json:"mode"`
	Bytes  int64  `json:"bytes,omitempty"`
	SHA256 string `json:"sha256,omitempty"`
	Target string `json:"target,omitempty"`
}
type packageTree struct {
	Schema        int                    `json:"schema"`
	PackageID     string                 `json:"packageID"`
	ArchiveSHA256 string                 `json:"archiveSHA256"`
	Files         map[string]packageFile `json:"files"`
}
type PackageIntegrityResult struct {
	Status    string    `json:"status"` // Verified, Needs repair, Not recorded
	CheckedAt time.Time `json:"checkedAt"`
	Files     int       `json:"files"`
	Issues    []string  `json:"issues,omitempty"`
	Baseline  string    `json:"baseline,omitempty"`
}

func (r PackageIntegrityResult) Text() string {
	s := fmt.Sprintf("%s · %d entries checked", r.Status, r.Files)
	for _, issue := range r.Issues {
		s += "\n" + issue
	}
	return s
}
func (m *Manager) integrityPath(id string) string {
	return filepath.Join(m.Root, "integrity", id+".json")
}

// The index is created only from a digest-verified fresh archive. It records
// contents, file kinds, permissions and symlink targets, without timestamps or
// xattrs. Its own digest is anchored in the local state, outside the package.
func (m *Manager) scanPackageTree(ctx context.Context, root string, p Package) (packageTree, error) {
	tree := packageTree{Schema: 1, PackageID: p.ID(), ArchiveSHA256: p.SHA256, Files: map[string]packageFile{}}
	info, err := os.Lstat(root)
	if err != nil {
		return tree, err
	}
	if !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
		return tree, errors.New("installation root must be a directory, not a symlink")
	}
	tree.Files["."] = packageFile{Kind: "directory", Mode: integrityMode(info.Mode())}
	var total int64
	last := time.Now()
	err = filepath.WalkDir(root, func(path string, entry fs.DirEntry, walkErr error) error {
		if err := ctx.Err(); err != nil {
			return err
		}
		if walkErr != nil {
			return walkErr
		}
		if path == root {
			return nil
		}
		if len(tree.Files) >= 100000 {
			return errors.New("package contains too many filesystem entries")
		}
		rel, err := filepath.Rel(root, path)
		if err != nil || !safeRelative(rel) {
			return errors.New("package path is outside the installation")
		}
		info, err := entry.Info()
		if err != nil {
			return err
		}
		record := packageFile{Mode: integrityMode(info.Mode())}
		switch {
		case info.IsDir():
			record.Kind = "directory"
		case info.Mode()&os.ModeSymlink != 0:
			record.Kind = "symlink"
			record.Target, err = os.Readlink(path)
			if err != nil {
				return err
			}
			if filepath.IsAbs(record.Target) || !inside(root, filepath.Join(filepath.Dir(path), record.Target)) {
				return fmt.Errorf("symlink escapes installation: %s", rel)
			}
		case info.Mode().IsRegular():
			record.Kind = "file"
			record.Bytes = info.Size()
			total += record.Bytes
			if total > maxExpanded {
				return errors.New("installed files exceed the supported size")
			}
			if !inside(root, path) {
				return errors.New("file escapes installation")
			}
			f, err := os.OpenFile(path, os.O_RDONLY|syscall.O_NOFOLLOW, 0)
			if err != nil {
				return err
			}
			h := sha256.New()
			n, copyErr := io.Copy(h, io.LimitReader(contextReader{ctx, f}, maxExpanded+1))
			after, statErr := f.Stat()
			closeErr := f.Close()
			if err = errors.Join(copyErr, statErr, closeErr); err != nil {
				return err
			}
			if n != info.Size() || !os.SameFile(info, after) || info.ModTime() != after.ModTime() || info.Mode() != after.Mode() {
				return fmt.Errorf("file changed while checking: %s", rel)
			}
			record.SHA256 = hex.EncodeToString(h.Sum(nil))
		default:
			return fmt.Errorf("unsupported filesystem entry: %s", rel)
		}
		tree.Files[filepath.ToSlash(rel)] = record
		if time.Since(last) > 500*time.Millisecond {
			m.emit(fmt.Sprintf("Checking %s · %d entries", p.ID(), len(tree.Files)))
			last = time.Now()
		}
		return nil
	})
	return tree, err
}

func integrityMode(mode os.FileMode) uint32 {
	return uint32(mode & (os.ModePerm | os.ModeSetuid | os.ModeSetgid | os.ModeSticky))
}

func encodePackageTree(tree packageTree) ([]byte, string, error) {
	b, err := json.MarshalIndent(tree, "", "  ")
	if err != nil {
		return nil, "", err
	}
	b = append(b, '\n')
	if len(b) > 32<<20 {
		return nil, "", errors.New("package integrity index exceeds limit")
	}
	sum := sha256.Sum256(b)
	return b, hex.EncodeToString(sum[:]), nil
}

func (m *Manager) VerifyPackage(ctx context.Context, id string) (PackageIntegrityResult, error) {
	unlock, err := m.packageLease(ctx, id, false)
	if err != nil {
		return PackageIntegrityResult{}, err
	}
	defer unlock()
	s, err := m.Read()
	if err != nil {
		return PackageIntegrityResult{}, err
	}
	p, ok := s.Packages[id]
	if !ok {
		return PackageIntegrityResult{}, errors.New("package is not installed")
	}
	if p.ID() != id {
		return PackageIntegrityResult{}, errors.New("installed package identity mismatch")
	}
	return m.verifyPackage(ctx, &s, p)
}
func (m *Manager) verifyPackage(ctx context.Context, s *State, p Package) (PackageIntegrityResult, error) {
	r := PackageIntegrityResult{Status: "Needs repair", CheckedAt: time.Now().UTC(), Baseline: s.PackageIntegrity[p.ID()]}
	if err := ctx.Err(); err != nil {
		return r, err
	}
	if err := p.Validate(); err != nil {
		return r, err
	}
	for _, name := range diagnosticKeys(p.Binaries) {
		if _, err := m.Binary(s, p.ID(), name); err != nil {
			r.Issues = append(r.Issues, "Required executable is missing or invalid: "+name)
		}
		if p.Formula != "" {
			r.Files++
		}
	}
	if p.Formula != "" {
		if len(r.Issues) == 0 {
			r.Status = "Homebrew managed"
		}
		return r, nil
	}
	if r.Baseline == "" {
		if len(r.Issues) == 0 {
			r.Status = "Not recorded"
		}
		r.Issues = append(r.Issues, "No trusted installation index is recorded. Repair from the verified archive to establish one.")
		return r, nil
	}
	indexPath := m.integrityPath(p.ID())
	info, err := os.Lstat(indexPath)
	if err != nil || !info.Mode().IsRegular() {
		r.Issues = append(r.Issues, "Installation integrity index is missing or is not a regular file.")
		return r, nil
	}
	f, err := os.OpenFile(indexPath, os.O_RDONLY|syscall.O_NOFOLLOW|syscall.O_NONBLOCK, 0)
	if err != nil {
		r.Issues = append(r.Issues, "Installation integrity index could not be read.")
		return r, nil
	}
	if opened, err := f.Stat(); err != nil || !opened.Mode().IsRegular() {
		f.Close()
		r.Issues = append(r.Issues, "Installation integrity index changed while opening it.")
		return r, nil
	}
	b, readErr := io.ReadAll(io.LimitReader(contextReader{ctx, f}, (32<<20)+1))
	f.Close()
	if ctx.Err() != nil {
		return r, ctx.Err()
	}
	sum := sha256.Sum256(b)
	if readErr != nil || len(b) > 32<<20 || hex.EncodeToString(sum[:]) != r.Baseline {
		r.Issues = append(r.Issues, "Installation integrity index is damaged or has changed.")
		return r, nil
	}
	var expected packageTree
	if json.Unmarshal(b, &expected) != nil || expected.Schema != 1 || expected.PackageID != p.ID() || expected.ArchiveSHA256 != p.SHA256 || expected.Files == nil {
		r.Issues = append(r.Issues, "Installation integrity index does not match this package.")
		return r, nil
	}
	m.emit("Checking installed files for " + p.ID())
	actual, err := m.scanPackageTree(ctx, m.packageDir(p), p)
	r.Files = len(actual.Files)
	if ctx.Err() != nil {
		return r, ctx.Err()
	}
	if err != nil {
		r.Issues = append(r.Issues, err.Error())
		return r, nil
	}
	var changes []string
	for path, want := range expected.Files {
		got, ok := actual.Files[path]
		if !ok {
			changes = append(changes, "Missing: "+path)
		} else if got != want {
			changes = append(changes, "Changed: "+path)
		}
	}
	for path := range actual.Files {
		if _, ok := expected.Files[path]; !ok {
			changes = append(changes, "Unexpected: "+path)
		}
	}
	sort.Strings(changes)
	if len(changes) > 50 {
		changes = append(changes[:50], fmt.Sprintf("%d more differences", len(changes)-50))
	}
	r.Issues = append(r.Issues, changes...)
	if len(r.Issues) == 0 {
		r.Status = "Verified"
	}
	return r, nil
}

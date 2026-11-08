package devplatform

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"syscall"

	"howett.net/plist"
)

// JavaInstallation describes a directory entry, not a downloadable package.
// Vendors and identical Java versions remain distinct by their installation path.
type JavaInstallation struct {
	ID, Path, Home, Name, Vendor, Version, Build, Arch string
	PackageID, Fingerprint, Problem                    string
	Symlink, Managed, CanUse                           bool
}

func (j JavaInstallation) Label() string {
	return j.Name + " · " + j.Version + " · " + j.Arch
}
func (m *Manager) JavaInstallationDirectory() string { return m.javaDirectory() }

func javaInstallationID(path string) string {
	sum := sha256.Sum256([]byte(filepath.Clean(path)))
	return "java-local-" + hex.EncodeToString(sum[:])
}

func readJavaMetadata(path string) ([]byte, error) {
	f, err := os.OpenFile(path, os.O_RDONLY|syscall.O_NONBLOCK, 0)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	info, err := f.Stat()
	if err != nil || !info.Mode().IsRegular() {
		return nil, errors.New("JDK metadata is not a regular file")
	}
	b, err := io.ReadAll(io.LimitReader(f, (1<<20)+1))
	if err == nil && len(b) > 1<<20 {
		err = errors.New("JDK metadata is too large")
	}
	return b, err
}
func javaReleaseValues(b []byte) map[string]string {
	result := map[string]string{}
	for _, line := range strings.Split(string(b), "\n") {
		key, value, ok := strings.Cut(line, "=")
		if !ok {
			continue
		}
		value = strings.TrimSpace(value)
		if unquoted, err := strconv.Unquote(value); err == nil {
			value = unquoted
		}
		result[strings.TrimSpace(key)] = value
	}
	return result
}
func firstJavaValue(values ...string) string {
	for _, v := range values {
		if v != "" {
			return v
		}
	}
	return "Unknown"
}

func (m *Manager) JavaInstallations(ctx context.Context) ([]JavaInstallation, error) {
	s, err := m.Read()
	if err != nil {
		return nil, err
	}
	return m.javaInstallations(ctx, &s)
}
func (m *Manager) javaInstallations(ctx context.Context, s *State) ([]JavaInstallation, error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	parent := m.javaDirectory()
	entries, err := os.ReadDir(parent)
	if errors.Is(err, os.ErrNotExist) {
		return []JavaInstallation{}, nil
	}
	if err != nil {
		return nil, err
	}
	parentInfo, err := os.Stat(parent)
	if err != nil {
		return nil, err
	}
	parentIdentity, err := deletionIdentity(parentInfo)
	if err != nil {
		return nil, err
	}
	rows := []JavaInstallation{}
	for _, entry := range entries {
		if err := ctx.Err(); err != nil {
			return nil, err
		}
		if strings.HasPrefix(entry.Name(), ".") {
			continue
		}
		path := filepath.Join(parent, entry.Name())
		info, err := os.Lstat(path)
		if err != nil {
			continue
		}
		if !info.IsDir() && info.Mode()&os.ModeSymlink == 0 {
			continue
		}
		identity, err := deletionIdentity(info)
		if err != nil {
			continue
		}
		home := filepath.Join(path, "Contents", "Home")
		plistBytes, plistErr := readJavaMetadata(filepath.Join(path, "Contents", "Info.plist"))
		releaseBytes, releaseErr := readJavaMetadata(filepath.Join(home, "release"))
		if releaseErr != nil {
			if flat, err := readJavaMetadata(filepath.Join(path, "release")); err == nil && javaReleaseValues(flat)["JAVA_VERSION"] != "" {
				home, releaseBytes, releaseErr = path, flat, nil
			}
		}
		var metadata struct {
			CFBundleName string
			JavaVM       struct{ JVMVendor, JVMVersion string }
		}
		if plistErr == nil {
			plistErr = func() error { _, e := plist.Unmarshal(plistBytes, &metadata); return e }()
		}
		release := javaReleaseValues(releaseBytes)
		if metadata.JavaVM.JVMVersion == "" && release["JAVA_VERSION"] == "" {
			continue
		}
		j := JavaInstallation{ID: javaInstallationID(path), Path: path, Home: home, Symlink: info.Mode()&os.ModeSymlink != 0}
		j.Vendor = firstJavaValue(release["IMPLEMENTOR"], metadata.JavaVM.JVMVendor)
		j.Name = firstJavaValue(metadata.CFBundleName, release["JAVA_RUNTIME_NAME"], j.Vendor+" JDK")
		j.Version = firstJavaValue(release["JAVA_RUNTIME_VERSION"], release["JAVA_VERSION"], metadata.JavaVM.JVMVersion)
		j.Build = release["IMPLEMENTOR_VERSION"]
		j.Arch = firstJavaValue(release["OS_ARCH"])
		resolved, resolveErr := filepath.EvalSymlinks(home)
		link, _ := os.Readlink(path)
		// Bind reviewed actions to the directory identity, link target and metadata.
		digest := sha256.New()
		fmt.Fprintf(digest, "%s\x00%s\x00%s\x00%s\x00%s\x00", path, parentIdentity, identity, resolved, link)
		digest.Write(plistBytes)
		digest.Write(releaseBytes)
		j.Fingerprint = hex.EncodeToString(digest.Sum(nil))
		if resolveErr != nil {
			j.Problem = "Java home is unavailable"
		} else {
			j.Home = resolved
			binary := filepath.Join(resolved, "bin", "java")
			f, e := os.Stat(binary)
			if e != nil || !f.Mode().IsRegular() || f.Mode()&0111 == 0 {
				j.Problem = "Java executable is missing or not executable"
			} else if e = requireAppleSiliconExecutable(binary); e != nil {
				j.Problem = "This Java executable does not support Apple Silicon"
			} else if j.Arch == "x86_64" || j.Arch == "amd64" || j.Arch == "x86" {
				j.Problem = "Intel JDK; can be removed but cannot be selected on Apple Silicon"
			} else {
				j.CanUse = true
			}
		}
		if plistErr != nil && releaseErr != nil {
			j.Problem = "JDK metadata could not be read"
			j.CanUse = false
		}
		for id, p := range s.Packages {
			if p.Name == "java" && id == p.ID() && p.Validate() == nil && m.packageDir(p) == path {
				j.PackageID = id
				j.Managed = m.checkJavaOwnership(p) == nil
				break
			}
		}
		rows = append(rows, j)
	}
	sort.Slice(rows, func(i, j int) bool {
		if rows[i].Name != rows[j].Name {
			return rows[i].Name < rows[j].Name
		}
		if rows[i].Version != rows[j].Version {
			return rows[i].Version > rows[j].Version
		}
		return rows[i].Path < rows[j].Path
	})
	return rows, nil
}
func (m *Manager) reviewedJava(ctx context.Context, s *State, selected JavaInstallation) (JavaInstallation, error) {
	rows, err := m.javaInstallations(ctx, s)
	if err != nil {
		return JavaInstallation{}, err
	}
	for _, row := range rows {
		if row.ID == selected.ID && row.Path == selected.Path && row.Fingerprint == selected.Fingerprint {
			return row, nil
		}
	}
	return JavaInstallation{}, errors.New("Java installation changed or disappeared; refresh and review it again")
}
func (m *Manager) runtimeBinary(s *State, id, name string) (string, error) {
	if name == "java" && strings.HasPrefix(id, "java-local-") {
		rows, err := m.javaInstallations(context.Background(), s)
		if err != nil {
			return "", err
		}
		for _, row := range rows {
			if row.ID == id {
				if !row.CanUse {
					return "", errors.New(row.Problem)
				}
				return filepath.Join(row.Home, "bin", "java"), nil
			}
		}
		return "", errors.New("selected Java installation is no longer available; choose another default")
	}
	return m.Binary(s, id, name)
}
func (m *Manager) SetJavaDefault(ctx context.Context, selected JavaInstallation) error {
	return m.withStateTransaction(ctx, func(s *State) (func() error, func(), error) {
		row, err := m.reviewedJava(ctx, s, selected)
		if err != nil {
			return nil, nil, err
		}
		if !row.CanUse {
			return nil, nil, errors.New(row.Problem)
		}
		return m.saveRuntimeDefault(s, "java", row.ID, filepath.Join(row.Home, "bin", "java"))
	})
}

// RemoveJavaInstallation is an explicit reviewed action for ANY detected JDK.
// Discovery alone never adopts, overwrites, repairs or deletes external software.
// A symlink is moved as a link; its target is never removed.
func (m *Manager) RemoveJavaInstallation(ctx context.Context, selected JavaInstallation) (string, error) {
	var trashPath string
	var release func()
	defer func() {
		if release != nil {
			release()
		}
	}()
	err := m.withStateTransaction(ctx, func(s *State) (func() error, func(), error) {
		row, err := m.reviewedJava(ctx, s, selected)
		if err != nil {
			return nil, nil, err
		}
		parent, err := os.Lstat(m.javaDirectory())
		if err != nil || !parent.IsDir() || parent.Mode()&os.ModeSymlink != 0 {
			return nil, nil, errors.New("Java installation directory must be a real directory")
		}
		if row.PackageID != "" {
			release, err = m.packageLease(ctx, row.PackageID, true)
			if err != nil {
				return nil, nil, err
			}
			if err = m.requirePackageIdle(ctx, s, s.Packages[row.PackageID]); err != nil {
				return nil, nil, err
			}
		}
		home, err := m.home()
		if err != nil {
			return nil, nil, err
		}
		trash := filepath.Join(home, ".Trash")
		if !m.usesUserStorage() && m.Home == "" {
			trash = filepath.Join(m.Root, "trash")
		}
		if err = os.MkdirAll(trash, 0700); err != nil {
			return nil, nil, err
		}
		f, err := os.Lstat(trash)
		if err != nil || !f.IsDir() || f.Mode()&os.ModeSymlink != 0 {
			return nil, nil, errors.New("Trash must be a real directory")
		}
		trashPath = filepath.Join(trash, filepath.Base(row.Path)+"-"+randomID())
		before, err := os.Lstat(row.Path)
		if err != nil {
			return nil, nil, err
		}
		// Recheck after acquiring leases and preparing Trash, before touching files.
		if _, err = m.reviewedJava(ctx, s, row); err != nil {
			return nil, nil, err
		}
		if err = publishNewPackageDirectory(row.Path, trashPath); err != nil {
			return nil, nil, err
		}
		var restoreScripts func() error
		undo := func() error {
			if err := publishNewPackageDirectory(trashPath, row.Path); err != nil {
				return fmt.Errorf("JDK retained in Trash at %s: %w", trashPath, err)
			}
			if restoreScripts != nil {
				return restoreScripts()
			}
			return nil
		}
		after, err := os.Lstat(trashPath)
		if err != nil || !os.SameFile(before, after) {
			return undo, nil, errors.New("Java installation changed while moving; refresh and review again")
		}
		if s.RuntimeDefaults["java"] == row.ID || row.PackageID != "" && s.RuntimeDefaults["java"] == row.PackageID {
			restoreScripts, err = m.updateRuntimeScripts("java", nil)
			if err != nil {
				return undo, nil, err
			}
			delete(s.RuntimeDefaults, "java")
		}
		if row.PackageID != "" {
			delete(s.Packages, row.PackageID)
			delete(s.PackageIntegrity, row.PackageID)
		}
		commit := func() {
			if row.PackageID != "" {
				_ = os.Remove(m.integrityPath(row.PackageID))
			}
		}
		m.emit("Committing Java removal: " + row.Path)
		return undo, commit, nil
	})
	return trashPath, err
}

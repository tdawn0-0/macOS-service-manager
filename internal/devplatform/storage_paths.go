package devplatform

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"

	"howett.net/plist"
)

func defaultStorageRoot(home string) string {
	return filepath.Join(home, "Library", "Application Support", "ServiceManager", "Development")
}

// Explicit storage roots (including MSM_DEV_HOME) stay self-contained. This
// keeps disposable CLI environments out of the user's shared runtime folders.
func (m *Manager) usesUserStorage() bool {
	home, err := m.home()
	return err == nil && filepath.Clean(m.Root) == defaultStorageRoot(home)
}

func (m *Manager) cacheDirectory() string {
	if m.usesUserStorage() {
		home, _ := m.home()
		return filepath.Join(home, "Library", "Caches", "ServiceManager", "Development")
	}
	return filepath.Join(m.Root, "cache")
}

func (m *Manager) downloadDirectory() string {
	if m.usesUserStorage() {
		return filepath.Join(m.cacheDirectory(), "downloads")
	}
	return filepath.Join(m.Root, "downloads")
}

func (m *Manager) javaDirectory() string {
	if m.usesUserStorage() {
		home, _ := m.home()
		return filepath.Join(home, "Library", "Java", "JavaVirtualMachines")
	}
	return filepath.Join(m.Root, "JavaVirtualMachines")
}

const javaOwnerFile = ".service-manager-java-owner"

var javaFolderSuffix = regexp.MustCompile(`^[1-9][0-9]*$`)
var javaInstallNameRE = regexp.MustCompile(`^[a-zA-Z0-9][a-zA-Z0-9._+\-]{0,119}$`)

func javaFolderBase(p Package) string {
	if p.JavaInstallName != "" {
		return p.JavaInstallName
	}
	return "temurin-" + strings.SplitN(p.Version, "_", 2)[0]
}

// IntelliJ uses the distribution/version name and numeric collision suffixes.
// Save the actual folder in the package receipt; never infer it from a version.
func (m *Manager) nextJavaFolder(p Package) (string, error) {
	base := javaFolderBase(p)
	for n := 0; n < 10000; n++ {
		name := base
		if n > 0 {
			name = fmt.Sprintf("%s-%d", base, n)
		}
		if _, err := os.Lstat(filepath.Join(m.javaDirectory(), name)); errors.Is(err, os.ErrNotExist) {
			return name, nil
		} else if err != nil {
			return "", err
		}
	}
	return "", errors.New("too many Java installation folders with this version")
}

func (m *Manager) javaOwner(p Package) string {
	return filepath.Clean(m.Root) + "\n" + p.ID() + "\n" + p.SHA256 + "\n"
}

// The user-level JVM directory is shared with IDEs and other installers.
// Only our marked bundle may be replaced or removed, even if state names it.
func (m *Manager) checkJavaOwnership(p Package) error {
	if p.Name != "java" || p.Formula != "" {
		return nil
	}
	dir := m.packageDir(p)
	for _, path := range []string{filepath.Dir(dir), dir} {
		info, err := os.Lstat(path)
		if err != nil {
			return err
		}
		if !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
			return fmt.Errorf("Java installation path is not a plain directory: %s", path)
		}
	}
	marker := filepath.Join(dir, javaOwnerFile)
	info, err := os.Lstat(marker)
	if err != nil || !info.Mode().IsRegular() {
		return fmt.Errorf("Java installation ownership is missing or invalid; inspect %s", dir)
	}
	b, err := os.ReadFile(marker)
	if err != nil {
		return err
	}
	if string(b) != m.javaOwner(p) {
		return fmt.Errorf("Java installation belongs to another installation; inspect %s", dir)
	}
	return nil
}

func validateJavaBundle(dir string) error {
	path := filepath.Join(dir, "Contents", "Info.plist")
	info, err := os.Lstat(path)
	if err != nil || !info.Mode().IsRegular() {
		return errors.New("JDK archive must contain Contents/Info.plist")
	}
	b, err := os.ReadFile(path)
	if err != nil {
		return err
	}
	var metadata struct {
		JavaVM struct{ JVMVersion string }
	}
	if _, err = plist.Unmarshal(b, &metadata); err != nil || metadata.JavaVM.JVMVersion == "" {
		return errors.New("JDK archive has invalid JavaVM metadata")
	}
	return nil
}

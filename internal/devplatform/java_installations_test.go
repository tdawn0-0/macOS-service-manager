package devplatform

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"howett.net/plist"
)

func writeExistingJDK(t *testing.T, parent, folder, name, vendor, version, arch string) string {
	t.Helper()
	path := filepath.Join(parent, folder)
	home := filepath.Join(path, "Contents", "Home")
	if err := os.MkdirAll(filepath.Join(home, "bin"), 0700); err != nil {
		t.Fatal(err)
	}
	binary, err := os.ReadFile("/bin/echo")
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(filepath.Join(home, "bin", "java"), binary, 0755); err != nil {
		t.Fatal(err)
	}
	metadata, err := plist.Marshal(map[string]any{"CFBundleName": name, "JavaVM": map[string]string{"JVMVendor": vendor, "JVMVersion": version}}, plist.XMLFormat)
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(filepath.Join(path, "Contents", "Info.plist"), metadata, 0600); err != nil {
		t.Fatal(err)
	}
	release := fmt.Sprintf("JAVA_VERSION=%q\nJAVA_RUNTIME_VERSION=%q\nIMPLEMENTOR=%q\nIMPLEMENTOR_VERSION=%q\nOS_ARCH=%q\n", version, version+"+9", vendor, name+" build-9", arch)
	if err = os.WriteFile(filepath.Join(home, "release"), []byte(release), 0600); err != nil {
		t.Fatal(err)
	}
	return path
}
func javaRowAt(t *testing.T, m *Manager, path string) JavaInstallation {
	t.Helper()
	rows, err := m.JavaInstallations(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	for _, row := range rows {
		if row.Path == path {
			return row
		}
	}
	t.Fatal("JDK was not discovered", path, rows)
	return JavaInstallation{}
}
func TestJavaDiscoveryPreservesDistributionIdentity(t *testing.T) {
	m, _ := userStorageFixture(t)
	m.Run = func(context.Context, string, ...string) (string, error) {
		t.Fatal("discovery executed an external command")
		return "", nil
	}
	vendors := []struct{ folder, name, vendor, arch string }{
		{"temurin-21.0.13", "OpenJDK 21", "Eclipse Adoptium", "aarch64"},
		{"semeru-21.0.13", "IBM Semeru Runtime Open Edition 21", "IBM Corporation", "aarch64"},
		{"graalvm-21.0.13", "GraalVM Community", "GraalVM Community", "aarch64"},
		{"zulu-21.jdk", "Zulu 21", "Azul Systems, Inc.", "x86_64"},
		{"oracle-21", "Java SE 21", "Oracle Corporation", "aarch64"},
	}
	for _, v := range vendors {
		writeExistingJDK(t, m.javaDirectory(), v.folder, v.name, v.vendor, "21.0.13", v.arch)
	}
	if err := os.Mkdir(filepath.Join(m.javaDirectory(), "unrelated"), 0700); err != nil {
		t.Fatal(err)
	}
	rows, err := m.JavaInstallations(context.Background())
	if err != nil || len(rows) != len(vendors) {
		t.Fatal(rows, err)
	}
	seen := map[string]bool{}
	for _, v := range vendors {
		r := javaRowAt(t, m, filepath.Join(m.javaDirectory(), v.folder))
		if r.Name != v.name || r.Vendor != v.vendor || r.Version != "21.0.13+9" || r.Arch != v.arch || seen[r.ID] || r.Managed || r.PackageID != "" {
			t.Fatal(r)
		}
		seen[r.ID] = true
		if (v.arch == "aarch64") != r.CanUse {
			t.Fatal("wrong architecture availability", r)
		}
	}
	s, err := m.Read()
	if err != nil || len(s.Packages) != 0 {
		t.Fatal("scan adopted external JDKs as downloadable packages", s, err)
	}
}
func TestExistingJavaDefaultAndRemoval(t *testing.T) {
	m, _ := userStorageFixture(t)
	ctx := context.Background()
	path := writeExistingJDK(t, m.javaDirectory(), "semeru-25", "IBM Semeru", "IBM Corporation", "25.0.4.1", "aarch64")
	other := writeExistingJDK(t, m.javaDirectory(), "temurin-25", "OpenJDK 25", "Eclipse Adoptium", "25.0.4.1", "aarch64")
	row := javaRowAt(t, m, path)
	if err := m.SetJavaDefault(ctx, row); err != nil {
		t.Fatal(err)
	}
	if err := m.ConfigureRuntimeShell(ctx, "java", true); err != nil {
		t.Fatal(err)
	}
	script, err := os.ReadFile(m.runtimeScript("java"))
	if err != nil || !strings.Contains(string(script), shellLiteral(row.Home)) {
		t.Fatal(string(script), err)
	}
	s, _ := m.Read()
	if s.RuntimeDefaults["java"] != row.ID || len(s.Packages) != 0 {
		t.Fatal(s)
	}
	trash, err := m.RemoveJavaInstallation(ctx, row)
	if err != nil {
		t.Fatal(err)
	}
	if _, err = os.Stat(path); !os.IsNotExist(err) {
		t.Fatal("JDK not moved", err)
	}
	if _, err = os.Stat(filepath.Join(trash, "Contents/Home/bin/java")); err != nil {
		t.Fatal("JDK was erased instead of trashed", err)
	}
	if _, err = os.Stat(other); err != nil {
		t.Fatal("another distribution changed", err)
	}
	s, _ = m.Read()
	if s.RuntimeDefaults["java"] != "" {
		t.Fatal("removed JDK remains default")
	}
	if _, err = os.Stat(m.runtimeScript("java")); !os.IsNotExist(err) {
		t.Fatal("removed JDK remains in terminal script", err)
	}
}
func TestJavaRemovalRejectsStaleReviewAndKeepsLinkTarget(t *testing.T) {
	m, _ := userStorageFixture(t)
	ctx := context.Background()
	path := writeExistingJDK(t, m.javaDirectory(), "semeru-21", "IBM Semeru", "IBM", "21", "aarch64")
	reviewed := javaRowAt(t, m, path)
	if err := os.Rename(path, path+"-old"); err != nil {
		t.Fatal(err)
	}
	writeExistingJDK(t, m.javaDirectory(), "semeru-21", "IBM Semeru", "IBM", "21", "aarch64")
	if _, err := m.RemoveJavaInstallation(ctx, reviewed); err == nil {
		t.Fatal("deleted replacement at stale path")
	}
	if err := m.SetJavaDefault(ctx, reviewed); err == nil {
		t.Fatal("selected replacement at stale path")
	}
	target := writeExistingJDK(t, t.TempDir(), "external", "GraalVM", "Oracle", "25", "aarch64")
	link := filepath.Join(m.javaDirectory(), "graalvm-link")
	if err := os.Symlink(target, link); err != nil {
		t.Fatal(err)
	}
	row := javaRowAt(t, m, link)
	if !row.Symlink {
		t.Fatal("link not identified")
	}
	forged := row
	forged.Path = target
	if _, err := m.RemoveJavaInstallation(ctx, forged); err == nil {
		t.Fatal("accepted forged path")
	}
	trash, err := m.RemoveJavaInstallation(ctx, row)
	if err != nil {
		t.Fatal(err)
	}
	if _, err = os.Stat(filepath.Join(target, "Contents/Home/bin/java")); err != nil {
		t.Fatal("link target removed", err)
	}
	if f, err := os.Lstat(trash); err != nil || f.Mode()&os.ModeSymlink == 0 {
		t.Fatal("did not trash the link", err)
	}
}
func TestJavaRemovalRollsBackOnCancellation(t *testing.T) {
	m, _ := userStorageFixture(t)
	path := writeExistingJDK(t, m.javaDirectory(), "jdk", "Custom Runtime", "Custom Vendor", "21", "aarch64")
	row := javaRowAt(t, m, path)
	if err := m.SetJavaDefault(context.Background(), row); err != nil {
		t.Fatal(err)
	}
	old, err := os.ReadFile(m.runtimeScript("java"))
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	m.Event = func(message string) {
		if strings.HasPrefix(message, "Committing Java removal:") {
			cancel()
		}
	}
	_, err = m.RemoveJavaInstallation(ctx, row)
	if !errors.Is(err, context.Canceled) {
		t.Fatal(err)
	}
	if _, err = os.Stat(path); err != nil {
		t.Fatal("rollback lost JDK", err)
	}
	script, err := os.ReadFile(m.runtimeScript("java"))
	if err != nil || string(script) != string(old) {
		t.Fatal("rollback lost default script", err)
	}
	s, err := m.Read()
	if err != nil || s.RuntimeDefaults["java"] != row.ID {
		t.Fatal("rollback changed default", err)
	}
}
func TestJavaIdeaNamingAndManagedTrash(t *testing.T) {
	m, p := userStorageFixture(t)
	ctx := context.Background()
	existing := writeExistingJDK(t, m.javaDirectory(), "temurin-21.0.13", "OpenJDK 21", "Eclipse Adoptium", "21.0.13", "aarch64")
	if err := m.Install(ctx, p); err != nil {
		t.Fatal(err)
	}
	s, err := m.Read()
	if err != nil {
		t.Fatal(err)
	}
	p = s.Packages[p.ID()]
	if p.JavaFolder != "temurin-21.0.13-1" {
		t.Fatal("expected IDEA collision suffix", p.JavaFolder)
	}
	row := javaRowAt(t, m, m.PackageDirectory(p))
	if !row.Managed || row.PackageID != p.ID() {
		t.Fatal("managed JDK was not recognized", row)
	}
	if err := m.SetJavaDefault(ctx, row); err != nil {
		t.Fatal(err)
	}
	if _, err := m.RemoveJavaInstallation(ctx, row); err != nil {
		t.Fatal(err)
	}
	s, err = m.Read()
	if err != nil || len(s.Packages) != 0 || s.RuntimeDefaults["java"] != "" {
		t.Fatal(s, err)
	}
	if _, err = os.Stat(existing); err != nil {
		t.Fatal("existing IDEA JDK changed", err)
	}
	if _, err = os.Stat(m.integrityPath(p.ID())); !os.IsNotExist(err) {
		t.Fatal("integrity index retained", err)
	}
}

func TestJavaFlatArchiveLayout(t *testing.T) {
	m, _ := userStorageFixture(t)
	p := cacheRuntimeArchive(t, m, Package{Name: "java", Version: "liberica-21.0.13", JavaInstallName: "liberica-21.0.13", JavaProduct: "BellSoft Liberica JDK", Arch: "arm64", Binaries: map[string]string{"java": "bin/java"}})
	ctx := context.Background()
	if err := m.Install(ctx, p); err != nil {
		t.Fatal(err)
	}
	row := javaRowAt(t, m, m.PackageDirectory(p))
	want, err := filepath.EvalSymlinks(m.PackageDirectory(p))
	if err != nil || row.Home != want || !row.CanUse || !row.Managed {
		t.Fatal(row, err)
	}
	if err = m.SetJavaDefault(ctx, row); err != nil {
		t.Fatal(err)
	}
	if _, err = m.RemoveJavaInstallation(ctx, row); err != nil {
		t.Fatal(err)
	}
}

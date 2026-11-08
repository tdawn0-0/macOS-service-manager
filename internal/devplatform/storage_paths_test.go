package devplatform

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

func userStorageFixture(t *testing.T) (*Manager, Package) {
	t.Helper()
	home := t.TempDir()
	m := New(defaultStorageRoot(home))
	m.Home = home
	if err := m.init(); err != nil {
		t.Fatal(err)
	}
	p := cacheRuntimeArchive(t, m, Package{Name: "java", Version: "21.0.13_9", Arch: "arm64", Binaries: map[string]string{"java": "Contents/Home/bin/java"}})
	return m, p
}

func TestUserStorageLocations(t *testing.T) {
	m, p := userStorageFixture(t)
	want := filepath.Join(m.Home, "Library/Java/JavaVirtualMachines/temurin-21.0.13")
	if got := m.PackageDirectory(p); got != want {
		t.Fatalf("JDK location = %s, want %s", got, want)
	}
	cache := filepath.Join(m.Home, "Library/Caches/ServiceManager/Development")
	if m.downloadDirectory() != filepath.Join(cache, "downloads") || m.nodeIndexPath() != filepath.Join(cache, "node-index.json") {
		t.Fatal("download and catalog caches must use Library/Caches")
	}
	if _, err := os.Stat(filepath.Join(m.Root, "downloads")); !os.IsNotExist(err) {
		t.Fatal("created download cache in persistent application storage", err)
	}
	for _, name := range []string{"go", "node", "mysql", "redis", "mongodb", "caddy"} {
		pkg := Package{Name: name, Version: "1.2.3", Arch: "arm64"}
		if m.PackageDirectory(pkg) != filepath.Join(m.Root, "packages", pkg.ID()) {
			t.Fatalf("unexpected location for %s", name)
		}
	}
	isolated := New(t.TempDir())
	isolated.Home = m.Home
	if !inside(isolated.Root, isolated.PackageDirectory(p)) || !inside(isolated.Root, isolated.downloadDirectory()) || !inside(isolated.Root, isolated.nodeIndexPath()) {
		t.Fatal("explicit storage root escaped into the shared user directories")
	}
}

func TestJavaStandardDirectoryLifecycle(t *testing.T) {
	m, p := userStorageFixture(t)
	ctx := context.Background()
	stage, err := m.stagePackage(ctx, p)
	if err != nil {
		t.Fatal(err)
	}
	if filepath.Dir(stage.Dir) != filepath.Dir(m.PackageDirectory(p)) {
		t.Fatal("JDK staging must be on the destination volume")
	}
	os.RemoveAll(stage.Dir)
	if err = m.Install(ctx, p); err != nil {
		t.Fatal(err)
	}
	assertIntegrity(t, m, p, "Verified")
	if err = m.SetRuntimeDefault(ctx, "java", p.ID()); err != nil {
		t.Fatal(err)
	}
	script, err := os.ReadFile(m.runtimeScript("java"))
	if err != nil || !strings.Contains(string(script), m.PackageDirectory(p)+"/Contents/Home") {
		t.Fatal("JAVA_HOME does not point to the standard bundle", string(script), err)
	}
	info, err := m.PackageDetails(ctx, p)
	if err != nil || info.Location != m.PackageDirectory(p) || !info.Cached {
		t.Fatal(info, err)
	}
	binary := filepath.Join(m.PackageDirectory(p), p.Binaries["java"])
	if err = os.WriteFile(binary, []byte("damaged"), 0700); err != nil {
		t.Fatal(err)
	}
	assertIntegrity(t, m, p, "Needs repair")
	// Cancellation after the swap must restore both the bundle and its index.
	cancelCtx, cancel := context.WithCancel(ctx)
	defer cancel()
	m.Event = func(message string) {
		if strings.HasPrefix(message, "Committing package files") {
			cancel()
		}
	}
	if err = m.repairPackage(cancelCtx, p.ID()); !errors.Is(err, context.Canceled) {
		t.Fatal(err)
	}
	if b, err := os.ReadFile(binary); err != nil || string(b) != "damaged" {
		t.Fatal("repair cancellation lost the original bundle", err)
	}
	m.Event = nil
	if err = m.repairPackage(ctx, p.ID()); err != nil {
		t.Fatal(err)
	}
	assertIntegrity(t, m, p, "Verified")
	if err = os.RemoveAll(m.cacheDirectory()); err != nil {
		t.Fatal(err)
	}
	s, err := m.Read()
	if err != nil {
		t.Fatal(err)
	}
	if _, err = m.Binary(&s, p.ID(), "java"); err != nil {
		t.Fatal("purging cache broke installed Java", err)
	}
	info, err = m.PackageDetails(ctx, p)
	if err != nil || info.Cached || !info.Installed {
		t.Fatal(info, err)
	}
	if err = m.Uninstall(ctx, p.ID()); err != nil {
		t.Fatal(err)
	}
	if _, err = os.Stat(m.PackageDirectory(p)); !os.IsNotExist(err) {
		t.Fatal("uninstall retained owned JDK", err)
	}
	if _, err = os.Stat(m.runtimeScript("java")); !os.IsNotExist(err) {
		t.Fatal("uninstall retained JAVA_HOME", err)
	}
}

func TestJavaProtectsForeignInstallations(t *testing.T) {
	for _, mode := range []string{"unregistered", "marker-missing", "marker-changed", "symlink"} {
		t.Run(mode, func(t *testing.T) {
			m, p := userStorageFixture(t)
			ctx := context.Background()
			dest := m.PackageDirectory(p)
			if mode == "unregistered" {
				if err := os.MkdirAll(dest, 0700); err != nil {
					t.Fatal(err)
				}
			} else if err := m.Install(ctx, p); err != nil {
				t.Fatal(err)
			}
			marker := filepath.Join(dest, javaOwnerFile)
			switch mode {
			case "marker-missing":
				if err := os.Remove(marker); err != nil {
					t.Fatal(err)
				}
			case "marker-changed":
				if err := os.WriteFile(marker, []byte("another installer"), 0600); err != nil {
					t.Fatal(err)
				}
			case "symlink":
				if err := os.RemoveAll(dest); err != nil {
					t.Fatal(err)
				}
				if err := os.Symlink(t.TempDir(), dest); err != nil {
					t.Fatal(err)
				}
			}
			sentinel := filepath.Join(dest, "keep")
			if err := os.WriteFile(sentinel, []byte("user JDK"), 0600); err != nil {
				t.Fatal(err)
			}
			if mode == "unregistered" {
				if err := m.Install(ctx, p); err != nil {
					t.Fatal("could not install beside unregistered JDK", err)
				}
			} else {
				if err := m.repairPackage(ctx, p.ID()); err == nil {
					t.Fatal("repaired foreign JDK")
				}
				if err := m.Uninstall(ctx, p.ID()); err == nil {
					t.Fatal("deleted foreign JDK")
				}
			}
			if b, err := os.ReadFile(sentinel); err != nil || string(b) != "user JDK" {
				t.Fatal("foreign JDK changed", err)
			}
			stages, err := filepath.Glob(filepath.Join(filepath.Dir(dest), ".install-*"))
			if err != nil || len(stages) != 0 {
				t.Fatal("failed publication leaked staging directories", stages, err)
			}
		})
	}
}

func TestJavaCoexistsWithOtherInstallers(t *testing.T) {
	m, p := userStorageFixture(t)
	parent := filepath.Dir(m.PackageDirectory(p))
	// The same upstream version installed by another tool has its own bundle.
	foreign := filepath.Join(parent, "temurin-"+p.Version+"-arm64.jdk")
	if err := os.MkdirAll(foreign, 0700); err != nil {
		t.Fatal(err)
	}
	sentinel := filepath.Join(foreign, "keep")
	if err := os.WriteFile(sentinel, []byte("other installer"), 0600); err != nil {
		t.Fatal(err)
	}
	ctx := context.Background()
	if err := m.Install(ctx, p); err != nil {
		t.Fatal(err)
	}
	if err := m.repairPackage(ctx, p.ID()); err != nil {
		t.Fatal(err)
	}
	if err := m.Uninstall(ctx, p.ID()); err != nil {
		t.Fatal(err)
	}
	if b, err := os.ReadFile(sentinel); err != nil || string(b) != "other installer" {
		t.Fatal("changed another installer's JDK", err)
	}
}

func TestPackagePublicationRejectsLateCollision(t *testing.T) {
	if runtime.GOOS != "darwin" {
		t.Skip("requires macOS exclusive rename")
	}
	root := t.TempDir()
	stage, dest := filepath.Join(root, ".install-test"), filepath.Join(root, "jdk.jdk")
	if err := os.Mkdir(stage, 0700); err != nil {
		t.Fatal(err)
	}
	// Model another installer creating an empty bundle after our last check.
	if err := os.Mkdir(dest, 0700); err != nil {
		t.Fatal(err)
	}
	before, err := os.Stat(dest)
	if err != nil {
		t.Fatal(err)
	}
	if err := publishNewPackageDirectory(stage, dest); err == nil {
		t.Fatal("replaced a concurrently created destination")
	}
	after, err := os.Stat(dest)
	if err != nil || !os.SameFile(before, after) {
		t.Fatal("collision changed the destination", err)
	}
	if _, err := os.Stat(stage); err != nil {
		t.Fatal("collision lost the staged package", err)
	}
}

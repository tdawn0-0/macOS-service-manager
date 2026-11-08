package devplatform

import (
	"archive/tar"
	"bytes"
	"compress/gzip"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

type runtimeTransport func(*http.Request) (*http.Response, error)

func (f runtimeTransport) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }
func TestRuntimeCatalogs(t *testing.T) {
	old := http.DefaultClient
	defer func() { http.DefaultClient = old }()
	checksum := strings.Repeat("a", 64)
	http.DefaultClient = &http.Client{Transport: runtimeTransport(func(r *http.Request) (*http.Response, error) {
		if e := r.Context().Err(); e != nil {
			return nil, e
		}
		body := `{"jdks":[{"vendor":"Eclipse","product":"Temurin","jdk_version":"21.0.13","packages":[{"os":"macOS","arch":"aarch64","url":"https://example.test/jdk.tar.gz","sha256":"` + checksum + `","package_type":"targz","package_root_prefix":"jdk-21.0.13+9","package_to_java_home_prefix":"Contents/Home","install_folder_name":"temurin-21.0.13"}]}]}`
		if r.URL.Host == "go.dev" {
			body = `[{"version":"go1.27.3","stable":true,"files":[{"os":"darwin","arch":"arm64","kind":"archive"}]}]`
		}
		return &http.Response{StatusCode: 200, Body: io.NopCloser(strings.NewReader(body)), Header: http.Header{}}, nil
	})}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if _, e := resolveJava(ctx, "temurin-21.0.13"); e == nil {
		t.Fatal("ignored cancellation")
	}
	m := testManager(t)
	p, e := resolveJava(context.Background(), "temurin-21.0.13")
	if e != nil || p.Binaries["java"] != "Contents/Home/bin/java" || len(p.SHA256) != 64 {
		t.Fatal(p, e)
	}
	if _, e = resolveJava(context.Background(), "temurin-21.0.12"); e == nil {
		t.Fatal("accepted unavailable JDK")
	}
	checksum = "invalid"
	if _, e := resolveJava(context.Background(), "temurin-21.0.13"); e == nil {
		t.Fatal("accepted invalid upstream checksum")
	}
	rows, e := m.RuntimeVersions(context.Background(), "go")
	if e != nil || len(rows) != 1 || rows[0] != "1.27.3" {
		t.Fatal(rows, e)
	}
}
func TestUVPythonManagement(t *testing.T) {
	m := testManager(t)
	m.Home = t.TempDir()
	uv := filepath.Join(m.Home, "uv")
	os.WriteFile(uv, []byte("fixture"), 0700)
	m.withState(context.Background(), func(s *State) error { s.UVExecutable = uv; return nil })
	var commands []string
	m.Run = func(ctx context.Context, p string, args ...string) (string, error) {
		line := strings.Join(args, " ")
		commands = append(commands, line)
		if strings.HasSuffix(line, "python pin --global --no-project") {
			return "cpython-3.12.12-macos-aarch64-none\n", nil
		}
		if strings.Contains(line, "python list") {
			rows := []UVPython{{Key: "cpython-3.12.12-macos-aarch64-none", Version: "3.12.12", Path: "/fixture/python", OS: "macos", Arch: "aarch64"}, {Key: "cpython-3.11.13-macos-aarch64-none", Version: "3.11.13", Path: "/fixture/other", OS: "macos", Arch: "aarch64"}}
			b, _ := json.Marshal(rows)
			return string(b), nil
		}
		return "", ctx.Err()
	}
	key := "cpython-3.12.12-macos-aarch64-none"
	if pin, e := m.UVGlobalPin(context.Background()); e != nil || pin != key {
		t.Fatal(pin, e)
	}
	if e := m.InstallUVPython(context.Background(), key); e != nil {
		t.Fatal(e)
	}
	if e := m.UVPythonAction(context.Background(), key, "default"); e != nil {
		t.Fatal(e)
	}
	if e := m.UVPythonAction(context.Background(), key, "uninstall"); e != nil {
		t.Fatal(e)
	}
	if e := m.UVPythonAction(context.Background(), "system", "uninstall"); e == nil {
		t.Fatal("accepted system interpreter")
	}
	all := strings.Join(commands, "\n")
	if !strings.Contains(all, "uninstall "+key) || strings.Contains(all, "uninstall cpython-3.11") {
		t.Fatal(all)
	}
	if !strings.Contains(all, "install --no-bin "+key) || !strings.Contains(all, "pin --global --no-project "+key) || strings.Contains(all, "UV_PYTHON_INSTALL_DIR") {
		t.Fatal(all)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if e := m.InstallUVPython(ctx, key); !errors.Is(e, context.Canceled) {
		t.Fatal(e)
	}
	m.Run = func(context.Context, string, ...string) (string, error) {
		return `[{"key":"--all","version":"3.12"}]`, nil
	}
	if _, e := m.UVPythons(context.Background(), true); e == nil {
		t.Fatal("accepted unknown JSON entry")
	}
	os.Remove(uv)
	if _, e := m.UVPath(); e == nil {
		t.Fatal("silently fell back after selected uv removed")
	}
}
func TestAdditionalRuntimeSelection(t *testing.T) {
	t.Run("installed-versions", additionalRuntimeInstalledVersions)
	m := testManager(t)
	m.Home = t.TempDir()
	m.withState(context.Background(), func(s *State) error { s.RuntimeDefaults = map[string]string{"go": "keep"}; return nil })
	if e := m.SetRuntimeDefault(context.Background(), "java", "missing"); e == nil {
		t.Fatal("accepted missing runtime")
	}
	if e := m.ConfigureRuntimeShell(context.Background(), "java", true); e == nil {
		t.Fatal("enabled without default")
	}
	s, e := m.Read()
	if e != nil || s.RuntimeDefaults["go"] != "keep" {
		t.Fatal(s, e)
	}
	rc := filepath.Join(m.Home, ".zshrc")
	os.WriteFile(rc, []byte("# user setting\n"), 0600)
	if e = m.ConfigureRuntimeShell(context.Background(), "java", false); e != nil {
		t.Fatal(e)
	}
	b, _ := os.ReadFile(rc)
	if string(b) != "# user setting\n" {
		t.Fatal(string(b))
	}
}

func additionalRuntimeInstalledVersions(t *testing.T) {
	m := testManager(t)
	m.Home = t.TempDir()
	ctx := context.Background()
	var javaIDs []string
	for _, pair := range [][2]string{{"java", "21.0.13_9.0.LTS"}, {"java", "25.0.1_8.0.LTS"}, {"go", "1.27.3"}} {
		p := Package{Name: pair[0], Version: pair[1], Arch: "arm64", Binaries: map[string]string{pair[0]: "bin/" + pair[0]}}
		if p.Name == "java" {
			p.Binaries["java"] = "Contents/Home/bin/java"
			javaIDs = append(javaIDs, p.ID())
		}
		p = cacheRuntimeArchive(t, m, p)
		if e := m.Install(ctx, p); e != nil {
			t.Fatal(e)
		}
		if e := m.SetRuntimeDefault(ctx, p.Name, p.ID()); e != nil {
			t.Fatal(e)
		}
	}
	rc := filepath.Join(m.Home, ".zshrc")
	target := filepath.Join(m.Home, "user rc")
	os.WriteFile(target, []byte("# user settings\n"), 0600)
	os.Symlink(target, rc)
	for _, name := range []string{"java", "go"} {
		if e := m.ConfigureRuntimeShell(ctx, name, true); e != nil {
			t.Fatal(e)
		}
		if e := m.ConfigureRuntimeShell(ctx, name, true); e != nil {
			t.Fatal(e)
		}
	}
	b, _ := os.ReadFile(rc)
	if strings.Count(string(b), "# BEGIN Service Manager java") != 1 || !strings.Contains(string(b), "# user settings") {
		t.Fatal(string(b))
	}
	f, _ := os.Lstat(rc)
	if f.Mode()&os.ModeSymlink == 0 {
		t.Fatal("shell symlink replaced")
	}
	if e := m.SetRuntimeDefault(ctx, "java", javaIDs[0]); e != nil {
		t.Fatal(e)
	}
	s, _ := m.Read()
	if s.RuntimeDefaults["java"] != javaIDs[0] || s.RuntimeDefaults["go"] != "go@1.27.3-arm64" {
		t.Fatal(s.RuntimeDefaults)
	}
	b, _ = os.ReadFile(m.runtimeScript("java"))
	if !strings.Contains(string(b), "JAVA_HOME=") || !strings.Contains(string(b), filepath.Join(m.packageDir(s.Packages[javaIDs[0]]), "Contents/Home")) {
		t.Fatal(string(b))
	}
	b, _ = os.ReadFile(m.runtimeScript("go"))
	if strings.Contains(string(b), "GOROOT") || strings.Contains(string(b), "GOPATH") {
		t.Fatal(string(b))
	}
	if e := m.ConfigureRuntimeShell(ctx, "java", false); e != nil {
		t.Fatal(e)
	}
	b, _ = os.ReadFile(rc)
	if strings.Contains(string(b), "Service Manager java") || !strings.Contains(string(b), "Service Manager go") {
		t.Fatal(string(b))
	}
	if e := m.Uninstall(ctx, javaIDs[0]); e != nil {
		t.Fatal(e)
	}
	s, _ = m.Read()
	if s.RuntimeDefaults["java"] != "" {
		t.Fatal("uninstalled version still default")
	}
	if _, e := os.Stat(m.runtimeScript("java")); !os.IsNotExist(e) {
		t.Fatal(e)
	}
}

// Uses a native echo binary to test the installer without downloading or running a JDK.
func cacheRuntimeArchive(t *testing.T, m *Manager, p Package) Package {
	t.Helper()
	p.URL = "https://example.invalid/runtime.tar.gz"
	p.Format = "tar.gz"
	p.Strip = 1
	binary, e := os.ReadFile("/bin/echo")
	if e != nil {
		t.Fatal(e)
	}
	var archive bytes.Buffer
	gz := gzip.NewWriter(&archive)
	tw := tar.NewWriter(gz)
	if e = tw.WriteHeader(&tar.Header{Name: "runtime/" + p.Binaries[p.Name], Mode: 0755, Size: int64(len(binary))}); e != nil {
		t.Fatal(e)
	}
	if _, e = tw.Write(binary); e != nil {
		t.Fatal(e)
	}
	if p.Name == "java" {
		info := []byte(`<?xml version="1.0"?><plist version="1.0"><dict><key>JavaVM</key><dict><key>JVMVersion</key><string>21.0.13</string></dict></dict></plist>`)
		if e = tw.WriteHeader(&tar.Header{Name: "runtime/Contents/Info.plist", Mode: 0644, Size: int64(len(info))}); e != nil {
			t.Fatal(e)
		}
		if _, e = tw.Write(info); e != nil {
			t.Fatal(e)
		}
		release := []byte("JAVA_VERSION=\"21.0.13\"\nIMPLEMENTOR=\"Fixture Vendor\"\nOS_ARCH=\"aarch64\"\n")
		releasePath := filepath.Join("runtime", filepath.Dir(filepath.Dir(p.Binaries["java"])), "release")
		if e = tw.WriteHeader(&tar.Header{Name: releasePath, Mode: 0644, Size: int64(len(release))}); e != nil {
			t.Fatal(e)
		}
		if _, e = tw.Write(release); e != nil {
			t.Fatal(e)
		}
	}
	if e = tw.Close(); e != nil {
		t.Fatal(e)
	}
	if e = gz.Close(); e != nil {
		t.Fatal(e)
	}
	sum := sha256.Sum256(archive.Bytes())
	p.SHA256 = hex.EncodeToString(sum[:])
	if e = os.WriteFile(filepath.Join(m.downloadDirectory(), p.SHA256), archive.Bytes(), 0600); e != nil {
		t.Fatal(e)
	}
	return p
}

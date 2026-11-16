package devplatform

import (
	"context"
	"devbase/internal/privhelper"
	"errors"
	"howett.net/plist"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
)

func TestSocketJobBindsLoopbackAndDropsPrivileges(t *testing.T) {
	if os.Getuid() == 0 {
		t.Skip("normal-user job")
	}
	m := testManager(t)
	sites := []proxyRoute{{Domain: "web.devbase.localhost", LocalPort: 3000, Port: 443}}
	data, e := m.socketJob(sites, "/Applications/Devbase.app/Contents/MacOS/Devbase", "developer")
	if e != nil {
		t.Fatal(e)
	}
	var job map[string]any
	if _, e = plist.Unmarshal(data, &job); e != nil {
		t.Fatal(e)
	}
	if job["UserName"] != "developer" {
		t.Fatal("proxy runs as wrong user")
	}
	sockets := job["Sockets"].(map[string]any)
	socket := sockets["HTTPS-443"].(map[string]any)
	if socket["SockNodeName"] != "127.0.0.1" || socket["SockServiceName"] != "443" || socket["SockFamily"] != "IPv4" {
		t.Fatal(socket)
	}
	if _, e = m.socketJob(sites, "/bin/test", "root"); e == nil {
		t.Fatal("allowed root proxy")
	}
}
func TestSocketJobInstallAndStopDelegateToPrivilegedHelper(t *testing.T) {
	if os.Getuid() == 0 {
		t.Skip("normal-user job")
	}
	m := testManager(t)
	exe := filepath.Join(t.TempDir(), "devbase")
	if err := os.WriteFile(exe, []byte("bin"), 0o700); err != nil {
		t.Fatal(err)
	}
	m.Program = exe
	sockDir := t.TempDir()
	m.SocketJobDir = sockDir
	loaded := false
	m.Run = func(_ context.Context, name string, args ...string) (string, error) {
		if filepath.Base(name) == "id" {
			return "developer\n", nil
		}
		if filepath.Base(name) == "launchctl" && len(args) > 0 && args[0] == "print" {
			if loaded {
				return "active", nil
			}
			return "", errors.New("not loaded")
		}
		return "", nil
	}
	sites := []proxyRoute{{Domain: "web.devbase.localhost", LocalPort: 3000, Port: 443}}

	if err := m.installSocketJob(context.Background(), sites); !errors.Is(err, privhelper.ErrNotInstalled) {
		t.Fatalf("expected ErrNotInstalled without helper, got %v", err)
	}

	var ran [][]string
	privhelper.InstallTestHelper(t, privhelper.ServerConfig{
		SocketJobDirs: []string{sockDir},
		Run: func(_ context.Context, name string, args ...string) ([]byte, error) {
			ran = append(ran, append([]string{name}, args...))
			if len(args) > 0 && args[0] == "print" && !loaded {
				return []byte("not loaded"), errors.New("exit 113")
			}
			if len(args) > 0 && args[0] == "bootstrap" {
				loaded = true
			}
			if len(args) > 0 && args[0] == "bootout" {
				loaded = false
			}
			return nil, nil
		},
	})

	if err := m.installSocketJob(context.Background(), sites); err != nil {
		t.Fatalf("installSocketJob: %v", err)
	}
	jobPath := m.socketJobPath()
	data, err := os.ReadFile(jobPath)
	if err != nil {
		t.Fatalf("expected socket plist written at %s: %v", jobPath, err)
	}
	var job map[string]any
	if _, err := plist.Unmarshal(data, &job); err != nil {
		t.Fatal(err)
	}
	if job["Label"] != m.label("proxy-sockets") || job["UserName"] != "developer" {
		t.Fatalf("unexpected socket job plist: %+v", job)
	}
	if !strings.HasPrefix(job["Label"].(string), "xyz.jyeu.devbase.") {
		t.Fatalf("expected devbase label prefix, got %v", job["Label"])
	}

	if err := m.stopSocketJob(context.Background()); err != nil {
		t.Fatalf("stopSocketJob: %v", err)
	}
	if _, err := os.Stat(jobPath); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("expected socket job plist removed, stat err=%v", err)
	}
	wantBoot := []string{"/bin/launchctl", "bootstrap", "system", jobPath}
	if !slices.ContainsFunc(ran, func(cmd []string) bool { return slices.Equal(cmd, wantBoot) }) {
		t.Fatalf("expected helper bootstrap %v in %v", wantBoot, ran)
	}
}
func TestSocketListenerRequiresLaunchd(t *testing.T) {
	if _, e := activatedListener("NonexistentTestSocket"); e == nil {
		t.Fatal("accepted a socket outside launchd activation")
	}
}
func TestEquivalentSocketConfigurationIgnoresFormatting(t *testing.T) {
	a, _ := plist.Marshal(map[string]any{"Label": "same", "RunAtLoad": true}, plist.XMLFormat)
	b, _ := plist.Marshal(map[string]any{"RunAtLoad": true, "Label": "same"}, plist.BinaryFormat)
	if !equivalentPlists(a, b) {
		t.Fatal("reinstall prompted for identical configuration")
	}
	if equivalentPlists(a, []byte("bad")) {
		t.Fatal("invalid plist accepted")
	}
}
func TestUnprivilegedPortSets(t *testing.T) {
	sites := []proxyRoute{{Port: 8443}, {Port: 8443}, {Port: 9443}}
	if privilegedPorts(sites) || len(proxyListenerPorts(sites)) != 2 {
		t.Fatal("incorrect port set")
	}
	if !privilegedPorts(append(sites, proxyRoute{Port: 443})) {
		t.Fatal("missed privileged port")
	}
}

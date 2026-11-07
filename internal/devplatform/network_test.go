package devplatform

import (
	"howett.net/plist"
	"os"
	"strings"
	"testing"
)

func TestSocketJobBindsLoopbackAndDropsPrivileges(t *testing.T) {
	if os.Getuid() == 0 {
		t.Skip("normal-user job")
	}
	m := testManager(t)
	sites := []Site{{ID: "web", Domain: "web.test", Upstream: "http://127.0.0.1:3000", Port: 443}}
	data, e := m.socketJob(sites, "/Applications/Service Manager.app/Contents/MacOS/Service Manager", "developer")
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
func TestAuthorizedSocketInstallEmbedsImmutableConfiguration(t *testing.T) {
	payload := []byte("config with 'quoted' names and $(shell)")
	script := socketInstallScript(payload, "/Library/LaunchDaemons/example.plist", "system/example")
	if strings.Contains(script, "$(shell)") || strings.Contains(script, "config with") {
		t.Fatal("configuration injected into shell")
	}
	if !strings.Contains(script, "/Library/LaunchDaemons/.msm-socket.") || !strings.Contains(script, "root:wheel") || !strings.Contains(script, "bootstrap system") {
		t.Fatal("missing secure root install steps")
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
	sites := []Site{{Port: 8443}, {Port: 8443}, {Port: 9443}}
	if privilegedPorts(sites) || len(sitePorts(sites)) != 2 {
		t.Fatal("incorrect port set")
	}
	if !privilegedPorts(append(sites, Site{Port: 443})) {
		t.Fatal("missed privileged port")
	}
}

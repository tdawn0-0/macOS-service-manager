package main

import (
	"context"
	"crypto/tls"
	"crypto/x509"
	"encoding/json"
	"errors"
	"fmt"
	"github.com/egoist/mygo/ui"
	"go.mongodb.org/mongo-driver/v2/bson"
	"go.mongodb.org/mongo-driver/v2/mongo"
	"go.mongodb.org/mongo-driver/v2/mongo/options"
	"io"
	"macos-service-manager/internal/devplatform"
	"macos-service-manager/internal/service"
	"macos-service-manager/internal/system"
	"net"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

type devUIHarness struct {
	foregroundOnly bool
	t              *testing.T
	a              *app
	ui             *ui.Tester
	root           string
	opened         []string
	copied         []string
	posted         chan func()
	width, height  int
}

func newDevUIHarness(t *testing.T) *devUIHarness {
	return newDevUIHarnessSize(t, 1120, 780, false)
}
func newDevUIHarnessSize(t *testing.T, width, height int, dark bool) *devUIHarness {
	t.Helper()
	oldAdmin := system.RunAdmin
	system.RunAdmin = func(string, ...string) (string, error) {
		return "", errors.New("privileged operations are disabled in unattended UI tests")
	}
	t.Cleanup(func() { system.RunAdmin = oldAdmin })
	root := t.TempDir()
	binary := filepath.Join(t.TempDir(), "devmgr")
	c := exec.Command("go", "build", "-o", binary, "./cmd/devmgr")
	if out, e := c.CombinedOutput(); e != nil {
		t.Fatalf("build app entry point: %v %s", e, out)
	}
	h := &devUIHarness{t: t, root: root, posted: make(chan func(), 256), width: width, height: height}
	a := newAppWith(nil, nil, t.TempDir())
	h.a = a
	a.nav = "development"
	a.copy = func(value string) { h.copied = append(h.copied, value) }
	a.dev.loaded = true
	a.post = func(f func()) { h.posted <- f }
	a.spawn = func(f func()) { go f() }
	a.dev.nodeReleasesTried = true // the release list is read from nodejs.org only where a test asks
	home := t.TempDir()
	a.dev.managerFactory = func() *devplatform.Manager {
		m := devplatform.New(root)
		m.Home = home
		m.Program = binary
		realRun := m.Run
		m.Run = func(ctx context.Context, name string, args ...string) (string, error) {
			if name == "/usr/bin/open" {
				h.opened = append(h.opened, args[0])
				return "", nil
			}
			return realRun(ctx, name, args...)
		}
		return m
	}
	h.ui = ui.NewTester(a.view, width, height)
	h.ui.SetDark(dark)
	h.ui.Frame()
	t.Cleanup(func() {
		h.foregroundOnly = false
		a.cancelBackgroundTasks()
		if a.dev.cancel != nil {
			a.dev.cancel()
		}
		h.waitIdle()
	})
	return h
}
func (h *devUIHarness) click(label string) {
	h.t.Helper()
	for _, section := range devSections {
		if section == label {
			r, found := h.ui.Find(label)
			if !found {
				h.t.Fatal("navigation missing")
			}
			h.ui.ClickAt(r.X+r.W/2, r.Y+r.H/2)
			h.ui.Frame()
			h.waitIdle()
			return
		}
	}
	for n := 0; n < 18; n++ {
		r, ok := h.ui.Find(label)
		if ok && r.W > 0 && r.H > 0 && r.Y >= barHeight && r.Y+r.H < float32(h.height-45) {
			if e := h.ui.Click(label); e != nil {
				h.t.Fatal(e)
			}
			h.ui.Frame()
			h.waitIdle()
			return
		}
		if h.a.quit.open && ok && r.H > 0 && r.Y >= 0 && r.Y+r.H < float32(h.height) {
			if e := h.ui.Click(label); e != nil {
				h.t.Fatal(e)
			}
			h.ui.Frame()
			h.waitIdle()
			return
		}
		dy := float32(230)
		if ok && r.Y < 100 {
			dy = -230
		}
		if label == "Save project" || label == "Details" || label == "Refresh" || label == "Cancel operation" || label == "Diagnostics" || label == "Tasks" || label == "Package tasks" || label == "Database tasks" || label == "Project tasks" {
			if e := h.ui.Click(label); e == nil {
				h.ui.Frame()
				h.waitIdle()
				return
			}
		}
		h.ui.Scroll(float32(h.width-20), float32(h.height/2), 0, dy)
		h.ui.Frame()
	}
	r, ok := h.ui.Find(label)
	h.t.Fatalf("control %q is not reachable (%+v, %v): %v", label, r, ok, h.ui.Texts())
}
func (h *devUIHarness) fill(label, value string) {
	h.t.Helper()
	h.click(label)
	h.ui.Key(ui.Cmd, ui.KeyA)
	h.ui.Type(value)
	h.ui.Frame()
}
func freeUIPort(t *testing.T) int {
	t.Helper()
	l, e := net.Listen("tcp4", "127.0.0.1:0")
	if e != nil {
		t.Fatal(e)
	}
	port := l.Addr().(*net.TCPAddr).Port
	l.Close()
	return port
}
func TestUIEndToEndImportConfigureRunHTTPS(t *testing.T) {
	for _, size := range [][2]int{{1120, 720}, {900, 520}} {
		for _, dark := range []bool{false, true} {
			t.Run(fmt.Sprintf("%dx%d-dark-%t", size[0], size[1], dark), func(t *testing.T) {
				runUIProjectLifecycle(t, newDevUIHarnessSize(t, size[0], size[1], dark))
			})
		}
	}
}
func runUIProjectLifecycle(t *testing.T, h *devUIHarness) {
	dir := t.TempDir()
	if e := os.WriteFile(filepath.Join(dir, "index.html"), []byte("UI end-to-end served through managed HTTPS"), 0600); e != nil {
		t.Fatal(e)
	}
	h.fill("Project folder", dir)
	h.click("Import project")
	if len(h.a.dev.state.Projects) != 1 {
		t.Fatalf("UI import failed: %s", h.a.dev.output)
	}
	id := h.a.dev.id
	manager := devplatform.New(h.root)
	defer func() { _ = manager.Down(context.Background(), id); _ = manager.StopProxy(context.Background()) }()
	h.fill("Project name", "UI E2E project")
	h.click("Services")
	h.click("Add service")
	h.fill("Service name", "web")
	port := freeUIPort(t)
	python, e := exec.LookPath("python3")
	if e != nil {
		t.Fatal(e)
	}
	h.fill("Program and arguments", fmt.Sprintf("%s\n-m\nhttp.server\n%d\n--bind\n127.0.0.1", python, port))
	h.fill("Readiness check", fmt.Sprintf("http://127.0.0.1:%d", port))
	h.fill("Readiness timeout (seconds)", "5")
	h.click("Save project")
	saved, e := manager.Read()
	if e != nil {
		t.Fatal(e)
	}
	p := saved.Projects[id]
	if p.Name != "UI E2E project" || len(p.Services) != 1 || p.Services[0].ID != "web" || p.Services[0].ReadinessTimeoutSeconds != 5 {
		t.Fatalf("form did not persist: %+v; %s", p, h.a.dev.output)
	}
	h.click("HTTPS")
	httpsPort := freeUIPort(t)
	h.fill("Site name", "ui-e2e")
	h.fill("Local domain", "ui-e2e.test")
	h.fill("Upstream address", fmt.Sprintf("http://127.0.0.1:%d", port))
	h.fill("HTTPS port", fmt.Sprint(httpsPort))
	h.click("Save HTTPS site")
	h.click("Projects")
	h.click("Start project")
	if strings.Contains(h.a.dev.output, "Error:") {
		t.Fatal(h.a.dev.output)
	}
	ca, e := os.ReadFile(filepath.Join(h.root, "proxy", "ca.pem"))
	if e != nil {
		t.Fatal(e)
	}
	pool := x509.NewCertPool()
	pool.AppendCertsFromPEM(ca)
	transport := &http.Transport{TLSClientConfig: &tls.Config{RootCAs: pool, ServerName: "ui-e2e.test"}}
	defer transport.CloseIdleConnections()
	client := &http.Client{Transport: transport, Timeout: 3 * time.Second}
	request, _ := http.NewRequest("GET", fmt.Sprintf("https://127.0.0.1:%d/", httpsPort), nil)
	request.Host = "ui-e2e.test"
	response, e := client.Do(request)
	if e != nil {
		t.Fatal(e)
	}
	body, _ := io.ReadAll(response.Body)
	response.Body.Close()
	if !strings.Contains(string(body), "UI end-to-end served") {
		t.Fatal(string(body))
	}
	h.click("Restart project")
	response, e = client.Do(request.Clone(context.Background()))
	if e != nil {
		t.Fatal("UI restart did not restore HTTPS access", e)
	}
	body, _ = io.ReadAll(response.Body)
	response.Body.Close()
	if response.StatusCode != 200 || !strings.Contains(string(body), "UI end-to-end served") {
		t.Fatal("UI restart left the service unavailable", string(body))
	}
	h.click("HTTPS")
	h.click("Site ui-e2e.test")
	h.click(fmt.Sprintf("Open https://ui-e2e.test:%d", httpsPort))
	if len(h.opened) != 1 || h.opened[0] != fmt.Sprintf("https://ui-e2e.test:%d", httpsPort) {
		t.Fatal("open website did not hand off correct URL", h.opened)
	}
	h.click("Diagnostics")
	if h.a.dev.diagnosticReport == nil || !h.a.dev.diagnosticsOpen {
		t.Fatal("UI did not display the diagnostic report", h.a.dev.output)
	}
	processReady, systemTrustFailed := false, false
	for _, check := range h.a.dev.diagnosticReport.Checks {
		if check.Layer == "Readiness" && check.Status == "OK" {
			processReady = true
		}
		if check.Layer == "Proxy TLS" && check.Status == "Failed" {
			systemTrustFailed = true
		}
	}
	// This isolated CA is deliberately absent from system trust. A positive
	// private-root request above must not make the OS-trust diagnostic pass.
	if !processReady || !systemTrustFailed {
		t.Fatal("diagnostics confused application readiness and system TLS trust", h.a.dev.diagnosticReport.Text())
	}
	h.click("Copy redacted report")
	if len(h.copied) != 1 {
		t.Fatal("UI did not copy the report")
	}
	var redacted devplatform.DiagnosticReport
	if e = json.Unmarshal([]byte(h.copied[0]), &redacted); e != nil || !redacted.Redacted || strings.Contains(h.copied[0], "UI E2E project") || strings.Contains(h.copied[0], "ui-e2e.test") || strings.Contains(h.copied[0], dir) {
		t.Fatal("UI copied identifying data", e, h.copied)
	}
	var revealed string
	h.a.reveal = func(path string) { revealed = path }
	h.click("Export redacted report")
	exported, e := os.ReadFile(revealed)
	if e != nil || string(exported) != h.copied[0] {
		t.Fatal("UI export differs from the reviewed report", e, string(exported))
	}
	h.click("Back to workspace")
	h.click("Projects")
	h.click("Logs")
	h.click("Read service logs")
	if !strings.Contains(h.a.dev.logText, "GET /") {
		t.Fatal("service output is missing from the log viewer", h.a.dev.logText)
	}
	if h.width == 1120 {
		panel := ui.NewTester(func(c *ui.Context) { h.a.menubarView(c, func() {}) }, 320, h.a.panelHeight())
		panel.Frame()
		if e = panel.Click("Run project UI E2E project"); e != nil {
			t.Fatal(e)
		}
		panel.Frame()
		h.waitIdle()
		if status := h.a.dev.statuses["project:"+id]; status != "Stopped" {
			t.Fatal("menu bar did not stop the project", status)
		}
		panel.Frame()
		if e = panel.Click("Run project UI E2E project"); e != nil {
			t.Fatal(e)
		}
		panel.Frame()
		h.waitIdle()
		if status := h.a.dev.statuses["project:"+id]; status != "Healthy" {
			t.Fatal("menu bar did not start the project", status)
		}
	}
	h.click("Stop project")
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	if c, e := (&net.Dialer{}).DialContext(ctx, "tcp", fmt.Sprintf("127.0.0.1:%d", port)); e == nil {
		c.Close()
		t.Fatal("UI stop left the project running")
	}
}

func (h *devUIHarness) choose(label, value string) {
	h.t.Helper()
	h.click(label)
	h.ui.Key(0, ui.KeySpace)
	h.ui.Frame()
	// Popups can overlay the fixed footer and remain clickable there.
	if r, ok := h.ui.Find(value); ok && r.H > 0 && r.Y >= 0 && r.Y+r.H < float32(h.height) {
		if e := h.ui.Click(value); e != nil {
			h.t.Fatal(e)
		}
		h.ui.Frame()
		h.waitIdle()
		return
	}
	h.click(value)
}
func TestUIValidationDoesNotPersistInvalidInput(t *testing.T) {
	h := newDevUIHarness(t)
	h.click("Databases")
	h.click("New database instance")
	h.fill("Instance name", "ui-invalid-db")
	h.fill("Local port", "invalid")
	h.click("Create instance")
	if !strings.Contains(h.a.dev.output, "valid port") {
		t.Fatal(h.a.dev.output)
	}
	manager := devplatform.New(h.root)
	s, e := manager.Read()
	if e != nil || len(s.Instances) != 0 {
		t.Fatal("invalid form persisted an instance", e)
	}
	h.click("HTTPS")
	h.fill("Site name", "invalid-site")
	h.fill("Local domain", "public-domain.com")
	h.fill("Upstream address", "http://127.0.0.1:3000")
	h.click("Save HTTPS site")
	if !strings.Contains(h.a.dev.output, ".test") {
		t.Fatal(h.a.dev.output)
	}
	s, e = manager.Read()
	if e != nil || len(s.Sites) != 0 {
		t.Fatal("invalid site persisted", e)
	}
}

func TestUISoftwareInstallAndDatabaseWorkflow(t *testing.T) {
	cache := os.Getenv("MSM_UI_E2E_PACKAGE_CACHE")
	if cache == "" {
		t.Skip("set MSM_UI_E2E_PACKAGE_CACHE to a verified binary download cache")
	}
	h := newDevUIHarnessSize(t, 900, 520, false)
	pkg, e := devplatform.Resolve(context.Background(), "redis", "7.4.7")
	if e != nil {
		t.Fatal(e)
	}
	source := ""
	for _, dir := range filepath.SplitList(cache) {
		path := filepath.Join(dir, pkg.SHA256)
		if _, err := os.Stat(path); err == nil {
			source = path
			break
		}
	}
	f, e := os.Open(source)
	if e != nil {
		t.Fatal(e)
	}
	defer f.Close()
	if e = os.MkdirAll(filepath.Join(h.root, "downloads"), 0700); e != nil {
		t.Fatal(e)
	}
	dest, e := os.Create(filepath.Join(h.root, "downloads", pkg.SHA256))
	if e != nil {
		t.Fatal(e)
	}
	if _, e = io.Copy(dest, f); e != nil {
		dest.Close()
		t.Fatal(e)
	}
	dest.Close()
	h.click("Software")
	h.click("Software redis")
	h.click("Review package")
	h.click("Download and install")
	if _, ok := h.a.dev.state.Packages[pkg.ID()]; !ok {
		t.Fatal("UI did not install software", h.a.dev.output)
	}
	h.click("Databases")
	h.click("New database instance")
	h.fill("Instance name", "ui-redis")
	h.choose("Database engine", "redis")
	h.fill("Local port", fmt.Sprint(freeUIPort(t)))
	h.click("Persist database contents")
	h.click("Create instance")
	if _, ok := h.a.dev.state.Instances["ui-redis"]; !ok {
		t.Fatal("UI did not create database", h.a.dev.output)
	}
	manager := devplatform.New(h.root)
	defer manager.InstanceAction(context.Background(), "ui-redis", "remove")
	h.click("Start instance")
	if strings.Contains(h.a.dev.output, "Error:") {
		t.Fatal(h.a.dev.output)
	}
	h.click("Restart instance")
	if strings.Contains(h.a.dev.output, "Error:") {
		t.Fatal(h.a.dev.output)
	}
	h.click("Copy connection URI")
	if len(h.copied) != 1 || !strings.HasPrefix(h.copied[0], "redis://default@127.0.0.1:") {
		t.Fatal("UI did not copy a usable connection URI")
	}
	h.click("Copy database password")
	password, e := os.ReadFile(filepath.Join(h.root, "instances", "ui-redis", "password"))
	if e != nil || len(h.copied) != 2 || h.copied[1] != string(password) || strings.Contains(h.a.dev.output, string(password)) {
		t.Fatal("password copy failed or exposed the password in task output", e)
	}
	h.click("Open external database client")
	if len(h.opened) != 1 || h.opened[0] != h.copied[0] {
		t.Fatal("external client handoff used an invalid connection URI")
	}
	h.fill("Database number (0–15)", "0")
	h.click("Back up database")
	backup := h.a.dev.output
	if _, e = os.Stat(backup); e != nil {
		t.Fatalf("UI did not write backup: %s", backup)
	}
	h.fill("Database number (0–15)", "1")
	h.fill("Backup file", backup)
	h.click("Restore into new database")
	if strings.Contains(h.a.dev.output, "Error:") {
		t.Fatal(h.a.dev.output)
	}
	h.click("Stop instance")

	// Share through the UI, then import a second working copy and prepare it.
	// Setup copies the exported files exactly; users never edit declarations.
	h.click("Projects")
	originalDir := t.TempDir()
	h.fill("Project folder", originalDir)
	h.click("Import project")
	originalID := h.a.dev.id
	h.click("ui-redis")
	h.click("Save project")
	h.click("Export environment")
	if strings.Contains(h.a.dev.output, "Error:") {
		t.Fatal(h.a.dev.output)
	}
	cloneDir := t.TempDir()
	for _, name := range []string{"devmanager.json", "devmanager.lock.json"} {
		b, err := os.ReadFile(filepath.Join(originalDir, name))
		if err != nil {
			t.Fatal(err)
		}
		if err = os.WriteFile(filepath.Join(cloneDir, name), b, 0600); err != nil {
			t.Fatal(err)
		}
	}
	h.click("Import a project")
	h.fill("Project folder", cloneDir)
	h.click("Import project")
	cloneID := h.a.dev.id
	h.click("Prepare imported environment")
	if strings.Contains(h.a.dev.output, "Error:") {
		t.Fatal(h.a.dev.output)
	}
	clone := h.a.dev.state.Projects[cloneID]
	if len(clone.Instances) != 1 || clone.Instances[0] == "ui-redis" {
		t.Fatal("import shared the original database")
	}
	instance, ok := h.a.dev.state.Instances[clone.Instances[0]]
	if !ok || instance.Port == h.a.dev.state.Instances["ui-redis"].Port {
		t.Fatal("prepared environment did not create a separate database and port")
	}
	pw1, err := os.ReadFile(filepath.Join(h.root, "instances", "ui-redis", "password"))
	if err != nil {
		t.Fatal(err)
	}
	pw2, err := os.ReadFile(filepath.Join(h.root, "instances", instance.ID, "password"))
	if err != nil || string(pw1) == string(pw2) {
		t.Fatal("prepared database did not get new credentials", err)
	}
	h.click("Databases")
	h.click("Instance " + instance.ID)
	h.fill("Database port", fmt.Sprint(freeUIPort(t)))
	h.click("Save database port")
	changedPort := h.a.dev.state.Instances[instance.ID].Port
	if changedPort == instance.Port || strings.Contains(h.a.dev.output, "Error:") {
		t.Fatal("could not change imported instance port", h.a.dev.output)
	}
	// Simulate endpoint configuration saved just before a process interruption.
	configPath := filepath.Join(h.root, "instances", instance.ID, "valkey.conf")
	config, err := os.ReadFile(configPath)
	if err != nil {
		t.Fatal(err)
	}
	damaged := strings.Replace(string(config), fmt.Sprintf("port %d\n", changedPort), fmt.Sprintf("port %d\n", freeUIPort(t)), 1)
	if err = os.WriteFile(configPath, []byte(damaged), 0600); err != nil {
		t.Fatal(err)
	}
	h.click("Repair saved port settings")
	if strings.Contains(h.a.dev.output, "Error:") {
		t.Fatal("GUI could not repair saved endpoint", h.a.dev.output)
	}
	repaired, err := os.ReadFile(configPath)
	if err != nil || string(repaired) != string(config) {
		t.Fatal("GUI endpoint repair changed unrelated settings", err)
	}
	h.click("Projects")
	h.click("Prepare imported environment")
	if h.a.dev.state.Instances[instance.ID].Port != changedPort {
		t.Fatal("prepare undid the local port change")
	}
	if strings.Contains(h.a.dev.output, "Error:") {
		t.Fatal("prepare was not repeatable after changing the database port", h.a.dev.output)
	}
	defer func() {
		_ = manager.RemoveProject(context.Background(), cloneID)
		_ = manager.RemoveProject(context.Background(), originalID)
		_ = manager.InstanceAction(context.Background(), instance.ID, "remove")
	}()
}

func TestUISoftwareCatalogAllSix(t *testing.T) {
	cache := os.Getenv("MSM_UI_E2E_PACKAGE_CACHE")
	if cache == "" {
		t.Skip("set MSM_UI_E2E_PACKAGE_CACHE to verified package cache directories")
	}
	h := newDevUIHarnessSize(t, 900, 520, true)
	t.Log("UI viewport: 900×520, dark theme")
	h.click("Software")
	for _, name := range []string{"php", "node", "mysql", "postgresql", "redis", "mongodb"} {
		t.Run(name, func(t *testing.T) {
			if (name == "php" || name == "postgresql") && os.Getenv("MSM_UI_E2E_HOMEBREW") != "1" {
				t.Skip("set MSM_UI_E2E_HOMEBREW=1 with the required Homebrew keg preinstalled")
			}
			previousT := h.t
			h.t = t
			defer func() { h.t = previousT }()
			version := devplatform.AvailableVersions(name)[0]
			pkg, e := devplatform.Resolve(context.Background(), name, version)
			if e != nil {
				t.Fatal(e)
			}
			seedRuntimeCache(t, h.root, pkg)
			h.click("Software " + name)
			h.click("Review package")
			if pkg.Formula != "" {
				h.click("Install with Homebrew")
			} else {
				h.click("Download and install")
			}
			if _, ok := h.a.dev.state.Packages[pkg.ID()]; !ok {
				t.Fatal("software was not installed through the UI", h.a.dev.output)
			}
			if name != "php" && name != "node" {
				exerciseUIDatabase(t, h, pkg)
				h.click("Software")
				h.click("Software " + name)
			}
			if pkg.Formula != "" {
				h.click("Remove from workspace " + name + " " + pkg.Version)
			} else {
				h.click("Uninstall " + name + " " + pkg.Version)
			}
			if _, ok := h.a.dev.state.Packages[pkg.ID()]; ok {
				t.Fatal("software was not uninstalled through the UI", h.a.dev.output)
			}
		})
	}
}

func exerciseUIDatabase(t *testing.T, h *devUIHarness, pkg devplatform.Package) {
	t.Helper()
	id := "e2e-" + pkg.Name
	manager := devplatform.New(h.root)
	t.Cleanup(func() { _ = manager.InstanceAction(context.Background(), id, "remove") })
	h.click("Databases")
	h.click("New database instance")
	h.fill("Instance name", id)
	h.choose("Database engine", pkg.Name)
	h.choose("Database version", softwareTitle(pkg.Name)+" · "+pkg.Version)
	h.fill("Local port", fmt.Sprint(freeUIPort(t)))
	h.click("Create instance")
	if _, ok := h.a.dev.state.Instances[id]; !ok {
		t.Fatal("UI did not initialize database", h.a.dev.output)
	}
	h.click("Start instance")
	if strings.Contains(h.a.dev.output, "Error:") {
		t.Fatal(h.a.dev.output)
	}
	h.click("Show connection details")
	if !strings.Contains(h.a.dev.output, "127.0.0.1") {
		t.Fatal("connection details are missing", h.a.dev.output)
	}
	source, target := "source_db", "restored_db"
	field := "Database name"
	if pkg.Name == "redis" {
		source, target, field = "0", "1", "Database number (0–15)"
	}
	h.fill(field, source)
	if pkg.Name != "redis" {
		h.click("Create database")
		if strings.Contains(h.a.dev.output, "Error:") {
			t.Fatal(h.a.dev.output)
		}
		h.fill("Database user", "e2e_user")
		h.fill("New user password", "ui-e2e-temporary-password")
		h.click("Create database user")
		if strings.Contains(h.a.dev.output, "Error:") {
			t.Fatal(h.a.dev.output)
		}
	}
	i := h.a.dev.state.Instances[id]
	pw, e := os.ReadFile(filepath.Join(h.root, "instances", id, "password"))
	if e != nil {
		t.Fatal(e)
	}
	command := func(binary string, args ...string) *exec.Cmd {
		return exec.Command(filepath.Join(manager.PackageDirectory(pkg), pkg.Binaries[binary]), args...)
	}
	run := func(c *exec.Cmd) string {
		t.Helper()
		b, e := c.CombinedOutput()
		if e != nil {
			t.Fatalf("database fixture failed: %v %s", e, b)
		}
		return strings.TrimSpace(string(b))
	}
	var query func(string) string
	switch pkg.Name {
	case "postgresql":
		sql := func(db, sql string) string {
			c := command("psql", "-h", "127.0.0.1", "-p", fmt.Sprint(i.Port), "-U", "devmanager", "-d", db, "-Atc", sql)
			c.Env = append(os.Environ(), "PGPASSWORD="+string(pw))
			return run(c)
		}
		writer := command("psql", "-h", "127.0.0.1", "-p", fmt.Sprint(i.Port), "-U", "e2e_user", "-d", source, "--set", "ON_ERROR_STOP=1", "-Atc", "CREATE TABLE sample(n int); INSERT INTO sample VALUES(42)")
		writer.Env = append(os.Environ(), "PGPASSWORD=ui-e2e-temporary-password")
		run(writer)
		query = func(db string) string { return sql(db, "SELECT n FROM sample") }
	case "mysql":
		sql := func(sql string) string {
			c := command("mysql", "--defaults-file="+filepath.Join(h.root, "instances", id, "client.cnf"), "--batch", "--skip-column-names")
			c.Stdin = strings.NewReader(sql)
			return run(c)
		}
		writer := command("mysql", "--no-defaults", "--host=127.0.0.1", "--port="+fmt.Sprint(i.Port), "--user=e2e_user", source)
		writer.Env = append(os.Environ(), "MYSQL_PWD=ui-e2e-temporary-password")
		writer.Stdin = strings.NewReader("CREATE TABLE sample(n int); INSERT INTO sample VALUES(42)")
		run(writer)
		query = func(db string) string { return sql("SELECT n FROM " + db + ".sample") }
	case "redis":
		redis := func(db string, args ...string) string {
			c := command("redis-cli", append([]string{"-h", "127.0.0.1", "-p", fmt.Sprint(i.Port), "-n", db}, args...)...)
			c.Env = append(os.Environ(), "REDISCLI_AUTH="+string(pw))
			return run(c)
		}
		redis(source, "SET", "sample", "42")
		query = func(db string) string { return redis(db, "GET", "sample") }
	case "mongodb":
		disconnect := func(client *mongo.Client) {
			ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
			defer cancel()
			_ = client.Disconnect(ctx)
		}
		ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
		defer cancel()
		writer, e := mongo.Connect(options.Client().ApplyURI(fmt.Sprintf("mongodb://127.0.0.1:%d/?directConnection=true", i.Port)).SetAuth(options.Credential{Username: "e2e_user", Password: "ui-e2e-temporary-password", AuthSource: source}))
		if e != nil {
			t.Fatal(e)
		}
		defer disconnect(writer)
		if _, e = writer.Database(source).Collection("sample").InsertOne(ctx, bson.D{{Key: "n", Value: 42}}); e != nil {
			t.Fatal(e)
		}
		query = func(db string) string {
			queryCtx, queryCancel := context.WithTimeout(context.Background(), 5*time.Second)
			defer queryCancel()
			client, err := mongo.Connect(options.Client().ApplyURI(fmt.Sprintf("mongodb://127.0.0.1:%d/?directConnection=true", i.Port)).SetAuth(options.Credential{Username: "devmanager", Password: string(pw), AuthSource: "admin"}).SetServerSelectionTimeout(3 * time.Second))
			if err != nil {
				t.Fatal(err)
			}
			defer disconnect(client)
			var row struct {
				N int `bson:"n"`
			}
			if e := client.Database(db).Collection("sample").FindOne(queryCtx, bson.D{}).Decode(&row); e != nil {
				t.Fatal(e)
			}
			return fmt.Sprint(row.N)
		}
	}
	h.click("Back up database")
	backup := h.a.dev.output
	if _, e = os.Stat(backup); e != nil {
		t.Fatal("UI backup failed", h.a.dev.output)
	}
	h.fill(field, target)
	h.fill("Backup file", backup)
	h.click("Restore into new database")
	if strings.Contains(h.a.dev.output, "Error:") {
		t.Fatal(h.a.dev.output)
	}
	if value := query(target); value != "42" {
		t.Fatal("UI restore lost database contents", value)
	}
	// A running database cannot be retargeted. Once stopped, an external
	// listener must be identified without being stopped or mistaken for readiness.
	oldPort := i.Port
	h.fill("Database port", fmt.Sprint(freeUIPort(t)))
	h.click("Save database port")
	if h.a.dev.state.Instances[id].Port != oldPort || !strings.Contains(h.a.dev.output, "stop the database") {
		t.Fatal("running instance changed its port", h.a.dev.output)
	}
	h.click("Stop instance")
	occupied, err := net.Listen("tcp4", fmt.Sprintf("127.0.0.1:%d", oldPort))
	if err != nil {
		t.Fatal(err)
	}
	defer occupied.Close()
	h.fill("Database port", fmt.Sprint(oldPort))
	h.click("Start instance")
	if check, ok := h.a.dev.portChecks[oldPort]; !ok || check.Status != "In use" {
		t.Fatal("startup conflict did not produce a port inspection", h.a.dev.output)
	}
	h.click("Check database port")
	if !h.ui.HasText(fmt.Sprintf("PID %d", os.Getpid())) {
		t.Fatal("UI did not display the actual listener process", h.ui.Texts())
	}
	h.click("Find available database port")
	h.click("Save database port")
	i = h.a.dev.state.Instances[id]
	if i.Port == oldPort || strings.Contains(h.a.dev.output, "Error:") {
		t.Fatal("could not save new port", h.a.dev.output)
	}
	currentPassword, err := os.ReadFile(filepath.Join(h.root, "instances", id, "password"))
	if err != nil || string(currentPassword) != string(pw) {
		t.Fatal("port change replaced database credentials", err)
	}
	h.click("Start instance")
	if strings.Contains(h.a.dev.output, "Error:") {
		t.Fatal("new endpoint failed", h.a.dev.output)
	}
	if query(source) != "42" || query(target) != "42" {
		t.Fatal("database port change lost source or restored contents")
	}
	conn, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", oldPort), time.Second)
	if err != nil {
		t.Fatal("port repair stopped the external listener", err)
	}
	conn.Close()
	h.click("Stop instance")
	h.click("Remove instance (keep data files)")
	if _, ok := h.a.dev.state.Instances[id]; ok {
		t.Fatal("UI removal failed", h.a.dev.output)
	}
	if _, e = os.Stat(filepath.Join(h.root, "instances", id)); e != nil {
		t.Fatal("UI removal deleted database files", e)
	}
	deleteID := "delete-" + pkg.Name
	h.click("New database instance")
	h.fill("Instance name", deleteID)
	h.choose("Database engine", pkg.Name)
	h.choose("Database version", softwareTitle(pkg.Name)+" · "+pkg.Version)
	h.fill("Local port", fmt.Sprint(freeUIPort(t)))
	h.click("Create instance")
	t.Cleanup(func() { _ = manager.InstanceAction(context.Background(), deleteID, "remove") })
	h.click("Start instance")
	h.click("Stop instance")
	h.click("Review permanent data deletion")
	if preview := h.a.dev.instanceDeletionPreview; preview == nil || !preview.CanDelete {
		t.Fatal("stopped database did not produce a deletable preview", h.a.dev.output)
	}
	h.fill("Confirm instance ID", deleteID)
	h.click("Permanently delete instance data")
	if _, ok := h.a.dev.state.Instances[deleteID]; ok {
		t.Fatal("UI data deletion retained instance record", h.a.dev.output)
	}
	if _, err := os.Stat(filepath.Join(h.root, "instances", deleteID)); !os.IsNotExist(err) {
		t.Fatal("UI data deletion retained files", err)
	}
	for _, preserved := range []string{filepath.Join(h.root, "instances", id), backup} {
		if _, err := os.Stat(preserved); err != nil {
			t.Fatal("deletion changed retained data or backup", preserved, err)
		}
	}
}

func (h *devUIHarness) waitIdle() {
	h.t.Helper()
	deadline := time.NewTimer(2 * time.Minute)
	defer deadline.Stop()
	for h.a.dev.busy || (!h.foregroundOnly && (h.a.backgroundTasksRunning() || h.a.dev.packageNeedsRefresh || h.a.dev.databaseNeedsRefresh || h.a.dev.projectNeedsRefresh)) || h.a.dev.statusPolling || h.a.quit.loading {
		select {
		case f := <-h.posted:
			f()
			h.ui.Frame()
		case <-deadline.C:
			h.t.Fatal("UI operation did not complete", h.a.dev.output)
		}
	}
}

func TestUIBusyCancellationAndDraftIsolation(t *testing.T) {
	h := newDevUIHarness(t)
	m := devplatform.New(h.root)
	p, e := m.Import(context.Background(), t.TempDir())
	if e != nil {
		t.Fatal(e)
	}
	p.Services = []devplatform.Service{{ID: "web", Command: []string{"/usr/bin/true"}}}
	if e = m.SaveProject(context.Background(), p); e != nil {
		t.Fatal(e)
	}
	h.click("Refresh")
	h.click("Project " + p.Name)
	h.fill("Project name", "Unsaved draft")
	started := make(chan struct{}, 1)
	h.a.dev.managerFactory = func() *devplatform.Manager {
		m := devplatform.New(h.root)
		m.Run = func(ctx context.Context, name string, args ...string) (string, error) {
			started <- struct{}{}
			<-ctx.Done()
			return "", ctx.Err()
		}
		return m
	}
	if e = h.ui.Click("Refresh"); e != nil {
		t.Fatal(e)
	}
	h.ui.Frame()
	select {
	case <-started:
	case <-time.After(3 * time.Second):
		t.Fatal("background operation did not start")
	}
	if !h.a.dev.busy {
		t.Fatal("UI did not expose operation in progress")
	}
	if e = h.ui.Click("Save project"); e != nil {
		t.Fatal(e)
	}
	h.ui.Frame()
	if e = h.ui.Click("Cancel operation"); e != nil {
		t.Fatal(e)
	}
	h.ui.Frame()
	h.waitIdle()
	state, e := m.Read()
	if e != nil || state.Projects[p.ID].Name != p.Name || h.a.dev.draft.Name != "Unsaved draft" {
		t.Fatal("refresh or disabled save lost draft isolation", e)
	}
}

func TestUIDevelopmentRefreshDoesNotReadLoginItems(t *testing.T) {
	h := newDevUIHarness(t)
	login := &fake{src: service.LoginItem}
	h.a.providers = map[service.Source]service.Provider{service.LoginItem: login}
	h.a.sources[service.LoginItem] = &sourceState{}
	h.a.refreshNavigation()
	h.waitIdle()
	h.ui.Key(ui.Cmd, ui.KeyR)
	h.ui.Frame()
	h.waitIdle()
	if h.a.sources[service.LoginItem].loaded || h.a.sources[service.LoginItem].loading {
		t.Fatal("Development refresh read legacy login items")
	}
}

func TestUISiteCanMoveRenameAndDelete(t *testing.T) {
	h := newDevUIHarness(t)
	h.fill("Project folder", t.TempDir())
	h.click("Import project")
	h.fill("Project name", "Site owner")
	h.click("Save project")
	project := h.a.dev.id
	h.click("HTTPS")
	h.fill("Site name", "web")
	h.fill("Local domain", "move.test")
	h.fill("Upstream address", "http://127.0.0.1:3000")
	h.fill("HTTPS port", fmt.Sprint(freeUIPort(t)))
	h.click("Save HTTPS site")
	if len(h.a.dev.state.Sites) != 1 {
		t.Fatal("site was not created", h.a.dev.output)
	}
	h.choose("Project", "Site owner")
	h.fill("Site name", "renamed")
	h.click("Save HTTPS site")
	p := h.a.dev.state.Projects[project]
	if len(h.a.dev.state.Sites) != 0 || len(p.Sites) != 1 || p.Sites[0].ID != "renamed" {
		t.Fatal("site move left an old or duplicate rule", h.a.dev.output)
	}
	h.click("Delete HTTPS site")
	if len(h.a.dev.state.Projects[project].Sites) != 0 || h.a.dev.siteOriginalID != "" {
		t.Fatal("site was not deleted", h.a.dev.output)
	}
}

func TestUIServiceEditsSaveOnceAndRename(t *testing.T) {
	h := newDevUIHarness(t)
	h.fill("Project folder", t.TempDir())
	h.click("Import project")
	project := h.a.dev.id
	h.click("Services")
	h.click("Add service")
	h.fill("Service name", "web")
	h.fill("Program and arguments", "/usr/bin/true")
	previous := h.a.dev.output
	h.click("Start project")
	if h.a.dev.output != previous {
		t.Fatal("start ignored unsaved environment changes")
	}
	h.click("Save project")
	h.fill("Service name", "worker")
	h.click("Save project")
	h.click("Environment")
	h.fill("Environment variables", "GREETING=hello world")
	h.click("Save project")
	p := h.a.dev.state.Projects[project]
	if len(p.Services) != 1 || p.Services[0].ID != "worker" || p.Services[0].Environment["GREETING"] != "hello world" {
		t.Fatal("saving or renaming the service lost fields", h.a.dev.output)
	}
	h.click("Import a project")
	h.fill("Project folder", t.TempDir())
	h.click("Import project")
	h.click("Environment")
	if h.a.dev.environment != "" || h.a.dev.arguments != "" {
		t.Fatal("previous project environment leaked into an empty project form")
	}
}

func TestUIProxySettingsPersistAndValidate(t *testing.T) {
	h := newDevUIHarness(t)
	h.click("HTTPS")
	h.fill("Site name", "configured")
	h.fill("Local domain", "configured.test")
	h.fill("Upstream address", "http://127.0.0.1:3000")
	h.fill("HTTPS port", fmt.Sprint(freeUIPort(t)))
	h.click("Customize proxy settings")
	h.fill("Connection timeout (seconds)", "17")
	h.fill("Response header timeout (seconds)", "42")
	h.choose("Upstream Host header", "Use upstream host")
	h.click("Save HTTPS site")
	site, ok := h.a.dev.state.Sites["configured"]
	if !ok || site.Proxy == nil || site.Proxy.HostPolicy != "upstream" || site.Proxy.ConnectTimeoutSeconds != 17 || site.Proxy.ResponseHeaderTimeoutSeconds != 42 {
		t.Fatalf("proxy settings did not persist: %+v; fields %q / %q / %q; %s", site.Proxy, h.a.dev.siteConnectTimeout, h.a.dev.siteHeaderTimeout, h.a.dev.siteHostPolicy, h.a.dev.output)
	}
	h.fill("Response header timeout (seconds)", "-1")
	h.click("Save HTTPS site")
	if !strings.Contains(h.a.dev.output, "0–3600") || h.a.dev.state.Sites["configured"].Proxy.ResponseHeaderTimeoutSeconds != 42 {
		t.Fatal("invalid timeout replaced the saved configuration", h.a.dev.output)
	}
	h.click("Site configured.test")
	if h.a.dev.siteHeaderTimeout != "42" || !h.a.dev.siteAdvanced {
		t.Fatal("reselecting the site lost proxy settings")
	}
	h.click("Customize proxy settings")
	h.click("Save HTTPS site")
	if h.a.dev.state.Sites["configured"].Proxy != nil {
		t.Fatal("site did not return to the default proxy settings")
	}
}

func TestUIStorageReadFailureRetainsTheCurrentProjectAndDraft(t *testing.T) {
	h := newDevUIHarness(t)
	h.fill("Project folder", t.TempDir())
	h.click("Import project")
	id := h.a.dev.id
	h.fill("Project name", "Unsaved project name")
	path := filepath.Join(h.root, "state.json")
	previous, e := os.ReadFile(path)
	if e != nil {
		t.Fatal(e)
	}
	t.Cleanup(func() { _ = os.WriteFile(path, previous, 0600) })
	if e = os.WriteFile(path, []byte("{damaged during read"), 0600); e != nil {
		t.Fatal(e)
	}
	h.click("Refresh")
	if _, ok := h.a.dev.state.Projects[id]; !ok || h.a.dev.id != id || h.a.dev.draft.Name != "Unsaved project name" || !strings.Contains(h.a.dev.output, "Could not read saved settings") {
		t.Fatal("failed storage read discarded known settings or the draft", h.a.dev.output)
	}
}

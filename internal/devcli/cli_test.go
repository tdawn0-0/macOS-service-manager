package devcli

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"testing"

	"devbase/internal/devplatform"
	"devbase/internal/privhelper"
)

func setupIsolatedEnv(t *testing.T) (string, *Env, *[]string) {
	t.Helper()
	home := t.TempDir()
	root := filepath.Join(home, "devbase-data")
	t.Setenv("HOME", home)
	t.Setenv("MSM_DEV_HOME", root)

	var mu sync.Mutex
	var commands []string
	recordCmd := func(cmd string) {
		mu.Lock()
		commands = append(commands, cmd)
		mu.Unlock()
	}
	env := DefaultEnv(nil, nil, nil)
	env.AppPath = ""
	env.LoginProgram = filepath.Join(home, "Devbase.app", "Contents", "MacOS", "Devbase")
	env.LoginAgentDir = filepath.Join(home, "login-agents")
	env.UserAgentDir = filepath.Join(home, "user-agents")
	env.CLIBinDir = filepath.Join(home, "bin")
	env.LoginRunner = func(args ...string) error {
		recordCmd("launchctl " + strings.Join(args, " "))
		return nil
	}
	env.HelperClient = func() *privhelper.Client {
		return privhelper.Default()
	}
	env.ManagerFactory = func() *devplatform.Manager {
		m := devplatform.New(root)
		m.Home = home
		m.LoginDir = filepath.Join(home, "login-agents")
		m.SocketJobDir = filepath.Join(home, "system-daemons")
		m.Run = func(_ context.Context, name string, args ...string) (string, error) {
			recordCmd(name + " " + strings.Join(args, " "))
			if name == "launchctl" && len(args) > 0 && args[0] == "print" {
				return "", errors.New("Could not find service")
			}
			return "", nil
		}
		return m
	}
	return root, env, &commands
}

func writeTestState(t *testing.T, root string, st devplatform.State) {
	t.Helper()
	st.SchemaVersion = devplatform.Schema
	if st.Packages == nil {
		st.Packages = map[string]devplatform.Package{}
	}
	if st.Instances == nil {
		st.Instances = map[string]devplatform.Instance{}
	}
	if st.ConnectionProfiles == nil {
		st.ConnectionProfiles = map[string]devplatform.ConnectionProfile{}
	}
	if st.ProxyRules == nil {
		st.ProxyRules = []devplatform.ProxyRule{}
	}
	if st.ProxyListenerPort == 0 {
		st.ProxyListenerPort = 443
	}
	if err := os.MkdirAll(root, 0o700); err != nil {
		t.Fatal(err)
	}
	data, err := json.MarshalIndent(st, "", "  ")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "state.json"), data, 0o600); err != nil {
		t.Fatal(err)
	}
}

func runCLIWithTestEnv(env *Env, in string, args ...string) (int, string, string) {
	var outBuf, errBuf bytes.Buffer
	env.In = strings.NewReader(in)
	env.Out = &outBuf
	env.Err = &errBuf
	env.JSON = wantsJSONFlag(args)
	err := RunCLIWithEnv(context.Background(), args, env)
	if err == nil {
		return 0, outBuf.String(), errBuf.String()
	}
	writeActionError(env, err)
	var ec interface{ ExitCode() int }
	if errors.As(err, &ec) {
		return ec.ExitCode(), outBuf.String(), errBuf.String()
	}
	return 1, outBuf.String(), errBuf.String()
}

func TestRunCLISchemaVersionAndHelp(t *testing.T) {
	_, env, _ := setupIsolatedEnv(t)

	code, out, errOut := runCLIWithTestEnv(env, "", "schema")
	if code != 0 || errOut != "" {
		t.Fatalf("schema failed: code=%d err=%q", code, errOut)
	}
	var schema CLISchema
	if err := json.Unmarshal([]byte(out), &schema); err != nil {
		t.Fatalf("invalid schema JSON: %v\n%s", err, out)
	}
	if schema.Name != "devbase" || schema.Version != CLIVersion || len(schema.Commands) < 15 {
		t.Fatalf("unexpected schema summary: %+v", schema)
	}
	names := map[string]bool{}
	for _, c := range schema.Commands {
		names[c.Name] = true
	}
	for _, want := range []string{"status", "diagnostics", "tasks", "logs", "ports", "shutdown", "software", "db", "https", "tunnel", "ca", "services", "settings", "cli", "schema", "version", "help"} {
		if !names[want] {
			t.Errorf("schema missing command %q", want)
		}
	}

	code, filtered, errOut := runCLIWithTestEnv(env, "", "schema", "--command", "db")
	if code != 0 || errOut != "" {
		t.Fatalf("schema --command db failed: code=%d err=%q", code, errOut)
	}
	var filteredSchema CLISchema
	if err := json.Unmarshal([]byte(filtered), &filteredSchema); err != nil || len(filteredSchema.Commands) != 1 || filteredSchema.Commands[0].Name != "db" {
		t.Fatalf("unexpected filtered schema: err=%v out=%s", err, filtered)
	}

	code, verOut, _ := runCLIWithTestEnv(env, "", "version", "--json")
	if code != 0 || !strings.Contains(verOut, `"version"`) || !strings.Contains(verOut, CLIVersion) {
		t.Fatalf("unexpected version output: code=%d out=%s", code, verOut)
	}

	code, helpOut, _ := runCLIWithTestEnv(env, "", "help", "db")
	if code != 0 || !strings.Contains(helpOut, "devbase db") {
		t.Fatalf("unexpected help db output: code=%d out=%s", code, helpOut)
	}

	code, _, errOut = runCLIWithTestEnv(env, "", "does-not-exist", "--json")
	if code != 2 || !strings.Contains(errOut, `"exitCode": 2`) {
		t.Fatalf("unknown command expected exit 2 JSON error, got code=%d err=%s", code, errOut)
	}
}

func TestRunCLISecretFlagRejection(t *testing.T) {
	_, _, _ = setupIsolatedEnv(t)

	cases := [][]string{
		{"db", "create-user", "pg1", "alice", "--password", "plain"},
		{"db", "create-user", "pg1", "alice", "--password=plain"},
		{"tunnel", "configure", "--token", "secret"},
		{"tunnel", "configure", "--token=secret"},
		{"tunnel", "configure", "--api-token", "secret"},
		{"tunnel", "configure", "--api-token=secret"},
		{"--json", "tunnel", "configure", "--api-token=very-secret"},
	}
	for _, argv := range cases {
		var out, errOut bytes.Buffer
		code := RunCLI(context.Background(), argv, strings.NewReader(""), &out, &errOut)
		if code != 2 {
			t.Errorf("%v: expected exit 2, got %d (err=%q)", argv, code, errOut.String())
		}
		if out.Len() != 0 {
			t.Errorf("%v: expected empty stdout, got %q", argv, out.String())
		}
		if slices.Contains(argv, "--json") {
			var parsed map[string]any
			if err := json.Unmarshal(errOut.Bytes(), &parsed); err != nil {
				t.Errorf("%v: expected JSON stderr, got %q (%v)", argv, errOut.String(), err)
			}
		}
	}
}

func TestRunCLIWorkspaceLogsPortsAndShutdown(t *testing.T) {
	root, env, _ := setupIsolatedEnv(t)
	writeTestState(t, root, devplatform.State{
		ProxyListenerPort: 8443,
		ProxyRules:        []devplatform.ProxyRule{{Domain: "app.devbase.localhost", LocalPort: 3000}},
	})
	if err := os.MkdirAll(filepath.Join(root, "logs"), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "logs", "proxy.err.log"), []byte("INFO boot\nERROR failed upstream\n"), 0o600); err != nil {
		t.Fatal(err)
	}

	code, out, errOut := runCLIWithTestEnv(env, "", "status", "--json")
	if code != 0 || errOut != "" {
		t.Fatalf("status failed: %d %s", code, errOut)
	}
	var st map[string]any
	if err := json.Unmarshal([]byte(out), &st); err != nil {
		t.Fatalf("status json: %v", err)
	}
	if _, ok := st["packages"]; !ok {
		t.Fatalf("status missing packages key: %s", out)
	}

	code, logsOut, errOut := runCLIWithTestEnv(env, "", "logs", "proxy", "--filter", "ERROR", "--json")
	if code != 0 || errOut != "" {
		t.Fatalf("logs failed: %d %s", code, errOut)
	}
	var logPayload struct {
		Lines []string `json:"lines"`
	}
	if err := json.Unmarshal([]byte(logsOut), &logPayload); err != nil || !slices.Equal(logPayload.Lines, []string{"ERROR failed upstream"}) {
		t.Fatalf("unexpected logs payload: err=%v out=%s", err, logsOut)
	}

	code, portOut, errOut := runCLIWithTestEnv(env, "", "ports", "free", "--json")
	if code != 0 || errOut != "" || !strings.Contains(portOut, `"port"`) {
		t.Fatalf("ports free failed: %d out=%s err=%s", code, portOut, errOut)
	}

	code, _, _ = runCLIWithTestEnv(env, "", "shutdown", "stop")
	if code != 2 {
		t.Fatalf("shutdown stop without --yes should fail exit 2, got %d", code)
	}
	code, shutOut, errOut := runCLIWithTestEnv(env, "", "shutdown", "stop", "--yes", "--json")
	if code != 0 || errOut != "" || !strings.Contains(shutOut, `"stopped": 0`) {
		t.Fatalf("shutdown stop failed: %d out=%s err=%s", code, shutOut, errOut)
	}
}

func TestRunCLISoftwareCommands(t *testing.T) {
	root, baseEnv, _ := setupIsolatedEnv(t)
	nodePkg := devplatform.Package{
		Name:     "node",
		Version:  "22.14.0",
		Arch:     "arm64",
		URL:      "https://nodejs.org/dist/v22.14.0/node-v22.14.0-darwin-arm64.tar.gz",
		SHA256:   strings.Repeat("a", 64),
		Format:   "tar.gz",
		Binaries: map[string]string{"node": "bin/node"},
		License:  "MIT",
	}
	writeTestState(t, root, devplatform.State{
		Packages: map[string]devplatform.Package{nodePkg.ID(): nodePkg},
	})
	nodeBin := filepath.Join(root, "packages", nodePkg.ID(), "bin", "node")
	if err := os.MkdirAll(filepath.Dir(nodeBin), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(nodeBin, []byte("#!/bin/sh\necho v22.14.0\n"), 0o755); err != nil {
		t.Fatal(err)
	}

	origFactory := baseEnv.ManagerFactory
	baseEnv.ManagerFactory = func() *devplatform.Manager {
		m := origFactory()
		m.FetchNodeReleases = func(context.Context) ([]devplatform.NodeRelease, error) {
			return []devplatform.NodeRelease{{Version: "22.14.0", LTS: "Jod"}}, nil
		}
		return m
	}

	code, versOut, errOut := runCLIWithTestEnv(baseEnv, "", "software", "versions", "node", "--json")
	if code != 0 || errOut != "" || !strings.Contains(versOut, `"22.14.0"`) {
		t.Fatalf("software versions node failed: %d out=%s err=%s", code, versOut, errOut)
	}

	code, revOut, errOut := runCLIWithTestEnv(baseEnv, "", "software", "review", "mysql", "8.4.4", "--json")
	if code != 0 || errOut != "" || !strings.Contains(revOut, `"Compatible"`) {
		t.Fatalf("software review mysql failed: %d out=%s err=%s", code, revOut, errOut)
	}

	code, defOut, errOut := runCLIWithTestEnv(baseEnv, "", "software", "default", "node", "22.14.0", "--json")
	if code != 0 || errOut != "" || !strings.Contains(defOut, nodePkg.ID()) {
		t.Fatalf("software default node failed: %d out=%s err=%s", code, defOut, errOut)
	}

	projectDir := t.TempDir()
	if err := os.WriteFile(filepath.Join(projectDir, ".nvmrc"), []byte("22.14.0\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	code, whichOut, errOut := runCLIWithTestEnv(baseEnv, "", "software", "which", "node", "--dir", projectDir, "--json")
	if code != 0 || errOut != "" || !strings.Contains(whichOut, nodeBin) {
		t.Fatalf("software which node failed: %d out=%s err=%s", code, whichOut, errOut)
	}

	code, shOut, errOut := runCLIWithTestEnv(baseEnv, "", "software", "shell", "node", "enable", "--json")
	if code != 0 || errOut != "" || !strings.Contains(shOut, `"enabled": true`) {
		t.Fatalf("software shell node enable failed: %d out=%s err=%s", code, shOut, errOut)
	}

	code, _, _ = runCLIWithTestEnv(baseEnv, "", "software", "uninstall", nodePkg.ID())
	if code != 2 {
		t.Fatalf("uninstall without --yes expected exit 2, got %d", code)
	}
}

func TestRunCLIDatabasesSafetyConnectionsProfilesAndDeleteData(t *testing.T) {
	root, baseEnv, _ := setupIsolatedEnv(t)
	pgPkg := devplatform.Package{
		Name:     "postgresql",
		Version:  "17.2",
		Arch:     "arm64",
		URL:      "https://ghcr.io/v2/homebrew/core/postgresql/17/manifests/17.2",
		SHA256:   strings.Repeat("b", 64),
		Format:   "homebrew",
		Formula:  "postgresql@17",
		Binaries: map[string]string{"postgres": "bin/postgres", "psql": "bin/psql"},
		License:  "PostgreSQL",
	}
	extDir := t.TempDir()
	managedInst := devplatform.Instance{ID: "pg-managed", Kind: "postgresql", Package: pgPkg.ID(), Port: 5432, Persistent: true}
	externalInst := devplatform.Instance{ID: "pg-ext", Kind: "postgresql", Package: pgPkg.ID(), Port: 5433, DataDirectory: extDir}
	writeTestState(t, root, devplatform.State{
		Packages:  map[string]devplatform.Package{pgPkg.ID(): pgPkg},
		Instances: map[string]devplatform.Instance{managedInst.ID: managedInst, externalInst.ID: externalInst},
	})
	if err := os.MkdirAll(filepath.Join(root, "instances", managedInst.ID, "data"), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "instances", managedInst.ID, "data", "PG_VERSION"), []byte("17\n"), 0o600); err != nil {
		t.Fatal(err)
	}

	origFactory := baseEnv.ManagerFactory
	baseEnv.ManagerFactory = func() *devplatform.Manager {
		m := origFactory()
		m.InspectCatalog = func(_ context.Context, inst devplatform.Instance) (devplatform.InstanceCatalog, error) {
			return devplatform.InstanceCatalog{Databases: []string{"app"}, Users: []string{"devbase"}}, nil
		}
		return m
	}

	code, inspOut, errOut := runCLIWithTestEnv(baseEnv, "", "db", "inspect", "pg-managed", "--json")
	if code != 0 || errOut != "" || !strings.Contains(inspOut, `"app"`) {
		t.Fatalf("db inspect failed: %d out=%s err=%s", code, inspOut, errOut)
	}

	code, connOut, errOut := runCLIWithTestEnv(baseEnv, "", "db", "connection", "pg-managed", "--database", "app", "--json")
	if code != 0 || errOut != "" || !strings.Contains(connOut, `"jdbc:postgresql://`) || !strings.Contains(connOut, `"DATABASE_URL=`) {
		t.Fatalf("db connection failed: %d out=%s err=%s", code, connOut, errOut)
	}

	code, profOut, errOut := runCLIWithTestEnv(baseEnv, "", "db", "profile", "save", "--instance", "pg-managed", "--name", "App DB", "--database", "app", "--username", "devbase", "--json")
	if code != 0 || errOut != "" {
		t.Fatalf("db profile save failed: %d out=%s err=%s", code, profOut, errOut)
	}
	var profResp struct {
		Profile devplatform.ConnectionProfile `json:"profile"`
	}
	if err := json.Unmarshal([]byte(profOut), &profResp); err != nil || profResp.Profile.ID == "" {
		t.Fatalf("unexpected profile save JSON: %v %s", err, profOut)
	}

	code, _, _ = runCLIWithTestEnv(baseEnv, "", "db", "profile", "delete", "pg-managed", profResp.Profile.ID)
	if code != 2 {
		t.Fatalf("profile delete without --yes expected exit 2, got %d", code)
	}
	code, _, errOut = runCLIWithTestEnv(baseEnv, "", "db", "profile", "delete", "pg-managed", profResp.Profile.ID, "--yes", "--json")
	if code != 0 || errOut != "" {
		t.Fatalf("profile delete --yes failed: %d %s", code, errOut)
	}

	code, _, _ = runCLIWithTestEnv(baseEnv, "", "db", "create-user", "pg-managed", "alice")
	if code != 2 {
		t.Fatalf("create-user without --password-stdin expected exit 2, got %d", code)
	}
	code, _, _ = runCLIWithTestEnv(baseEnv, "   \n", "db", "create-user", "pg-managed", "alice", "--password-stdin")
	if code != 2 {
		t.Fatalf("create-user with empty stdin expected exit 2, got %d", code)
	}

	// External instance deletion is always blocked, while removing its config keeps external files intact.
	code, extPrevOut, errOut := runCLIWithTestEnv(baseEnv, "", "db", "delete-data", "pg-ext", "--preview", "--json")
	if code != 0 || errOut != "" {
		t.Fatalf("delete-data preview failed: %d %s", code, errOut)
	}
	var extPreview devplatform.InstanceDeletionPreview
	if err := json.Unmarshal([]byte(extPrevOut), &extPreview); err != nil || extPreview.CanDelete {
		t.Fatalf("expected CanDelete=false for external data, got err=%v %+v", err, extPreview)
	}
	code, _, errOut = runCLIWithTestEnv(baseEnv, "", "db", "remove", "pg-ext", "--yes", "--json")
	if code != 0 || errOut != "" {
		t.Fatalf("db remove --yes failed: %d %s", code, errOut)
	}
	if _, err := os.Stat(extDir); err != nil {
		t.Fatalf("expected external data directory retained after db remove, got %v", err)
	}

	// delete-data --preview -> --confirm + --fingerprint erases app-managed instance and its data.
	code, prevOut, errOut := runCLIWithTestEnv(baseEnv, "", "db", "delete-data", "pg-managed", "--preview", "--json")
	if code != 0 || errOut != "" {
		t.Fatalf("delete-data preview managed failed: %d %s", code, errOut)
	}
	var preview devplatform.InstanceDeletionPreview
	if err := json.Unmarshal([]byte(prevOut), &preview); err != nil || !preview.CanDelete || preview.Fingerprint == "" {
		t.Fatalf("expected CanDelete=true and non-empty fingerprint, got %v %+v", err, preview)
	}
	code, _, errOut = runCLIWithTestEnv(baseEnv, "", "db", "delete-data", "pg-managed", "--confirm", "pg-managed", "--fingerprint", preview.Fingerprint, "--json")
	if code != 0 || errOut != "" {
		t.Fatalf("delete-data confirm failed: %d %s", code, errOut)
	}
	if _, err := os.Stat(filepath.Join(root, "instances", "pg-managed", "data")); !os.IsNotExist(err) {
		t.Fatalf("expected app-managed data deleted, stat err=%v", err)
	}
}

func TestRunCLIHTTPSTunnelAndCA(t *testing.T) {
	_, env, _ := setupIsolatedEnv(t)

	code, addOut, errOut := runCLIWithTestEnv(env, "", "https", "add", "api", "3000", "--tunnel", "--tunnel-auth", "--json")
	if code != 0 || errOut != "" || !strings.Contains(addOut, `"api.devbase.localhost"`) {
		t.Fatalf("https add failed: %d out=%s err=%s", code, addOut, errOut)
	}

	code, _, errOut = runCLIWithTestEnv(env, "", "https", "set-port", "8443", "--json")
	if code != 0 || errOut != "" {
		t.Fatalf("https set-port failed: %d %s", code, errOut)
	}

	code, repOut, errOut := runCLIWithTestEnv(env, `[{"domain":"web","localPort":4000,"tunnel":true}]`, "https", "replace", "--json-stdin", "--json")
	if code != 0 || errOut != "" || !strings.Contains(repOut, `"web.devbase.localhost"`) {
		t.Fatalf("https replace failed: %d out=%s err=%s", code, repOut, errOut)
	}

	secretToken := "cf-super-secret-token-value"
	code, cfgOut, errOut := runCLIWithTestEnv(env, secretToken+"\n", "tunnel", "configure", "--account-id", "acct-123", "--token-stdin", "--json")
	if code != 0 || errOut != "" || !strings.Contains(cfgOut, `"hasToken": true`) {
		t.Fatalf("tunnel configure failed: %d out=%s err=%s", code, cfgOut, errOut)
	}
	if strings.Contains(cfgOut, secretToken) {
		t.Fatal("tunnel configure echoed secret token")
	}

	code, tunOut, errOut := runCLIWithTestEnv(env, "", "tunnel", "status", "--json")
	if code != 0 || errOut != "" || !strings.Contains(tunOut, `"hasToken": true`) || strings.Contains(tunOut, secretToken) {
		t.Fatalf("tunnel status unexpected or leaked token: %d out=%s err=%s", code, tunOut, errOut)
	}

	code, _, _ = runCLIWithTestEnv(env, "", "tunnel", "remove-config")
	if code != 2 {
		t.Fatalf("tunnel remove-config without --yes expected exit 2, got %d", code)
	}
	code, rmOut, errOut := runCLIWithTestEnv(env, "", "tunnel", "remove-config", "--yes", "--json")
	if code != 0 || errOut != "" || !strings.Contains(rmOut, `"removed": true`) {
		t.Fatalf("tunnel remove-config --yes failed: %d out=%s err=%s", code, rmOut, errOut)
	}

	code, _, _ = runCLIWithTestEnv(env, "", "ca", "untrust")
	if code != 2 {
		t.Fatalf("ca untrust without --yes expected exit 2, got %d", code)
	}
	code, caPrep, errOut := runCLIWithTestEnv(env, "", "ca", "prepare", "--json")
	if code != 0 || errOut != "" || !strings.Contains(caPrep, `"Prepared"`) {
		t.Fatalf("ca prepare failed: %d out=%s err=%s", code, caPrep, errOut)
	}
	code, caDisc, errOut := runCLIWithTestEnv(env, "", "ca", "discard-prepared", "--json")
	if code != 0 || errOut != "" || !strings.Contains(caDisc, `"discard-prepared"`) {
		t.Fatalf("ca discard-prepared failed: %d out=%s err=%s", code, caDisc, errOut)
	}
}

func TestRunCLISettingsAndWrapperInstall(t *testing.T) {
	root, env, launchctlCalls := setupIsolatedEnv(t)
	downloadsDir := filepath.Join(root, "downloads")
	if err := os.MkdirAll(downloadsDir, 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(downloadsDir, "dummy.tar.gz"), []byte("123456"), 0o600); err != nil {
		t.Fatal(err)
	}

	code, rotOut, errOut := runCLIWithTestEnv(env, "", "settings", "log-rotation", "on", "--json")
	if code != 0 || errOut != "" || !strings.Contains(rotOut, `"enabled": true`) {
		t.Fatalf("settings log-rotation on failed: %d out=%s err=%s", code, rotOut, errOut)
	}

	code, loginOut, errOut := runCLIWithTestEnv(env, "", "settings", "login-item", "on", "--json")
	if code != 0 || errOut != "" || !strings.Contains(loginOut, `"enabled": true`) {
		t.Fatalf("settings login-item on failed: %d out=%s err=%s", code, loginOut, errOut)
	}
	if len(*launchctlCalls) == 0 {
		t.Fatal("expected launchctl enable call for login-item")
	}

	code, _, _ = runCLIWithTestEnv(env, "", "settings", "cache", "clear")
	if code != 2 {
		t.Fatalf("settings cache clear without --yes expected exit 2, got %d", code)
	}
	code, clearOut, errOut := runCLIWithTestEnv(env, "", "settings", "cache", "clear", "--yes", "--json")
	if code != 0 || errOut != "" || !strings.Contains(clearOut, `"Files": 1`) {
		t.Fatalf("settings cache clear --yes failed: %d out=%s err=%s", code, clearOut, errOut)
	}

	// CLI wrapper install, status, uninstall, and foreign file refusal.
	fakeExe := filepath.Join(t.TempDir(), "fake-devbase")
	if err := os.WriteFile(fakeExe, []byte("#!/bin/sh\nprintf 'forwarded:%s\\n' \"$*\"\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	env.LoginProgram = fakeExe

	code, cliOut, errOut := runCLIWithTestEnv(env, "", "cli", "install", "--json")
	if code != 0 || errOut != "" {
		t.Fatalf("cli install failed: %d out=%s err=%s", code, cliOut, errOut)
	}
	wrapperPath := filepath.Join(env.CLIBinDir, "devbase")
	cmd := exec.Command(wrapperPath, "version", "--json")
	got, err := cmd.CombinedOutput()
	if err != nil || strings.TrimSpace(string(got)) != "forwarded:version --json" {
		t.Fatalf("wrapper execution = %q, %v", string(got), err)
	}

	code, _, errOut = runCLIWithTestEnv(env, "", "cli", "uninstall", "--json")
	if code != 0 || errOut != "" {
		t.Fatalf("cli uninstall failed: %d %s", code, errOut)
	}
	if _, err := os.Stat(wrapperPath); !os.IsNotExist(err) {
		t.Fatalf("expected wrapper removed, stat err=%v", err)
	}

	// Foreign file is never overwritten or deleted.
	if err := os.MkdirAll(env.CLIBinDir, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(wrapperPath, []byte("#!/bin/sh\necho third-party\n"), 0o755); err != nil {
		t.Fatal(err)
	}
	code, _, _ = runCLIWithTestEnv(env, "", "cli", "install")
	if code == 0 {
		t.Fatal("expected cli install to refuse overwriting foreign file")
	}
	code, _, _ = runCLIWithTestEnv(env, "", "cli", "uninstall")
	if code == 0 {
		t.Fatal("expected cli uninstall to refuse removing foreign file")
	}
}

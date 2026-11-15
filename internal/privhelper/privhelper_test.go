//go:build darwin

package privhelper

import (
	"context"
	"encoding/json"
	"errors"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"

	"github.com/ebitengine/purego"
)

func shortSocketDir(t *testing.T) string {
	t.Helper()
	dir, err := os.MkdirTemp("/tmp", "msm-ph-")
	if err != nil {
		t.Fatalf("MkdirTemp: %v", err)
	}
	t.Cleanup(func() {
		_ = os.RemoveAll(dir)
	})
	return dir
}

func startTestHelper(t *testing.T, cfg ServerConfig) (*Client, string, func()) {
	t.Helper()
	sockDir := shortSocketDir(t)
	sockPath := filepath.Join(sockDir, "h.sock")
	ln, err := net.Listen("unix", sockPath)
	if err != nil {
		t.Fatalf("net.Listen unix %s: %v", sockPath, err)
	}
	if ul, ok := ln.(*net.UnixListener); ok {
		ul.SetUnlinkOnClose(false)
	}
	if cfg.OwnerUID == 0 {
		cfg.OwnerUID = os.Geteuid()
	}
	if cfg.ExpectedVersion == "" {
		cfg.ExpectedVersion = "test-ver-1"
	}
	srv := NewServer(cfg)
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() {
		done <- srv.Serve(ctx, ln)
	}()

	stop := func() {
		cancel()
		_ = ln.Close()
		select {
		case <-done:
		case <-time.After(3 * time.Second):
			t.Fatalf("server did not shut down cleanly")
		}
	}
	t.Cleanup(stop)

	client := NewClient(ClientConfig{
		SocketPath: sockPath,
		Expected: BinaryInfo{
			Version:  cfg.ExpectedVersion,
			OwnerUID: cfg.OwnerUID,
		},
	})
	return client, sockPath, stop
}

func TestPeerUIDAuthorization(t *testing.T) {
	dir := t.TempDir()
	hostsFile := filepath.Join(dir, "hosts")
	if err := os.WriteFile(hostsFile, []byte("127.0.0.1 localhost\n"), 0644); err != nil {
		t.Fatal(err)
	}
	wrongUID := os.Geteuid() + 1
	client, _, _ := startTestHelper(t, ServerConfig{
		OwnerUID:  wrongUID,
		HostsPath: hostsFile,
	})
	// Test PeerUID mismatch returns HTTP 403 error message.
	_, err := client.Ping(context.Background())
	if err == nil || !strings.Contains(err.Error(), "not authorized") {
		t.Fatalf("expected peer uid rejection, got %v", err)
	}
}

func TestHostsSyncSuccessAndConflict(t *testing.T) {
	dir := t.TempDir()
	hostsFile := filepath.Join(dir, "hosts")
	orig := "127.0.0.1 localhost\n"
	if err := os.WriteFile(hostsFile, []byte(orig), 0644); err != nil {
		t.Fatal(err)
	}
	client, _, _ := startTestHelper(t, ServerConfig{
		HostsPath: hostsFile,
	})

	next := orig + "127.0.0.1 app.test\n"
	if err := client.SyncHosts(context.Background(), HostsSyncRequest{
		ExpectedOld: orig,
		NewContent:  next,
	}); err != nil {
		t.Fatalf("SyncHosts: %v", err)
	}
	got, err := os.ReadFile(hostsFile)
	if err != nil {
		t.Fatal(err)
	}
	if string(got) != next {
		t.Fatalf("hosts mismatch: got %q, want %q", string(got), next)
	}

	// Stale ExpectedOld should fail with conflict and leave file unchanged.
	err = client.SyncHosts(context.Background(), HostsSyncRequest{
		ExpectedOld: orig,
		NewContent:  orig + "127.0.0.1 stale.test\n",
	})
	if err == nil || !strings.Contains(err.Error(), "changed concurrently") {
		t.Fatalf("expected conflict error, got %v", err)
	}

	// NUL bytes must be rejected.
	err = client.SyncHosts(context.Background(), HostsSyncRequest{
		ExpectedOld: next,
		NewContent:  next + "\x00bad",
	})
	if err == nil || !strings.Contains(err.Error(), "NUL") {
		t.Fatalf("expected NUL rejection, got %v", err)
	}
}

func TestProxySocketInstallStopKickstartAndPrint(t *testing.T) {
	dir := t.TempDir()
	var ran [][]string
	client, _, _ := startTestHelper(t, ServerConfig{
		SocketJobDirs: []string{dir},
		Run: func(_ context.Context, name string, args ...string) ([]byte, error) {
			ran = append(ran, append([]string{name}, args...))
			if len(args) >= 2 && args[0] == "print" {
				return []byte("state = active\n"), nil
			}
			return nil, nil
		},
	})

	jobPath := filepath.Join(dir, "xyz.jyeu.devbase.proxy.plist")
	target := "system/xyz.jyeu.devbase.proxy"
	plistData := []byte(`<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>Label</key><string>xyz.jyeu.devbase.proxy</string></dict></plist>`)

	if err := client.InstallProxySocket(context.Background(), ProxySocketInstallRequest{
		Plist:   plistData,
		JobPath: jobPath,
		Target:  target,
	}); err != nil {
		t.Fatalf("InstallProxySocket: %v", err)
	}
	saved, err := os.ReadFile(jobPath)
	if err != nil || string(saved) != string(plistData) {
		t.Fatalf("saved plist mismatch: %v, %q", err, string(saved))
	}

	if err := client.KickstartProxySocket(context.Background(), target); err != nil {
		t.Fatalf("KickstartProxySocket: %v", err)
	}
	printResp, err := client.PrintProxySocket(context.Background(), target)
	if err != nil {
		t.Fatalf("PrintProxySocket: %v", err)
	}
	if printResp.ExitCode != 0 || !strings.Contains(string(printResp.Output), "active") {
		t.Fatalf("unexpected PrintProxySocket response: %+v", printResp)
	}
	if err := client.StopProxySocket(context.Background(), ProxySocketStopRequest{
		JobPath: jobPath,
		Target:  target,
	}); err != nil {
		t.Fatalf("StopProxySocket: %v", err)
	}
	if _, err := os.Stat(jobPath); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("expected %s removed, stat err=%v", jobPath, err)
	}

	wantCmds := [][]string{
		{"/bin/launchctl", "bootout", target},
		{"/bin/launchctl", "bootstrap", "system", jobPath},
		{"/bin/launchctl", "kickstart", "-k", target},
		{"/bin/launchctl", "print", target},
		{"/bin/launchctl", "print", target},
		{"/bin/launchctl", "bootout", target},
	}
	if len(ran) != len(wantCmds) {
		t.Fatalf("ran %v, want %v", ran, wantCmds)
	}
	for i := range wantCmds {
		if !slices.Equal(ran[i], wantCmds[i]) {
			t.Fatalf("cmd[%d] = %v, want %v", i, ran[i], wantCmds[i])
		}
	}
}

func TestPortInspectPreservesNulDelimitedLsof(t *testing.T) {
	rawLsof := []byte("p4242\x00cnode\x00u501\x00Ldev\x00n127.0.0.1:443\x00")
	client, _, _ := startTestHelper(t, ServerConfig{
		Run: func(_ context.Context, name string, args ...string) ([]byte, error) {
			if name != "/usr/sbin/lsof" {
				t.Fatalf("unexpected binary %q", name)
			}
			return rawLsof, nil
		},
	})

	out, err := client.InspectPort(context.Background(), 443)
	if err != nil {
		t.Fatalf("InspectPort: %v", err)
	}
	if string(out) != string(rawLsof) {
		t.Fatalf("got %q, want %q", out, rawLsof)
	}

	if _, err := client.InspectPort(context.Background(), 70000); err == nil {
		t.Fatalf("expected invalid port rejection")
	}
}

func TestLaunchdSaveRemoveControlAndSafetyGuards(t *testing.T) {
	dir := t.TempDir()
	trashDir := t.TempDir()
	failLoad := false
	var ran [][]string

	client, _, _ := startTestHelper(t, ServerConfig{
		ManagedLaunchdDirs: []string{dir},
		Run: func(_ context.Context, name string, args ...string) ([]byte, error) {
			ran = append(ran, append([]string{name}, args...))
			if failLoad && len(args) > 0 && args[0] == "bootstrap" {
				return []byte("Bootstrap failed: 5"), errors.New("exit 5")
			}
			return nil, nil
		},
	})

	jobPath := filepath.Join(dir, "com.example.worker.plist")
	jobPlist := []byte(`<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>Label</key><string>com.example.worker</string></dict></plist>`)

	// Save with successful load.
	resp, err := client.LaunchdSave(context.Background(), LaunchdSaveRequest{
		Plist:  jobPlist,
		Path:   jobPath,
		Domain: "system",
		Target: "system/com.example.worker",
		Load:   true,
	})
	if err != nil {
		t.Fatalf("LaunchdSave: %v", err)
	}
	if !resp.Saved || resp.Error != "" {
		t.Fatalf("unexpected LaunchdSave response: %+v", resp)
	}

	// Save succeeds on disk even if bootstrap fails.
	failLoad = true
	resp, err = client.LaunchdSave(context.Background(), LaunchdSaveRequest{
		Plist:        jobPlist,
		Path:         jobPath,
		Domain:       "system",
		Target:       "system/com.example.worker",
		ExistingPath: jobPath,
		Load:         true,
	})
	if err != nil {
		t.Fatalf("LaunchdSave partial fail RPC err: %v", err)
	}
	if !resp.Saved || !strings.Contains(resp.Error, "Bootstrap failed") {
		t.Fatalf("expected Saved=true with load error, got %+v", resp)
	}
	failLoad = false

	// Reject symlinks in managed directory.
	symlinkPath := filepath.Join(dir, "com.example.link.plist")
	if err := os.Symlink(jobPath, symlinkPath); err != nil {
		t.Fatal(err)
	}
	if _, err := client.LaunchdSave(context.Background(), LaunchdSaveRequest{
		Plist:  jobPlist,
		Path:   symlinkPath,
		Domain: "system",
		Target: "system/com.example.link",
	}); err == nil || !strings.Contains(err.Error(), "symlink") {
		t.Fatalf("expected symlink rejection, got %v", err)
	}

	// Self-protection: reject modifying/controlling/removing the helper itself.
	helperPlist := []byte(`<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>Label</key><string>` + Label + `</string></dict></plist>`)
	if _, err := client.LaunchdSave(context.Background(), LaunchdSaveRequest{
		Plist:  helperPlist,
		Path:   filepath.Join(dir, Label+".plist"),
		Domain: "system",
		Target: "system/" + Label,
	}); err == nil || !strings.Contains(err.Error(), "privileged helper") {
		t.Fatalf("expected helper self-protection on save, got %v", err)
	}
	if err := client.LaunchdControl(context.Background(), [][]string{{"bootout", "system/" + Label}}); err == nil {
		t.Fatalf("expected helper self-protection on control")
	}
	if err := client.LaunchdControl(context.Background(), [][]string{{"rm", "-rf", "/"}}); err == nil {
		t.Fatalf("expected unsupported command rejection")
	}

	// Valid control commands (including kill and gui targets) work.
	if err := client.LaunchdControl(context.Background(), [][]string{
		{"kickstart", "-k", "system/com.example.worker"},
		{"kill", "SIGTERM", "system/com.example.worker"},
	}); err != nil {
		t.Fatalf("LaunchdControl: %v", err)
	}

	// Remove moves plist to TrashDir.
	if err := client.LaunchdRemove(context.Background(), LaunchdRemoveRequest{
		Path:     jobPath,
		Target:   "system/com.example.worker",
		TrashDir: trashDir,
	}); err != nil {
		t.Fatalf("LaunchdRemove: %v", err)
	}
	if _, err := os.Stat(jobPath); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("expected %s moved out", jobPath)
	}
	if _, err := os.Stat(filepath.Join(trashDir, "com.example.worker.plist")); err != nil {
		t.Fatalf("expected trashed plist: %v", err)
	}

	// System agent save + remove with gui/<uid>/<label> target.
	agentPath := filepath.Join(dir, "com.example.agent.plist")
	agentPlist := []byte(`<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>Label</key><string>com.example.agent</string></dict></plist>`)
	resp, err = client.LaunchdSave(context.Background(), LaunchdSaveRequest{
		Plist:  agentPlist,
		Path:   agentPath,
		Domain: "gui/501",
		Target: "gui/501/com.example.agent",
		Load:   false,
	})
	if err != nil || !resp.Saved {
		t.Fatalf("LaunchdSave gui target: resp=%+v err=%v", resp, err)
	}
	if err := client.LaunchdRemove(context.Background(), LaunchdRemoveRequest{
		Path:     agentPath,
		Target:   "gui/501/com.example.agent",
		TrashDir: trashDir,
	}); err != nil {
		t.Fatalf("LaunchdRemove gui target: %v", err)
	}
	_ = ran
}

func TestPassiveStatusTransitions(t *testing.T) {
	dir := t.TempDir()
	helperDir := filepath.Join(dir, "helper")
	plistPath := filepath.Join(dir, Label+".plist")
	sockPath := filepath.Join(dir, "helper.sock")
	stagedExe := filepath.Join(helperDir, "devbase-helper")

	expected := BinaryInfo{
		Version:    "v-abc123",
		Executable: "/Applications/Devbase.app/Contents/MacOS/Devbase",
		OwnerUID:   501,
	}
	client := NewClient(ClientConfig{
		SocketPath: sockPath,
		PlistPath:  plistPath,
		HelperDir:  helperDir,
		Expected:   expected,
	})

	if st := client.Status(); st.State != StateNotInstalled {
		t.Fatalf("expected not-installed, got %+v", st)
	}

	if err := os.MkdirAll(helperDir, 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(stagedExe, []byte("bin"), 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(helperDir, "version"), []byte("old-ver\n"), 0644); err != nil {
		t.Fatal(err)
	}
	plistBytes, err := buildHelperPlist(stagedExe, expected.OwnerUID, sockPath)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(plistPath, plistBytes, 0644); err != nil {
		t.Fatal(err)
	}

	if st := client.Status(); st.State != StateOutdated || st.InstalledVersion != "old-ver" {
		t.Fatalf("expected outdated with installed version, got %+v", st)
	}

	if err := os.WriteFile(filepath.Join(helperDir, "version"), []byte(expected.Version+"\n"), 0644); err != nil {
		t.Fatal(err)
	}
	if st := client.Status(); st.State != StateInstalled || st.InstalledVersion != expected.Version {
		t.Fatalf("expected installed, got %+v", st)
	}
}

func TestIdleTimeoutPreservesSocketFile(t *testing.T) {
	sockDir := shortSocketDir(t)
	sockPath := filepath.Join(sockDir, "idle.sock")
	ln, err := net.Listen("unix", sockPath)
	if err != nil {
		t.Fatal(err)
	}
	if ul, ok := ln.(*net.UnixListener); ok {
		ul.SetUnlinkOnClose(false)
	}

	srv := NewServer(ServerConfig{
		OwnerUID:        os.Geteuid(),
		ExpectedVersion: "v1",
		IdleTimeout:     80 * time.Millisecond,
	})
	done := make(chan error, 1)
	go func() {
		done <- srv.Serve(context.Background(), ln)
	}()

	client := NewClient(ClientConfig{SocketPath: sockPath})
	resp, err := client.Ping(context.Background())
	if err != nil || resp.Version != "v1" {
		t.Fatalf("Ping: resp=%+v err=%v", resp, err)
	}

	select {
	case err := <-done:
		if err != nil {
			t.Fatalf("Serve returned error after idle timeout: %v", err)
		}
	case <-time.After(2 * time.Second):
		t.Fatalf("server did not shut down after idle timeout")
	}

	if _, err := os.Stat(sockPath); err != nil {
		t.Fatalf("socket file %s should remain present after idle shutdown: %v", sockPath, err)
	}
	_ = json.Marshal
}

func TestInstallWaitsForDelayedSocketCreation(t *testing.T) {
	dir := t.TempDir()
	sockDir := shortSocketDir(t)
	sockPath := filepath.Join(sockDir, "delayed.sock")
	helperDir := filepath.Join(dir, "helper")
	plistPath := filepath.Join(dir, Label+".plist")
	fakeExe := filepath.Join(dir, "Devbase")
	if err := os.WriteFile(fakeExe, []byte("#!/bin/sh\n"), 0755); err != nil {
		t.Fatal(err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	srvDone := make(chan error, 1)
	var lnMu sync.Mutex
	var ln net.Listener
	t.Cleanup(func() {
		cancel()
		lnMu.Lock()
		l := ln
		lnMu.Unlock()
		if l != nil {
			_ = l.Close()
		}
		select {
		case <-srvDone:
		case <-time.After(2 * time.Second):
		}
	})

	var gotScript string
	client := NewClient(ClientConfig{
		SocketPath: sockPath,
		PlistPath:  plistPath,
		HelperDir:  helperDir,
		Expected: BinaryInfo{
			Version:    "v-new",
			Executable: fakeExe,
			OwnerUID:   os.Geteuid(),
		},
		HealthTimeout:      2 * time.Second,
		HealthPollInterval: 20 * time.Millisecond,
		RunAdmin: func(name string, args ...string) (string, error) {
			if name != "/bin/sh" || len(args) != 2 || args[0] != "-c" {
				t.Fatalf("unexpected RunAdmin invocation: %s %v", name, args)
			}
			gotScript = args[1]
			go func() {
				time.Sleep(120 * time.Millisecond)
				l, err := net.Listen("unix", sockPath)
				if err != nil {
					srvDone <- err
					return
				}
				lnMu.Lock()
				ln = l
				lnMu.Unlock()
				srv := NewServer(ServerConfig{
					OwnerUID:        os.Geteuid(),
					ExpectedVersion: "v-new",
				})
				srvDone <- srv.Serve(ctx, l)
			}()
			return "", nil
		},
	})

	if err := client.Install(context.Background()); err != nil {
		t.Fatalf("Install should succeed after delayed socket creation: %v", err)
	}
	if !strings.Contains(gotScript, "bootstrap system") || !strings.Contains(gotScript, sockPath) {
		t.Fatalf("expected install script to bootstrap and wait for socket %q, got %q", sockPath, gotScript)
	}
}

func TestInstallFailsWhenHealthCheckTimesOut(t *testing.T) {
	dir := t.TempDir()
	sockDir := shortSocketDir(t)
	sockPath := filepath.Join(sockDir, "missing.sock")
	fakeExe := filepath.Join(dir, "Devbase")
	if err := os.WriteFile(fakeExe, []byte("#!/bin/sh\n"), 0755); err != nil {
		t.Fatal(err)
	}

	client := NewClient(ClientConfig{
		SocketPath: sockPath,
		PlistPath:  filepath.Join(dir, Label+".plist"),
		HelperDir:  filepath.Join(dir, "helper"),
		Expected: BinaryInfo{
			Version:    "v-missing",
			Executable: fakeExe,
			OwnerUID:   os.Geteuid(),
		},
		HealthTimeout:      120 * time.Millisecond,
		HealthPollInterval: 15 * time.Millisecond,
		RunAdmin: func(string, ...string) (string, error) {
			return "", nil
		},
	})

	err := client.Install(context.Background())
	if err == nil || !strings.Contains(err.Error(), "helper installed, but health check failed") {
		t.Fatalf("expected health check timeout failure, got %v", err)
	}
}

func TestStagedBundleEnvelopeAvoidsDotAppSuffixAndPreservesVersion(t *testing.T) {
	dir := t.TempDir()
	appBundle := filepath.Join(dir, "Devbase.app")
	appContents := filepath.Join(appBundle, "Contents")
	appExe := filepath.Join(appContents, "MacOS", "Devbase")
	if err := os.MkdirAll(filepath.Dir(appExe), 0755); err != nil {
		t.Fatal(err)
	}
	plistBytes := []byte("<plist version=\"1.0\"><dict><key>CFBundleName</key><string>Devbase</string></dict></plist>\n")
	exeBytes := []byte("devbase-binary-payload")
	if err := os.WriteFile(filepath.Join(appContents, "Info.plist"), plistBytes, 0644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(appExe, exeBytes, 0755); err != nil {
		t.Fatal(err)
	}

	gotBundle := appBundleForExecutable(appExe)
	if gotBundle != appBundle {
		t.Fatalf("appBundleForExecutable(%q) = %q, want %q", appExe, gotBundle, appBundle)
	}
	info := BinaryInfo{
		Executable: appExe,
		BundlePath: gotBundle,
		Version:    computeArtifactVersion(appExe, gotBundle),
		OwnerUID:   os.Geteuid(),
	}

	helperDir := filepath.Join(dir, "helper-root")
	stagedExe := expectedStagedExecutable(helperDir, info)
	if strings.Contains(stagedExe, ".app") {
		t.Fatalf("staged helper executable path %q must not contain .app suffix", stagedExe)
	}
	wantStagedExe := filepath.Join(helperDir, "bundle", "Contents", "MacOS", "Devbase")
	if stagedExe != wantStagedExe {
		t.Fatalf("expectedStagedExecutable = %q, want %q", stagedExe, wantStagedExe)
	}

	stagedBundle := filepath.Join(helperDir, "bundle")
	if err := os.MkdirAll(filepath.Dir(stagedExe), 0755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(stagedBundle, "Contents", "Info.plist"), plistBytes, 0644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(stagedExe, exeBytes, 0755); err != nil {
		t.Fatal(err)
	}
	if gotStagedBundle := appBundleForExecutable(stagedExe); gotStagedBundle != stagedBundle {
		t.Fatalf("staged appBundleForExecutable(%q) = %q, want %q", stagedExe, gotStagedBundle, stagedBundle)
	}
	if stagedVer := computeArtifactVersion(stagedExe, stagedBundle); stagedVer != info.Version {
		t.Fatalf("staged artifact version = %q, want %q", stagedVer, info.Version)
	}
}

// pidSatisfiesSelf is a test-only check that the process with pid runs code
// satisfying this process's designated requirement.
func pidSatisfiesSelf(pid int) error {
	if err := initSecurity(); err != nil {
		return err
	}
	req, err := selfRequirement()
	if err != nil {
		return err
	}
	var cfNumberCreate func(alloc uintptr, typ int, val *int32) uintptr
	sec, err := purego.Dlopen("/System/Library/Frameworks/Security.framework/Security", purego.RTLD_LAZY)
	if err != nil {
		return err
	}
	cf, err := purego.Dlopen("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation", purego.RTLD_LAZY)
	if err != nil {
		return err
	}
	purego.RegisterLibFunc(&cfNumberCreate, cf, "CFNumberCreate")
	sym, err := purego.Dlsym(sec, "kSecGuestAttributePid")
	if err != nil {
		return err
	}
	key := derefSymbol(sym)
	p := int32(pid)
	num := cfNumberCreate(0, 3, &p) // kCFNumberSInt32Type
	if num == 0 {
		return errors.New("CFNumberCreate failed")
	}
	defer cfRelease(num)
	return codeSatisfies(key, num, req)
}

func TestCodeRequirementAcceptsSelfAndRejectsOtherCode(t *testing.T) {
	if err := pidSatisfiesSelf(os.Getpid()); err != nil {
		t.Fatalf("own pid should satisfy own requirement: %v", err)
	}
	cmd := exec.Command("/bin/sleep", "30")
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		_ = cmd.Process.Kill()
		_ = cmd.Wait()
	})
	if err := pidSatisfiesSelf(cmd.Process.Pid); err == nil {
		t.Fatal("/bin/sleep must not satisfy the test binary's requirement")
	}
}

func TestPeerAuditTokenIdentifiesSelf(t *testing.T) {
	sock := filepath.Join(shortSocketDir(t), "p.sock")
	ln, err := net.Listen("unix", sock)
	if err != nil {
		t.Fatal(err)
	}
	defer ln.Close()
	accepted := make(chan net.Conn, 1)
	go func() {
		c, _ := ln.Accept()
		accepted <- c
	}()
	dialed, err := net.Dial("unix", sock)
	if err != nil {
		t.Fatal(err)
	}
	defer dialed.Close()
	conn := <-accepted
	if conn == nil {
		t.Fatal("accept failed")
	}
	defer conn.Close()
	if err := peerRunsHelperCode(conn); err != nil {
		t.Fatalf("peerRunsHelperCode: %v", err)
	}
}

func TestPeerCodeAuthorizationRejectsForeignCode(t *testing.T) {
	hostsFile := filepath.Join(t.TempDir(), "hosts")
	orig := []byte("127.0.0.1 localhost\n")
	if err := os.WriteFile(hostsFile, orig, 0644); err != nil {
		t.Fatal(err)
	}
	client, _, _ := startTestHelper(t, ServerConfig{
		PeerAuthorized: func(net.Conn) error { return errors.New("foreign") },
		HostsPath:      hostsFile,
	})
	_, err := client.Ping(context.Background())
	if err == nil || !strings.Contains(err.Error(), "update the helper in Settings") {
		t.Fatalf("expected peer code rejection, got %v", err)
	}
	err = client.SyncHosts(context.Background(), HostsSyncRequest{
		ExpectedOld: string(orig),
		NewContent:  string(orig) + "127.0.0.1 evil.test\n",
	})
	if err == nil {
		t.Fatal("SyncHosts must fail for foreign code")
	}
	got, rerr := os.ReadFile(hostsFile)
	if rerr != nil {
		t.Fatal(rerr)
	}
	if string(got) != string(orig) {
		t.Fatalf("hosts file changed: %q", got)
	}
}

func TestPeerCodeAuthorizationRunsAfterUIDCheck(t *testing.T) {
	client, _, _ := startTestHelper(t, ServerConfig{
		OwnerUID:       os.Geteuid() + 1,
		PeerAuthorized: func(net.Conn) error { t.Error("must not be reached"); return nil },
	})
	_, err := client.Ping(context.Background())
	if err == nil || !strings.Contains(err.Error(), "not authorized") {
		t.Fatalf("expected peer uid rejection, got %v", err)
	}
}

func TestDefaultPeerAuthorizationAcceptsSameBinary(t *testing.T) {
	client, _, _ := startTestHelper(t, ServerConfig{})
	if _, err := client.Ping(context.Background()); err != nil {
		t.Fatalf("Ping with default authorization: %v", err)
	}
}

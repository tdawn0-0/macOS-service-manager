//go:build darwin

package privhelper

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"testing"
	"time"

	"devbase/internal/system"
	"howett.net/plist"
)

// BinaryInfo identifies the Devbase artifact that the helper expects to match.
type BinaryInfo struct {
	Executable string
	BundlePath string
	OwnerUID   int
}

// CurrentBinaryInfo returns the artifact identity for the running Devbase process.
func CurrentBinaryInfo() BinaryInfo {
	exe, _ := os.Executable()
	if resolved, err := filepath.EvalSymlinks(exe); err == nil && resolved != "" {
		exe = resolved
	}
	info := BinaryInfo{
		Executable: exe,
		OwnerUID:   os.Geteuid(),
	}
	if bundle := appBundleForExecutable(exe); bundle != "" {
		info.BundlePath = bundle
	}
	return info
}

func appBundleForExecutable(exe string) string {
	// <Envelope>/Contents/MacOS/<binary> sealed with Contents/Info.plist
	dir := filepath.Dir(exe)
	if filepath.Base(dir) != "MacOS" {
		return ""
	}
	contents := filepath.Dir(dir)
	if filepath.Base(contents) != "Contents" {
		return ""
	}
	if _, err := os.Stat(filepath.Join(contents, "Info.plist")); err != nil {
		return ""
	}
	return filepath.Dir(contents)
}

// ClientConfig configures a privileged helper RPC client.
type ClientConfig struct {
	SocketPath         string
	PlistPath          string
	HelperDir          string
	Expected           BinaryInfo
	RunAdmin           func(name string, args ...string) (string, error)
	HealthTimeout      time.Duration
	HealthPollInterval time.Duration
}

// Client communicates with the launchd-managed privileged helper over a Unix domain socket.
type Client struct {
	cfg      ClientConfig
	disabled bool
	http     *http.Client
}

var (
	overrideMu     sync.RWMutex
	overrideClient *Client
)

// SetDefaultForTest overrides the Client returned by Default() and returns a restore callback.
func SetDefaultForTest(c *Client) func() {
	overrideMu.Lock()
	prev := overrideClient
	overrideClient = c
	overrideMu.Unlock()
	return func() {
		overrideMu.Lock()
		overrideClient = prev
		overrideMu.Unlock()
	}
}

// Default returns the default privileged helper client for the current process.
// Under go test or isolated MSM_DEV_HOME runs without an explicit socket override,
// Default returns a disabled client so tests never touch the host helper.
func Default() *Client {
	overrideMu.RLock()
	ov := overrideClient
	overrideMu.RUnlock()
	if ov != nil {
		return ov
	}
	if testing.Testing() || (os.Getenv("MSM_DEV_HOME") != "" && os.Getenv("MSM_PRIVHELPER_SOCKET") == "") {
		return &Client{disabled: true}
	}
	sock := os.Getenv("MSM_PRIVHELPER_SOCKET")
	if sock == "" {
		sock = SocketPath
	}
	return NewClient(ClientConfig{
		SocketPath: sock,
		PlistPath:  PlistPath,
		HelperDir:  HelperDir,
		Expected:   CurrentBinaryInfo(),
	})
}

// NewClient creates an RPC client with explicit configuration.
func NewClient(cfg ClientConfig) *Client {
	if cfg.SocketPath == "" {
		cfg.SocketPath = SocketPath
	}
	if cfg.PlistPath == "" {
		cfg.PlistPath = PlistPath
	}
	if cfg.HelperDir == "" {
		cfg.HelperDir = HelperDir
	}
	if cfg.RunAdmin == nil {
		cfg.RunAdmin = system.RunAdmin
	}
	if cfg.HealthTimeout <= 0 {
		cfg.HealthTimeout = 8 * time.Second
	}
	if cfg.HealthPollInterval <= 0 {
		cfg.HealthPollInterval = 50 * time.Millisecond
	}
	tr := &http.Transport{
		DialContext: func(ctx context.Context, _, _ string) (net.Conn, error) {
			return dialUnixWithRetry(ctx, cfg.SocketPath)
		},
	}
	return &Client{
		cfg: cfg,
		http: &http.Client{
			Transport: tr,
			Timeout:   30 * time.Second,
		},
	}
}

func dialUnixWithRetry(ctx context.Context, sockPath string) (net.Conn, error) {
	backoffs := []time.Duration{15 * time.Millisecond, 30 * time.Millisecond, 60 * time.Millisecond}
	dialer := &net.Dialer{Timeout: 5 * time.Second}
	for attempt := 0; ; attempt++ {
		conn, err := dialer.DialContext(ctx, "unix", sockPath)
		if err == nil {
			return conn, nil
		}
		if attempt >= len(backoffs) || ctx.Err() != nil {
			return nil, err
		}
		if !errors.Is(err, syscall.ENOENT) && !errors.Is(err, syscall.ECONNREFUSED) {
			return nil, err
		}
		if !sleepWithContext(ctx, backoffs[attempt]) {
			return nil, ctx.Err()
		}
	}
}

func sleepWithContext(ctx context.Context, d time.Duration) bool {
	timer := time.NewTimer(d)
	defer timer.Stop()
	select {
	case <-ctx.Done():
		return false
	case <-timer.C:
		return true
	}
}

type helperPlistDoc struct {
	Label            string   `plist:"Label"`
	ProgramArguments []string `plist:"ProgramArguments"`
	Sockets          map[string]struct {
		SockPathName string `plist:"SockPathName"`
		SockPathMode int    `plist:"SockPathMode"`
	} `plist:"Sockets"`
}

// Status passively inspects the installed helper plist and staged artifact without
// activating the helper or prompting for a password.
func (c *Client) Status() HelperStatus {
	expected := c.cfg.Expected
	if c == nil || c.disabled {
		return HelperStatus{
			State:   StateNotInstalled,
			Message: "Privileged helper is disabled in this environment.",
		}
	}

	raw, err := os.ReadFile(c.cfg.PlistPath)
	if err != nil {
		if errors.Is(err, os.ErrNotExist) {
			return HelperStatus{
				State:   StateNotInstalled,
				Message: "Not installed. Install once to avoid repeated administrator password prompts.",
			}
		}
		return HelperStatus{
			State:   StateNotInstalled,
			Message: fmt.Sprintf("Could not inspect helper configuration: %v", err),
		}
	}

	var doc helperPlistDoc
	if _, err := plist.Unmarshal(raw, &doc); err != nil {
		return HelperStatus{
			State:   StateNotInstalled,
			Message: "Installed helper configuration is invalid and needs to be reinstalled.",
		}
	}

	stagedExe := expectedStagedExecutable(c.cfg.HelperDir, expected)
	if doc.Label != Label || len(doc.ProgramArguments) < 4 || doc.ProgramArguments[0] != stagedExe {
		return HelperStatus{
			State:   StateNotInstalled,
			Message: "Installed helper points to a different location and needs to be reinstalled.",
		}
	}

	if !hasOwnerArg(doc.ProgramArguments, expected.OwnerUID) {
		return HelperStatus{
			State:   StateNotInstalled,
			Message: "Installed helper is configured for a different user and needs to be reinstalled.",
		}
	}

	sockCfg, ok := doc.Sockets[SocketName]
	if !ok || sockCfg.SockPathName != c.cfg.SocketPath {
		return HelperStatus{
			State:   StateNotInstalled,
			Message: "Installed helper socket configuration is invalid and needs to be reinstalled.",
		}
	}

	if _, err := os.Stat(stagedExe); err != nil {
		return HelperStatus{
			State:   StateNotInstalled,
			Message: "Installed helper binary is missing and needs to be reinstalled.",
		}
	}

	return HelperStatus{
		State:   StateInstalled,
		Message: "Installed. Root operations run without password prompts.",
	}
}

// Ready reports whether the helper is currently installed and matches this app.
func (c *Client) Ready() bool {
	return c != nil && !c.disabled && c.Status().State == StateInstalled
}

// Require returns the default helper client when installed and ready, or ErrNotInstalled.
func Require() (*Client, error) {
	c := Default()
	if !c.Ready() {
		return nil, ErrNotInstalled
	}
	return c, nil
}

func expectedStagedExecutable(helperDir string, info BinaryInfo) string {
	if info.BundlePath != "" {
		relExe := filepath.Base(info.Executable)
		return filepath.Join(helperDir, "bundle", "Contents", "MacOS", relExe)
	}
	return filepath.Join(helperDir, "devbase-helper")
}

func hasOwnerArg(args []string, uid int) bool {
	want := strconv.Itoa(uid)
	for i := 0; i+1 < len(args); i++ {
		if args[i] == "--owner-uid" && args[i+1] == want {
			return true
		}
	}
	return false
}

// Install stages the helper binary/bundle under /Library/PrivilegedHelperTools,
// installs the launchd plist via a single administrator prompt, and verifies connectivity.
func (c *Client) Install(ctx context.Context) error {
	if c == nil || c.disabled {
		return fmt.Errorf("privileged helper is disabled")
	}
	if err := ctx.Err(); err != nil {
		return err
	}
	expected := c.cfg.Expected
	if expected.Executable == "" {
		return fmt.Errorf("could not locate running Devbase executable")
	}
	stagedExe := expectedStagedExecutable(c.cfg.HelperDir, expected)
	plistBytes, err := buildHelperPlist(stagedExe, expected.OwnerUID, c.cfg.SocketPath)
	if err != nil {
		return err
	}

	tmpDir, err := os.MkdirTemp("", "devbase-helper-install-*")
	if err != nil {
		return err
	}
	defer os.RemoveAll(tmpDir)

	tmpPlist := filepath.Join(tmpDir, Label+".plist")
	if err := os.WriteFile(tmpPlist, plistBytes, 0644); err != nil {
		return err
	}

	script := installHelperScript(expected, c.cfg.HelperDir, c.cfg.PlistPath, c.cfg.SocketPath, tmpPlist, stagedExe)
	if out, err := c.cfg.RunAdmin("/bin/sh", "-c", script); err != nil {
		msg := strings.TrimSpace(string(out))
		if msg != "" {
			return fmt.Errorf("helper install failed: %s: %w", msg, err)
		}
		return err
	}

	return c.waitForHealthy(ctx)
}

func (c *Client) waitForHealthy(ctx context.Context) error {
	timeout := c.cfg.HealthTimeout
	if timeout <= 0 {
		timeout = 8 * time.Second
	}
	pollInterval := c.cfg.HealthPollInterval
	if pollInterval <= 0 {
		pollInterval = 50 * time.Millisecond
	}
	expected := c.cfg.Expected
	healthCtx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()

	var lastErr error
	for {
		attemptCtx, attemptCancel := context.WithTimeout(healthCtx, 2*time.Second)
		resp, err := c.Ping(attemptCtx)
		attemptCancel()
		if err == nil {
			if resp.OwnerUID != expected.OwnerUID {
				return fmt.Errorf("helper owner UID mismatch: got %d, want %d", resp.OwnerUID, expected.OwnerUID)
			}
			return nil
		} else {
			if ctx.Err() != nil {
				return ctx.Err()
			}
			if strings.Contains(err.Error(), "not authorized") || strings.Contains(err.Error(), "reinstall the helper") {
				return fmt.Errorf("helper installed, but health check failed: %w", err)
			}
			lastErr = err
		}
		if !sleepWithContext(healthCtx, pollInterval) {
			if ctx.Err() != nil {
				return ctx.Err()
			}
			if lastErr == nil {
				lastErr = healthCtx.Err()
			}
			return fmt.Errorf("helper installed, but health check failed: %w", lastErr)
		}
	}
}

// Uninstall stops and removes the helper launchd job, plist, staged files, and socket.
func (c *Client) Uninstall(ctx context.Context) error {
	if c == nil || c.disabled {
		return fmt.Errorf("privileged helper is disabled")
	}
	if err := ctx.Err(); err != nil {
		return err
	}
	script := strings.Join([]string{
		"set -eu",
		"/bin/launchctl bootout " + system.ShellQuote("system/"+Label) + " >/dev/null 2>&1 || true",
		"/bin/rm -f " + system.ShellQuote(c.cfg.PlistPath),
		"/bin/rm -rf " + system.ShellQuote(c.cfg.HelperDir),
		"/bin/rm -f " + system.ShellQuote(c.cfg.SocketPath),
	}, "; ")
	out, err := c.cfg.RunAdmin("/bin/sh", "-c", script)
	if err != nil {
		msg := strings.TrimSpace(string(out))
		if msg != "" {
			return fmt.Errorf("helper uninstall failed: %s: %w", msg, err)
		}
		return err
	}
	return nil
}

func buildHelperPlist(stagedExe string, ownerUID int, sockPath string) ([]byte, error) {
	doc := map[string]any{
		"Label": Label,
		"ProgramArguments": []string{
			stagedExe,
			"--dev",
			"helper",
			"--owner-uid",
			strconv.Itoa(ownerUID),
		},
		"Sockets": map[string]any{
			SocketName: map[string]any{
				"SockPathName": sockPath,
				"SockPathMode": 0666,
			},
		},
		"StandardErrorPath": "/var/log/xyz.jyeu.devbase.helper.log",
	}
	return plist.MarshalIndent(doc, plist.XMLFormat, "\t")
}

func installHelperScript(info BinaryInfo, helperDir, plistPath, sockPath, tmpPlist, stagedExe string) string {
	parentDir := filepath.Dir(helperDir)
	quotedSock := system.ShellQuote(sockPath)
	lines := []string{
		"set -eu",
		"/bin/launchctl bootout " + system.ShellQuote("system/"+Label) + " >/dev/null 2>&1 || true",
		"/bin/rm -f " + quotedSock,
		"/bin/mkdir -p " + system.ShellQuote(parentDir),
		"/usr/sbin/chown root:wheel " + system.ShellQuote(parentDir),
		"/bin/chmod 0755 " + system.ShellQuote(parentDir),
		"/bin/rm -rf " + system.ShellQuote(helperDir),
		"/bin/mkdir -p " + system.ShellQuote(helperDir),
	}
	if info.BundlePath != "" {
		destBundle := filepath.Join(helperDir, "bundle")
		lines = append(lines,
			"/usr/bin/ditto --noqtn "+system.ShellQuote(info.BundlePath)+" "+system.ShellQuote(destBundle),
			"/usr/bin/codesign --verify --deep --strict "+system.ShellQuote(destBundle),
		)
	} else {
		lines = append(lines,
			"/usr/bin/install -m 0755 "+system.ShellQuote(info.Executable)+" "+system.ShellQuote(stagedExe),
		)
	}
	lines = append(lines,
		"/usr/sbin/chown -R root:wheel "+system.ShellQuote(helperDir),
		"/bin/chmod 0755 "+system.ShellQuote(helperDir),
		"/usr/bin/install -m 0644 "+system.ShellQuote(tmpPlist)+" "+system.ShellQuote(plistPath),
		"/usr/sbin/chown root:wheel "+system.ShellQuote(plistPath),
		"/bin/launchctl bootstrap system "+system.ShellQuote(plistPath),
		"i=0; while [ ! -S "+quotedSock+" ] && [ \"$i\" -lt 50 ]; do /bin/sleep 0.1; i=$((i + 1)); done",
	)
	return strings.Join(lines, "; ")
}

// Ping queries GET /v1/status on the helper socket.
func (c *Client) Ping(ctx context.Context) (StatusResponse, error) {
	var resp StatusResponse
	err := c.doJSON(ctx, http.MethodGet, "/v1/status", nil, &resp)
	return resp, err
}

// SyncHosts atomically replaces hosts content if ExpectedOld still matches on disk.
func (c *Client) SyncHosts(ctx context.Context, req HostsSyncRequest) error {
	return c.doJSON(ctx, http.MethodPost, "/v1/hosts/sync", req, nil)
}

// InstallProxySocket writes the Devbase HTTPS socket daemon plist and bootstraps it.
func (c *Client) InstallProxySocket(ctx context.Context, req ProxySocketInstallRequest) error {
	return c.doJSON(ctx, http.MethodPost, "/v1/proxy-socket/install", req, nil)
}

// StopProxySocket boots out and removes the Devbase HTTPS socket daemon plist.
func (c *Client) StopProxySocket(ctx context.Context, req ProxySocketStopRequest) error {
	return c.doJSON(ctx, http.MethodPost, "/v1/proxy-socket/stop", req, nil)
}

// KickstartProxySocket restarts the Devbase HTTPS socket daemon.
func (c *Client) KickstartProxySocket(ctx context.Context, target string) error {
	return c.doJSON(ctx, http.MethodPost, "/v1/proxy-socket/kickstart", ProxySocketKickstartRequest{
		Target: target,
	}, nil)
}

// PrintProxySocket runs launchctl print on the proxy socket target as root.
func (c *Client) PrintProxySocket(ctx context.Context, target string) (ProxySocketPrintResponse, error) {
	var resp ProxySocketPrintResponse
	err := c.doJSON(ctx, http.MethodPost, "/v1/proxy-socket/print", ProxySocketPrintRequest{
		Target: target,
	}, &resp)
	return resp, err
}

// InspectPort runs lsof as root for the given TCP listening port.
func (c *Client) InspectPort(ctx context.Context, port int) ([]byte, error) {
	var resp PortInspectResponse
	err := c.doJSON(ctx, http.MethodPost, "/v1/ports/inspect", PortInspectRequest{Port: port}, &resp)
	return resp.Output, err
}

// LaunchdControl executes validated launchctl commands as root.
func (c *Client) LaunchdControl(ctx context.Context, commands [][]string) error {
	return c.doJSON(ctx, http.MethodPost, "/v1/launchd/control", LaunchdControlRequest{Commands: commands}, nil)
}

// LaunchdSave saves and optionally bootstraps a system LaunchAgent or LaunchDaemon plist.
func (c *Client) LaunchdSave(ctx context.Context, req LaunchdSaveRequest) (LaunchdSaveResponse, error) {
	var resp LaunchdSaveResponse
	err := c.doJSON(ctx, http.MethodPost, "/v1/launchd/save", req, &resp)
	return resp, err
}

// LaunchdRemove boots out a system launchd job and moves its plist into TrashDir.
func (c *Client) LaunchdRemove(ctx context.Context, req LaunchdRemoveRequest) error {
	return c.doJSON(ctx, http.MethodPost, "/v1/launchd/remove", req, nil)
}

func (c *Client) doJSON(ctx context.Context, method, path string, in, out any) error {
	if c == nil || c.disabled {
		return fmt.Errorf("privileged helper is not enabled")
	}
	var body io.Reader
	if in != nil {
		raw, err := json.Marshal(in)
		if err != nil {
			return err
		}
		body = bytes.NewReader(raw)
	}
	req, err := http.NewRequestWithContext(ctx, method, "http://unix"+path, body)
	if err != nil {
		return err
	}
	if in != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	resp, err := c.http.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	respBody, err := io.ReadAll(io.LimitReader(resp.Body, maxRequestBytes))
	if err != nil {
		return err
	}
	if resp.StatusCode >= 400 {
		var errResp ErrorResponse
		if json.Unmarshal(respBody, &errResp) == nil && errResp.Error != "" {
			return errors.New(errResp.Error)
		}
		return fmt.Errorf("helper %s %s failed with HTTP %d", method, path, resp.StatusCode)
	}
	if out != nil {
		if err := json.Unmarshal(respBody, out); err != nil {
			return err
		}
	}
	return nil
}

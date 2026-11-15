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
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"syscall"
	"time"

	"howett.net/plist"
)

const (
	defaultIdleTimeout = 60 * time.Second
	maxRequestBytes    = 4 << 20 // 4 MiB
)

type connContextKey struct{}

// ServerConfig configures the privileged helper HTTP-over-Unix server.
type ServerConfig struct {
	OwnerUID           int
	IdleTimeout        time.Duration
	HostsPath          string
	ManagedLaunchdDirs []string
	SocketJobDirs      []string
	Run                func(ctx context.Context, name string, args ...string) ([]byte, error)
	PeerUID            func(net.Conn) (int, error)
	// PeerAuthorized reports whether the peer runs code satisfying this
	// helper's designated requirement. Defaults to peerRunsHelperCode.
	PeerAuthorized func(net.Conn) error
	Chown          func(path string, uid, gid int) error
}

// Server serves privileged operations over a Unix domain socket for OwnerUID.
type Server struct {
	cfg     ServerConfig
	mux     *http.ServeMux
	active  atomic.Int64
	touchCh chan struct{}
}

// NewServer constructs a helper Server with validated defaults.
func NewServer(cfg ServerConfig) *Server {
	if cfg.IdleTimeout <= 0 {
		cfg.IdleTimeout = defaultIdleTimeout
	}
	if cfg.HostsPath == "" {
		cfg.HostsPath = "/etc/hosts"
	}
	if len(cfg.ManagedLaunchdDirs) == 0 {
		cfg.ManagedLaunchdDirs = []string{"/Library/LaunchAgents", "/Library/LaunchDaemons"}
	}
	if len(cfg.SocketJobDirs) == 0 {
		cfg.SocketJobDirs = []string{"/Library/LaunchDaemons"}
	}
	if cfg.Run == nil {
		cfg.Run = defaultServerRun
	}
	if cfg.PeerUID == nil {
		cfg.PeerUID = peerEffectiveUID
	}
	if cfg.PeerAuthorized == nil {
		cfg.PeerAuthorized = peerRunsHelperCode
	}
	if cfg.Chown == nil {
		cfg.Chown = os.Lchown
	}

	s := &Server{
		cfg:     cfg,
		mux:     http.NewServeMux(),
		touchCh: make(chan struct{}, 1),
	}
	s.routes()
	return s
}

func defaultServerRun(ctx context.Context, name string, args ...string) ([]byte, error) {
	cmd := exec.CommandContext(ctx, name, args...)
	return cmd.CombinedOutput()
}

// ServeActivated claims the launchd-managed Unix socket and serves until idle or cancelled.
func ServeActivated(ctx context.Context, ownerUID int) error {
	if ownerUID <= 0 {
		return fmt.Errorf("invalid owner uid %d", ownerUID)
	}
	ln, err := activatedUnixListener(SocketName)
	if err != nil {
		return err
	}
	defer ln.Close()
	srv := NewServer(ServerConfig{
		OwnerUID: ownerUID,
	})
	return srv.Serve(ctx, ln)
}

// Serve accepts requests on ln until ctx is cancelled or the idle timeout expires.
func (s *Server) Serve(ctx context.Context, ln net.Listener) error {
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()

	httpSrv := &http.Server{
		Handler:           s.withAuth(s.mux),
		ReadHeaderTimeout: 5 * time.Second,
		ConnContext: func(reqCtx context.Context, c net.Conn) context.Context {
			return context.WithValue(reqCtx, connContextKey{}, c)
		},
	}

	var wg sync.WaitGroup
	wg.Add(1)
	go func() {
		defer wg.Done()
		s.idleLoop(ctx, httpSrv)
	}()

	go func() {
		<-ctx.Done()
		shutCtx, shutCancel := context.WithTimeout(context.Background(), 5*time.Second)
		defer shutCancel()
		_ = httpSrv.Shutdown(shutCtx)
	}()

	err := httpSrv.Serve(ln)
	cancel()
	wg.Wait()
	if errors.Is(err, http.ErrServerClosed) {
		return nil
	}
	return err
}

func (s *Server) touch() {
	select {
	case s.touchCh <- struct{}{}:
	default:
	}
}

func (s *Server) idleLoop(ctx context.Context, httpSrv *http.Server) {
	timer := time.NewTimer(s.cfg.IdleTimeout)
	defer timer.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-s.touchCh:
			if !timer.Stop() {
				select {
				case <-timer.C:
				default:
				}
			}
			timer.Reset(s.cfg.IdleTimeout)
		case <-timer.C:
			if s.active.Load() > 0 {
				timer.Reset(s.cfg.IdleTimeout)
				continue
			}
			shutCtx, shutCancel := context.WithTimeout(context.Background(), 5*time.Second)
			_ = httpSrv.Shutdown(shutCtx)
			shutCancel()
			return
		}
	}
}

func (s *Server) withAuth(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		s.active.Add(1)
		defer s.active.Add(-1)
		s.touch()

		conn, _ := r.Context().Value(connContextKey{}).(net.Conn)
		if conn == nil {
			writeError(w, http.StatusForbidden, "missing peer connection")
			return
		}
		uid, err := s.cfg.PeerUID(conn)
		if err != nil {
			writeError(w, http.StatusForbidden, "failed to verify peer uid")
			return
		}
		if s.cfg.OwnerUID <= 0 || uid != s.cfg.OwnerUID {
			writeError(w, http.StatusForbidden, fmt.Sprintf("peer uid %d is not authorized", uid))
			return
		}
		if err := s.cfg.PeerAuthorized(conn); err != nil {
			fmt.Fprintln(os.Stderr, "privhelper: peer code check failed:", err)
			writeError(w, http.StatusForbidden, "peer is not the Devbase build this helper was installed from; reinstall the helper in Settings")
			return
		}
		next.ServeHTTP(w, r)
	})
}

func (s *Server) routes() {
	s.mux.HandleFunc("GET /v1/status", s.handleStatus)
	s.mux.HandleFunc("POST /v1/hosts/sync", s.handleHostsSync)
	s.mux.HandleFunc("POST /v1/proxy-socket/install", s.handleProxySocketInstall)
	s.mux.HandleFunc("POST /v1/proxy-socket/stop", s.handleProxySocketStop)
	s.mux.HandleFunc("POST /v1/proxy-socket/kickstart", s.handleProxySocketKickstart)
	s.mux.HandleFunc("POST /v1/proxy-socket/print", s.handleProxySocketPrint)
	s.mux.HandleFunc("POST /v1/ports/inspect", s.handlePortInspect)
	s.mux.HandleFunc("POST /v1/launchd/control", s.handleLaunchdControl)
	s.mux.HandleFunc("POST /v1/launchd/save", s.handleLaunchdSave)
	s.mux.HandleFunc("POST /v1/launchd/remove", s.handleLaunchdRemove)
}

func (s *Server) handleStatus(w http.ResponseWriter, _ *http.Request) {
	writeJSON(w, http.StatusOK, StatusResponse{
		ProtocolVersion: ProtocolVersion,
		OwnerUID:        s.cfg.OwnerUID,
	})
}

func (s *Server) handleHostsSync(w http.ResponseWriter, r *http.Request) {
	var req HostsSyncRequest
	if !decodeJSON(w, r, &req) {
		return
	}
	if strings.ContainsRune(req.NewContent, '\x00') {
		writeError(w, http.StatusBadRequest, "hosts content must not contain NUL bytes")
		return
	}
	if len(req.NewContent) > 64<<10 {
		writeError(w, http.StatusBadRequest, "hosts configuration exceeds size limit")
		return
	}
	hostsPath, err := validateCleanTargetFile(s.cfg.HostsPath, []string{filepath.Dir(s.cfg.HostsPath)}, "")
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	perm := os.FileMode(0644)
	if info, statErr := os.Stat(hostsPath); statErr == nil {
		perm = info.Mode().Perm()
	}
	current, err := os.ReadFile(hostsPath)
	if err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	if string(current) != req.ExpectedOld {
		writeError(w, http.StatusConflict, "hosts file changed concurrently")
		return
	}
	if err := atomicWriteFile(hostsPath, []byte(req.NewContent), perm); err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	_, _ = s.cfg.Run(r.Context(), "/usr/bin/dscacheutil", "-flushcache")
	writeJSON(w, http.StatusOK, map[string]bool{"ok": true})
}

func (s *Server) handleProxySocketInstall(w http.ResponseWriter, r *http.Request) {
	var req ProxySocketInstallRequest
	if !decodeJSON(w, r, &req) {
		return
	}
	jobPath, err := s.validateProxySocketRequest(req.JobPath, req.Target)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	if err := validateXMLPlist(req.Plist); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	if err := atomicWriteFile(jobPath, req.Plist, 0644); err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	_ = s.chownRoot(jobPath)
	_, _ = s.cfg.Run(r.Context(), "/bin/launchctl", "bootout", req.Target)
	if out, err := s.cfg.Run(r.Context(), "/bin/launchctl", "bootstrap", "system", jobPath); err != nil {
		writeError(w, http.StatusInternalServerError, cmdError(err, out))
		return
	}
	writeJSON(w, http.StatusOK, map[string]bool{"ok": true})
}

func (s *Server) handleProxySocketStop(w http.ResponseWriter, r *http.Request) {
	var req ProxySocketStopRequest
	if !decodeJSON(w, r, &req) {
		return
	}
	jobPath, err := s.validateProxySocketRequest(req.JobPath, req.Target)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	if _, err := s.cfg.Run(r.Context(), "/bin/launchctl", "print", req.Target); err == nil {
		if out, bootErr := s.cfg.Run(r.Context(), "/bin/launchctl", "bootout", req.Target); bootErr != nil {
			writeError(w, http.StatusInternalServerError, cmdError(bootErr, out))
			return
		}
	}
	if err := os.Remove(jobPath); err != nil && !errors.Is(err, os.ErrNotExist) {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	writeJSON(w, http.StatusOK, map[string]bool{"ok": true})
}

func (s *Server) handleProxySocketKickstart(w http.ResponseWriter, r *http.Request) {
	var req ProxySocketKickstartRequest
	if !decodeJSON(w, r, &req) {
		return
	}
	if err := validateProxySocketTarget(req.Target); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	if out, err := s.cfg.Run(r.Context(), "/bin/launchctl", "kickstart", "-k", req.Target); err != nil {
		writeError(w, http.StatusInternalServerError, cmdError(err, out))
		return
	}
	writeJSON(w, http.StatusOK, map[string]bool{"ok": true})
}

func (s *Server) handleProxySocketPrint(w http.ResponseWriter, r *http.Request) {
	var req ProxySocketPrintRequest
	if !decodeJSON(w, r, &req) {
		return
	}
	if err := validateProxySocketTarget(req.Target); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	out, err := s.cfg.Run(r.Context(), "/bin/launchctl", "print", req.Target)
	code := 0
	if err != nil {
		var exitErr *exec.ExitError
		if errors.As(err, &exitErr) {
			code = exitErr.ExitCode()
		} else {
			code = 1
		}
	}
	writeJSON(w, http.StatusOK, ProxySocketPrintResponse{
		Output:   out,
		ExitCode: code,
	})
}

func (s *Server) handlePortInspect(w http.ResponseWriter, r *http.Request) {
	var req PortInspectRequest
	if !decodeJSON(w, r, &req) {
		return
	}
	if req.Port <= 0 || req.Port > 65535 {
		writeError(w, http.StatusBadRequest, fmt.Sprintf("invalid port %d", req.Port))
		return
	}
	out, err := s.cfg.Run(
		r.Context(),
		"/usr/sbin/lsof",
		"-nP", "-a", "-iTCP:"+strconv.Itoa(req.Port), "-sTCP:LISTEN", "+c0", "-F0pcun",
	)
	if err != nil && len(out) == 0 {
		writeError(w, http.StatusInternalServerError, cmdError(err, out))
		return
	}
	writeJSON(w, http.StatusOK, PortInspectResponse{Output: out})
}

func (s *Server) handleLaunchdControl(w http.ResponseWriter, r *http.Request) {
	var req LaunchdControlRequest
	if !decodeJSON(w, r, &req) {
		return
	}
	if len(req.Commands) == 0 {
		writeError(w, http.StatusBadRequest, "no commands specified")
		return
	}
	for _, cmd := range req.Commands {
		if err := validateLaunchctlCommand(cmd); err != nil {
			writeError(w, http.StatusBadRequest, err.Error())
			return
		}
	}
	for _, cmd := range req.Commands {
		if out, err := s.cfg.Run(r.Context(), "/bin/launchctl", cmd...); err != nil {
			writeError(w, http.StatusInternalServerError, cmdError(err, out))
			return
		}
	}
	writeJSON(w, http.StatusOK, map[string]bool{"ok": true})
}

func (s *Server) handleLaunchdSave(w http.ResponseWriter, r *http.Request) {
	var req LaunchdSaveRequest
	if !decodeJSON(w, r, &req) {
		return
	}
	path, err := validateCleanTargetFile(req.Path, s.cfg.ManagedLaunchdDirs, ".plist")
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	if targetsHelper(req.Target, path) || targetsHelper("", req.ExistingPath) {
		writeError(w, http.StatusForbidden, "cannot modify privileged helper job")
		return
	}
	if req.ExistingPath != "" {
		existing, err := validateCleanTargetFile(req.ExistingPath, s.cfg.ManagedLaunchdDirs, ".plist")
		if err != nil {
			writeError(w, http.StatusBadRequest, err.Error())
			return
		}
		req.ExistingPath = existing
	}
	if err := validateDomain(req.Domain); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	if err := validateLaunchdTarget(req.Target); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	if err := validateXMLPlist(req.Plist); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}

	if req.ExistingPath == "" {
		if _, err := os.Lstat(path); !errors.Is(err, os.ErrNotExist) {
			if err == nil {
				writeError(w, http.StatusConflict, fmt.Sprintf("job %s already exists", path))
			} else {
				writeError(w, http.StatusInternalServerError, err.Error())
			}
			return
		}
	} else {
		info, err := os.Lstat(req.ExistingPath)
		if err != nil {
			writeError(w, http.StatusBadRequest, err.Error())
			return
		}
		if !info.Mode().IsRegular() {
			writeError(w, http.StatusBadRequest, fmt.Sprintf("existing job %s is not a regular file", req.ExistingPath))
			return
		}
	}

	if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	if err := atomicWriteFile(path, req.Plist, 0644); err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	_ = s.chownRoot(path)

	if req.ExistingPath != "" && req.ExistingPath != path {
		if _, err := s.cfg.Run(r.Context(), "/bin/launchctl", "print", req.Target); err == nil {
			if out, bootErr := s.cfg.Run(r.Context(), "/bin/launchctl", "bootout", req.Target); bootErr != nil {
				writeError(w, http.StatusInternalServerError, cmdError(bootErr, out))
				return
			}
		}
		if err := os.Remove(req.ExistingPath); err != nil && !errors.Is(err, os.ErrNotExist) {
			writeError(w, http.StatusInternalServerError, err.Error())
			return
		}
	}

	if req.Load {
		if req.ExistingPath == path {
			if _, err := s.cfg.Run(r.Context(), "/bin/launchctl", "print", req.Target); err == nil {
				if out, bootErr := s.cfg.Run(r.Context(), "/bin/launchctl", "bootout", req.Target); bootErr != nil {
					writeJSON(w, http.StatusOK, LaunchdSaveResponse{
						Saved: true,
						Error: cmdError(bootErr, out),
					})
					return
				}
			}
		}
		if out, err := s.cfg.Run(r.Context(), "/bin/launchctl", "bootstrap", req.Domain, path); err != nil {
			writeJSON(w, http.StatusOK, LaunchdSaveResponse{
				Saved: true,
				Error: cmdError(err, out),
			})
			return
		}
	}
	writeJSON(w, http.StatusOK, LaunchdSaveResponse{Saved: true})
}

func (s *Server) handleLaunchdRemove(w http.ResponseWriter, r *http.Request) {
	var req LaunchdRemoveRequest
	if !decodeJSON(w, r, &req) {
		return
	}
	path, err := validateCleanTargetFile(req.Path, s.cfg.ManagedLaunchdDirs, ".plist")
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	if targetsHelper(req.Target, path) {
		writeError(w, http.StatusForbidden, "cannot remove privileged helper job")
		return
	}
	if err := validateLaunchdTarget(req.Target); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	trashDir, err := s.validateTrashDir(req.TrashDir)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}

	info, err := os.Lstat(path)
	if err != nil || !info.Mode().IsRegular() {
		writeError(w, http.StatusBadRequest, fmt.Sprintf("job %s is not a regular file", path))
		return
	}
	dest := filepath.Join(trashDir, filepath.Base(path))
	if _, err := os.Lstat(dest); !errors.Is(err, os.ErrNotExist) {
		writeError(w, http.StatusConflict, fmt.Sprintf("destination %s already exists", dest))
		return
	}

	if _, err := s.cfg.Run(r.Context(), "/bin/launchctl", "print", req.Target); err == nil {
		if out, bootErr := s.cfg.Run(r.Context(), "/bin/launchctl", "bootout", req.Target); bootErr != nil {
			writeError(w, http.StatusInternalServerError, cmdError(bootErr, out))
			return
		}
	}
	if err := moveFile(path, dest); err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	_ = s.cfg.Chown(dest, s.cfg.OwnerUID, -1)
	writeJSON(w, http.StatusOK, map[string]bool{"ok": true})
}

func (s *Server) chownRoot(path string) error {
	if os.Geteuid() != 0 {
		return nil
	}
	return s.cfg.Chown(path, 0, 0)
}

func (s *Server) validateProxySocketRequest(jobPath, target string) (string, error) {
	cleanPath, err := validateCleanTargetFile(jobPath, s.cfg.SocketJobDirs, ".plist")
	if err != nil {
		return "", err
	}
	if targetsHelper(target, cleanPath) {
		return "", fmt.Errorf("cannot target privileged helper")
	}
	base := filepath.Base(cleanPath)
	if !strings.HasPrefix(base, "xyz.jyeu.devbase.") {
		return "", fmt.Errorf("unexpected socket job path %q", cleanPath)
	}
	if err := validateProxySocketTarget(target); err != nil {
		return "", err
	}
	return cleanPath, nil
}

func (s *Server) validateTrashDir(dir string) (string, error) {
	if !filepath.IsAbs(dir) {
		return "", fmt.Errorf("trash directory must be absolute")
	}
	clean := filepath.Clean(dir)
	if clean != dir || clean == "/" {
		return "", fmt.Errorf("invalid trash directory %q", dir)
	}
	info, err := os.Lstat(clean)
	if err != nil {
		return "", err
	}
	if info.Mode()&os.ModeSymlink != 0 || !info.IsDir() {
		return "", fmt.Errorf("trash directory %q must be a real directory", clean)
	}
	if st, ok := info.Sys().(*syscall.Stat_t); ok && os.Geteuid() == 0 && int(st.Uid) != s.cfg.OwnerUID {
		return "", fmt.Errorf("trash directory %q is not owned by caller", clean)
	}
	return clean, nil
}

func validateProxySocketTarget(target string) error {
	if err := validateLaunchdTarget(target); err != nil {
		return err
	}
	if targetsHelper(target, "") {
		return fmt.Errorf("cannot target privileged helper")
	}
	if !strings.HasPrefix(target, "system/xyz.jyeu.devbase.") {
		return fmt.Errorf("unexpected proxy socket target %q", target)
	}
	return nil
}

func validateCleanTargetFile(path string, allowedDirs []string, suffix string) (string, error) {
	if path == "" || !filepath.IsAbs(path) {
		return "", fmt.Errorf("path %q must be absolute", path)
	}
	if strings.ContainsRune(path, '\x00') {
		return "", fmt.Errorf("path contains NUL byte")
	}
	clean := filepath.Clean(path)
	if clean != path {
		return "", fmt.Errorf("path %q is not normalized", path)
	}
	if suffix != "" && !strings.HasSuffix(clean, suffix) {
		return "", fmt.Errorf("path %q must end with %s", clean, suffix)
	}
	parent := filepath.Dir(clean)
	allowed := false
	for _, dir := range allowedDirs {
		if filepath.Clean(dir) == parent {
			allowed = true
			break
		}
	}
	if !allowed {
		return "", fmt.Errorf("directory %q is not permitted", parent)
	}
	if err := rejectSymlinkIfExists(parent); err != nil {
		return "", err
	}
	if err := rejectSymlinkIfExists(clean); err != nil {
		return "", err
	}
	return clean, nil
}

func rejectSymlinkIfExists(path string) error {
	info, err := os.Lstat(path)
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	if err != nil {
		return err
	}
	if info.Mode()&os.ModeSymlink != 0 {
		return fmt.Errorf("refusing symlink %q", path)
	}
	return nil
}

func targetsHelper(target, path string) bool {
	if target == Label || target == "system/"+Label || strings.HasSuffix(target, "/"+Label) {
		return true
	}
	if path != "" {
		clean := filepath.Clean(path)
		if clean == PlistPath || filepath.Base(clean) == Label+".plist" || strings.HasPrefix(clean, HelperDir) {
			return true
		}
	}
	return false
}

func validateXMLPlist(data []byte) error {
	if len(bytes.TrimSpace(data)) == 0 {
		return fmt.Errorf("plist data is empty")
	}
	var doc map[string]any
	if _, err := plist.Unmarshal(data, &doc); err != nil {
		return fmt.Errorf("invalid plist: %w", err)
	}
	label, _ := doc["Label"].(string)
	if label == "" {
		return fmt.Errorf("plist is missing Label")
	}
	if label == Label {
		return fmt.Errorf("cannot write helper label %q", label)
	}
	return nil
}

func validateDomain(domain string) error {
	if domain == "system" {
		return nil
	}
	if uidStr, ok := strings.CutPrefix(domain, "gui/"); ok && isDigits(uidStr) {
		return nil
	}
	if uidStr, ok := strings.CutPrefix(domain, "user/"); ok && isDigits(uidStr) {
		return nil
	}
	return fmt.Errorf("invalid launchd domain %q", domain)
}

func validateLaunchdTarget(target string) error {
	domain, rest, ok := strings.Cut(target, "/")
	if !ok || rest == "" {
		return fmt.Errorf("invalid launchd target %q", target)
	}
	if domain == "system" {
		if strings.Contains(rest, "/") {
			return fmt.Errorf("invalid launchd target %q", target)
		}
		return nil
	}
	if domain != "gui" && domain != "user" {
		return fmt.Errorf("invalid launchd target %q", target)
	}
	uidStr, label, ok := strings.Cut(rest, "/")
	if !ok || !isDigits(uidStr) || label == "" || strings.Contains(label, "/") {
		return fmt.Errorf("invalid launchd target %q", target)
	}
	return nil
}

func validateLaunchctlCommand(args []string) error {
	if len(args) < 2 {
		return fmt.Errorf("launchctl command requires arguments")
	}
	for _, a := range args {
		if a == "" || strings.ContainsRune(a, '\x00') {
			return fmt.Errorf("invalid launchctl argument %q", a)
		}
		if targetsHelper(a, a) {
			return fmt.Errorf("cannot target privileged helper")
		}
	}
	switch args[0] {
	case "bootstrap":
		if len(args) != 3 {
			return fmt.Errorf("bootstrap expects domain and plist path")
		}
		if err := validateDomain(args[1]); err != nil {
			return err
		}
		return validateLaunchctlPlistArg(args[2])
	case "bootout":
		if len(args) != 2 {
			return fmt.Errorf("bootout expects target")
		}
		return validateLaunchdTarget(args[1])
	case "enable", "disable", "print":
		if len(args) != 2 {
			return fmt.Errorf("%s expects target", args[0])
		}
		return validateLaunchdTarget(args[1])
	case "kickstart":
		idx := 1
		if len(args) == 3 && (args[1] == "-k" || args[1] == "-p") {
			idx = 2
		} else if len(args) != 2 {
			return fmt.Errorf("unexpected kickstart arguments")
		}
		return validateLaunchdTarget(args[idx])
	case "kill":
		if len(args) != 3 {
			return fmt.Errorf("kill expects signal and target")
		}
		switch args[1] {
		case "SIGTERM", "SIGKILL", "SIGINT", "SIGHUP":
		default:
			return fmt.Errorf("unsupported signal %q", args[1])
		}
		return validateLaunchdTarget(args[2])
	default:
		return fmt.Errorf("unsupported launchctl subcommand %q", args[0])
	}
}

func validateLaunchctlPlistArg(path string) error {
	if !filepath.IsAbs(path) || filepath.Clean(path) != path || !strings.HasSuffix(path, ".plist") {
		return fmt.Errorf("invalid plist path %q", path)
	}
	return rejectSymlinkIfExists(path)
}

func isDigits(s string) bool {
	if s == "" {
		return false
	}
	for i := 0; i < len(s); i++ {
		if s[i] < '0' || s[i] > '9' {
			return false
		}
	}
	return true
}

func atomicWriteFile(path string, data []byte, mode os.FileMode) error {
	dir := filepath.Dir(path)
	tmp, err := os.CreateTemp(dir, ".devbase-tmp-*")
	if err != nil {
		return err
	}
	tmpPath := tmp.Name()
	cleanup := true
	defer func() {
		if cleanup {
			_ = os.Remove(tmpPath)
		}
	}()
	if _, err := tmp.Write(data); err != nil {
		_ = tmp.Close()
		return err
	}
	if err := tmp.Chmod(mode); err != nil {
		_ = tmp.Close()
		return err
	}
	if err := tmp.Close(); err != nil {
		return err
	}
	if err := os.Rename(tmpPath, path); err != nil {
		return err
	}
	cleanup = false
	return nil
}

func moveFile(src, dst string) error {
	err := os.Rename(src, dst)
	if err == nil {
		return nil
	}
	var linkErr *os.LinkError
	if !errors.As(err, &linkErr) || !errors.Is(linkErr.Err, syscall.EXDEV) {
		return err
	}
	data, readErr := os.ReadFile(src)
	if readErr != nil {
		return readErr
	}
	info, statErr := os.Lstat(src)
	mode := os.FileMode(0644)
	if statErr == nil {
		mode = info.Mode().Perm()
	}
	if writeErr := atomicWriteFile(dst, data, mode); writeErr != nil {
		return writeErr
	}
	return os.Remove(src)
}

func decodeJSON(w http.ResponseWriter, r *http.Request, dst any) bool {
	r.Body = http.MaxBytesReader(w, r.Body, maxRequestBytes)
	dec := json.NewDecoder(r.Body)
	if err := dec.Decode(dst); err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return false
	}
	if err := dec.Decode(&struct{}{}); !errors.Is(err, io.EOF) {
		writeError(w, http.StatusBadRequest, "unexpected trailing JSON data")
		return false
	}
	return true
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	data, err := json.Marshal(v)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_, _ = w.Write(data)
}

func writeError(w http.ResponseWriter, status int, msg string) {
	writeJSON(w, status, ErrorResponse{Error: msg})
}

func cmdError(err error, out []byte) string {
	msg := strings.TrimSpace(string(out))
	if msg != "" {
		return msg
	}
	return err.Error()
}

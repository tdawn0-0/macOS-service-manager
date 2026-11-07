// Package devplatform manages local development environments. Binaries are
// app-owned archives or Homebrew kegs; long-lived processes belong to launchd.
package devplatform

import (
	"bytes"
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"syscall"
	"time"
)

const Schema = 2

var nameRE = regexp.MustCompile(`^[a-zA-Z0-9][a-zA-Z0-9._-]{0,79}$`)

type Package struct {
	Formula  string            `json:"formula,omitempty"` // Homebrew canonical full name; empty for app archives
	Name     string            `json:"name"`
	Version  string            `json:"version"`
	Arch     string            `json:"arch"`
	URL      string            `json:"url"`
	SHA256   string            `json:"sha256"`
	Format   string            `json:"format"` // tar.gz, zip, or homebrew
	Strip    int               `json:"strip,omitempty"`
	Binaries map[string]string `json:"binaries"`
	License  string            `json:"license"`
	MinMacOS string            `json:"minMacOS,omitempty"`
}

func (p Package) ID() string { return p.Name + "@" + p.Version + "-" + p.Arch }
func (p Package) Validate() error {
	if p.Arch != "arm64" && p.Arch != "universal" {
		return errors.New("managed software requires Apple Silicon (arm64); Intel packages are not supported")
	}
	if !nameRE.MatchString(p.Name) || !nameRE.MatchString(p.Version) || (p.Arch != "arm64" && p.Arch != "universal") {
		return errors.New("invalid package identity")
	}
	if p.Name == "php" || p.Name == "php-fpm" {
		return errors.New("PHP is not managed by this app")
	}
	if p.Name == "postgresql" && p.Formula == "" {
		return errors.New("PostgreSQL requires a Homebrew package")
	}
	u, e := url.Parse(p.URL)
	if e != nil || u.Scheme != "https" || u.Host == "" || u.User != nil {
		return errors.New("package URL must use HTTPS")
	}
	b, e := hex.DecodeString(p.SHA256)
	if e != nil || len(b) != 32 {
		return errors.New("package requires a SHA-256 checksum")
	}
	if p.Format == "homebrew" {
		if !validHomebrewPackage(p) {
			return errors.New("invalid Homebrew package source, version or binary layout")
		}
	} else if p.Formula != "" {
		return errors.New("Homebrew formula requires the Homebrew package format")
	} else if p.Format != "tar.gz" && p.Format != "zip" {
		return errors.New("unsupported archive format")
	}
	if p.Strip < 0 || p.Strip > 8 || len(p.Binaries) == 0 {
		return errors.New("invalid package layout")
	}
	for name, path := range p.Binaries {
		if !nameRE.MatchString(name) || !safeRelative(path) {
			return errors.New("invalid binary path")
		}
	}
	return nil
}

// processProbe carries only readiness data for a managed launchd job.
type processProbe struct {
	ID                      string
	Health                  string
	ReadinessTimeoutSeconds int
}

type Instance struct {
	DataIdentity  string `json:"dataIdentity,omitempty"`
	DataDirectory string `json:"dataDirectory,omitempty"`
	DataOwner     string `json:"dataOwner,omitempty"`
	ID            string `json:"id"`
	Kind          string `json:"kind"`
	Package       string `json:"package"`
	Port          int    `json:"port"`
	Persistent    bool   `json:"persistent"`
}
type State struct {
	UVExecutable        string                       `json:"uvExecutable,omitempty"`
	RuntimeDefaults     map[string]string            `json:"runtimeDefaults,omitempty"`
	ConnectionProfiles  map[string]ConnectionProfile `json:"connectionProfiles"`
	ProxyRules          []ProxyRule                  `json:"proxyRules"`
	ProxyListenerPort   int                          `json:"proxyListenerPort"`
	ProxyRulesRevision  uint64                       `json:"proxyRulesRevision"`
	PackageIntegrity    map[string]string            `json:"packageIntegrity,omitempty"`
	LogRotationDisabled bool                         `json:"logRotationDisabled,omitempty"`
	NodeDefault         string                       `json:"nodeDefault,omitempty"`
	SchemaVersion       int                          `json:"schemaVersion"`
	Packages            map[string]Package           `json:"packages"`
	Instances           map[string]Instance          `json:"instances"`
}

func emptyState() State {
	return State{SchemaVersion: Schema, Packages: map[string]Package{}, Instances: map[string]Instance{}, ConnectionProfiles: map[string]ConnectionProfile{}, ProxyRules: []ProxyRule{}, ProxyListenerPort: 443}
}

type Manager struct {
	brewCellar     string // Test fixture override; production uses Apple Silicon Homebrew's Cellar.
	initialization *initializationContext
	databaseWork   *databaseWorkContext
	operation      *operationWriter
	Program        string
	LoginDir       string
	Home           string // Test fixture override of the user's home folder.
	Context        context.Context
	Root           string
	Run            func(context.Context, string, ...string) (string, error)
	Event          func(string)
}

func DefaultRoot() string {
	if p := os.Getenv("MSM_DEV_HOME"); p != "" {
		return p
	}
	h, _ := os.UserHomeDir()
	return filepath.Join(h, "Library", "Application Support", "ServiceManager", "Development")
}
func New(root string) *Manager {
	return &Manager{Context: context.Background(), Root: root, Run: runCommand, Event: func(string) {}}
}
func (m *Manager) emit(s string) {
	if m.Event != nil {
		m.Event(s)
	}
}
func (m *Manager) init() error {
	if !filepath.IsAbs(m.Root) {
		return errors.New("storage root must be absolute")
	}
	for _, d := range []string{"", "packages", "downloads", "instances", "jobs", "logs", "proxy", "backups", "integrity", "package-locks"} {
		if e := os.MkdirAll(filepath.Join(m.Root, d), 0700); e != nil {
			return e
		}
	}
	return nil
}

// withState serializes complete mutations across GUI and CLI processes.
func (m *Manager) withState(ctx context.Context, fn func(*State) error) error {
	return m.withStateTransaction(ctx, func(s *State) (func() error, func(), error) { return nil, nil, fn(s) })
}

// Transaction hooks run while the state lock is held, including rollback when
// saving state fails. Ordinary callers do not need hooks.
func (m *Manager) withStateTransaction(ctx context.Context, fn func(*State) (func() error, func(), error)) error {
	if e := ctx.Err(); e != nil {
		return e
	}
	if e := m.init(); e != nil {
		return e
	}
	f, e := os.OpenFile(filepath.Join(m.Root, "state.lock"), os.O_CREATE|os.O_RDWR, 0600)
	if e != nil {
		return e
	}
	defer f.Close()
	waitingAnnounced := false
	for {
		e = syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB)
		if e == nil {
			break
		}
		if e != syscall.EWOULDBLOCK {
			return e
		}
		if !waitingAnnounced {
			m.emit("Waiting for another workspace change")
			waitingAnnounced = true
		}
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(100 * time.Millisecond):
		}
	}
	defer syscall.Flock(int(f.Fd()), syscall.LOCK_UN)
	if e = ctx.Err(); e != nil {
		return e
	}
	if m.operation != nil {
		m.operation.phase(operationWorkPhase(m.operation.record.Request.Kind))
	}
	s, e := m.Read()
	if e != nil {
		return e
	}
	undo, commit, e := fn(&s)
	if e == nil && undo != nil {
		e = ctx.Err()
	}
	if e == nil {
		e = writeJSON(filepath.Join(m.Root, "state.json"), s)
	}
	if e != nil {
		if undo != nil {
			e = errors.Join(e, undo())
		}
		return e
	}
	if commit != nil {
		commit()
	}
	return nil
}
func (m *Manager) Read() (State, error) {
	s := emptyState()
	b, e := os.ReadFile(filepath.Join(m.Root, "state.json"))
	if errors.Is(e, os.ErrNotExist) {
		return s, nil
	}
	if e != nil {
		return s, e
	}
	var decoded State
	reader := json.NewDecoder(bytes.NewReader(b))
	reader.DisallowUnknownFields()
	if e = reader.Decode(&decoded); e != nil {
		return s, fmt.Errorf("read state: %w", e)
	}
	var trailing any
	if e = reader.Decode(&trailing); e != io.EOF {
		return s, errors.New("read state: trailing data")
	}
	if decoded.SchemaVersion != Schema {
		return s, fmt.Errorf("unsupported state schema %d", decoded.SchemaVersion)
	}
	if decoded.Packages == nil || decoded.Instances == nil || decoded.ProxyRules == nil || decoded.ProxyListenerPort < 1 || decoded.ProxyListenerPort > 65535 {
		return s, errors.New("incomplete state")
	}
	if decoded.ConnectionProfiles == nil {
		decoded.ConnectionProfiles = map[string]ConnectionProfile{}
	}
	for id, profile := range decoded.ConnectionProfiles {
		instance, ok := decoded.Instances[profile.InstanceID]
		if !ok || id != profile.ID || validateConnectionProfile(profile, instance.Kind) != nil {
			return s, errors.New("invalid connection profile state")
		}
	}
	if _, e = normalizeProxyRules(decoded.ProxyListenerPort, decoded.ProxyRules); e != nil {
		return s, fmt.Errorf("invalid state proxy rules: %w", e)
	}
	s = decoded
	return s, nil
}
func writeJSON(path string, v any) error {
	b, e := json.MarshalIndent(v, "", "  ")
	if e != nil {
		return e
	}
	return atomicWrite(path, append(b, '\n'), 0600)
}
func atomicWrite(path string, b []byte, mode os.FileMode) error {
	if e := os.MkdirAll(filepath.Dir(path), 0700); e != nil {
		return e
	}
	f, e := os.CreateTemp(filepath.Dir(path), ".write-*")
	if e != nil {
		return e
	}
	defer os.Remove(f.Name())
	if e = f.Chmod(mode); e == nil {
		_, e = f.Write(b)
	}
	if e == nil {
		e = f.Sync()
	}
	ce := f.Close()
	if e != nil {
		return e
	}
	if ce != nil {
		return ce
	}
	return os.Rename(f.Name(), path)
}
func safeRelative(p string) bool {
	return p != "" && !filepath.IsAbs(p) && filepath.Clean(p) != ".." && !strings.HasPrefix(filepath.Clean(p), ".."+string(os.PathSeparator))
}
func randomID() string { var b [8]byte; _, _ = rand.Read(b[:]); return hex.EncodeToString(b[:]) }
func validID(s string) error {
	if !nameRE.MatchString(s) {
		return fmt.Errorf("invalid identifier %q", s)
	}
	return nil
}
func (m *Manager) executable() (string, error) {
	if m.Program != "" {
		if !filepath.IsAbs(m.Program) {
			return "", errors.New("application executable must be absolute")
		}
		return m.Program, nil
	}
	return os.Executable()
}

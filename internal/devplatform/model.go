// Package devplatform manages local development environments. Binaries are
// app-owned archives or Homebrew kegs; long-lived processes belong to launchd.
package devplatform

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"net/url"
	"os"
	"path/filepath"
	"reflect"
	"regexp"
	"slices"
	"strings"
	"syscall"
	"time"
)

const Schema = 1

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
	if (p.Name == "php" || p.Name == "postgresql") && p.Formula == "" {
		return errors.New("PHP and PostgreSQL require Homebrew packages")
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

type Service struct {
	LocalArguments          []LocalArgument   `json:"localArguments,omitempty"`
	RequiredEnvironment     []string          `json:"requiredEnvironment,omitempty"`
	NeedsEnvironmentFile    bool              `json:"needsEnvironmentFile,omitempty"`
	NeedsHealthReview       bool              `json:"needsHealthReview,omitempty"`
	EnvFile                 string            `json:"envFile,omitempty"`
	ID                      string            `json:"id"`
	Package                 string            `json:"package,omitempty"`
	Command                 []string          `json:"command"`
	Directory               string            `json:"directory,omitempty"`
	Environment             map[string]string `json:"environment,omitempty"`
	DependsOn               []string          `json:"dependsOn,omitempty"`
	Health                  string            `json:"health,omitempty"` // tcp://127.0.0.1:3000 or http://...
	Restart                 bool              `json:"restart,omitempty"`
	ReadinessTimeoutSeconds int               `json:"readinessTimeoutSeconds,omitempty"` // zero uses 30 seconds
	RestartThrottleSeconds  int               `json:"restartThrottleSeconds,omitempty"`  // zero uses 10 seconds
}

func (s Service) Validate() error {
	if e := validID(s.ID); e != nil {
		return e
	}
	if len(s.Command) == 0 || s.Command[0] == "" {
		return errors.New("service command is empty")
	}
	positions := map[int]bool{}
	for _, arg := range s.LocalArguments {
		if arg.Index < 0 || arg.Index >= len(s.Command) || positions[arg.Index] {
			return errors.New("invalid local argument position")
		}
		positions[arg.Index] = true
		if arg.Kind != "executable" && arg.Kind != "path" && arg.Kind != "secret" && arg.Kind != "text" {
			return errors.New("invalid local argument kind")
		}
		if len(arg.Label) > 160 || strings.ContainsAny(arg.Label, "\x00\r\n") {
			return errors.New("invalid local argument label")
		}
		if arg.Prefix != "" && arg.Prefix != "-p" && arg.Prefix != "-a" && (!strings.HasPrefix(arg.Prefix, "-") || !strings.HasSuffix(arg.Prefix, "=") || strings.ContainsAny(arg.Prefix, " /\\\x00\r\n")) {
			return errors.New("invalid local argument prefix")
		}
		if arg.Value != "" && s.Command[arg.Index] != arg.Prefix+arg.Value {
			return errors.New("local argument value does not match the command")
		}
	}
	for _, key := range s.RequiredEnvironment {
		if key == "" || strings.ContainsAny(key, "=\x00\r\n") {
			return errors.New("invalid required environment name")
		}
	}
	if s.ReadinessTimeoutSeconds < 0 || s.ReadinessTimeoutSeconds > 3600 || s.RestartThrottleSeconds < 0 || s.RestartThrottleSeconds > 3600 {
		return errors.New("service timeouts and restart throttle must be 1–3600 seconds, or zero for defaults")
	}
	if err := validateHealth(s.Health); err != nil {
		return err
	}
	if s.Directory != "" && !safeRelative(s.Directory) {
		return errors.New("service directory must be relative to project")
	}
	for k := range s.Environment {
		if k == "" || strings.ContainsAny(k, "=\x00") {
			return errors.New("invalid environment name")
		}
	}
	return nil
}

type Project struct {
	RuntimeHints      []RuntimeHint `json:"runtimeHints,omitempty"`
	DetectionWarnings []string      `json:"detectionWarnings,omitempty"`
	Autostart         bool          `json:"autostart,omitempty"`
	SchemaVersion     int           `json:"schemaVersion"`
	ID                string        `json:"id"`
	Name              string        `json:"name"`
	Directory         string        `json:"directory"`
	Services          []Service     `json:"services"`
	Instances         []string      `json:"instances,omitempty"`
	Sites             []Site        `json:"sites,omitempty"`
}
type Site struct {
	NeedsReview bool          `json:"needsReview,omitempty"`
	ID          string        `json:"id"`
	Domain      string        `json:"domain"`
	Upstream    string        `json:"upstream"`
	Port        int           `json:"port"` // browser HTTPS port
	Disabled    bool          `json:"disabled,omitempty"`
	Proxy       *ProxyOptions `json:"proxy,omitempty"`
}

type ProxyOptions struct {
	ConnectTimeoutSeconds        int    `json:"connectTimeoutSeconds"`
	ResponseHeaderTimeoutSeconds int    `json:"responseHeaderTimeoutSeconds"`
	HostPolicy                   string `json:"hostPolicy,omitempty"` // local or upstream
}

func (s Site) ProxyOptions() ProxyOptions {
	if s.Proxy == nil {
		return ProxyOptions{ConnectTimeoutSeconds: 10, ResponseHeaderTimeoutSeconds: 30, HostPolicy: "local"}
	}
	options := *s.Proxy
	if options.HostPolicy == "" {
		options.HostPolicy = "local"
	}
	return options
}

type Instance struct {
	ID         string `json:"id"`
	Kind       string `json:"kind"`
	Package    string `json:"package"`
	Port       int    `json:"port"`
	Persistent bool   `json:"persistent"`
}
type State struct {
	PackageIntegrity    map[string]string   `json:"packageIntegrity,omitempty"`
	LogRotationDisabled bool                `json:"logRotationDisabled,omitempty"`
	NodeDefault         string              `json:"nodeDefault,omitempty"`
	SchemaVersion       int                 `json:"schemaVersion"`
	Packages            map[string]Package  `json:"packages"`
	Projects            map[string]Project  `json:"projects"`
	Instances           map[string]Instance `json:"instances"`
	Sites               map[string]Site     `json:"sites"`
}

func emptyState() State {
	return State{SchemaVersion: Schema, Packages: map[string]Package{}, Projects: map[string]Project{}, Instances: map[string]Instance{}, Sites: map[string]Site{}}
}

type Manager struct {
	brewCellar     string // Test fixture override; production uses Apple Silicon Homebrew's Cellar.
	initialization *initializationContext
	databaseWork   *databaseWorkContext
	commandWork    *commandWorkContext
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
	if e = json.Unmarshal(b, &s); e != nil {
		return s, fmt.Errorf("read state: %w", e)
	}
	if s.SchemaVersion != Schema {
		return s, fmt.Errorf("unsupported state schema %d", s.SchemaVersion)
	}
	if s.Packages == nil || s.Projects == nil || s.Instances == nil || s.Sites == nil {
		return s, errors.New("incomplete state")
	}
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
func (s Site) Validate() error {
	if e := validID(s.ID); e != nil {
		return e
	}
	if s.Port < 1 || s.Port > 65535 {
		return errors.New("HTTPS port must be 1–65535")
	}
	options := s.ProxyOptions()
	if options.ConnectTimeoutSeconds < 0 || options.ConnectTimeoutSeconds > 3600 || options.ResponseHeaderTimeoutSeconds < 0 || options.ResponseHeaderTimeoutSeconds > 3600 {
		return errors.New("proxy timeouts must be 0–3600 seconds")
	}
	if options.HostPolicy != "local" && options.HostPolicy != "upstream" {
		return errors.New("host policy must be local or upstream")
	}
	if len(s.Domain) > 253 || !strings.HasSuffix(s.Domain, ".test") {
		return errors.New("use a local .test domain")
	}
	for _, part := range strings.Split(s.Domain, ".") {
		if !regexp.MustCompile(`^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$`).MatchString(part) {
			return errors.New("invalid domain")
		}
	}
	u, e := url.Parse(s.Upstream)
	if e != nil || u.User != nil || (u.Scheme != "http" && u.Scheme != "https") || u.Host == "" || u.RawQuery != "" || u.Fragment != "" || (u.Path != "" && u.Path != "/") {
		return errors.New("upstream must be an HTTP(S) origin")
	}
	host := u.Hostname()
	ip := net.ParseIP(host)
	if host != "localhost" && (ip == nil || !ip.IsLoopback()) {
		return errors.New("upstream must be a loopback address")
	}
	return nil
}
func (p Project) Validate() error {
	if p.SchemaVersion != Schema {
		return errors.New("unsupported project schema")
	}
	if e := validID(p.ID); e != nil {
		return e
	}
	if p.Name == "" || !filepath.IsAbs(p.Directory) {
		return errors.New("project needs a name and absolute directory")
	}
	seen := map[string]bool{}
	for _, s := range p.Services {
		if e := s.Validate(); e != nil {
			return e
		}
		if seen[s.ID] {
			return errors.New("duplicate service")
		}
		seen[s.ID] = true
	}
	if _, e := serviceOrder(p.Services); e != nil {
		return e
	}
	for _, s := range p.Sites {
		if e := s.Validate(); e != nil {
			return e
		}
	}
	return nil
}
func serviceOrder(services []Service) ([]Service, error) {
	by := map[string]Service{}
	for _, s := range services {
		by[s.ID] = s
	}
	marks := map[string]int{}
	var out []Service
	var visit func(string) error
	visit = func(id string) error {
		if marks[id] == 1 {
			return errors.New("service dependency cycle")
		}
		if marks[id] == 2 {
			return nil
		}
		s, ok := by[id]
		if !ok {
			return fmt.Errorf("unknown dependency %s", id)
		}
		marks[id] = 1
		for _, dep := range s.DependsOn {
			if e := visit(dep); e != nil {
				return e
			}
		}
		marks[id] = 2
		out = append(out, s)
		return nil
	}
	for _, s := range services {
		if e := visit(s.ID); e != nil {
			return nil, e
		}
	}
	return out, nil
}
func (m *Manager) SaveProject(ctx context.Context, p Project) error {
	if e := p.Validate(); e != nil {
		return e
	}
	keys := append(projectResourceKeys(p.ID), instanceResourceKeys(p.Instances)...)
	release, e := m.resourceLeases(ctx, keys...)
	if e != nil {
		return e
	}
	defer release()
	return m.withStateTransaction(ctx, func(s *State) (func() error, func(), error) {
		if old, ok := s.Projects[p.ID]; ok && !reflect.DeepEqual(old, p) {
			active, err := m.projectActiveKnown(ctx, old)
			if err != nil {
				return nil, nil, err
			}
			if active {
				return nil, nil, errors.New("stop the project before changing its settings")
			}
		}
		s.Projects[p.ID] = p
		if e := validateSites(allSites(s)); e != nil {
			return nil, nil, e
		}
		undo, e := m.snapshotLoginConfiguration(p.ID)
		if e != nil {
			return nil, nil, e
		}
		return undo, nil, m.configureLogin(ctx, p)
	})
}
func (m *Manager) Import(ctx context.Context, dir string) (Project, error) {
	if strings.TrimSpace(dir) == "" {
		return Project{}, errors.New("select an existing project directory")
	}
	if e := m.init(); e != nil {
		return Project{}, e
	}
	dir, e := filepath.Abs(dir)
	if e != nil {
		return Project{}, e
	}
	info, e := os.Stat(dir)
	if e != nil || !info.IsDir() {
		return Project{}, errors.New("select an existing project directory")
	}
	dir, e = canonicalProjectDirectory(dir)
	if e != nil {
		return Project{}, e
	}
	unlock, e := lockEnvironmentFiles(ctx, dir)
	if e != nil {
		return Project{}, e
	}
	defer unlock()
	if pending, _, err := readExportRecord(dir); err != nil {
		return Project{}, err
	} else if pending != nil {
		return Project{}, fmt.Errorf("export %s is unfinished; review export recovery before importing", pending.ID)
	}
	p := Project{SchemaVersion: Schema, ID: randomID(), Name: filepath.Base(dir), Directory: dir, Services: []Service{}}
	b, e := readProjectFile(dir, "devmanager.json", 8<<20)
	explicit := e == nil
	if e == nil {
		var header struct {
			SchemaVersion int `json:"schemaVersion"`
		}
		if e = json.Unmarshal(b, &header); e != nil {
			return p, e
		}
		if header.SchemaVersion == 2 {
			var shared sharedProject
			if e = json.Unmarshal(b, &shared); e != nil {
				return p, e
			}
			if e = verifySharedEnvironmentPair(dir, shared); e != nil {
				return p, e
			}
			p, e = m.materializeSharedProject(shared, dir, p.ID)
			if e != nil {
				_ = os.Remove(m.projectLockPath(p.ID))
				return p, e
			}
		} else {
			if e = json.Unmarshal(b, &p); e != nil {
				return p, e
			}
			p.Directory = dir
			p.ID = randomID()
			p.Autostart = false
			if e = p.Validate(); e != nil {
				return p, e
			}
			if e = m.importEnvironmentLock(&p); e != nil {
				return p, e
			}
		}
	} else if !errors.Is(e, os.ErrNotExist) {
		return p, e
	}
	inspection, inspectionError := InspectProject(dir)
	p.RuntimeHints = inspection.RuntimeHints
	p.DetectionWarnings = nil
	if inspectionError != nil {
		p.DetectionWarnings = []string{inspectionError.Error()}
	}
	if !explicit && inspectionError == nil {
		p.Services = inspection.Services
	}
	if e = m.SaveProject(ctx, p); e != nil {
		_ = os.Remove(m.projectLockPath(p.ID))
	}
	return p, e
}

func (s Site) URL() string {
	if s.Port == 443 {
		return "https://" + s.Domain
	}
	return fmt.Sprintf("https://%s:%d", s.Domain, s.Port)
}

// Clone separates editable UI drafts from snapshots passed to background work.
func (p Project) Clone() Project {
	p.RuntimeHints = slices.Clone(p.RuntimeHints)
	p.DetectionWarnings = slices.Clone(p.DetectionWarnings)
	p.Services = slices.Clone(p.Services)
	p.Instances = slices.Clone(p.Instances)
	p.Sites = slices.Clone(p.Sites)
	for n := range p.Sites {
		if p.Sites[n].Proxy != nil {
			options := *p.Sites[n].Proxy
			p.Sites[n].Proxy = &options
		}
	}
	for n := range p.Services {
		p.Services[n].LocalArguments = slices.Clone(p.Services[n].LocalArguments)
		p.Services[n].RequiredEnvironment = slices.Clone(p.Services[n].RequiredEnvironment)
		s := &p.Services[n]
		s.Command = slices.Clone(s.Command)
		s.DependsOn = slices.Clone(s.DependsOn)
		if s.Environment != nil {
			env := map[string]string{}
			for k, v := range s.Environment {
				env[k] = v
			}
			s.Environment = env
		}
	}
	return p
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

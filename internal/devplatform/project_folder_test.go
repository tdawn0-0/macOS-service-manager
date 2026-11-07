package devplatform

import (
	"bytes"
	"context"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"macos-service-manager/internal/launchd"
)

func folderFixture(t *testing.T) (*Manager, Project, string) {
	t.Helper()
	m := testManager(t)
	m.LoginDir = t.TempDir()
	from, to := t.TempDir(), t.TempDir()
	var err error
	to, err = canonicalProjectDirectory(to)
	if err != nil {
		t.Fatal(err)
	}
	p := Project{SchemaVersion: Schema, ID: "relocate", Name: "Relocation", Directory: from, Autostart: true, Instances: []string{"shared-database"}, Sites: []Site{{ID: "web", Domain: "relocate.test", Upstream: "http://127.0.0.1:3000", Port: 8443}}, Services: []Service{{ID: "web", Command: []string{filepath.Join(from, "bin", "app"), "--config=" + filepath.Join(from, "config.json"), filepath.Join(from, "data"), "--token=" + filepath.Join(from, "secret")}, EnvFile: filepath.Join(from, ".env.local"), Environment: map[string]string{"CONFIG_DIR": filepath.Join(from, "config")}, LocalArguments: []LocalArgument{{Index: 2, Kind: "path", Label: "Data directory", Value: filepath.Join(from, "data")}}}}}
	if err := m.SaveProject(context.Background(), p); err != nil {
		t.Fatal(err)
	}
	pkg, err := Resolve(context.Background(), "redis", "7.4.7")
	if err != nil {
		t.Fatal(err)
	}
	if err = m.withState(context.Background(), func(s *State) error {
		s.Packages[pkg.ID()] = pkg
		s.Instances["shared-database"] = Instance{ID: "shared-database", Kind: "redis", Package: pkg.ID(), Port: 6379, Persistent: true}
		return nil
	}); err != nil {
		t.Fatal(err)
	}
	data := filepath.Join(m.Root, "instances", "shared-database", "data")
	if err = os.MkdirAll(data, 0700); err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(filepath.Join(data, "sentinel"), []byte("preserved-database-data"), 0600); err != nil {
		t.Fatal(err)
	}
	return m, p, to
}
func TestProjectFolderPreviewRebasesOwnedPathsWithoutChangingIdentity(t *testing.T) {
	m, p, to := folderFixture(t)
	draft := p.Clone()
	draft.Directory = to
	preview, err := m.PreviewProjectFolder(context.Background(), draft)
	if err != nil {
		t.Fatal(err)
	}
	if len(preview.Changes) != 4 || len(preview.Warnings) != 2 {
		t.Fatal("incomplete path preview", preview)
	}
	for _, change := range preview.Changes {
		if strings.Contains(change.From, "secret") {
			t.Fatal("secret appears in folder preview", preview)
		}
	}
	if err = m.ApplyProjectFolder(context.Background(), draft, preview.Fingerprint); err != nil {
		t.Fatal(err)
	}
	s, err := New(m.Root).Read()
	if err != nil {
		t.Fatal(err)
	}
	got := s.Projects[p.ID]
	data, err := os.ReadFile(filepath.Join(m.Root, "instances", "shared-database", "data", "sentinel"))
	if err != nil || string(data) != "preserved-database-data" || s.Instances["shared-database"].Port != 6379 {
		t.Fatal("relocation changed independent database data or metadata", err)
	}
	if got.ID != p.ID || got.Instances[0] != p.Instances[0] || got.Sites[0] != p.Sites[0] || got.Services[0].EnvFile != filepath.Join(to, ".env.local") || got.Services[0].Command[0] != filepath.Join(to, "bin", "app") || got.Services[0].Command[1] != "--config="+filepath.Join(to, "config.json") || got.Services[0].LocalArguments[0].Value != filepath.Join(to, "data") {
		t.Fatal("relocation lost private configuration", got)
	}
	if got.Services[0].Command[3] != p.Services[0].Command[3] || got.Services[0].Environment["CONFIG_DIR"] != p.Services[0].Environment["CONFIG_DIR"] {
		t.Fatal("relocation guessed a secret or environment value")
	}
	spec, err := launchd.LoadSpec(m.loginPath(p.ID))
	if err != nil || spec.WorkingDir != to || !got.Autostart {
		t.Fatal("login launcher did not follow project", spec, err)
	}
}
func TestProjectFolderRejectsRunningOrUnobservedJobs(t *testing.T) {
	m, p, to := folderFixture(t)
	draft := p.Clone()
	draft.Directory = to
	for _, answer := range []struct {
		output string
		err    error
	}{{"state = running", nil}, {"", errors.New("launchctl observation denied")}} {
		m.Run = func(context.Context, string, ...string) (string, error) { return answer.output, answer.err }
		if _, err := m.PreviewProjectFolder(context.Background(), draft); err == nil {
			t.Fatal("unverified project can change folder")
		}
	}
}
func TestProjectFolderRejectsStaleStateAndReplacedDestination(t *testing.T) {
	for _, kind := range []string{"project", "destination"} {
		t.Run(kind, func(t *testing.T) {
			m, p, to := folderFixture(t)
			draft := p.Clone()
			draft.Directory = to
			preview, err := m.PreviewProjectFolder(context.Background(), draft)
			if err != nil {
				t.Fatal(err)
			}
			if kind == "project" {
				p.Name = "Changed elsewhere"
				if err = m.SaveProject(context.Background(), p); err != nil {
					t.Fatal(err)
				}
			} else {
				if err = os.Rename(to, to+"-old"); err != nil {
					t.Fatal(err)
				}
				if err = os.Mkdir(to, 0700); err != nil {
					t.Fatal(err)
				}
			}
			if err = m.ApplyProjectFolder(context.Background(), draft, preview.Fingerprint); err == nil {
				t.Fatal("stale relocation accepted")
			}
			s, _ := m.Read()
			if s.Projects[p.ID].Directory != p.Directory {
				t.Fatal("failed relocation changed state")
			}
		})
	}
}
func TestProjectFolderStateFailureRestoresLoginPlist(t *testing.T) {
	m, p, to := folderFixture(t)
	draft := p.Clone()
	draft.Directory = to
	preview, err := m.PreviewProjectFolder(context.Background(), draft)
	if err != nil {
		t.Fatal(err)
	}
	before, err := os.ReadFile(m.loginPath(p.ID))
	if err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(m.Root, "state.json")
	backup := filepath.Join(m.Root, "state.backup")
	t.Cleanup(func() { _ = os.Remove(path); _ = os.Rename(backup, path) })
	injected := false
	m.Run = func(context.Context, string, ...string) (string, error) {
		if !injected {
			if err := os.Rename(path, backup); err != nil {
				t.Fatal(err)
			}
			if err := os.Mkdir(path, 0700); err != nil {
				t.Fatal(err)
			}
			injected = true
		}
		return "", errors.New("Could not find service")
	}
	if err = m.ApplyProjectFolder(context.Background(), draft, preview.Fingerprint); err == nil {
		t.Fatal("state publication failure accepted")
	}
	after, err := os.ReadFile(m.loginPath(p.ID))
	if err != nil || !bytes.Equal(before, after) {
		t.Fatal("login plist not rolled back", err)
	}
}
func TestProjectFolderUnavailableBlocksBeforeResourcesStart(t *testing.T) {
	m, p, _ := folderFixture(t)
	if err := os.RemoveAll(p.Directory); err != nil {
		t.Fatal(err)
	}
	m.Run = func(context.Context, string, ...string) (string, error) {
		t.Fatal("started or queried a resource before checking missing project folder")
		return "", nil
	}
	if err := m.Up(context.Background(), p.ID); err == nil || !strings.Contains(err.Error(), "folder is unavailable") {
		t.Fatal(err)
	}
}

func TestProjectFolderDiagnosticReportsMissingLocationWithoutExportingPaths(t *testing.T) {
	m := testManager(t)
	s := emptyState()
	s.Projects["missing"] = Project{ID: "missing", Name: "Moved project", Directory: filepath.Join(t.TempDir(), "missing")}
	r, err := m.diagnose(context.Background(), s, diagnosticProbes{})
	if err != nil {
		t.Fatal(err)
	}
	check := findDiagnostic(t, r, "Project folder", "")
	if check.Status != "Failed" || !strings.Contains(check.Remedy, "working folder") {
		t.Fatal("missing project is not actionable", check)
	}
	b, err := r.RedactedJSON()
	if err != nil || strings.Contains(string(b), s.Projects["missing"].Directory) {
		t.Fatal("missing-folder report disclosed a path", err)
	}
}

func TestProjectSiteSuggestionUsesLocalEnvironmentAndAvoidsAllDeclarations(t *testing.T) {
	s := emptyState()
	s.Projects["app"] = Project{ID: "app", Name: "Atlas API", Directory: "/projects/atlas", Services: []Service{{Health: "tcp://127.0.0.1:49172"}}, Sites: []Site{{ID: "web", Domain: "atlas-api.test", Port: 8443}}}
	s.Sites["disabled"] = Site{ID: "disabled", Domain: "atlas-api-2.test", Port: 443, Disabled: true}
	site, err := SuggestProjectSite(s, "app")
	if err != nil {
		t.Fatal(err)
	}
	if site.ID != "web-2" || site.Domain != "atlas-api-3.test" || site.Port != 8443 || site.Upstream != "http://127.0.0.1:49172" {
		t.Fatal(site)
	}
	if err = site.Validate(); err != nil {
		t.Fatal(err)
	}
	p := s.Projects["app"]
	p.Name = "开发环境"
	p.Directory = "/开发/项目"
	p.Services = []Service{{Environment: map[string]string{"PORT": "45678"}}}
	s.Projects["app"] = p
	site, err = SuggestProjectSite(s, "app")
	if err != nil || site.Domain != "project-app.test" || site.Upstream != "http://127.0.0.1:45678" {
		t.Fatal(site, err)
	}
}

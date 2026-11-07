package devplatform

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
)

func TestSharedEnvironmentPrivateSettingsAndIndependentPorts(t *testing.T) {
	m := testManager(t)
	dir := t.TempDir()
	p := Project{SchemaVersion: Schema, ID: "source", Name: "Shared project", Directory: dir,
		Services: []Service{{ID: "web", Package: "node@22.14.0-arm64", Command: []string{"node", "server.js", "--port", "38471", "--config=/Users/private/config.json", "--token=private-token", "--expression=process.env.SECRET;demo.js"}, Environment: map[string]string{"PORT": "38471", "SECRET": "private-env"}, EnvFile: "/Users/private/project.env", Health: "http://127.0.0.1:38471"}},
		Sites:    []Site{{ID: "web", Domain: "sharing.test", Port: 38472, Upstream: "http://127.0.0.1:38471"}}}
	// Export only needs a registered descriptor; import must not run or install it.
	pkg := Package{Name: "node", Version: "22.14.0", Arch: "arm64", URL: "https://nodejs.org/example.tar.gz", Format: "tar.gz", Binaries: map[string]string{"node": "bin/node"}, SHA256: strings.Repeat("a", 64)}
	if err := m.withState(context.Background(), func(s *State) error { s.Projects[p.ID] = p; s.Packages[pkg.ID()] = pkg; return nil }); err != nil {
		t.Fatal(err)
	}
	if err := m.ExportProject(context.Background(), p.ID); err != nil {
		t.Fatal(err)
	}
	b, err := os.ReadFile(filepath.Join(dir, "devmanager.json"))
	if err != nil {
		t.Fatal(err)
	}
	for _, private := range []string{"/Users/private", "38471", "38472", "private-token", "private-env", "process.env.SECRET"} {
		if strings.Contains(string(b), private) {
			t.Fatalf("shared file contains private setting %q: %s", private, b)
		}
	}
	var shared sharedProject
	if err := json.Unmarshal(b, &shared); err != nil {
		t.Fatal(err)
	}
	if shared.SchemaVersion != 2 || len(shared.Services[0].LocalArguments) != 3 || !shared.Services[0].NeedsEnvironmentFile {
		t.Fatalf("missing local definitions: %+v", shared)
	}
	if shared.Services[0].LocalArguments[1].Kind != "secret" || shared.Services[0].LocalArguments[1].Prefix != "--token=" {
		t.Fatal("secret argument lost its type or prefix")
	}
	m.Run = func(context.Context, string, ...string) (string, error) {
		t.Fatal("import ran a command")
		return "", nil
	}
	var copies []Project
	for range 2 {
		copy, err := m.Import(context.Background(), dir)
		if err != nil {
			t.Fatal(err)
		}
		copies = append(copies, copy)
		svc := copy.Services[0]
		port := addressPort(svc.Health)
		if port == 38471 || svc.Command[3] != strconv.Itoa(port) || svc.Environment["PORT"] != strconv.Itoa(port) || addressPort(copy.Sites[0].Upstream) != port {
			t.Fatalf("port bindings differ: %+v", copy)
		}
		if copy.Sites[0].Port != 38472 || copy.Sites[0].Domain == "sharing.test" {
			t.Fatal("did not reuse local listener or isolate domain", copy.Sites)
		}
		if len(copy.MissingLocalSettings()) != 5 || copy.requireLocalSettings(context.Background()) == nil {
			t.Fatal("incomplete local setup can start", copy.MissingLocalSettings())
		}
	}
	if copies[0].ID == copies[1].ID || addressPort(copies[0].Services[0].Health) == addressPort(copies[1].Services[0].Health) || copies[0].Sites[0].Domain == copies[1].Sites[0].Domain {
		t.Fatal("working copies share identity or addresses")
	}
	s, _ := m.Read()
	if s.Projects[p.ID].Services[0].Environment["SECRET"] != "private-env" {
		t.Fatal("export mutated original")
	}
}

func TestSharedEnvironmentSecretArgumentClassification(t *testing.T) {
	for _, command := range [][]string{{"redis-cli", "-aprivate"}, {"redis-cli", "-a", "private"}, {"mysql", "-pprivate"}, {"node", "--token=/private/secret"}, {"node", "--token", "private"}} {
		p := Project{Services: []Service{{ID: "web", Package: "managed", Command: command}}}
		d, err := makeSharedProject(p, &State{}, &EnvironmentLock{})
		if err != nil {
			t.Fatal(err)
		}
		args := d.Services[0].LocalArguments
		if len(args) != 1 || args[0].Kind != "secret" {
			t.Fatalf("secret field not masked: %+v", args)
		}
		b, _ := json.Marshal(d)
		if strings.Contains(string(b), "private") {
			t.Fatal("secret leaked", string(b))
		}
	}
}

func TestSharedEnvironmentUnmanagedUpstreamNeedsReview(t *testing.T) {
	p := Project{Sites: []Site{{ID: "web", Domain: "external.test", Port: 443, Upstream: "http://127.0.0.1:3000"}}}
	d, err := makeSharedProject(p, &State{}, &EnvironmentLock{})
	if err != nil {
		t.Fatal(err)
	}
	if !d.Sites[0].NeedsReview {
		t.Fatal("external upstream was silently rebound")
	}
	p.Sites[0].NeedsReview = true
	if sites := allSites(&State{Projects: map[string]Project{"project": p}}); len(sites) != 0 {
		t.Fatal("pending site entered proxy routes")
	}
}

func TestSharedEnvironmentRejectsUnknownPortAndInjectedLocalValues(t *testing.T) {
	m := testManager(t)
	d := sharedProject{SchemaVersion: 2, Name: "Import", Services: []sharedService{{Service: Service{ID: "web", Command: []string{"/private/injected"}, LocalArguments: []LocalArgument{{Index: 0, Kind: "executable", Label: "Executable", Value: "/private/injected"}}, Environment: map[string]string{"SECRET": "injected"}}}}}
	p, err := m.materializeSharedProject(d, t.TempDir(), "new-project")
	if err != nil {
		t.Fatal(err)
	}
	if p.Services[0].LocalArguments[0].Value != "" || len(p.Services[0].Environment) != 0 || p.Services[0].Command[0] != LocalArgumentPlaceholder(0) {
		t.Fatal("import trusted shared private overrides", p)
	}
	d.Services[0].Health = "tcp://127.0.0.1:{{port:unknown}}"
	if _, err := m.materializeSharedProject(d, t.TempDir(), "new-project"); err == nil {
		t.Fatal("unknown reference accepted")
	}
}

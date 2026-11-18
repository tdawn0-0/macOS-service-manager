package devcli

import (
	"errors"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"devbase/internal/launchd"
	"devbase/internal/service"
)

type fakeServiceProvider struct {
	src        service.Source
	items      []service.Item
	detail     map[string]service.Detail
	actions    map[string][]service.Action
	confirm    map[service.Action]string
	done       []string
	savedRoot  []string
	loadResult error
}

func (f *fakeServiceProvider) Source() service.Source { return f.src }
func (f *fakeServiceProvider) List() ([]service.Item, error) {
	return slices.Clone(f.items), nil
}
func (f *fakeServiceProvider) Detail(it service.Item) (service.Detail, error) {
	if d, ok := f.detail[it.ID]; ok {
		return d, nil
	}
	return service.Detail{Rows: []service.Row{{Group: "Info", Key: "Name", Value: it.Name}}}, nil
}
func (f *fakeServiceProvider) Actions(it service.Item) []service.Action {
	if a, ok := f.actions[it.ID]; ok {
		return a
	}
	return []service.Action{service.Start, service.Stop, service.Load, service.Unload, service.Remove}
}
func (f *fakeServiceProvider) Confirm(_ service.Item, a service.Action) string {
	if f.confirm != nil {
		return f.confirm[a]
	}
	if a == service.Remove {
		return "Remove item?"
	}
	return ""
}
func (f *fakeServiceProvider) Do(it service.Item, a service.Action) error {
	f.done = append(f.done, it.ID+":"+string(a))
	return f.loadResult
}
func (f *fakeServiceProvider) SaveAsRoot(s launchd.Spec, existing string, load bool) (string, error) {
	f.savedRoot = append(f.savedRoot, s.Label)
	return "/Library/LaunchDaemons/" + s.Label + ".plist", nil
}

type fakeLoginAdder struct {
	added []string
}

func (f *fakeLoginAdder) Add(path string) error {
	if !strings.HasSuffix(path, ".app") {
		return errors.New("expected .app path")
	}
	f.added = append(f.added, path)
	return nil
}

func TestRunCLIServicesListInspectActionsAndJobSave(t *testing.T) {
	_, env, _ := setupIsolatedEnv(t)

	uaPlist := filepath.Join(env.UserAgentDir, "com.example.worker.plist")
	uaSpec := launchd.Spec{Label: "com.example.worker", Args: []string{"/bin/echo", "hello"}, RunAtLoad: true}
	savedPath, err := launchd.Save(env.UserAgentDir, uaSpec, "")
	if err != nil {
		t.Fatal(err)
	}
	if savedPath != uaPlist {
		t.Fatalf("unexpected saved path %q", savedPath)
	}

	uaProvider := &fakeServiceProvider{
		src: service.UserAgent,
		items: []service.Item{
			{
				ID:        "user-agent:" + uaPlist,
				Source:    service.UserAgent,
				Name:      "com.example.worker",
				State:     service.StateRunning,
				PID:       4242,
				Autostart: service.AutostartOn,
				Path:      uaPlist,
			},
		},
		confirm: map[service.Action]string{service.Unload: "Stop and unload job?", service.Remove: "Delete plist?"},
	}
	brewProvider := &fakeServiceProvider{
		src: service.Brew,
		items: []service.Item{
			{ID: "brew:postgresql@17", Source: service.Brew, Name: "postgresql@17", State: service.StateStopped},
		},
	}
	daemonProvider := &fakeServiceProvider{src: service.Daemon}
	loginAdder := &fakeLoginAdder{}

	env.LoginAdder = loginAdder
	env.ServiceProviders = func() map[service.Source]service.Provider {
		return map[service.Source]service.Provider{
			service.UserAgent: uaProvider,
			service.Brew:      brewProvider,
			service.Daemon:    daemonProvider,
		}
	}

	code, listOut, errOut := runCLIWithTestEnv(env, "", "services", "list", "--source", "user-agent", "--state", "running", "--query", "worker", "--json")
	if code != 0 || errOut != "" || !strings.Contains(listOut, `"com.example.worker"`) || strings.Contains(listOut, `"postgresql@17"`) {
		t.Fatalf("services list failed: %d out=%s err=%s", code, listOut, errOut)
	}

	code, inspOut, errOut := runCLIWithTestEnv(env, "", "services", "inspect", "com.example.worker", "--json")
	if code != 0 || errOut != "" || !strings.Contains(inspOut, `"Label": "com.example.worker"`) {
		t.Fatalf("services inspect failed: %d out=%s err=%s", code, inspOut, errOut)
	}

	// Unload requires --yes because Confirm returns non-empty.
	code, _, _ = runCLIWithTestEnv(env, "", "services", "unload", "com.example.worker")
	if code != 2 {
		t.Fatalf("services unload without --yes expected exit 2, got %d", code)
	}
	code, _, errOut = runCLIWithTestEnv(env, "", "services", "unload", "com.example.worker", "--yes", "--json")
	if code != 0 || errOut != "" || !slices.Contains(uaProvider.done, "user-agent:"+uaPlist+":unload") {
		t.Fatalf("services unload --yes failed: %d err=%s done=%v", code, errOut, uaProvider.done)
	}

	// Add login item.
	code, addOut, errOut := runCLIWithTestEnv(env, "", "services", "add-login-item", "/Applications/Test.app", "--json")
	if code != 0 || errOut != "" || !strings.Contains(addOut, `"/Applications/Test.app"`) || !slices.Equal(loginAdder.added, []string{"/Applications/Test.app"}) {
		t.Fatalf("services add-login-item failed: %d out=%s err=%s", code, addOut, errOut)
	}

	// Save new user-agent via flags inside isolated env.UserAgentDir.
	code, saveOut, errOut := runCLIWithTestEnv(env, "", "services", "job-save", "--source", "user-agent", "--label", "com.example.cli", "--command", "echo hi", "--run-at-load", "--env", "FOO=bar", "--load", "--json")
	if code != 0 || errOut != "" || !strings.Contains(saveOut, `"com.example.cli"`) {
		t.Fatalf("services job-save user-agent failed: %d out=%s err=%s", code, saveOut, errOut)
	}
	expectedNewPlist := filepath.Join(env.UserAgentDir, "com.example.cli.plist")
	if _, err := os.Stat(expectedNewPlist); err != nil {
		t.Fatalf("expected new plist in isolated UserAgentDir %s: %v", expectedNewPlist, err)
	}
	if !slices.Contains(uaProvider.done, ":load") {
		t.Fatalf("expected user-agent provider Load called, got %v", uaProvider.done)
	}

	// Save system daemon via --json-stdin using rootSaver mock.
	code, daemonOut, errOut := runCLIWithTestEnv(env, `{"Label":"com.example.daemon","Args":["/usr/bin/true"],"RunAtLoad":true}`, "services", "job-save", "--source", "daemon", "--json-stdin", "--load", "--json")
	if code != 0 || errOut != "" || !strings.Contains(daemonOut, `"com.example.daemon"`) {
		t.Fatalf("services job-save daemon failed: %d out=%s err=%s", code, daemonOut, errOut)
	}
	if !slices.Equal(daemonProvider.savedRoot, []string{"com.example.daemon"}) {
		t.Fatalf("expected SaveAsRoot called for daemon, got %v", daemonProvider.savedRoot)
	}
}

package devcli

import (
	"bytes"
	"context"
	"strings"
	"testing"
)

// Node.js versions are managed in the window; the command line keeps only
// what the shell setup calls.
func TestNodeCommandLineKeepsOnlyTheShellHook(t *testing.T) {
	t.Setenv("MSM_DEV_HOME", t.TempDir())
	run := func(args ...string) (string, error) {
		var out bytes.Buffer
		err := Run(context.Background(), append([]string{"node"}, args...), &out)
		return out.String(), err
	}
	for _, args := range [][]string{{"list"}, {"install", "22"}, {"default", "22"}, {"shell", "enable"}, {}} {
		if _, err := run(args...); err == nil || !strings.Contains(err.Error(), "in the window") {
			t.Errorf("%v: %v", args, err)
		}
	}
	if out, err := run("path", t.TempDir()); err != nil || out != "" {
		t.Fatalf("path must stay silent on stdout when nothing is installed: %v %q", err, out)
	}
	var help bytes.Buffer
	if err := Run(context.Background(), []string{"help"}, &help); err != nil || strings.Contains(help.String(), "node <command>") {
		t.Fatalf("%v: the help lists a node command", err)
	}
}

// The shipped app answers only to what its own jobs and shell setup start.
func TestShippedAppHasNoCommandLineForPeople(t *testing.T) {
	t.Setenv("MSM_DEV_HOME", t.TempDir())
	for _, args := range [][]string{{}, {"help"}, {"install", "node", "22.14.0"}, {"import", "/tmp"}, {"status"}, {"doctor"}, {"exec", "p", "node"}, {"trust"}, {"db", "start", "x"}, {"node", "install", "22"}} {
		var out bytes.Buffer
		err := RunInternal(context.Background(), args, &out)
		if err == nil || out.Len() != 0 {
			t.Errorf("%v: %v %q", args, err, out.String())
		}
	}
	for name := range internalCommands {
		if name == "node" {
			continue
		}
		// Reaching the command (and failing for lack of arguments) shows it is allowed.
		if err := RunInternal(context.Background(), []string{name}, &bytes.Buffer{}); err != nil && strings.Contains(err.Error(), "no command line") {
			t.Errorf("%s is refused", name)
		}
	}
}

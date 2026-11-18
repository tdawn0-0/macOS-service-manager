package devcli

import (
	"bytes"
	"context"
	"strings"
	"testing"
)

// The shell setup calls node path on every cd; with nothing installed it must
// print nothing, so the shell keeps its own PATH.
func TestNodePathIsSilentWhenNothingIsInstalled(t *testing.T) {
	t.Setenv("MSM_DEV_HOME", t.TempDir())
	var out bytes.Buffer
	if err := Run(context.Background(), []string{"node", "path", t.TempDir()}, &out); err != nil || out.Len() != 0 {
		t.Fatalf("%v %q", err, out.String())
	}
}

// The shipped app answers only to what its own jobs and shell setup start.
func TestShippedAppHasNoCommandLineForPeople(t *testing.T) {
	t.Setenv("MSM_DEV_HOME", t.TempDir())
	// The commands only cmd/devmgr reaches, and anything else a person types.
	for _, args := range [][]string{{}, {"help"}, {"status"}, {"install", "node", "22.14.0"}, {"log-rotation", "on"}, {"node", "install", "22"}, {"unknown"}} {
		var out bytes.Buffer
		err := RunInternal(context.Background(), args, &out)
		if err == nil || out.Len() != 0 {
			t.Errorf("%v: %v %q", args, err, out.String())
		}
	}
	// What launchd jobs start. Reaching the command (and failing for lack of
	// arguments) shows it is allowed.
	for _, name := range []string{"serve", "tunnel", "maintain", "database-worker"} {
		if err := RunInternal(context.Background(), []string{name}, &bytes.Buffer{}); err != nil && strings.Contains(err.Error(), "no command line") {
			t.Errorf("%s is refused", name)
		}
	}
}

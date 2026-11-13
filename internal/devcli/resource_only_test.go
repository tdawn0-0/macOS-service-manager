package devcli

import (
	"bytes"
	"context"
	"strings"
	"testing"
)

func TestInternalResourceOnlyCommands(t *testing.T) {
	for _, command := range []string{"up", "command-worker", "import", "project", "site", "prepare", "exec", "export"} {
		t.Run(command, func(t *testing.T) {
			var out bytes.Buffer
			if err := RunInternal(context.Background(), []string{command}, &out); err == nil || !strings.Contains(err.Error(), "no command line") {
				t.Fatalf("retired command accepted: %v", err)
			}
			if out.Len() != 0 {
				t.Fatalf("retired command emitted output: %q", out.String())
			}
		})
	}
	for _, command := range []string{"serve", "tunnel", "maintain", "database-worker", "node"} {
		t.Run("dispatch-"+command, func(t *testing.T) {
			var out bytes.Buffer
			err := RunInternal(context.Background(), []string{command}, &out)
			if err != nil && strings.Contains(err.Error(), "no command line") {
				t.Fatal("allowed internal command rejected", err)
			}
		})
	}
}

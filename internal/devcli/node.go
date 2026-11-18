package devcli

import (
	"context"
	"devbase/internal/devplatform"
	"errors"
	"fmt"
	"io"
	"os"
)

// runNode is the one entry the shell setup of the Node.js manager calls on
// each cd. Everything a person does with Node.js versions is in the window.
func runNode(ctx context.Context, m *devplatform.Manager, args []string, out io.Writer) error {
	if len(args) < 1 || args[0] != "path" {
		return errors.New("node versions are managed in the window: Software, Node.js")
	}
	// A missing version warns on stderr and leaves PATH alone; it never fails
	// a prompt.
	dir, _ := os.Getwd()
	if len(args) > 1 {
		dir = args[1]
	}
	sel, e := m.SelectNode(dir, "")
	if e != nil {
		var missing *devplatform.NodeMissingError
		if errors.As(e, &missing) || errors.Is(e, devplatform.ErrNoNodeDefault) {
			fmt.Fprintln(os.Stderr, e)
			return nil
		}
		return e
	}
	if sel.System {
		return nil
	}
	fmt.Fprintf(out, "%s\n%s\n", sel.Path(), sel.Prefix)
	return nil
}

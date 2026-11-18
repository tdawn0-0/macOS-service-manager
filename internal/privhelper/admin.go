//go:build darwin

package privhelper

import (
	"fmt"
	"strings"
	"time"

	"devbase/internal/system"
)

// adminTimeout bounds a command as root, which waits for the user to
// authenticate.
const adminTimeout = 5 * time.Minute

// runOsascript runs osascript for runAdmin. It is a variable so that tests
// can replace it.
var runOsascript = func(script string) (string, error) {
	return system.RunFor(adminTimeout, "osascript", "-e", script)
}

// runAdmin runs a command as root, after macOS asks the user to
// authenticate. Only installing and removing the helper itself does this:
// everything else that needs root asks the helper, so no other package can
// reach a password prompt.
func runAdmin(name string, args ...string) (string, error) {
	out, err := runOsascript(adminScript(name, args...))
	if err != nil && strings.Contains(err.Error(), "User canceled") {
		return "", fmt.Errorf("authorization canceled")
	}
	return out, err
}

// adminScript is the AppleScript that runs a command as root.
func adminScript(name string, args ...string) string {
	parts := make([]string, 0, len(args)+1)
	for _, p := range append([]string{name}, args...) {
		parts = append(parts, system.ShellQuote(p))
	}
	return "do shell script " + system.AppleScriptString(strings.Join(parts, " ")) + " with administrator privileges"
}

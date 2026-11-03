// Package system runs the macOS commands the app depends on.
package system

import (
	"bytes"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

func init() {
	// An app started from Finder gets a minimal PATH that lacks Homebrew's
	// directories, so add them.
	path := os.Getenv("PATH")
	for _, dir := range []string{"/opt/homebrew/bin", "/usr/local/bin"} {
		if !strings.Contains(":"+path+":", ":"+dir+":") {
			path += ":" + dir
		}
	}
	_ = os.Setenv("PATH", strings.TrimPrefix(path, ":"))
}

// Run runs a command and returns its standard output. It is a variable so
// that tests can replace it. A command that fails returns an error holding
// its standard error.
var Run = run

func run(name string, args ...string) (string, error) {
	var stdout, stderr bytes.Buffer
	cmd := exec.Command(name, args...)
	cmd.Stdout, cmd.Stderr = &stdout, &stderr
	if err := cmd.Run(); err != nil {
		if msg := strings.TrimSpace(stderr.String()); msg != "" {
			return "", fmt.Errorf("%s", msg)
		}
		if msg := strings.TrimSpace(stdout.String()); msg != "" {
			return "", fmt.Errorf("%s", msg)
		}
		return "", fmt.Errorf("%s: %w", name, err)
	}
	return stdout.String(), nil
}

// OpenInConsole opens a log file in Console.app.
func OpenInConsole(path string) error {
	info, err := os.Stat(path)
	if err != nil {
		return fmt.Errorf("log file does not exist: %s", path)
	}
	if !info.Mode().IsRegular() {
		return fmt.Errorf("path is not a file: %s", path)
	}
	abs, err := filepath.Abs(path)
	if err != nil {
		return fmt.Errorf("resolve path: %w", err)
	}
	if _, err := Run("open", "-a", "Console", abs); err != nil {
		return fmt.Errorf("open log file: %w", err)
	}
	return nil
}

// Package system runs the macOS commands the app depends on.
package system

import (
	"bytes"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"
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

// RunAdmin runs a command as root, after macOS asks the user to
// authenticate. It is a variable so that tests can replace it.
var RunAdmin = runAdmin

func runAdmin(name string, args ...string) (string, error) {
	out, err := Run("osascript", "-e", AdminScript(name, args...))
	if err != nil && strings.Contains(err.Error(), "User canceled") {
		return "", fmt.Errorf("authorization canceled")
	}
	return out, err
}

// AdminScript is the AppleScript that runs a command as root.
func AdminScript(name string, args ...string) string {
	parts := make([]string, 0, len(args)+1)
	for _, p := range append([]string{name}, args...) {
		parts = append(parts, ShellQuote(p))
	}
	return "do shell script " + AppleScriptString(strings.Join(parts, " ")) + " with administrator privileges"
}

// ShellQuote quotes s as one word of a shell command.
func ShellQuote(s string) string { return "'" + strings.ReplaceAll(s, "'", `'\''`) + "'" }

// AppleScriptString quotes s as an AppleScript string literal.
func AppleScriptString(s string) string {
	s = strings.ReplaceAll(s, `\`, `\\`)
	s = strings.ReplaceAll(s, `"`, `\"`)
	return `"` + s + `"`
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

// OpenLoginItemsSettings opens System Settings at its login items.
func OpenLoginItemsSettings() error {
	_, err := Run("open", "x-apple.systempreferences:com.apple.LoginItems-Settings.extension")
	return err
}

// Trash moves a file to the user's Trash.
func Trash(path string) error {
	home, err := os.UserHomeDir()
	if err != nil {
		return err
	}
	dir := filepath.Join(home, ".Trash")
	dest := filepath.Join(dir, filepath.Base(path))
	if _, err := os.Lstat(dest); err == nil {
		ext := filepath.Ext(dest)
		dest = strings.TrimSuffix(dest, ext) + time.Now().Format(" 15.04.05") + ext
	}
	return os.Rename(path, dest)
}

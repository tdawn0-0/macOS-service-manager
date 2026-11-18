package devcli

import (
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"

	"devbase/internal/launchd"
)

const cliManagedMarker = "# Managed by Devbase CLI"

// CLIInstallStatus describes the installed public devbase CLI wrapper script.
type CLIInstallStatus struct {
	Installed        bool   `json:"installed"`
	WrapperPath      string `json:"wrapperPath"`
	TargetExecutable string `json:"targetExecutable,omitempty"`
	InPATH           bool   `json:"inPath"`
	ManagedByDevbase bool   `json:"managedByDevbase"`
}

// DefaultCLIBinDir returns the user directory where the devbase CLI wrapper is installed.
// When MSM_DEV_HOME is set, it stays isolated inside MSM_DEV_HOME/bin.
func DefaultCLIBinDir() string {
	if envHome := os.Getenv("MSM_DEV_HOME"); envHome != "" {
		return filepath.Join(envHome, "bin")
	}
	home, _ := os.UserHomeDir()
	return filepath.Join(home, ".local", "bin")
}

func cliShellQuote(s string) string {
	return "'" + strings.ReplaceAll(s, "'", "'\\''") + "'"
}

func appBundleForExecutable(exe string) string {
	dir := filepath.Dir(exe)
	if filepath.Base(dir) != "MacOS" {
		return ""
	}
	contents := filepath.Dir(dir)
	if filepath.Base(contents) != "Contents" {
		return ""
	}
	if _, err := os.Stat(filepath.Join(contents, "Info.plist")); err != nil {
		return ""
	}
	return filepath.Dir(contents)
}

func isDirInPATH(binDir string) bool {
	if binDir == "" {
		return false
	}
	cleanTarget := filepath.Clean(binDir)
	resolvedTarget, targetErr := filepath.EvalSymlinks(cleanTarget)
	for _, entry := range filepath.SplitList(os.Getenv("PATH")) {
		if entry == "" {
			continue
		}
		cleanEntry := filepath.Clean(entry)
		if cleanEntry == cleanTarget {
			return true
		}
		if targetErr == nil {
			if resolvedEntry, err := filepath.EvalSymlinks(cleanEntry); err == nil && resolvedEntry == resolvedTarget {
				return true
			}
		}
	}
	return false
}

func readBoundedRegularFile(path string, limit int64) ([]byte, os.FileInfo, error) {
	info, err := os.Lstat(path)
	if err != nil {
		return nil, nil, err
	}
	if !info.Mode().IsRegular() {
		return nil, info, errors.New("not a regular file")
	}
	f, err := os.Open(path)
	if err != nil {
		return nil, info, err
	}
	defer f.Close()
	data, err := io.ReadAll(io.LimitReader(f, limit+1))
	if err != nil {
		return nil, info, err
	}
	if int64(len(data)) > limit {
		return nil, info, errors.New("file exceeds limit")
	}
	return data, info, nil
}

// CLIStatus inspects whether a devbase CLI wrapper is installed in binDir.
func CLIStatus(binDir, appPath, exeOverride string) CLIInstallStatus {
	if binDir == "" {
		binDir = DefaultCLIBinDir()
	}
	wrapperPath := filepath.Join(binDir, "devbase")
	targetExe, _ := launchd.ResolveLoginExecutable(appPath, exeOverride)
	st := CLIInstallStatus{
		WrapperPath:      wrapperPath,
		TargetExecutable: targetExe,
		InPATH:           isDirInPATH(binDir),
	}
	if data, _, err := readBoundedRegularFile(wrapperPath, 64<<10); err == nil {
		st.ManagedByDevbase = strings.Contains(string(data), cliManagedMarker)
	}
	if info, err := os.Stat(wrapperPath); err == nil && info.Mode().IsRegular() && info.Mode().Perm()&0o111 != 0 {
		st.Installed = true
	}
	return st
}

// InstallCLIWrapper installs or updates the managed devbase wrapper script in binDir.
func InstallCLIWrapper(binDir, appPath, exeOverride string) (CLIInstallStatus, error) {
	if binDir == "" {
		binDir = DefaultCLIBinDir()
	}
	if !filepath.IsAbs(binDir) {
		return CLIInstallStatus{}, errors.New("CLI directory must be an absolute path")
	}
	targetExe, err := launchd.ResolveLoginExecutable(appPath, exeOverride)
	if err != nil {
		return CLIInstallStatus{}, err
	}
	wrapperPath := filepath.Join(binDir, "devbase")
	if data, info, err := readBoundedRegularFile(wrapperPath, 64<<10); err == nil {
		if !strings.Contains(string(data), cliManagedMarker) {
			return CLIInstallStatus{}, fmt.Errorf("%s already exists and is not managed by Devbase", wrapperPath)
		}
	} else if !errors.Is(err, os.ErrNotExist) {
		if info != nil && !info.Mode().IsRegular() {
			return CLIInstallStatus{}, fmt.Errorf("%s already exists and is not a regular file", wrapperPath)
		}
		return CLIInstallStatus{}, fmt.Errorf("cannot inspect existing %s: %w", wrapperPath, err)
	}

	bundlePath := appPath
	if bundlePath == "" {
		bundlePath = appBundleForExecutable(targetExe)
	}

	script := fmt.Sprintf(`#!/bin/sh
%s
set -eu
TARGET=%s
BUNDLE=%s
if [ "$#" -eq 0 ]; then
  set -- help
fi
if [ "$1" = "gui" ] || [ "$1" = "open" ]; then
  shift
  if [ -n "$BUNDLE" ] && [ -d "$BUNDLE" ]; then
    if [ "$#" -gt 0 ]; then
      exec /usr/bin/open "$BUNDLE" --args "$@"
    fi
    exec /usr/bin/open "$BUNDLE"
  fi
  if [ -z "${MSM_DEV_HOME:-}" ] && [ -d "/Applications/Devbase.app" ]; then
    if [ "$#" -gt 0 ]; then
      exec /usr/bin/open "/Applications/Devbase.app" --args "$@"
    fi
    exec /usr/bin/open "/Applications/Devbase.app"
  fi
fi
if [ -x "$TARGET" ]; then
  exec "$TARGET" "$@"
fi
if [ -z "${MSM_DEV_HOME:-}" ] && [ -x "/Applications/Devbase.app/Contents/MacOS/Devbase" ]; then
  exec "/Applications/Devbase.app/Contents/MacOS/Devbase" "$@"
fi
echo "Devbase executable not found at $TARGET" >&2
exit 1
`, cliManagedMarker, cliShellQuote(targetExe), cliShellQuote(bundlePath))

	if err := os.MkdirAll(binDir, 0o755); err != nil {
		return CLIInstallStatus{}, err
	}
	tmp, err := os.CreateTemp(binDir, ".devbase-cli-*")
	if err != nil {
		return CLIInstallStatus{}, err
	}
	tmpPath := tmp.Name()
	cleanup := true
	defer func() {
		if cleanup {
			_ = os.Remove(tmpPath)
		}
	}()
	if _, err := tmp.WriteString(script); err != nil {
		_ = tmp.Close()
		return CLIInstallStatus{}, err
	}
	if err := tmp.Close(); err != nil {
		return CLIInstallStatus{}, err
	}
	if err := os.Chmod(tmpPath, 0o755); err != nil {
		return CLIInstallStatus{}, err
	}
	if err := os.Rename(tmpPath, wrapperPath); err != nil {
		return CLIInstallStatus{}, err
	}
	cleanup = false
	return CLIStatus(binDir, appPath, exeOverride), nil
}

// UninstallCLIWrapper removes the managed devbase wrapper script from binDir.
func UninstallCLIWrapper(binDir string) error {
	if binDir == "" {
		binDir = DefaultCLIBinDir()
	}
	wrapperPath := filepath.Join(binDir, "devbase")
	data, info, err := readBoundedRegularFile(wrapperPath, 64<<10)
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	if err != nil {
		if info != nil && !info.Mode().IsRegular() {
			return fmt.Errorf("%s is not managed by Devbase", wrapperPath)
		}
		return fmt.Errorf("cannot inspect %s: %w", wrapperPath, err)
	}
	if !strings.Contains(string(data), cliManagedMarker) {
		return fmt.Errorf("%s is not managed by Devbase", wrapperPath)
	}
	return os.Remove(wrapperPath)
}

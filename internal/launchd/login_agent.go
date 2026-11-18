package launchd

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"

	"howett.net/plist"
)

// LoginAgentLabel is the LaunchAgent label used when Devbase starts quietly in the menu bar at login.
const LoginAgentLabel = "xyz.jyeu.devbase.login"

// LoginAgentPlistPath returns the property list path inside dir for the login LaunchAgent.
func LoginAgentPlistPath(dir string) string {
	if dir == "" {
		return ""
	}
	return filepath.Join(dir, LoginAgentLabel+".plist")
}

// LoginAgentTarget returns the gui/<uid>/<label> launchd domain target.
func LoginAgentTarget() string {
	return fmt.Sprintf("gui/%d/%s", os.Getuid(), LoginAgentLabel)
}

// ResolveLoginExecutable returns the executable path recorded in the login LaunchAgent.
func ResolveLoginExecutable(appPath, override string) (string, error) {
	if override != "" {
		if !filepath.IsAbs(override) {
			return "", errors.New("executable path must be absolute")
		}
		return override, nil
	}
	if appPath != "" {
		if data, err := os.ReadFile(filepath.Join(appPath, "Contents", "Info.plist")); err == nil {
			var info struct {
				Executable string `plist:"CFBundleExecutable"`
			}
			if _, err := plist.Unmarshal(data, &info); err == nil && info.Executable != "" {
				candidate := filepath.Join(appPath, "Contents", "MacOS", info.Executable)
				if st, err := os.Stat(candidate); err == nil && st.Mode().IsRegular() {
					return candidate, nil
				}
			}
		}
	}
	exe, err := os.Executable()
	if err != nil {
		return "", err
	}
	if resolved, err := filepath.EvalSymlinks(exe); err == nil && resolved != "" {
		exe = resolved
	}
	if !filepath.IsAbs(exe) {
		return "", errors.New("executable path must be absolute")
	}
	return exe, nil
}

// LaunchAtLoginEnabled reports whether the login LaunchAgent plist file exists in dir.
func LaunchAtLoginEnabled(dir string) bool {
	path := LoginAgentPlistPath(dir)
	if path == "" {
		return false
	}
	info, err := os.Stat(path)
	return err == nil && info.Mode().IsRegular()
}

// EnableLaunchAtLogin writes the login LaunchAgent plist in dir for exe with --background.
func EnableLaunchAtLogin(dir, exe string, runner func(...string) error) error {
	path := LoginAgentPlistPath(dir)
	if path == "" {
		return errors.New("login agent directory is not configured")
	}
	if exe == "" || !filepath.IsAbs(exe) {
		return errors.New("executable path must be absolute")
	}
	spec := Spec{
		Label:     LoginAgentLabel,
		Args:      []string{exe, "--background"},
		RunAtLoad: true,
	}
	data, err := spec.Marshal(nil)
	if err != nil {
		return err
	}
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	tmp, err := os.CreateTemp(dir, ".devbase-login-*.plist")
	if err != nil {
		return err
	}
	tmpPath := tmp.Name()
	cleanup := true
	defer func() {
		if cleanup {
			_ = os.Remove(tmpPath)
		}
	}()
	if _, err := tmp.Write(data); err != nil {
		_ = tmp.Close()
		return err
	}
	if err := tmp.Close(); err != nil {
		return err
	}
	if err := os.Chmod(tmpPath, 0o644); err != nil {
		return err
	}
	if err := os.Rename(tmpPath, path); err != nil {
		return err
	}
	cleanup = false
	if runner != nil {
		_ = runner("enable", LoginAgentTarget())
	}
	return nil
}

// DisableLaunchAtLogin removes the login LaunchAgent plist from dir and boots it out via runner.
func DisableLaunchAtLogin(dir string, runner func(...string) error) error {
	path := LoginAgentPlistPath(dir)
	if path == "" {
		return errors.New("login agent directory is not configured")
	}
	if err := os.Remove(path); err != nil && !os.IsNotExist(err) {
		return err
	}
	if runner != nil {
		_ = runner("bootout", LoginAgentTarget())
	}
	return nil
}

package main

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"

	"howett.net/plist"

	"devbase/internal/launchd"
	"devbase/internal/system"
)

const loginAgentLabel = "xyz.jyeu.devbase.login"

func loginAgentPlistPath(dir string) string {
	if dir == "" {
		return ""
	}
	return filepath.Join(dir, loginAgentLabel+".plist")
}

func loginAgentTarget() string {
	return fmt.Sprintf("gui/%d/%s", os.Getuid(), loginAgentLabel)
}

func runLaunchctl(args ...string) error {
	_, err := system.Run("launchctl", args...)
	return err
}

// resolveLoginExecutable returns the binary path recorded in the login LaunchAgent.
func resolveLoginExecutable(appPath, override string) (string, error) {
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

func launchAtLoginEnabled(dir string) bool {
	path := loginAgentPlistPath(dir)
	if path == "" {
		return false
	}
	info, err := os.Stat(path)
	return err == nil && info.Mode().IsRegular()
}

func enableLaunchAtLogin(dir, exe string, runner func(...string) error) error {
	path := loginAgentPlistPath(dir)
	if path == "" {
		return errors.New("login agent directory is not configured")
	}
	if exe == "" || !filepath.IsAbs(exe) {
		return errors.New("executable path must be absolute")
	}
	spec := launchd.Spec{
		Label:     loginAgentLabel,
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
		_ = runner("enable", loginAgentTarget())
	}
	return nil
}

func disableLaunchAtLogin(dir string, runner func(...string) error) error {
	path := loginAgentPlistPath(dir)
	if path == "" {
		return errors.New("login agent directory is not configured")
	}
	if err := os.Remove(path); err != nil && !os.IsNotExist(err) {
		return err
	}
	if runner != nil {
		_ = runner("bootout", loginAgentTarget())
	}
	return nil
}

func (a *app) setLaunchAtLogin(enabled bool) {
	if a.loginBusy {
		return
	}
	dir := a.loginAgentDir
	appPath := a.appPath
	override := a.loginProgram
	runner := a.loginRunner
	a.loginBusy = true
	a.spawn(func() {
		var err error
		if enabled {
			var exe string
			if exe, err = resolveLoginExecutable(appPath, override); err == nil {
				err = enableLaunchAtLogin(dir, exe, runner)
			}
		} else {
			err = disableLaunchAtLogin(dir, runner)
		}
		actual := launchAtLoginEnabled(dir)
		a.post(func() {
			a.loginBusy = false
			a.loginAtLogin = actual
			if err != nil {
				a.toast("Could not update login item: %s", err)
				return
			}
			if enabled {
				a.toast("Devbase will start at login in the menu bar")
			} else {
				a.toast("Start at login disabled")
			}
		})
	})
}

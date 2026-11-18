package main

import (
	"devbase/internal/launchd"
	"devbase/internal/system"
)

const loginAgentLabel = launchd.LoginAgentLabel

func loginAgentPlistPath(dir string) string {
	return launchd.LoginAgentPlistPath(dir)
}

func loginAgentTarget() string {
	return launchd.LoginAgentTarget()
}

func runLaunchctl(args ...string) error {
	_, err := system.Run("launchctl", args...)
	return err
}

// resolveLoginExecutable returns the binary path recorded in the login LaunchAgent.
func resolveLoginExecutable(appPath, override string) (string, error) {
	return launchd.ResolveLoginExecutable(appPath, override)
}

func launchAtLoginEnabled(dir string) bool {
	return launchd.LaunchAtLoginEnabled(dir)
}

func enableLaunchAtLogin(dir, exe string, runner func(...string) error) error {
	return launchd.EnableLaunchAtLogin(dir, exe, runner)
}

func disableLaunchAtLogin(dir string, runner func(...string) error) error {
	return launchd.DisableLaunchAtLogin(dir, runner)
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

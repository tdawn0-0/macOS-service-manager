package main

import (
	"context"
	"errors"
	"fmt"

	"devbase/internal/devplatform"
	"devbase/internal/privhelper"
	"devbase/internal/service"
	"devbase/internal/system"
)

func (a *app) helperReady() bool {
	return a.helperStatus.State == privhelper.StateInstalled
}

func (a *app) refreshHelperStatus() {
	a.spawn(func() {
		st := privhelper.Default().Status()
		a.post(func() {
			a.helperStatus = st
		})
	})
}

func (a *app) refreshSettings() {
	if a.settingsBusy {
		return
	}
	a.settingsBusy = true
	providers := make([]service.Restricted, 0, len(a.providers))
	for _, p := range a.providers {
		if r, ok := p.(service.Restricted); ok {
			providers = append(providers, r)
		}
	}
	mgr := a.devManager()
	root := mgr.Root
	a.spawn(func() {
		st := privhelper.Default().Status()
		needsAccess := false
		for _, r := range providers {
			if r.NeedsAccess() {
				needsAccess = true
				break
			}
		}
		cacheInfo, _ := mgr.DownloadCacheInfo()
		a.post(func() {
			a.settingsBusy = false
			a.settingsLoaded = true
			a.helperStatus = st
			a.fdaNeedsAccess = needsAccess
			a.workspaceRoot = root
			a.cacheInfo = cacheInfo
		})
	})
}

func (a *app) clearDownloadCache() {
	if a.dev.busy || a.packageTasksRunning() || a.cacheInfo.Files == 0 {
		return
	}
	a.ask = askState{
		open:    true,
		title:   "Clear download cache?",
		message: "Removes cached installation archives. Installed runtimes and database engines are not changed.",
		button:  "Clear Cache",
		run: func() {
			var info devplatform.DownloadCacheInfo
			a.devTaskThen(func(m *devplatform.Manager) (string, error) {
				removed, err := m.ClearDownloadCache(m.Context)
				if refreshed, infoErr := m.DownloadCacheInfo(); infoErr == nil {
					info = refreshed
				}
				if err != nil {
					return "", err
				}
				if removed.Files == 0 {
					return "Download cache is already empty.", nil
				}
				return fmt.Sprintf("Cleared %s (%s) from download cache.", plural(removed.Files, "archive"), formatBackupSize(removed.Bytes)), nil
			}, func(err error) {
				a.cacheInfo = info
				if err == nil && a.dev.packagePreview != nil {
					a.dev.packagePreview.Cached = false
				}
			})
		},
	}
}

func (a *app) openFullDiskAccess() {
	a.spawn(func() {
		if err := system.OpenFullDiskAccessSettings(); err != nil {
			a.post(func() { a.toast("%s", err) })
		}
	})
}

func (a *app) setLogRotation(enabled bool) {
	a.devTask(func(m *devplatform.Manager) (string, error) {
		msg := "Automatic log rotation disabled."
		if enabled {
			msg = "Automatic log rotation enabled."
		}
		return msg, m.SetLogRotation(m.Context, enabled)
	})
}

func (a *app) installHelper() {
	a.installHelperThen(nil)
}

func (a *app) installHelperThen(after func()) {
	if a.helperBusy {
		return
	}
	a.helperBusy = true
	a.spawn(func() {
		err := privhelper.Default().Install(context.Background())
		st := privhelper.Default().Status()
		a.post(func() {
			a.helperBusy = false
			a.helperStatus = st
			if err != nil {
				a.toast("Could not install privileged helper: %s", err)
				return
			}
			a.toast("Privileged helper installed")
			if after != nil {
				after()
			}
		})
	})
}

// helperProblem asks the user to install or reinstall the privileged helper
// when err says the helper is missing or refuses this build, and reports
// whether it did. Nothing about a helper failure is left silent.
func (a *app) helperProblem(err error, retry func()) bool {
	switch {
	case errors.Is(err, privhelper.ErrNotInstalled):
		a.promptInstallHelper(retry)
	case privhelper.NeedsReinstall(err):
		a.promptReinstallHelper(retry)
	default:
		return false
	}
	return true
}

// promptReinstallHelper asks to install the helper again: the installed one
// was set up by another Devbase build, so it refuses this build's requests.
func (a *app) promptReinstallHelper(retry func()) {
	a.ask = askState{
		open:    true,
		title:   "Reinstall the privileged helper?",
		message: "The installed helper was set up by a different Devbase build, so it refuses requests from this one and nothing that needs administrator rights can run. Reinstalling asks for your administrator password once. Your settings and data are not changed.",
		button:  "Reinstall Helper",
		run:     func() { a.installHelperThen(retry) },
	}
}

func (a *app) promptInstallHelper(retry func()) {
	a.ask = askState{
		open:    true,
		title:   "Privileged helper required",
		message: "Root operations, such as local HTTPS on port 443 and system launchd jobs, run through the Devbase privileged helper. Installing it asks for your administrator password once.",
		button:  "Install Helper",
		run:     func() { a.installHelperThen(retry) },
	}
}

func (a *app) uninstallHelper() {
	if a.helperBusy {
		return
	}
	a.ask = askState{
		open:    true,
		title:   "Remove privileged helper?",
		message: "Privileged operations like local HTTPS port 443 and system launchd jobs require the privileged helper and will remain unavailable until you install it again.",
		button:  "Remove Helper",
		run:     a.runUninstallHelper,
	}
}

func (a *app) runUninstallHelper() {
	if a.helperBusy {
		return
	}
	a.helperBusy = true
	a.spawn(func() {
		err := privhelper.Default().Uninstall(context.Background())
		st := privhelper.Default().Status()
		a.post(func() {
			a.helperBusy = false
			a.helperStatus = st
			if err != nil {
				a.toast("Could not remove privileged helper: %s", err)
				return
			}
			a.toast("Privileged helper removed")
		})
	})
}

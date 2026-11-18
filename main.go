// Command devbase manages development software, databases, local HTTPS and
// macOS services in a MyGo window and from the menu bar.
package main

import (
	"context"
	"devbase/internal/devcli"
	"log"
	"os"
	"os/signal"
	"slices"
	"strings"
	"syscall"

	"github.com/egoist/mygo"
	"github.com/egoist/mygo/ui"

	"devbase/internal/system"
)

// The smallest window the layout fits.
const minWidth, minHeight = 900, 520

func startHiddenInBackground(args []string) bool {
	return slices.Contains(args, "--background")
}

func shouldRunPublicCLI(args []string) ([]string, bool) {
	filtered := make([]string, 0, len(args))
	for _, a := range args {
		if strings.HasPrefix(a, "-psn_") {
			continue
		}
		filtered = append(filtered, a)
	}
	if len(filtered) == 0 {
		return nil, false
	}
	if filtered[0] == "gui" || filtered[0] == "open" {
		return nil, false
	}
	onlyGUILaunchArgs := true
	for i := 0; i < len(filtered); i++ {
		a := filtered[i]
		if a == "--background" {
			continue
		}
		if strings.HasPrefix(a, "-NS") || strings.HasPrefix(a, "-Apple") {
			if !strings.Contains(a, "=") && i+1 < len(filtered) && !strings.HasPrefix(filtered[i+1], "-") {
				i++
			}
			continue
		}
		onlyGUILaunchArgs = false
		break
	}
	if onlyGUILaunchArgs {
		return nil, false
	}
	return filtered, true
}

func configureBackgroundLaunch(a *app, background bool) {
	if background {
		a.showDockIcon(false)
	}
}

func main() {
	if len(os.Args) > 1 && os.Args[1] == "--dev" {
		ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
		defer cancel()
		if e := devcli.RunInternal(ctx, os.Args[2:], os.Stdout); e != nil {
			log.Print(e)
			os.Exit(1)
		}
		return
	}

	if cliArgs, ok := shouldRunPublicCLI(os.Args[1:]); ok {
		ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
		defer cancel()
		os.Exit(devcli.RunCLI(ctx, cliArgs, os.Stdin, os.Stdout, os.Stderr))
	}

	background := startHiddenInBackground(os.Args[1:])
	a := newApp()
	// The Dock icon follows the windows: the menu bar carries the app when
	// they are closed.
	a.dockOn = func(on bool) {
		if on {
			mygo.App.Dock.Show()
			return
		}
		mygo.App.Dock.Hide()
	}
	configureBackgroundLaunch(a, background)
	a.copy = mygo.Clipboard.WriteText
	a.reveal = mygo.Shell.ShowItemInFolder
	system.Trash = mygo.Shell.TrashItem
	a.chooseApp = func() (string, bool) {
		paths, err := mygo.Dialog.Open(mygo.OpenDialogOptions{
			Title:       "Choose an app to open at login",
			DefaultPath: "/Applications",
			Filters:     []mygo.FileFilter{{Name: "Apps", Extensions: []string{"app"}}},
		})
		if err != nil || len(paths) == 0 {
			return "", false
		}
		return paths[0], true
	}

	quitting := false
	mygo.App.WhenReady(func() {
		win := mygo.NewWindow(mygo.WindowOptions{
			Title:     "Devbase",
			Width:     1120,
			Height:    720,
			MinWidth:  minWidth,
			MinHeight: minHeight,
			Hidden:    background,
			// The whole window is frosted glass over the desktop; the
			// sidebar goes up under the window controls, and the title bar
			// is the top of each pane.
			TitleBarStyle:  mygo.TitleBarHiddenInset,
			TitleBarHeight: barHeight,
			Vibrancy:       mygo.VibrancyUnderWindow,
			// Opens where the user left it last time.
			StateKey: "main",
			Content:  ui.View(a.view),
		})
		if background {
			win.Hide()
			a.showDockIcon(false)
		}
		// The size kept from an older version is restored as it is, even
		// below the minimum, where the layout does not fit.
		if width, height := win.Size(); width < minWidth || height < minHeight {
			win.SetSize(max(width, minWidth), max(height, minHeight))
		}

		// The app stays in the menu bar when its window is closed, and only
		// there: no window, no Dock icon.
		win.OnClose(func(e *mygo.CloseEvent) {
			if !quitting {
				e.PreventDefault()
				win.Hide()
				a.showDockIcon(false)
			}
		})
		mygo.App.OnActivate(func(hasVisibleWindows bool) {
			if !hasVisibleWindows {
				a.showDockIcon(true)
				win.Show()
			}
			if a.post != nil {
				a.post(a.windowShown)
			}
		})

		guide := &guideWindow{a: a}
		a.showGuide, a.hideGuide, a.joinGuide, a.redrawGuide = guide.show, guide.close, guide.join, guide.update
		a.appPath = bundlePath()

		bar := &menubar{a: a, win: win}
		if err := bar.install(); err != nil {
			log.Print("menu bar: ", err)
		}
		// Whatever background work changes, the menu bar shows too.
		a.post = func(f func()) {
			win.Update(func() {
				f()
				bar.update()
				guide.update()
			})
		}
		a.quitApp = mygo.App.Quit
		a.showQuit = bar.showWindow
		mygo.App.OnBeforeQuit(func(e *mygo.QuitEvent) {
			if a.quit.allow {
				quitting = true
				return
			}
			e.PreventDefault()
			a.post(a.requestQuit)
		})
		a.refreshHelperStatus()
		a.refreshNavigation()
	})
	if err := mygo.App.Run(); err != nil {
		log.Fatal(err)
	}
}

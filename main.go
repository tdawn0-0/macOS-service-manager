// Command macos-service-manager manages project environments, local HTTPS and
// macOS services in a MyGo window and from the menu bar.
package main

import (
	"context"
	"log"
	"macos-service-manager/internal/devcli"
	"os"
	"os/signal"
	"path/filepath"
	"syscall"

	"github.com/egoist/mygo"
	"github.com/egoist/mygo/ui"

	"macos-service-manager/internal/system"
)

// The smallest window the layout fits.
const minWidth, minHeight = 900, 520

func main() {
	if len(os.Args) > 1 && os.Args[1] == "--dev" {
		ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
		defer cancel()
		if e := devcli.Run(ctx, os.Args[2:], os.Stdout); e != nil {
			log.Print(e)
			os.Exit(1)
		}
		return
	}

	a := newApp()
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
			Title:     "Service Manager",
			Width:     1120,
			Height:    720,
			MinWidth:  minWidth,
			MinHeight: minHeight,
			// The sidebar goes up under the window controls, over the
			// desktop's material; the title bar is the top of each pane.
			TitleBarStyle:  mygo.TitleBarHiddenInset,
			TitleBarHeight: barHeight,
			Vibrancy:       mygo.VibrancySidebar,
			// Opens where the user left it last time.
			StateKey: "main",
			Content:  ui.View(a.view),
		})
		// The size kept from an older version is restored as it is, even
		// below the minimum, where the layout does not fit.
		if width, height := win.Size(); width < minWidth || height < minHeight {
			win.SetSize(max(width, minWidth), max(height, minHeight))
		}

		// The app stays in the menu bar when its window is closed.
		win.OnClose(func(e *mygo.CloseEvent) {
			if !quitting {
				e.PreventDefault()
				win.Hide()
			}
		})
		mygo.App.OnActivate(func(hasVisibleWindows bool) {
			if !hasVisibleWindows {
				win.Show()
			}
		})

		guide := &guideWindow{a: a}
		a.showGuide, a.hideGuide, a.joinGuide, a.redrawGuide = guide.show, guide.close, guide.join, guide.update
		a.appPath = bundlePath()
		if dir, err := mygo.App.Path(mygo.PathUserData); err == nil {
			a.welcomedPath = filepath.Join(dir, "welcomed")
		}

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
		if a.nav != "development" {
			a.welcome()
		}
		a.refreshNavigation()
	})
	if err := mygo.App.Run(); err != nil {
		log.Fatal(err)
	}
}

// Command macos-service-manager manages Homebrew services and the startup
// items of macOS (launch agents and daemons, login items) in a window drawn
// by MyGo, and from the menu bar.
package main

import (
	"log"

	"github.com/egoist/mygo"
	"github.com/egoist/mygo/ui"

	"macos-service-manager/internal/system"
)

func main() {
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
	mygo.App.OnBeforeQuit(func(*mygo.QuitEvent) { quitting = true })
	mygo.App.WhenReady(func() {
		win := mygo.NewWindow(mygo.WindowOptions{
			Title:     "Service Manager",
			Width:     1120,
			Height:    720,
			MinWidth:  900,
			MinHeight: 520,
			// The sidebar goes up under the window controls, over the
			// desktop's material; the title bar is the top of each pane.
			TitleBarStyle:  mygo.TitleBarHiddenInset,
			TitleBarHeight: barHeight,
			Vibrancy:       mygo.VibrancySidebar,
			// Opens where the user left it last time.
			StateKey: "main",
			Content:  ui.View(a.view),
		})

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

		bar := &menubar{a: a, win: win}
		if err := bar.install(); err != nil {
			log.Print("menu bar: ", err)
		}
		// Whatever background work changes, the menu bar shows too.
		a.post = func(f func()) {
			win.Update(func() {
				f()
				bar.update()
			})
		}
		a.refreshAll()
	})
	if err := mygo.App.Run(); err != nil {
		log.Fatal(err)
	}
}

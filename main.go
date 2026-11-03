// Command macos-service-manager manages Homebrew services and the startup
// items of macOS (launch agents and daemons, login items) in a window drawn
// by MyGo, and from the menu bar.
package main

import (
	"log"

	"github.com/egoist/mygo"
	"github.com/egoist/mygo/ui"
)

func main() {
	a := newApp()
	a.copy = mygo.Clipboard.WriteText
	a.reveal = mygo.Shell.ShowItemInFolder
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
			Width:     1080,
			Height:    680,
			MinWidth:  860,
			MinHeight: 480,
			// Opens where the user left it last time.
			StateKey: "main",
			Content:  ui.View(a.view),
		})
		a.post = win.Update

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
		a.changed = bar.update
		a.refreshAll()
	})
	if err := mygo.App.Run(); err != nil {
		log.Fatal(err)
	}
}

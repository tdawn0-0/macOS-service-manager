// Command macos-service-manager manages Homebrew services and launch agents
// in a window drawn by MyGo.
package main

import (
	"log"

	"github.com/egoist/mygo"
)

func main() {
	a := newApp()
	a.copy = mygo.Clipboard.WriteText
	mygo.App.WhenReady(func() {
		win := mygo.NewWindow(mygo.WindowOptions{
			Title:     "MacOS Service Manager",
			Width:     800,
			Height:    600,
			MinWidth:  560,
			MinHeight: 400,
			// Opens where the user left it last time.
			StateKey: "main",
			Content:  a.viewContent(),
		})
		a.post = win.Update
		a.refreshAll()
	})
	if err := mygo.App.Run(); err != nil {
		log.Fatal(err)
	}
}

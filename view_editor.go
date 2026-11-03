package main

import (
	"github.com/egoist/mygo/ui"
)

// editorDialog is the dialog that makes and edits launch agents.
func (a *app) editorDialog(c *ui.Context) {
	e := &a.editor
	t := c.Theme()
	title, subtitle := "New Launch Agent", "A job that launchd runs for you, in ~/Library/LaunchAgents."
	if e.existing != "" {
		title, subtitle = "Edit Launch Agent", e.existing
	}
	// The form scrolls, leaving room for the title and the buttons.
	_, windowHeight := c.Size()
	ui.Modal(c, &e.open, func() {
		ui.Column(c).Width(600).Gap(14).Children(func() {
			ui.Column(c).Gap(2).Children(func() {
				ui.Text(c, title).FontSize(16).FontWeight(650)
				ui.Text(c, subtitle).FontSize(textSmall).TextColor(t.TextMuted).SingleLine()
			})
			ui.Scroll(c).MaxHeight(min(470, windowHeight-170)).Children(func() {
				ui.Form(c, func() {
					ui.Fieldset(c, "Program", func() {
						ui.Field(c, "Label", func() {
							ui.TextInput(c, &e.label).Placeholder("com.example.my-job").ReadOnly(e.existing != "")
						}).Description(map[bool]string{
							true:  "The label of an existing agent does not change.",
							false: "A reverse-domain name that is unique among your jobs.",
						}[e.existing != ""])
						ui.Field(c, "Command", func() {
							ui.TextArea(c, &e.args).Lines(2, 5).Placeholder("/opt/homebrew/bin/node\n/Users/me/server.js")
						}).Description("One item per line: the program with its absolute path, then its arguments.")
						ui.Field(c, "Working directory", func() {
							ui.TextInput(c, &e.workDir).Placeholder("Optional, as /Users/me/project")
						})
						ui.Field(c, "Environment", func() {
							ui.TextArea(c, &e.env).Lines(1, 4).Placeholder("KEY=value")
						})
					})
					ui.Fieldset(c, "When it runs", func() {
						ui.Field(c, "Starts at login", func() { ui.Switch(c, &e.runAtLoad) })
						ui.Field(c, "Keep alive", func() {
							ui.Switch(c, &e.keepAlive).Disabled(e.keepCustom)
						}).Description(map[bool]string{
							true:  "This agent has its own conditions for staying alive, which stay as they are.",
							false: "Start the job again whenever it exits.",
						}[e.keepCustom])
						ui.Field(c, "Run every (seconds)", func() {
							ui.TextInput(c, &e.interval).Placeholder("Optional, as 3600")
						})
					})
					ui.Fieldset(c, "Logs", func() {
						ui.Field(c, "Output log", func() {
							ui.TextInput(c, &e.stdout).Placeholder("Optional absolute path")
						})
						ui.Field(c, "Error log", func() {
							ui.TextInput(c, &e.stderr).Placeholder("Optional absolute path")
						})
						if e.existing == "" {
							ui.Checkbox(c, &e.logDefault, "Write to ~/Library/Logs/<label>.log if no log file is set")
						}
					})
				})
			})
			if e.err != "" {
				ui.Text(c, e.err).TextColor(t.Danger).FontSize(textSmall)
			}
			ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
				applyLabel := "Load it now"
				if e.existing != "" {
					applyLabel = "Reload it after saving"
				}
				ui.Checkbox(c, &e.apply, applyLabel)
				ui.Spacer(c)
				if ui.Button(c, "Cancel").Clicked() {
					e.open = false
				}
				if ui.PrimaryButton(c, "Save").Disabled(e.saving).Clicked() {
					a.saveAgent()
				}
			})
		})
	})
}

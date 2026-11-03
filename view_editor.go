package main

import (
	"github.com/egoist/mygo/ui"
)

// editorDialog is the dialog that makes and edits launch agents.
func (a *app) editorDialog(c *ui.Context) {
	e := &a.editor
	t := c.Theme()
	title := "New Launch Agent"
	if e.existing != "" {
		title = "Edit Launch Agent"
	}
	ui.Modal(c, &e.open, func() {
		ui.Column(c).Width(640).Gap(10).Children(func() {
			ui.Text(c, title).FontSize(18).Bold()
			ui.Scroll(c).MaxHeight(470).Children(func() {
				ui.Form(c, func() {
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
					ui.Field(c, "Environment", func() {
						ui.TextArea(c, &e.env).Lines(1, 4).Placeholder("KEY=value")
					})
					ui.Field(c, "Output log", func() {
						ui.TextInput(c, &e.stdout).Placeholder("Optional absolute path")
					})
					ui.Field(c, "Error log", func() {
						ui.TextInput(c, &e.stderr).Placeholder("Optional absolute path")
					})
					if e.existing == "" {
						ui.Checkbox(c, &e.logDefault, "Write output to ~/Library/Logs/<label>.log if no log file is set")
					}
				})
			})
			applyLabel := "Load it now"
			if e.existing != "" {
				applyLabel = "Reload it after saving, if it is loaded"
			}
			ui.Checkbox(c, &e.apply, applyLabel)
			if e.err != "" {
				ui.Text(c, e.err).TextColor(t.Danger).FontSize(12)
			}
			ui.Row(c).Gap(8).Justify(ui.End).Children(func() {
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

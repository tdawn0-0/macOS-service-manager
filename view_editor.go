package main

import (
	"os"
	"strings"

	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/action"
	"github.com/lobehub/lobe-mygo/pkg/choice"
	"github.com/lobehub/lobe-mygo/pkg/theme"

	"macos-service-manager/internal/service"
)

// editorDialog is the dialog that makes and edits launchd's jobs.
func (a *app) editorDialog(c *ui.Context) {
	e := &a.editor
	t := c.Theme()
	title, subtitle := "New Job", ""
	if e.existing != "" {
		title, subtitle = "Edit Job", e.existing
	}
	if e.source() != service.UserAgent {
		if subtitle != "" {
			subtitle += " · "
		}
		subtitle += "Saving asks for your password"
	}
	// The form scrolls, leaving room for the title and the buttons.
	_, windowHeight := c.Size()
	ui.Modal(c, &e.open, func() {
		ui.Column(c).Width(600).Gap(14).Children(func() {
			ui.Column(c).Gap(2).Children(func() {
				ui.Text(c, title).FontSize(16).FontWeight(650)
				if subtitle != "" {
					ui.Text(c, subtitle).FontSize(textSmall).TextColor(t.TextMuted).SingleLine()
				}
			})
			if e.existing == "" {
				action.Segmented(c, &e.kind, []string{"User Agent", "System Agent", "Daemon"}, action.SegmentedProps{Variant: action.SegmentedFilled, Size: theme.Small}).AlignSelf(ui.Start)
			}
			ui.Scroll(c).MaxHeight(windowHeight - 210).Children(func() {
				ui.Form(c, func() {
					a.editorProgram(c)
					a.editorWhen(c)
					if e.source() == service.Daemon {
						ui.Fieldset(c, "Runs as", func() {
							ui.Field(c, "User", func() {
								ui.TextInput(c, &e.user).Placeholder("root")
							}).Tooltip("Without one, the job runs as root.")
							ui.Field(c, "Group", func() {
								ui.TextInput(c, &e.group).Placeholder("Optional, with a user")
							})
							ui.Field(c, "Root directory", func() {
								ui.TextInput(c, &e.rootDir).Placeholder("Optional absolute path")
							}).Tooltip("Runs the job chrooted into it, where the command must be found too.")
						})
					}
					ui.Fieldset(c, "Logs", func() {
						ui.Field(c, "Output log", func() {
							ui.TextInput(c, &e.stdout).Placeholder("Optional absolute path")
						})
						ui.Field(c, "Error log", func() {
							ui.TextInput(c, &e.stderr).Placeholder("Optional absolute path")
						})
						if e.existing == "" {
							if path := e.logDefaultPath("<label>"); path != "" {
								choice.Checkbox(c, &e.logDefault, "Write to "+tildePath(path)+" if no log file is set", choice.CheckboxProps{})
							}
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
				choice.Checkbox(c, &e.apply, applyLabel, choice.CheckboxProps{})
				ui.Spacer(c)
				if action.Button(c, "Cancel", action.ButtonProps{Variant: action.Default, Size: theme.Small}).Clicked() {
					e.open = false
				}
				if action.Button(c, "Save", action.ButtonProps{Variant: action.Primary, Size: theme.Small}).Disabled(e.saving).Clicked() {
					a.saveJob()
				}
			})
		})
	})
}

// editorProgram is what the job runs.
func (a *app) editorProgram(c *ui.Context) {
	e := &a.editor
	ui.Fieldset(c, "Program", func() {
		ui.Field(c, "Label", func() {
			ui.TextInput(c, &e.label).Placeholder("com.example.my-job").ReadOnly(e.existing != "")
		}).Tooltip(map[bool]string{
			true:  "The label of an existing job does not change.",
			false: "A reverse-domain name that is unique among your jobs.",
		}[e.existing != ""])
		ui.Field(c, "Runs", func() {
			ui.Segmented(c, &e.command, "Shell command", "Program").Label("How the command runs").AlignSelf(ui.Start)
		})
		if e.command == commandShell {
			ui.Field(c, "Command", func() {
				ui.TextArea(c, &e.args).Lines(2, 5).Placeholder("/opt/homebrew/bin/redis-cli ping")
			}).Tooltip("Runs with /bin/sh -c. launchd's PATH lacks Homebrew: give full paths, or set PATH below.")
		} else {
			ui.Field(c, "Command", func() {
				ui.TextArea(c, &e.args).Lines(2, 5).Placeholder("/opt/homebrew/bin/node\n/Users/me/server.js")
			}).Tooltip("One item per line: the program with its absolute path, then its arguments.")
		}
		ui.Field(c, "Working directory", func() {
			ui.TextInput(c, &e.workDir).Placeholder("Optional, as /Users/me/project")
		})
		ui.Field(c, "Environment", func() {
			ui.TextArea(c, &e.env).Lines(1, 4).Placeholder("KEY=value")
		})
	})
}

// editorWhen is what starts the job: nothing but launchd's keeping it, or a
// schedule, which of its nature ends.
func (a *app) editorWhen(c *ui.Context) {
	e := &a.editor
	ui.Fieldset(c, "When it runs", func() {
		ui.Field(c, "Starts", func() {
			mode := ui.Segmented(c, &e.when, "As a service", "At an interval", "On a calendar").Label("What starts the job").AlignSelf(ui.Start).Disabled(e.calendarCustom)
			if mode.Changed() {
				e.whenChanged()
			}
		})
		atLoad := "Starts at login"
		if e.source() == service.Daemon {
			atLoad = "Starts at boot"
		}
		switch e.when {
		case whenService:
			ui.Field(c, atLoad, func() {
				on := e.runAtLoad || e.restart == restartOnCrash
				if ui.Switch(c, &on).Disabled(e.restart == restartOnCrash).Changed() {
					e.runAtLoad = on
				}
			})
			ui.Field(c, "Restarts", func() {
				if e.keepCustom {
					ui.Text(c, "By conditions of its own").TextColor(c.Theme().TextMuted)
					return
				}
				ui.Segmented(c, &e.restart, "Never", "After a crash", "Always").Label("Restarts").AlignSelf(ui.Start)
			}).Tooltip(map[int]string{
				restartNever:   "launchd leaves the job stopped once it exits.",
				restartOnCrash: "Starts it again if it exits with a failure, and so also starts it at load.",
				restartAlways:  "Starts it again whenever it exits.",
			}[e.restart])
			ui.Field(c, "Starts when a disk mounts", func() { ui.Switch(c, &e.mount) })
			ui.Field(c, "Queue directories", func() {
				ui.TextInput(c, &e.queue).Placeholder("Optional, as /Users/me/inbox, /Users/me/outbox")
			}).Tooltip("Runs the job while any of these is not empty. Separate them with commas.")
		case whenInterval:
			ui.Field(c, "Run every (seconds)", func() {
				ui.TextInput(c, &e.interval).Placeholder("3600")
			})
			ui.Field(c, "Also "+strings.ToLower(atLoad), func() { ui.Switch(c, &e.runAtLoad) })
		case whenCalendar:
			if e.calendarCustom {
				ui.Field(c, "Calendar", func() {
					ui.Text(c, "A calendar of its own, which stays as it is").TextColor(c.Theme().TextMuted)
				})
			} else {
				ui.Field(c, "Calendar", func() { a.calendarFields(c) }).
					Tooltip("A number, a list (1,15), a range (1-5) or an interval (*/10). An empty field is any value.")
			}
			ui.Field(c, "Also "+strings.ToLower(atLoad), func() { ui.Switch(c, &e.runAtLoad) })
		}
	})
}

func (a *app) calendarFields(c *ui.Context) {
	e := &a.editor
	t := c.Theme()
	ui.Row(c).Gap(8).Children(func() {
		for _, f := range []struct {
			label, hint string
			text        *string
		}{
			{"Minute", "0-59", &e.minute},
			{"Hour", "0-23", &e.hour},
			{"Day", "1-31", &e.day},
			{"Month", "1-12", &e.month},
			{"Weekday", "0-6, Sun 0", &e.weekday},
		} {
			ui.Column(c.Key(f.label)).Grow(1).Basis(0).MinWidth(0).Gap(3).Children(func() {
				ui.Text(c, f.label).FontSize(textCaption).TextColor(t.TextMuted).SingleLine()
				ui.TextInput(c, f.text).Placeholder("*").Label(f.label).Tooltip(f.hint)
			})
		}
	})
}

// tildePath writes a path in the user's home as people do.
func tildePath(path string) string {
	home, err := os.UserHomeDir()
	if err == nil {
		if rest, ok := strings.CutPrefix(path, home+"/"); ok {
			return "~/" + rest
		}
	}
	return path
}

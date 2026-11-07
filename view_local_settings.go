package main

import (
	"fmt"
	"github.com/egoist/mygo"
	"github.com/egoist/mygo/ui"
	"macos-service-manager/internal/devplatform"
	"strings"
)

func displayServiceCommand(s devplatform.Service) string {
	command := append([]string{}, s.Command...)
	for _, arg := range s.LocalArguments {
		if arg.Index >= 0 && arg.Index < len(command) {
			command[arg.Index] = devplatform.LocalArgumentPlaceholder(arg.Index)
		}
	}
	return strings.Join(command, "\n")
}

func (a *app) devChooseLocalSetup() {
	d := &a.dev
	for _, svc := range d.draft.Services {
		for _, arg := range svc.LocalArguments {
			if arg.Value == "" {
				d.selectService(svc)
				d.projectTab = 1
				d.detailScroll[0].Y = 0
				return
			}
		}
		if svc.NeedsHealthReview && svc.Health == "" {
			d.selectService(svc)
			d.projectTab = 1
			d.detailScroll[0].Y = 0
			return
		}
		needsEnv := svc.NeedsEnvironmentFile && svc.EnvFile == ""
		for _, key := range svc.RequiredEnvironment {
			if _, ok := svc.Environment[key]; !ok && svc.EnvFile == "" {
				needsEnv = true
			}
		}
		if needsEnv {
			d.selectService(svc)
			d.projectTab = 2
			d.detailScroll[0].Y = 0
			return
		}
	}
	for _, site := range d.draft.Sites {
		if site.NeedsReview && !site.Disabled {
			d.selectSite(site, d.id)
			d.tab = 3
			return
		}
	}
}

func (a *app) devLocalArguments(c *ui.Context) {
	d := &a.dev
	if len(d.localArguments) == 0 {
		return
	}
	devDivider(c, "On this Mac")
	for n := range d.localArguments {
		arg := &d.localArguments[n]
		label := fmt.Sprintf("Local argument %d · %s", arg.Index+1, arg.Label)
		ui.Field(c, label, func() {
			input := ui.TextInput(c, &arg.Value)
			if arg.Kind == "secret" {
				input.Password()
			}
		})
		if arg.Prefix != "" {
			ui.Text(c, "Command prefix: "+arg.Prefix).FontSize(textSmall).TextColor(c.Theme().TextMuted)
		}
		if arg.Kind == "executable" || arg.Kind == "path" {
			if ui.Button(c, fmt.Sprintf("Choose file for argument %d", arg.Index+1)).AlignSelf(ui.Start).Clicked() {
				paths, err := mygo.Dialog.Open(mygo.OpenDialogOptions{Title: "Choose a local file"})
				if err != nil {
					d.output = "Error: " + err.Error()
				} else if len(paths) > 0 {
					arg.Value = paths[0]
				}
			}
		}
	}
}

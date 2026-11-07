package main

import (
	"fmt"
	"reflect"

	"github.com/egoist/mygo"
	"github.com/egoist/mygo/ui"
	"macos-service-manager/internal/devplatform"
)

func (d *developmentState) folderReviewCurrent() bool {
	if d.folderDraft == nil {
		return false
	}
	copy := *d
	copy.draft = d.draft.Clone()
	if copy.serviceID != "" {
		if err := copy.applyService(); err != nil {
			return false
		}
	}
	return reflect.DeepEqual(copy.draft, *d.folderDraft)
}
func (a *app) reviewProjectFolder(p devplatform.Project) {
	a.devTask(func(m *devplatform.Manager) (string, error) {
		preview, err := m.PreviewProjectFolder(m.Context, p)
		if err == nil {
			a.post(func() {
				a.dev.folderPreview = &preview
				draft := p.Clone()
				a.dev.folderDraft = &draft
				a.dev.projectTab = 0
			})
		}
		return "Review the folder change below, then apply it.", err
	})
}
func (a *app) devProjectFolder(c *ui.Context) {
	d := &a.dev
	devField(c, "Working folder", &d.draft.Directory, "")
	if ui.Button(c, "Choose working folder…").Disabled(d.busy).Clicked() {
		paths, err := mygo.Dialog.Open(mygo.OpenDialogOptions{Title: "Locate project folder", Directory: true})
		if err != nil {
			d.output = "Error: " + err.Error()
		} else if len(paths) > 0 {
			d.draft.Directory = paths[0]
		}
	}
	ui.Text(c, "Moved the source folder? Select its new location and save to review local path updates. The app keeps the project ID, databases and HTTPS domains.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
	r := d.folderPreview
	if r == nil || r.ProjectID != d.id {
		return
	}
	devDivider(c, "Review project folder change")
	ui.Text(c, "From: "+r.From+"\nTo: "+r.To).FontSize(textSmall).TextColor(c.Theme().TextMuted)
	ui.Text(c, fmt.Sprintf("Local path references to update: %d. Source files will stay where they are.", len(r.Changes))).FontSize(textSmall)
	for _, change := range r.Changes {
		ui.Text(c, change.Service+" · "+change.Field).FontSize(textSmall).FontWeight(600)
		ui.Text(c, change.From+"\n→ "+change.To).FontSize(textSmall).TextColor(c.Theme().TextMuted)
	}
	for _, warning := range r.Warnings {
		ui.Text(c, warning).FontSize(textSmall).TextColor(c.Theme().Warning)
	}
	current := d.folderReviewCurrent()
	if !current {
		ui.Text(c, "Settings changed after this preview. Save project again to refresh the review.").FontSize(textSmall).TextColor(c.Theme().Warning)
	}
	if ui.Button(c, "Apply folder change").Disabled(d.busy || !current).Clicked() {
		p, fingerprint := d.folderDraft.Clone(), r.Fingerprint
		a.devTaskThen(func(m *devplatform.Manager) (string, error) {
			return "Project folder updated. Start the project to verify its new location.", m.ApplyProjectFolder(m.Context, p, fingerprint)
		}, func(err error) {
			if err == nil {
				a.dev.folderPreview = nil
				a.dev.folderDraft = nil
			}
		})
	}
	if ui.Button(c, "Cancel folder change").Disabled(d.busy).Clicked() {
		d.draft.Directory = d.state.Projects[d.id].Directory
		d.folderPreview = nil
		d.folderDraft = nil
	}
}

func (a *app) addProjectHTTPS() {
	d := &a.dev
	site, err := devplatform.SuggestProjectSite(d.state, d.id)
	if err != nil {
		d.output = "Error: " + err.Error()
		return
	}
	d.newSite()
	d.siteID = site.ID
	d.siteDomain = site.Domain
	d.siteUpstream = site.Upstream
	d.sitePort = fmt.Sprint(site.Port)
	d.siteProject = d.id
	d.tab = 3
}

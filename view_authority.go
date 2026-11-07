package main

import (
	"context"
	"fmt"
	"time"

	"github.com/egoist/mygo/ui"
	"macos-service-manager/internal/devplatform"
)

func authorityFingerprint(value string) string {
	if len(value) == 64 {
		return value[:32] + "\n" + value[32:]
	}
	return value
}
func (a *app) authorityTask(fn func(*devplatform.Manager) error) {
	a.devTask(func(m *devplatform.Manager) (string, error) {
		var operationErr error
		if fn != nil {
			operationErr = fn(m)
		}
		ctx, cancel := context.WithTimeout(context.Background(), 6*time.Second)
		defer cancel()
		status, err := m.AuthorityStatus(ctx)
		if err == nil {
			a.post(func() { a.dev.authorityStatus = &status })
		}
		if operationErr != nil {
			return "Local certificate action stopped. Review the current status.", operationErr
		}
		return "Local certificate status refreshed.", err
	})
}
func (a *app) authorityInfo(c *ui.Context, title string, info devplatform.AuthorityInfo) {
	devDivider(c, title)
	if info.Name != "" {
		ui.Text(c, info.Name).FontSize(textSmall).FontWeight(600)
	}
	color := c.Theme().Warning
	if info.Usable && info.Status == "Ready" && info.Trust == "Trusted" {
		color = c.Theme().Success
	}
	ui.Text(c, info.Status+" · Trust: "+info.Trust).FontSize(textSmall).FontWeight(600).TextColor(color)
	ui.Text(c, info.Detail).FontSize(textSmall).TextColor(c.Theme().TextMuted)
	if info.Fingerprint != "" {
		ui.Text(c, "SHA-256\n"+authorityFingerprint(info.Fingerprint)).FontSize(textSmall)
		ui.Text(c, fmt.Sprintf("Valid %s → %s", info.NotBefore.Local().Format("2006-01-02"), info.NotAfter.Local().Format("2006-01-02"))).FontSize(textSmall).TextColor(c.Theme().TextMuted)
	}
}
func (a *app) devAuthority(c *ui.Context) {
	d := &a.dev
	devHeading(c, "Local certificates", "Manage the authority used by every local HTTPS site in this workspace.")
	if ui.Button(c, "Back to HTTPS sites").Clicked() {
		d.authorityOpen = false
	}
	if ui.Button(c, "Refresh certificate status").Disabled(d.busy).Clicked() {
		a.authorityTask(nil)
	}
	r := d.authorityStatus
	if r == nil {
		ui.Text(c, "Refresh to inspect the stored certificate, matching key, expiry and macOS trust. Inspection does not create or trust a CA.").FontSize(textSmall)
		return
	}
	ui.Text(c, "Checked "+r.CheckedAt.Local().Format("2006-01-02 15:04:05")).FontSize(textSmall).TextColor(c.Theme().TextMuted)
	a.authorityInfo(c, "Active authority", r.Active)
	if ui.Button(c, "Copy active CA fingerprint").Disabled(r.Active.Fingerprint == "" || a.copy == nil).Clicked() {
		a.copy(r.Active.Fingerprint)
	}
	if ui.Button(c, "Trust local CA").Disabled(d.busy || !r.Active.Usable && r.Active.Status != "Not created").Clicked() {
		a.authorityTask(func(m *devplatform.Manager) error { return m.ConfigureTrust(m.Context, false) })
	}
	if ui.Button(c, "Remove CA trust").Disabled(d.busy || r.Active.Fingerprint == "").Clicked() {
		a.authorityTask(func(m *devplatform.Manager) error { return m.ConfigureTrust(m.Context, true) })
	}
	ui.Text(c, "Trust changes use your login keychain and may ask for macOS authorization. Private keys are never shown or copied.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
	devDivider(c, "Replace or repair")
	ui.Text(c, "Prepare a new CA, review its fingerprint, and trust it. Stop the HTTPS proxy before activation, then apply the proxy again. The active CA stays in place until the switch.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
	if ui.Button(c, "Prepare replacement CA").Disabled(d.busy || r.Prepared != nil).Clicked() {
		a.authorityTask(func(m *devplatform.Manager) error { return m.PrepareAuthority(m.Context) })
	}
	if r.Prepared != nil {
		a.authorityInfo(c, "Prepared replacement", *r.Prepared)
		if ui.Button(c, "Copy prepared CA fingerprint").Disabled(a.copy == nil || r.Prepared.Fingerprint == "").Clicked() {
			a.copy(r.Prepared.Fingerprint)
		}
		if ui.Button(c, "Trust prepared CA").Disabled(d.busy || !r.Prepared.Usable).Clicked() {
			revision := r.Revision
			a.authorityTask(func(m *devplatform.Manager) error { return m.TrustPreparedAuthority(m.Context, revision) })
		}
		label := "Activate prepared CA"
		if r.PreparedIsActive {
			label = "Finish CA activation"
		}
		if ui.Button(c, label).Disabled(d.busy || !r.Prepared.Usable || !r.PreparedIsActive && r.Prepared.Trust != "Trusted").Clicked() {
			revision := r.Revision
			a.authorityTask(func(m *devplatform.Manager) error { return m.ActivateAuthority(m.Context, revision) })
		}
		ui.Text(c, "Discard removes the prepared selection. Its saved certificate remains available below so you can remove any trust already granted to it.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
		if ui.Button(c, "Discard prepared CA").Disabled(d.busy).Clicked() {
			revision := r.Revision
			a.authorityTask(func(m *devplatform.Manager) error { return m.DiscardPreparedAuthority(m.Context, revision) })
		}
	}
	if ui.Button(c, "Stop HTTPS proxy").Disabled(d.busy).Clicked() {
		a.authorityTask(func(m *devplatform.Manager) error { return m.StopProxy(m.Context) })
	}
	if ui.Button(c, "Apply proxy").Disabled(d.busy).Clicked() {
		a.authorityTask(func(m *devplatform.Manager) error { return m.ApplyProxy(m.Context) })
	}
	ui.Text(c, "Stopping or reinstalling a system listener can require administrator authorization. Sites, domain mappings and databases are retained.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
	if len(r.Archives) > 0 {
		ui.Text(c, "Removing saved trust affects that exact certificate wherever it is used on this Mac.").FontSize(textSmall).TextColor(c.Theme().TextMuted)
	}
	for _, saved := range r.Archives {
		if saved.Fingerprint == "" {
			continue
		}
		title := "Saved authority · " + saved.ID[:8]
		if r.Previous != nil && r.Previous.ID == saved.ID {
			title = "Previous authority · " + saved.ID[:8]
		}
		a.authorityInfo(c, title, saved)
		id, fingerprint, revision := saved.ID, saved.Fingerprint, r.Revision
		if ui.Button(c, "Trust saved CA "+id[:8]).Disabled(d.busy || !saved.Usable).Clicked() {
			a.authorityTask(func(m *devplatform.Manager) error {
				return m.ConfigureSavedAuthorityTrust(m.Context, id, fingerprint, false)
			})
		}
		if ui.Button(c, "Use saved CA "+id[:8]).Disabled(d.busy || !saved.Usable || r.Prepared != nil).Clicked() {
			a.authorityTask(func(m *devplatform.Manager) error { return m.RestoreAuthority(m.Context, id, fingerprint, revision) })
		}
		if ui.Button(c, "Remove saved CA trust "+id[:8]).Disabled(d.busy).Clicked() {
			a.authorityTask(func(m *devplatform.Manager) error { return m.RemoveArchivedAuthorityTrust(m.Context, id, fingerprint) })
		}
	}
}

package main

import (
	"context"
	"fmt"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/display"
	"github.com/lobehub/lobe-mygo/pkg/theme"
	"devbase/internal/devplatform"
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
func (a *app) authorityInfo(c *ui.Context, title string, info devplatform.AuthorityInfo, actions func()) {
	devCardWith(c, icoShield, title, info.Name, nil, func() {
		ui.Row(c).Gap(6).AlignItems(ui.Center).Children(func() {
			statusTag(c, info.Status)
			trust := info.Trust
			if trust == "Trusted" {
				statusTag(c, "Trusted")
			} else if trust != "" {
				display.Tag(c, "Trust: "+trust, display.TagProps{Color: display.TagWarning, Size: theme.Small, Round: true})
			}
		})
		if info.Detail != "" {
			devSubtle(c, info.Detail)
		}
		if info.Fingerprint != "" {
			devKV(c, "SHA-256", authorityFingerprint(info.Fingerprint))
			devKV(c, "Valid", fmt.Sprintf("%s → %s", info.NotBefore.Local().Format("2006-01-02"), info.NotAfter.Local().Format("2006-01-02")))
		}
		if actions != nil {
			devButtonRow(c, actions)
		}
	})
}
func (a *app) devAuthority(c *ui.Context) {
	d := &a.dev
	tk := theme.TokensOf(c)
	if btn(c, "Back to HTTPS rules", icoUndo, btnText).AlignSelf(ui.Start).Clicked() {
		d.authorityOpen = false
	}
	r := d.authorityStatus
	meta := ""
	if r != nil {
		meta = "Checked " + r.CheckedAt.Local().Format("2006-01-02 15:04:05")
	}
	pageHeader(c, icoShield, tk.TextSecondary, "Local certificates", meta, nil, func() {
		if btn(c, "Refresh certificate status", icoRotate, btnDefault).Disabled(d.busy).Clicked() {
			a.authorityTask(nil)
		}
	})
	if r == nil {
		return
	}
	a.authorityInfo(c, "Active authority", r.Active, func() {
		if btn(c, "Trust local CA", icoShield, btnPrimary).Tooltip("Uses your login keychain and may ask for macOS authorization.").Disabled(d.busy || !r.Active.Usable && r.Active.Status != "Not created").Clicked() {
			a.authorityTask(func(m *devplatform.Manager) error { return m.ConfigureTrust(m.Context, false) })
		}
		if btn(c, "Copy fingerprint", icoCopy, btnText).Disabled(r.Active.Fingerprint == "" || a.copy == nil).Clicked() {
			a.copy(r.Active.Fingerprint)
		}
		if btn(c, "Remove CA trust", nil, btnText).Disabled(d.busy || r.Active.Fingerprint == "").Clicked() {
			a.authorityTask(func(m *devplatform.Manager) error { return m.ConfigureTrust(m.Context, true) })
		}
	})
	devCardWith(c, icoWrench, "Replace or repair", "", nil, func() {
		devButtonRow(c, func() {
			if btn(c, "Prepare replacement CA", icoPlus, btnDefault).Disabled(d.busy || r.Prepared != nil).Clicked() {
				a.authorityTask(func(m *devplatform.Manager) error { return m.PrepareAuthority(m.Context) })
			}
			if btn(c, "Apply proxy", icoPlay, btnDefault).Disabled(d.busy).Clicked() {
				a.authorityTask(func(m *devplatform.Manager) error { return m.ApplyProxy(m.Context) })
			}
			if btn(c, "Stop HTTPS proxy", icoStop, btnDefault).Tooltip("Stopping or reinstalling a system listener can require administrator authorization.").Disabled(d.busy).Clicked() {
				a.authorityTask(func(m *devplatform.Manager) error { return m.StopProxy(m.Context) })
			}
		})
	})
	if r.Prepared != nil {
		a.authorityInfo(c, "Prepared replacement", *r.Prepared, func() {
			if btn(c, "Trust prepared CA", icoShield, btnPrimary).Disabled(d.busy || !r.Prepared.Usable).Clicked() {
				revision := r.Revision
				a.authorityTask(func(m *devplatform.Manager) error { return m.TrustPreparedAuthority(m.Context, revision) })
			}
			label := "Activate prepared CA"
			if r.PreparedIsActive {
				label = "Finish CA activation"
			}
			if btn(c, label, icoCheck, btnPrimary).Disabled(d.busy || !r.Prepared.Usable || !r.PreparedIsActive && r.Prepared.Trust != "Trusted").Clicked() {
				revision := r.Revision
				a.authorityTask(func(m *devplatform.Manager) error { return m.ActivateAuthority(m.Context, revision) })
			}
			if btn(c, "Copy fingerprint", icoCopy, btnText).Disabled(a.copy == nil || r.Prepared.Fingerprint == "").Clicked() {
				a.copy(r.Prepared.Fingerprint)
			}
			if btn(c, "Discard prepared CA", icoTrash, btnDanger).Tooltip("The saved certificate stays listed below, so any trust granted to it can still be removed.").Disabled(d.busy).Clicked() {
				revision := r.Revision
				a.authorityTask(func(m *devplatform.Manager) error { return m.DiscardPreparedAuthority(m.Context, revision) })
			}
		})
	}
	for _, saved := range r.Archives {
		if saved.Fingerprint == "" {
			continue
		}
		title := "Saved authority · " + saved.ID[:8]
		if r.Previous != nil && r.Previous.ID == saved.ID {
			title = "Previous authority · " + saved.ID[:8]
		}
		id, fingerprint, revision := saved.ID, saved.Fingerprint, r.Revision
		a.authorityInfo(c, title, saved, func() {
			if btn(c, "Use this CA", icoCheck, btnPrimary).Disabled(d.busy || !saved.Usable || r.Prepared != nil).Clicked() {
				a.authorityTask(func(m *devplatform.Manager) error { return m.RestoreAuthority(m.Context, id, fingerprint, revision) })
			}
			if btn(c, "Trust", icoShield, btnDefault).Disabled(d.busy || !saved.Usable).Clicked() {
				a.authorityTask(func(m *devplatform.Manager) error {
					return m.ConfigureSavedAuthorityTrust(m.Context, id, fingerprint, false)
				})
			}
			if btn(c, "Remove trust", nil, btnText).Tooltip("Affects that exact certificate wherever it is used on this Mac.").Disabled(d.busy).Clicked() {
				a.authorityTask(func(m *devplatform.Manager) error { return m.RemoveArchivedAuthorityTrust(m.Context, id, fingerprint) })
			}
		})
	}
}

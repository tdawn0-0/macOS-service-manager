package main

import (
	"fmt"
	"strconv"
	"strings"

	"devbase/internal/devplatform"
	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/feedback"
)

func (a *app) loadConnectionProfileDraft(instanceID, profileID string) {
	d := &a.dev
	if d.profileDraftInstance != "" && d.profileDraftInstance != instanceID {
		d.restoreDB = ""
	}
	d.profileDraftInstance = instanceID
	d.profileSelected = profileID
	d.profilePendingSelect = ""
	d.profileRedisDB = "0"
	d.profileDraft = devplatform.ConnectionProfile{InstanceID: instanceID}
	if profile, ok := d.state.ConnectionProfiles[profileID]; ok && profile.InstanceID == instanceID {
		d.profileDraft = profile
		d.profileRedisDB = strconv.Itoa(profile.RedisDB)
	}
}

func (a *app) connectionProfileDirty() bool {
	d := &a.dev
	if d.profileSelected == "" || d.profileDraftInstance == "" {
		return false
	}
	p := d.profileDraft
	if d.profileSelected == "new" {
		return p.Name != "" || p.Database != "" || p.Username != "" || p.MongoAuthSource != "" || d.profileRedisDB != "0"
	}
	saved, ok := d.state.ConnectionProfiles[d.profileSelected]
	return !ok || p != saved || d.profileRedisDB != strconv.Itoa(saved.RedisDB)
}

func (a *app) chooseConnectionProfile(instanceID, profileID string) {
	if a.dev.profileSelected != profileID && a.connectionProfileDirty() {
		a.dev.profilePendingSelect = profileID
		return
	}
	a.loadConnectionProfileDraft(instanceID, profileID)
}

func (a *app) connectionInfoFor(instanceID, profileID string, m *devplatform.Manager) (devplatform.ConnectionInfo, error) {
	if profileID == "" {
		return m.ConnectionInfo(instanceID)
	}
	return m.ProfileConnectionInfo(instanceID, profileID)
}

func (a *app) devDatabaseConnections(c *ui.Context, instance devplatform.Instance) {
	d := &a.dev
	if d.profileDraftInstance != instance.ID && !a.connectionProfileDirty() {
		a.loadConnectionProfileDraft(instance.ID, "")
	}
	devCardWith(c, icoKey, "Connections", "", nil, func() {
		// The connection the actions below use: the administrator's, or one of
		// the instance's profiles.
		ui.Row(c).Wrap().Gap(6).AlignItems(ui.Center).Children(func() {
			chip := func(selected bool) btnKind {
				if selected {
					return btnFilled
				}
				return btnText
			}
			if btn(c, "Administrator", icoKey, chip(d.profileSelected == "")).Clicked() {
				a.chooseConnectionProfile(instance.ID, "")
			}
			for _, id := range sortedKeys(d.state.ConnectionProfiles) {
				p := d.state.ConnectionProfiles[id]
				if p.InstanceID != instance.ID {
					continue
				}
				if btn(c, p.Name, icoUserRound, chip(d.profileSelected == id)).Clicked() {
					a.chooseConnectionProfile(instance.ID, id)
				}
			}
			if btn(c, "Add profile", icoPlus, chip(d.profileSelected == "new")).Clicked() {
				a.chooseConnectionProfile(instance.ID, "new")
			}
		})
		if d.profilePendingSelect != "" || d.profilePendingInstance != "" {
			devHint(c, feedback.Warning, "Unsaved connection changes. Discard or cancel the switch.", "")
			devButtonRow(c, func() {
				if btn(c, "Cancel connection switch", nil, btnDefault).Clicked() {
					d.profilePendingSelect = ""
					d.profilePendingInstance = ""
				}
				if btn(c, "Discard connection changes", nil, btnDanger).Clicked() {
					selected, target := d.profilePendingSelect, d.profilePendingInstance
					d.profilePendingSelect, d.profilePendingInstance = "", ""
					a.loadConnectionProfileDraft(instance.ID, "")
					if target != "" && target != "new" {
						if next, ok := d.state.Instances[target]; ok {
							d.dbID, d.dbKind, d.dbPort, d.dbPackage, d.persistent = next.ID, next.Kind, strconv.Itoa(next.Port), next.Package, next.Persistent
							a.loadConnectionProfileDraft(target, "")
						}
					} else if target == "new" {
						d.dbNewDraft = true
						d.dbID = ""
						d.dbKind = "postgresql"
						d.dbPort = "55432"
						d.dbPackage = ""
						d.dbDataDirectory = ""
						d.restoreDB = ""
					} else {
						a.loadConnectionProfileDraft(instance.ID, selected)
					}
				}
			})
		}
		if d.profileSelected != "" {
			devInset(c, func() {
				devFieldGrid(c, func() {
					devField(c, "Connection profile name", &d.profileDraft.Name, "Application")
					if instance.Kind == "redis" {
						devField(c, "Profile Redis database (0–15)", &d.profileRedisDB, "0")
					} else {
						devField(c, "Profile database", &d.profileDraft.Database, "app")
						devField(c, "Profile username", &d.profileDraft.Username, "app_user")
						if instance.Kind == "mongodb" {
							devField(c, "Mongo authSource", &d.profileDraft.MongoAuthSource, "app")
						}
					}
				})
				devButtonRow(c, func() {
					if btn(c, "Save connection profile", icoSave, btnPrimary).Disabled(d.busy || a.databaseTaskRunning(instance.ID)).Clicked() {
						p := d.profileDraft
						p.InstanceID = instance.ID
						if d.profileSelected == "new" {
							p.ID = ""
						}
						if instance.Kind == "redis" {
							n, err := strconv.Atoi(d.profileRedisDB)
							if err != nil {
								d.output = "Enter a Redis database number from 0 to 15"
								return
							}
							p.RedisDB = n
						}
						a.devTaskThen(func(m *devplatform.Manager) (string, error) {
							saved, err := m.SaveConnectionProfile(m.Context, p)
							if err != nil {
								return "", err
							}
							p = saved
							return "Connection profile saved", nil
						}, func(err error) {
							if err == nil {
								a.loadConnectionProfileDraft(instance.ID, p.ID)
							}
						})
					}
					if d.profileSelected != "new" && btn(c, "Delete connection profile", icoTrash, btnDanger).Disabled(d.busy).Clicked() {
						id := d.profileSelected
						a.devTaskThen(func(m *devplatform.Manager) (string, error) {
							return "Connection profile deleted; database and user remain", m.DeleteConnectionProfile(m.Context, instance.ID, id)
						}, func(err error) {
							if err == nil {
								a.loadConnectionProfileDraft(instance.ID, "")
							}
						})
					}
				})
			})
		}
		selected := d.profileSelected
		if selected == "new" {
			selected = ""
		}
		devButtonRow(c, func() {
			if btn(c, "Copy connection URI", icoCopy, btnDefault).Disabled(d.busy || d.profileSelected == "new").Clicked() {
				id, profileID := instance.ID, selected
				a.devTask(func(m *devplatform.Manager) (string, error) {
					info, err := a.connectionInfoFor(id, profileID, m)
					if err == nil {
						a.post(func() {
							if a.copy != nil {
								a.copy(info.URI)
							}
						})
					}
					return "Connection URI copied", err
				})
			}
			if btn(c, "Copy .env", icoCopy, btnDefault).Disabled(d.busy || d.profileSelected == "new").Clicked() {
				id, profileID, kind, dbOverride := instance.ID, selected, instance.Kind, d.dbName
				a.devTask(func(m *devplatform.Manager) (string, error) {
					info, err := a.connectionInfoFor(id, profileID, m)
					if err == nil {
						if profileID == "" && strings.TrimSpace(dbOverride) != "" {
							info = info.WithDatabase(kind, dbOverride)
						}
						a.post(func() {
							if a.copy != nil {
								a.copy(info.EnvAssignmentForKind(kind))
							}
						})
					}
					return ".env connection copied", err
				})
			}
			if (instance.Kind == "postgresql" || instance.Kind == "mysql") && btn(c, "Copy JDBC URL", icoCopy, btnDefault).Disabled(d.busy || d.profileSelected == "new").Clicked() {
				id, profileID, kind, dbOverride := instance.ID, selected, instance.Kind, d.dbName
				a.devTask(func(m *devplatform.Manager) (string, error) {
					info, err := a.connectionInfoFor(id, profileID, m)
					if err != nil {
						return "", err
					}
					if profileID == "" && strings.TrimSpace(dbOverride) != "" {
						info = info.WithDatabase(kind, dbOverride)
					}
					jdbc, err := info.JDBCURL(kind)
					if err == nil {
						a.post(func() {
							if a.copy != nil {
								a.copy(jdbc)
							}
						})
					}
					return "JDBC URL copied", err
				})
			}
			if d.profileSelected == "" && btn(c, "Copy database password", icoKey, btnDefault).Disabled(d.busy).Clicked() {
				id := instance.ID
				a.devTask(func(m *devplatform.Manager) (string, error) {
					password, err := m.DatabasePassword(id)
					if err == nil {
						a.post(func() {
							if a.copy != nil {
								a.copy(password)
							}
						})
					}
					return "Database password copied", err
				})
			}
			if btn(c, "Open in client", icoLink, btnDefault).Disabled(d.busy || d.profileSelected == "new").Clicked() {
				id, profileID := instance.ID, selected
				a.devTask(func(m *devplatform.Manager) (string, error) {
					info, err := a.connectionInfoFor(id, profileID, m)
					if err != nil {
						return "", err
					}
					_, err = m.Run(m.Context, "/usr/bin/open", info.URI)
					return "Opening the default client", err
				})
			}
			if instance.Kind != "mongodb" && btn(c, "Open CLI in Terminal", icoTerminal, btnDefault).Disabled(d.busy || d.profileSelected == "new").Clicked() {
				id, profileID, dbOverride := instance.ID, selected, d.dbName
				a.devTask(func(m *devplatform.Manager) (string, error) {
					return "Opening CLI in Terminal", m.OpenDatabaseCLI(m.Context, id, profileID, dbOverride)
				})
			}
			if btn(c, "Details", icoEye, btnText).Disabled(d.busy || d.profileSelected != "").Clicked() {
				id := instance.ID
				d.showResult = true
				a.devTask(func(m *devplatform.Manager) (string, error) { return m.Connection(id) })
			}
		})
		if offer := d.profileOffer; offer != nil && offer.InstanceID == instance.ID {
			if btn(c, "Use created database/user in connection profile", icoArrow, btnText).AlignSelf(ui.Start).Clicked() {
				current, ok := d.state.Instances[offer.InstanceID]
				if !ok || current.Kind != offer.Kind || a.connectionProfileDirty() {
					d.output = "Review the current connection draft and instance before using the result."
				} else {
					a.loadConnectionProfileDraft(instance.ID, "new")
					d.profileDraft.Name = "Application"
					d.profileDraft.Database = offer.Database
					d.profileDraft.Username = offer.Username
					if instance.Kind == "mongodb" {
						d.profileDraft.MongoAuthSource = offer.MongoAuthSource
					}
					d.dbPassword = ""
					d.profileOffer = nil
					d.output = fmt.Sprintf("Connection fields prefilled for %s. Save explicitly.", instance.ID)
				}
			}
		}
	})
}

package main

import (
	"fmt"
	"github.com/egoist/mygo/ui"
	"macos-service-manager/internal/devplatform"
	"strconv"
)

func (a *app) loadConnectionProfileDraft(instanceID, profileID string) {
	d := &a.dev
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
	ui.Column(c).Gap(8).Children(func() {
		devDivider(c, "Connections")
		ui.Row(c).Wrap().Gap(8).Children(func() {
			if ui.Button(c, "Administrator connection").Clicked() {
				a.chooseConnectionProfile(instance.ID, "")
			}
			for _, id := range sortedKeys(d.state.ConnectionProfiles) {
				p := d.state.ConnectionProfiles[id]
				if p.InstanceID != instance.ID {
					continue
				}
				if ui.Button(c, "Profile "+p.Name).Clicked() {
					a.chooseConnectionProfile(instance.ID, id)
				}
			}
			if ui.Button(c, "Add connection profile").Clicked() {
				a.chooseConnectionProfile(instance.ID, "new")
			}
		})
		if d.profilePendingSelect != "" || d.profilePendingInstance != "" {
			ui.Text(c, "Unsaved connection changes. Discard or cancel the switch.").FontSize(textSmall)
			ui.Row(c).Gap(8).Children(func() {
				if ui.Button(c, "Cancel connection switch").Clicked() {
					d.profilePendingSelect = ""
					d.profilePendingInstance = ""
				}
				if ui.Button(c, "Discard connection changes").Clicked() {
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
					} else {
						a.loadConnectionProfileDraft(instance.ID, selected)
					}
				}
			})
		}
		if d.profileSelected != "" {
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
			ui.Text(c, "Connection profiles store no password. Enter credentials in your client.").FontSize(textSmall)
			ui.Row(c).Wrap().Gap(8).Children(func() {
				if ui.Button(c, "Save connection profile").Disabled(d.busy || a.databaseTaskRunning(instance.ID)).Clicked() {
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
				if d.profileSelected != "new" && ui.Button(c, "Delete connection profile").Disabled(d.busy).Clicked() {
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
		}
		selected := d.profileSelected
		if selected == "new" {
			selected = ""
		}
		ui.Row(c).Wrap().Gap(8).Children(func() {
			if ui.Button(c, "Show connection details").Disabled(d.busy || d.profileSelected != "").Clicked() {
				id := instance.ID
				a.devTask(func(m *devplatform.Manager) (string, error) { return m.Connection(id) })
			}
			if ui.Button(c, "Copy connection URI").Disabled(d.busy || d.profileSelected == "new").Clicked() {
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
			if d.profileSelected == "" && ui.Button(c, "Copy database password").Disabled(d.busy).Clicked() {
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
			if ui.Button(c, "Open external database client").Disabled(d.busy || d.profileSelected == "new").Clicked() {
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
		})
		if offer := d.profileOffer; offer != nil && offer.InstanceID == instance.ID {
			if ui.Button(c, "Use created database/user in connection profile").Clicked() {
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

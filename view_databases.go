package main

import (
	"context"
	"fmt"
	"strconv"
	"strings"
	"time"

	"devbase/internal/devplatform"
	"github.com/egoist/mygo"
	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/choice"
	"github.com/lobehub/lobe-mygo/pkg/feedback"
)

// newDatabaseDraft opens the form of a new instance, unless a connection
// draft would be lost.
func (a *app) newDatabaseDraft() {
	d := &a.dev
	if a.connectionProfileDirty() {
		d.profilePendingInstance = "new"
		return
	}
	d.dbNewDraft = true
	d.databaseWorkOpen = false
	d.initializationsOpen = false
	d.deletionsOpen = false
	d.instanceDeletionPreview = nil
	d.deleteConfirmation = ""
	d.dbID = ""
	d.dbKind = "postgresql"
	d.dbPort = "55432"
	d.dbPackage = ""
	d.dbDataDirectory = ""
	d.persistent = true
}

func isDatabaseEngine(name string) bool {
	switch name {
	case "mysql", "postgresql", "redis", "mongodb":
		return true
	}
	return false
}

// openNewInstance goes to the form of a new instance of an engine.
func (a *app) openNewInstance(engine string) {
	d := &a.dev
	a.nav = navDevelopment
	d.tab = devTabDatabases
	d.detailScroll[devTabDatabases].Y = 0
	a.newDatabaseDraft()
	if !d.dbNewDraft {
		return
	}
	d.dbKind = engine
	d.dbPort = map[string]string{"mysql": "3306", "postgresql": "5432", "redis": "6379", "mongodb": "27017"}[engine]
	if p := d.packages(engine); len(p) > 0 {
		d.dbPackage = p[0]
	}
}

func (a *app) devDatabases(c *ui.Context) {
	d := &a.dev
	if d.databaseWorkOpen {
		a.devDatabaseWork(c)
		return
	}
	if d.initializationsOpen {
		a.devInitializations(c)
		return
	}
	if d.instanceDeletionPreview != nil {
		a.devInstanceDeletionPreview(c)
		return
	}
	if d.deletionsOpen {
		a.devInstanceDeletions(c)
		return
	}
	if d.dbKind == "" {
		d.dbKind = "postgresql"
		d.dbPort = "55432"
		d.persistent = true
	}
	instance, exists := d.state.Instances[d.dbID]
	if !exists || d.dbNewDraft {
		a.devDatabaseTasks(c)
		a.devNewInstance(c)
		return
	}
	busy := d.busy || a.databaseTaskRunning(instance.ID)
	status := d.statuses["instance:"+instance.ID]
	icon, tint := softwareMeta(instance.Kind)
	meta := softwareTitle(instance.Kind)
	if p, ok := d.state.Packages[instance.Package]; ok {
		meta += " " + p.Version
	}
	meta += " · 127.0.0.1:" + strconv.Itoa(instance.Port)
	pageHeader(c, icon, tint, instance.ID, meta, func() {
		if status == "" {
			status = "Stopped"
		}
		statusTag(c, status)
	}, func() {
		for _, act := range []struct {
			label, text string
			icon        *ui.SVG
		}{{"Start instance", "Start", icoPlay}, {"Restart instance", "Restart", icoRotate}, {"Stop instance", "Stop", icoStop}} {
			kind := btnDefault
			if act.text == "Start" && status != "Running" && status != "Healthy" {
				kind = btnPrimary
			}
			if btn(c, act.text, act.icon, kind).Disabled(busy).Clicked() {
				id := instance.ID
				verb := strings.TrimSuffix(strings.ToLower(act.label), " instance")
				a.startDatabaseOperation(instance, verb, func(m *devplatform.Manager) (string, error) {
					result := map[string]string{"start": "Started ", "stop": "Stopped ", "restart": "Restarted "}[verb] + id
					return result, m.InstanceAction(m.Context, id, verb)
				})
			}
		}
		if iconButton(c, icoScroll, "Read instance logs", c.Theme().TextMuted).Disabled(d.busy).Clicked() {
			id := "db-" + instance.ID
			d.showResult = true
			a.devTask(func(m *devplatform.Manager) (string, error) { return m.Logs(id) })
		}
	})
	a.devDatabaseTasks(c)
	a.devDatabaseConnections(c, instance)
	devCardWith(c, icoPlug, "Endpoint & storage", "", nil, func() {
		devKV(c, "Version", d.state.Packages[instance.Package].Version)
		a.devUpgradeControl(c, instance, status, busy)
		devKV(c, "Data", a.devManager().DatabaseDataDirectory(instance))
		a.devDatabasePort(c, instance)
	})
	title := "Databases & users"
	if d.dbKind == "redis" {
		title = "Database"
	}
	devCardWith(c, icoDatabase, title, "", nil, func() {
		if d.dbKind == "redis" {
			devField(c, "Database number (0–15)", &d.dbName, "0")
			return
		}
		devFieldGrid(c, func() {
			devField(c, "Database name", &d.dbName, "app")
		})
		if btn(c, "Create database", icoPlus, btnDefault).AlignSelf(ui.Start).Disabled(busy).Clicked() {
			id, name := d.dbID, d.dbName
			a.startDatabaseOperationWithOffer(d.state.Instances[id], "create database", func(m *devplatform.Manager) (string, error) {
				return "Created database " + name, m.CreateDatabase(m.Context, id, name)
			}, &connectionOperationOffer{InstanceID: id, Kind: d.dbKind, Database: name})
		}
		devDivider(c, "User")
		devFieldGrid(c, func() {
			devField(c, "Database user", &d.dbUser, "app_user")
			ui.Field(c, "New user password", func() { ui.TextInput(c, &d.dbPassword).Password() })
		})
		if btn(c, "Create database user", icoUser, btnDefault).AlignSelf(ui.Start).Disabled(busy).Clicked() {
			id, name, password, db := d.dbID, d.dbUser, d.dbPassword, d.dbName
			d.dbPassword = ""
			a.startDatabaseOperation(d.state.Instances[id], "create user", func(m *devplatform.Manager) (string, error) {
				return "Created user " + name, m.CreateUser(m.Context, id, name, password, db)
			})
		}
	})
	devCardWith(c, icoArchive, "Backups", "", nil, func() {
		a.loadBackups(instance.ID)
		if btn(c, "Back up database", icoArchive, btnDefault).AlignSelf(ui.Start).Disabled(busy).Clicked() {
			id, name := d.dbID, d.dbName
			a.startDatabaseOperation(d.state.Instances[id], "backup", func(m *devplatform.Manager) (string, error) { return m.Backup(m.Context, id, name) })
		}
		a.backupRows(c, instance.ID, busy)
		devDivider(c, "Restore")
		devField(c, "Backup file", &d.backupPath, "Select a backup file")
		devButtonRow(c, func() {
			if btn(c, "Choose backup file…", icoFolder, btnDefault).Clicked() {
				paths, e := mygo.Dialog.Open(mygo.OpenDialogOptions{Title: "Restore database backup"})
				if e != nil {
					d.output = e.Error()
				} else if len(paths) > 0 {
					d.backupPath = paths[0]
				}
			}
			if btn(c, "Restore into new database", icoRestore, btnDefault).Disabled(busy).Clicked() {
				id, name, path := d.dbID, d.dbName, d.backupPath
				a.startDatabaseOperation(d.state.Instances[id], "restore", func(m *devplatform.Manager) (string, error) {
					return "Restored into " + name, m.Restore(m.Context, id, name, path)
				})
			}
		})
	})
	devDangerCard(c, "Remove", func() {
		devButtonRow(c, func() {
			if btn(c, "Remove instance (keep data files)", icoTrash, btnDanger).Disabled(busy).Clicked() {
				id := d.dbID
				a.startDatabaseOperation(d.state.Instances[id], "remove", func(m *devplatform.Manager) (string, error) {
					return "Removed instance", m.InstanceAction(m.Context, id, "remove")
				})
			}
			if btn(c, "Review permanent data deletion", nil, btnDanger).Disabled(busy).Clicked() {
				a.previewInstanceDeletion(d.dbID)
			}
		})
	})
}

// devNewInstance is the form of a new database instance.
func (a *app) devNewInstance(c *ui.Context) {
	d := &a.dev
	d.dbNewDraft = true
	icon, tint := softwareMeta(d.dbKind)
	pageHeader(c, icon, tint, "New instance", softwareTitle(d.dbKind), nil, nil)
	packages := d.packages(d.dbKind)
	if len(packages) == 0 {
		ui.Row(c).Gap(8).AlignItems(ui.Center).Children(func() {
			ui.Column(c).Grow(1).MinWidth(0).Children(func() {
				devHint(c, feedback.Warning, "Install "+softwareTitle(d.dbKind)+" first", "")
			})
			kind := d.dbKind
			if btn(c, "Open Software", icoArrow, btnDefault).Clicked() {
				a.openSoftware(kind)
			}
		})
	} else if d.dbPackage == "" {
		d.dbPackage = packages[0]
	}
	devCardWith(c, icoDatabase, "Instance", "", nil, func() {
		devFieldGrid(c, func() {
			devField(c, "Instance name", &d.dbID, "app-db")
			if devChoice(c, "Database engine", &d.dbKind, []string{"mysql", "postgresql", "redis", "mongodb"}) {
				d.dbPackage = ""
				if p := d.packages(d.dbKind); len(p) > 0 {
					d.dbPackage = p[0]
				}
				d.dbPort = map[string]string{"mysql": "3306", "postgresql": "5432", "redis": "6379", "mongodb": "27017"}[d.dbKind]
			}
			devPackageChoice(c, "Database version", &d.dbPackage, d.packages(d.dbKind), d.state.Packages, false)
			devField(c, "Local port", &d.dbPort, "55432")
		})
		a.devPortControls(c, &d.dbPort, "database")
		if d.dbKind == "redis" {
			choice.Checkbox(c, &d.persistent, "Persist database contents", choice.CheckboxProps{})
		} else {
			d.persistent = true
		}
	})
	devCardWith(c, icoFolder, "Storage", "", nil, func() {
		devField(c, "Data location", &d.dbDataDirectory, "Blank for app storage, or a new absolute directory")
	})
	if btn(c, "Create instance", icoPlus, btnPrimary).AlignSelf(ui.Start).Disabled(d.busy || d.databaseTasks[d.dbID] != nil && d.databaseTasks[d.dbID].Running).Clicked() {
		port, e := strconv.Atoi(d.dbPort)
		if e != nil {
			d.output = "Error: Enter a valid port"
		} else {
			i := devplatform.Instance{ID: d.dbID, Kind: d.dbKind, Package: d.dbPackage, Port: port, Persistent: d.persistent, DataDirectory: d.dbDataDirectory}
			d.dbNewDraft = true
			a.startDatabaseTask(i)
		}
	}
}

func devPackageChoice(c *ui.Context, label string, selected *string, ids []string, packages map[string]devplatform.Package, optional bool) bool {
	var options []string
	lookup := map[string]string{}
	display := ""
	if optional {
		options = append(options, "External executable (unmanaged)")
		lookup["External executable (unmanaged)"] = ""
		if *selected == "" {
			display = "External executable (unmanaged)"
		}
	}
	for _, id := range ids {
		p := packages[id]
		name := softwareTitle(p.Name) + " · " + p.Version
		options = append(options, name)
		lookup[name] = id
		if *selected == id {
			display = name
		}
	}
	if devChoice(c, label, &display, options) {
		*selected = lookup[display]
		return true
	}
	return false
}

// loadBackups reads the instance's backups folder in the background.
func (a *app) loadBackups(id string) {
	d := &a.dev
	if a.post == nil || d.backupsLoading || d.backupsFor == id {
		return
	}
	d.backupsLoading = true
	m := a.devManager()
	a.spawn(func() {
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()
		rows, e := m.Backups(ctx, id)
		a.post(func() {
			a.dev.backups, a.dev.backupsError = rows, ""
			if e != nil {
				a.dev.backupsError = "Backups could not be listed: " + e.Error()
			}
			a.dev.backupsFor = id
			a.dev.backupsLoading = false
		})
	})
}

// reloadBackups marks the list stale so the next frame reads it again.
func (a *app) reloadBackups() { a.dev.backupsFor = "" }

func formatBackupSize(n int64) string {
	switch {
	case n >= 1<<30:
		return fmt.Sprintf("%.1f GiB", float64(n)/(1<<30))
	case n >= 1<<20:
		return fmt.Sprintf("%.1f MiB", float64(n)/(1<<20))
	case n >= 1<<10:
		return fmt.Sprintf("%.1f KiB", float64(n)/(1<<10))
	}
	return fmt.Sprintf("%d B", n)
}

// backupRows lists the instance's backups with their actions.
func (a *app) backupRows(c *ui.Context, id string, busy bool) {
	d := &a.dev
	if d.backupsFor != id {
		return
	}
	if d.backupsError != "" {
		devHint(c, feedback.Warning, d.backupsError, "")
		return
	}
	if len(d.backups) == 0 {
		devSubtle(c, "No backups")
		return
	}
	for _, row := range d.backups {
		row := row
		when := row.Modified.Format("2006-01-02 15:04")
		kc := c.Key("backup-" + row.Name)
		ui.Row(kc).Gap(12).AlignItems(ui.Center).MinWidth(0).Children(func() {
			devSubtle(kc, row.Database+" · "+when+" · "+formatBackupSize(row.Size)).Grow(1).MinWidth(0)
			if btn(kc, "Use for restore", icoRestore, btnDefault).Clicked() {
				d.backupPath = row.Path
			}
			if btn(kc, "Show in Finder", icoFolder, btnText).Disabled(a.reveal == nil).Clicked() {
				a.reveal(row.Path)
			}
			if btn(kc, "Delete", icoTrash, btnDanger).Disabled(busy).Clicked() {
				name := row.Name
				a.ask = askState{open: true, title: "Delete this backup?", message: "Permanently deletes the backup of " + row.Database + " from " + when + ". The database itself is not changed.", button: "Delete Backup", run: func() {
					a.devTaskThen(func(m *devplatform.Manager) (string, error) {
						return "Deleted backup", m.DeleteBackup(m.Context, id, name)
					}, func(error) { a.reloadBackups() })
				}}
			}
		})
	}
}

// devUpgradeControl offers the newest installed patch release of the same
// series. It only asks: nothing changes until the dialog is confirmed.
func (a *app) devUpgradeControl(c *ui.Context, instance devplatform.Instance, status string, busy bool) {
	d := &a.dev
	targets := devplatform.UpgradeTargets(d.state, instance)
	if len(targets) == 0 {
		return
	}
	target := targets[0]
	version := d.state.Packages[target].Version
	current := d.state.Packages[instance.Package].Version
	running := status == "Running" || status == "Healthy"
	devButtonRow(c, func() {
		if btn(c, "Upgrade to "+version+"\u2026", nil, btnDefault).Tooltip("Stop this instance first. Only the server version changes; data, port and credentials are kept.").Disabled(busy || running).Clicked() {
			id := instance.ID
			a.ask = askState{
				open:    true,
				title:   "Upgrade " + id + " to " + softwareTitle(instance.Kind) + " " + version + "?",
				message: "The instance will use the newer server the next time it starts. Its data is not copied or deleted, and no backup is taken: back up the databases you need before continuing. After the newer server has started, going back to " + current + " is not supported.",
				button:  "Upgrade Instance",
				run: func() {
					oldID := instance.Package
					oldPackage := d.state.Packages[oldID]
					a.devTaskThen(func(m *devplatform.Manager) (string, error) {
						return "Upgraded " + id + " to " + version + ". Start the instance to use it.", m.UpgradeInstancePackage(m.Context, instance, target)
					}, func(err error) {
						// The state is already refreshed when this runs.
						if err != nil || !packageUnused(a.dev.state, oldID) {
							return
						}
						name := softwareTitle(oldPackage.Name) + " " + oldPackage.Version
						a.askUninstall(oldID, oldPackage, "No instance uses "+name+" any more. Uninstalling removes it from Devbase; you can install it again later. Keep it if you plan to create another instance on this version.")
					})
				},
			}
		}
	})
}

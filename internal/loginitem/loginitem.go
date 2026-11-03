// Package loginitem reads the login items of System Settings, and adds and
// removes the apps that open at login.
package loginitem

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"sync"

	"macos-service-manager/internal/service"
	"macos-service-manager/internal/system"
)

// Record is an item of the background task management database that
// System Settings shows.
type Record struct {
	UUID, Name, Developer, Team, Type, Identifier, URL, Bundle, Parent, LastUse string
	Enabled                                                                     bool
}

// Kind of the records the app lists.
const (
	typeApp       = "app"        // an app the user set to open at login
	typeLoginItem = "login item" // a helper app inside another app's bundle
)

var (
	sectionRE = regexp.MustCompile(`^\s*Records for UID (-?\d+)`)
	recordRE  = regexp.MustCompile(`^\s*#\d+:\s*$`)
	fieldRE   = regexp.MustCompile(`^\s+([A-Za-z ]+):\s?(.*)$`)
	typeRE    = regexp.MustCompile(`^([a-z ]+?)\s*\(0x`)
)

func clean(v string) string {
	if v == "(null)" {
		return ""
	}
	return v
}

// ParseBTM reads the output of `sfltool dumpbtm` and returns the records of
// the user with the ID uid.
func ParseBTM(dump string, uid int) []Record {
	var records []Record
	inUser := false
	var cur *Record
	flush := func() {
		if cur != nil {
			records = append(records, *cur)
			cur = nil
		}
	}
	for _, line := range strings.Split(dump, "\n") {
		if m := sectionRE.FindStringSubmatch(line); m != nil {
			flush()
			inUser = m[1] == strconv.Itoa(uid)
			continue
		}
		if !inUser {
			continue
		}
		if recordRE.MatchString(line) {
			flush()
			cur = &Record{}
			continue
		}
		if cur == nil {
			continue
		}
		m := fieldRE.FindStringSubmatch(line)
		if m == nil {
			continue
		}
		key, val := m[1], strings.TrimSpace(m[2])
		switch key {
		case "UUID":
			cur.UUID = val
		case "Name":
			cur.Name = clean(val)
		case "Developer Name":
			cur.Developer = clean(val)
		case "Team Identifier":
			cur.Team = clean(val)
		case "Type":
			if t := typeRE.FindStringSubmatch(val); t != nil {
				cur.Type = t[1]
			}
		case "Disposition":
			// [enabled, allowed, notified]
			for _, w := range strings.Split(strings.Trim(val[:strings.Index(val+"]", "]")], "[ "), ",") {
				if strings.TrimSpace(w) == "enabled" {
					cur.Enabled = true
				}
			}
		case "Identifier":
			cur.Identifier = clean(val)
		case "URL":
			cur.URL = clean(val)
		case "Bundle Identifier":
			cur.Bundle = clean(val)
		case "Parent Identifier":
			cur.Parent = clean(val)
		case "Last Use":
			cur.LastUse = clean(val)
		}
	}
	flush()
	return records
}

// Provider is the login items of the user.
type Provider struct {
	mu sync.Mutex
	// records are those of the last List: dumping the database takes
	// seconds, so a detail view reads them rather than dump it again.
	records []Record
}

// New returns the provider of the user's login items.
func New() *Provider { return &Provider{} }

func (*Provider) Source() service.Source { return service.LoginItem }

// isOpenAtLogin reports whether the record is an app set to open at login,
// as System Settings lists under "Open at Login". The database also holds
// every app it has seen, disabled: those are not in the list.
func isOpenAtLogin(r Record) bool { return r.Type == typeApp && r.Enabled }

// itemsOf returns the records System Settings lists: the apps that open at
// login, and the helpers that apps have registered to run in the
// background, enabled or not.
func itemsOf(records []Record) []service.Item {
	var items []service.Item
	for _, r := range records {
		if !isOpenAtLogin(r) && r.Type != typeLoginItem {
			continue
		}
		name := r.Name
		if name == "" {
			name = r.Identifier
		}
		it := service.Item{
			ID:        "login-item:" + r.UUID,
			Source:    service.LoginItem,
			Name:      name,
			Owner:     r.Developer,
			Path:      r.URL,
			Autostart: service.AutostartOff,
		}
		if r.Enabled {
			it.Autostart = service.AutostartOn
		}
		items = append(items, it)
	}
	return items
}

func (p *Provider) List() ([]service.Item, error) {
	out, err := system.Run("sfltool", "dumpbtm")
	if err != nil {
		return nil, err
	}
	records := ParseBTM(out, os.Getuid())
	p.mu.Lock()
	p.records = records
	p.mu.Unlock()
	return itemsOf(records), nil
}

func (p *Provider) Detail(it service.Item) (service.Detail, error) {
	p.mu.Lock()
	defer p.mu.Unlock()
	for _, r := range p.records {
		if "login-item:"+r.UUID == it.ID {
			return detailOf(r), nil
		}
	}
	return service.Detail{}, fmt.Errorf("%s is no longer a login item", it.Name)
}

func detailOf(r Record) service.Detail {
	var d service.Detail
	add := func(k, v string) {
		if v != "" {
			d.Rows = append(d.Rows, service.Row{Key: k, Value: v})
		}
	}
	add("name", r.Name)
	if r.Type == typeApp {
		add("kind", "App that opens at login")
	} else {
		add("kind", "Helper that runs in the background")
	}
	add("enabled", strconv.FormatBool(r.Enabled))
	add("path", r.URL)
	add("developer", r.Developer)
	add("bundle identifier", r.Bundle)
	add("team identifier", r.Team)
	add("identifier", r.Identifier)
	add("parent", r.Parent)
	add("last use", r.LastUse)
	return d
}

// Actions: an app that opens at login can be removed from the list. A
// helper belongs to its app, which alone can register or unregister it:
// System Settings has its switch.
func (*Provider) Actions(it service.Item) []service.Action {
	if filepath.IsAbs(it.Path) {
		return []service.Action{service.Remove}
	}
	return nil
}

func (*Provider) Confirm(it service.Item, a service.Action) string {
	if a == service.Remove {
		return fmt.Sprintf("Stop %s from opening at login? The app itself is not deleted.", it.Name)
	}
	return ""
}

func (*Provider) Do(it service.Item, a service.Action) error {
	if a != service.Remove {
		return fmt.Errorf("login items: unknown action %q", a)
	}
	_, err := system.Run("osascript", "-e",
		"tell application \"System Events\" to delete (every login item whose path is "+
			system.AppleScriptString(it.Path)+")")
	return err
}

// Add makes the app at path open at login.
func (*Provider) Add(path string) error {
	if !strings.HasSuffix(path, ".app") {
		return fmt.Errorf("choose an app (a .app bundle)")
	}
	if _, err := os.Stat(path); err != nil {
		return fmt.Errorf("%s does not exist", path)
	}
	_, err := system.Run("osascript", "-e",
		"tell application \"System Events\" to make login item at end with properties {path:"+
			system.AppleScriptString(path)+", hidden:false}")
	return err
}

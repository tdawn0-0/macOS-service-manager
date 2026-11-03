// Package loginitem reads the login items of System Settings, and adds and
// removes the apps that open at login.
//
// The items come from the background task management database, which anyone
// may read, so listing them asks for nothing. Its format is Apple's own and
// changes with macOS: where it cannot be read, System Events lists the apps,
// and `sfltool dumpbtm` the rest, which takes an administrator. That asks the
// user at most once per launch, and its answer is kept until the database
// changes (see fallback).
package loginitem

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"sync"

	"macos-service-manager/internal/launchd"
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

// What Item.Kind says.
const (
	kindApp    = "App"
	kindHelper = "Background helper"
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
	// Dir holds the database, and CachePath keeps what `sfltool dumpbtm`
	// answered.
	Dir, CachePath string
	// UserID returns the identifier of the user in the database: their
	// GeneratedUID.
	UserID func() (string, error)

	mu sync.Mutex
	// records are those of the last List, which the details read.
	records []Record
	// asked says `sfltool dumpbtm` ran since launch, with what it answered.
	asked   bool
	dump    []Record
	dumpErr error
}

// New returns the provider of the user's login items.
func New() *Provider {
	cache := ""
	if dir, err := os.UserCacheDir(); err == nil {
		cache = filepath.Join(dir, "xyz.jyeu.msm.app", "login-items.json")
	}
	return &Provider{
		Dir:       "/private/var/db/com.apple.backgroundtaskmanagement",
		CachePath: cache,
		UserID:    generatedUID,
	}
}

func (*Provider) Source() service.Source { return service.LoginItem }

// isOpenAtLogin reports whether the record is an app set to open at login,
// as System Settings lists under "Open at Login". The database also holds
// every app it has seen, disabled: those are not in the list.
func isOpenAtLogin(r Record) bool { return r.Type == typeApp && r.Enabled }

// labelOf returns the launchd label of a helper: its identifier, without the
// number of its type before it.
func labelOf(r Record) string {
	if r.Bundle != "" {
		return r.Bundle
	}
	if _, rest, ok := strings.Cut(r.Identifier, "."); ok {
		return rest
	}
	return r.Identifier
}

// processOf finds a record's process among the jobs of the user's domain: a
// helper is a job of its own, and launchd holds an app that runs as
// application.<bundle identifier>.<numbers>.
func processOf(r Record, st launchd.State) int {
	if r.Type == typeLoginItem {
		return st.Jobs[labelOf(r)].PID
	}
	if r.Bundle == "" {
		return 0
	}
	prefix := "application." + r.Bundle + "."
	for label, j := range st.Jobs {
		if j.PID > 0 && strings.HasPrefix(label, prefix) {
			return j.PID
		}
	}
	return 0
}

// itemsOf returns the records System Settings lists: the apps that open at
// login, and the helpers that apps have registered to run in the
// background, enabled or not.
func itemsOf(records []Record, st launchd.State) []service.Item {
	parents := make(map[string]Record, len(records))
	for _, r := range records {
		if r.Identifier != "" {
			parents[r.Identifier] = r
		}
	}
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
			Kind:      kindApp,
			Autostart: service.AutostartOff,
		}
		if r.Type == typeLoginItem {
			it.Kind = kindHelper
			if parent, ok := parents[r.Parent]; ok {
				it.Owner = parent.Name
				if filepath.IsAbs(parent.URL) {
					it.Path = filepath.Join(parent.URL, r.URL)
				}
			}
		}
		if r.Enabled {
			it.Autostart = service.AutostartOn
		}
		if pid := processOf(r, st); pid > 0 {
			it.State, it.PID = service.StateRunning, pid
		}
		items = append(items, it)
	}
	return items
}

func (p *Provider) List() ([]service.Item, error) {
	records, err := p.readDatabase()
	if err != nil {
		records, err = p.fallback()
	}
	p.mu.Lock()
	p.records = records
	p.mu.Unlock()
	// The processes are a nicety: without them, the items are still right.
	var st launchd.State
	if out, printErr := system.Run("launchctl", "print", fmt.Sprintf("gui/%d", os.Getuid())); printErr == nil {
		st = launchd.ParsePrint(out)
	}
	return itemsOf(records, st), err
}

// fallback reads the items where the database cannot be: System Events
// lists the apps that open at login, as of now, and `sfltool dumpbtm` adds the
// helpers and what else it knows of the apps.
func (p *Provider) fallback() ([]Record, error) {
	apps, appsErr := openAtLogin()
	dump, dumpErr := p.dumpOnce()
	switch {
	case appsErr != nil && dumpErr != nil:
		return nil, dumpErr
	case dumpErr != nil:
		return apps, fmt.Errorf("background helpers are not listed: %w", dumpErr)
	case appsErr != nil:
		return dump, nil
	}
	// System Events is the newer of the two about the apps.
	byPath := make(map[string]Record, len(apps))
	for _, r := range apps {
		byPath[r.URL] = r
	}
	records := make([]Record, 0, len(dump)+len(apps))
	for _, r := range dump {
		if r.Type == typeApp {
			_, r.Enabled = byPath[r.URL]
			delete(byPath, r.URL)
		}
		records = append(records, r)
	}
	for _, r := range apps {
		if _, new := byPath[r.URL]; new {
			records = append(records, r)
		}
	}
	return records, nil
}

// openAtLogin reads the apps that open at login from System Events, which
// takes no administrator; macOS asks once whether the app may control it.
func openAtLogin() ([]Record, error) {
	out, err := system.Run("osascript", "-e",
		`tell application "System Events" to get the path of every login item`)
	if err != nil {
		return nil, err
	}
	var records []Record
	for _, path := range strings.Split(strings.TrimSpace(out), ", ") {
		if path == "" {
			continue
		}
		records = append(records, Record{
			UUID:    path,
			Name:    strings.TrimSuffix(filepath.Base(path), ".app"),
			Type:    typeApp,
			URL:     path,
			Enabled: true,
		})
	}
	return records, nil
}

// cached is what the cache file holds: what `sfltool dumpbtm` answered, while
// the database was as Stamp says.
type cached struct {
	Stamp   string
	Records []Record
}

// stamp says when the database last changed, or "" if that is unknown.
func (p *Provider) stamp() string {
	files, err := filepath.Glob(filepath.Join(p.Dir, "BackgroundItems-*"))
	if err != nil {
		return ""
	}
	newest := ""
	for _, file := range files {
		if info, err := os.Stat(file); err == nil {
			if s := info.ModTime().UTC().Format("2006-01-02T15:04:05.000000000"); s > newest {
				newest = s
			}
		}
	}
	return newest
}

// dumpOnce returns what `sfltool dumpbtm` says, which macOS asks the user to
// authenticate for each time. So it asks at most once per launch, and not at
// all while the database is as it was when it last answered.
func (p *Provider) dumpOnce() ([]Record, error) {
	stamp := p.stamp()
	if stamp != "" && p.CachePath != "" {
		if data, err := os.ReadFile(p.CachePath); err == nil {
			var c cached
			if json.Unmarshal(data, &c) == nil && c.Stamp == stamp {
				return c.Records, nil
			}
		}
	}
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.asked {
		return p.dump, p.dumpErr
	}
	p.asked = true
	out, err := system.Run("sfltool", "dumpbtm")
	if err != nil {
		p.dumpErr = err
		return nil, err
	}
	p.dump = ParseBTM(out, os.Getuid())
	if stamp != "" && p.CachePath != "" {
		if data, err := json.Marshal(cached{stamp, p.dump}); err == nil {
			if os.MkdirAll(filepath.Dir(p.CachePath), 0o700) == nil {
				_ = os.WriteFile(p.CachePath, data, 0o600)
			}
		}
	}
	return p.dump, nil
}

func (p *Provider) Detail(it service.Item) (service.Detail, error) {
	p.mu.Lock()
	defer p.mu.Unlock()
	for _, r := range p.records {
		if "login-item:"+r.UUID == it.ID {
			return detailOf(r, it), nil
		}
	}
	return service.Detail{}, fmt.Errorf("%s is no longer a login item", it.Name)
}

func detailOf(r Record, it service.Item) service.Detail {
	var d service.Detail
	add := func(group, k, v string) {
		if v != "" {
			d.Rows = append(d.Rows, service.Row{Group: group, Key: k, Value: v})
		}
	}
	const status, about, files = "Status", "About", "Files"
	if it.PID > 0 {
		add(status, "Process ID", strconv.Itoa(it.PID))
	}
	if r.Enabled {
		add(status, "Enabled", "Yes")
	} else {
		add(status, "Enabled", "No")
	}
	add(status, "Last used", r.LastUse)
	if r.Type == typeApp {
		add(about, "Kind", "App that opens at login")
	} else {
		add(about, "Kind", "Helper that runs in the background")
		add(about, "Part of", it.Owner)
	}
	add(about, "Developer", r.Developer)
	add(about, "Team ID", r.Team)
	add(about, "Bundle identifier", r.Bundle)
	add(files, "Path", it.Path)
	return d
}

// Actions: an app that opens at login can be removed from the list. A
// helper belongs to its app, which alone can register or unregister it:
// System Settings has its switch.
func (*Provider) Actions(it service.Item) []service.Action {
	if it.Kind == kindApp && filepath.IsAbs(it.Path) {
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

// Do removes an app from the login items. An app that registered itself
// (SMAppService) is in System Settings but not in System Events, so nothing
// would happen: that says so rather than report it removed.
func (*Provider) Do(it service.Item, a service.Action) error {
	if a != service.Remove {
		return fmt.Errorf("login items: unknown action %q", a)
	}
	path := system.AppleScriptString(it.Path)
	out, err := system.Run("osascript", "-e", `tell application "System Events"
	set found to count of (every login item whose path is `+path+`)
	if found > 0 then delete (every login item whose path is `+path+`)
	return found
end tell`)
	if err != nil {
		return err
	}
	if strings.TrimSpace(out) == "0" {
		return fmt.Errorf("%s registered itself: turn it off in System Settings or in the app", it.Name)
	}
	return nil
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

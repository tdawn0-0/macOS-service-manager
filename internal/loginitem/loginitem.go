// Package loginitem reads the login items of System Settings, and adds and
// removes the apps that open at login.
//
// The items come from the background task management database, which macOS
// lets an app read only with Full Disk Access, and whose format is Apple's own
// and changes with macOS. Where it cannot be read, System Events lists the apps
// that open at login, for which macOS asks the user's permission once, and the
// helpers that apps registered are left out. `sfltool dumpbtm` could list them,
// but macOS asks for an administrator each time it runs, so it runs only when
// the user asks (see ReadOnce), and its answer is kept.
package loginitem

import (
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"sync"
	"time"

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
	// restriction is what the last List left out.
	restriction *service.Restriction
	// dump is what `sfltool dumpbtm` last answered, and dumpAt when.
	dump   []Record
	dumpAt time.Time
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
	var restriction *service.Restriction
	if err != nil {
		restriction = p.restrictionOf(err)
		records, err = p.fallback()
	}
	p.mu.Lock()
	p.records, p.restriction = records, restriction
	p.mu.Unlock()
	// The processes are a nicety: without them, the items are still right.
	var st launchd.State
	if out, printErr := system.Run("launchctl", "print", fmt.Sprintf("gui/%d", os.Getuid())); printErr == nil {
		st = launchd.ParsePrint(out)
	}
	return itemsOf(records, st), err
}

// restrictionOf says what is left out because the database failed to read.
func (p *Provider) restrictionOf(err error) *service.Restriction {
	denied := errors.Is(err, fs.ErrPermission)
	why := "the app cannot read where this version of macOS keeps them"
	if denied {
		why = "macOS lets only apps with Full Disk Access read them"
	}
	text := "Helpers that apps registered are not listed: " + why + "."
	if _, at := p.remembered(); !at.IsZero() {
		text = "Helpers that apps registered are as of " + at.Format("2 Jan 2006, 15:04") + ": " + why + "."
	}
	return &service.Restriction{Text: text, Settings: denied}
}

// Restriction says what the last List left out.
func (p *Provider) Restriction() *service.Restriction {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.restriction
}

// NeedsAccess reports whether macOS refuses the app the database.
func (p *Provider) NeedsAccess() bool {
	_, err := os.ReadDir(p.Dir)
	return errors.Is(err, fs.ErrPermission)
}

// OpenSettings opens the setting that lets the app read the database.
func (*Provider) OpenSettings() error { return system.OpenFullDiskAccessSettings() }

// fallback reads the items where the database cannot be: System Events lists
// the apps that open at login, as of now, and the last answer of `sfltool
// dumpbtm`, if there is one, adds the helpers and what else it knows of the
// apps. It never runs that itself.
func (p *Provider) fallback() ([]Record, error) {
	dump, _ := p.remembered()
	apps, err := openAtLogin()
	if err != nil {
		return dump, err
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

// automationError says where to allow what macOS refused: its error -1743 is
// the user's answer to whether the app may control System Events.
func automationError(err error) error {
	if err != nil && strings.Contains(err.Error(), "-1743") {
		return errors.New("macOS does not let this app control System Events: allow it in System Settings, under Privacy & Security, Automation")
	}
	return err
}

// openAtLogin reads the apps that open at login from System Events, which
// takes no administrator; macOS asks once whether the app may control it.
func openAtLogin() ([]Record, error) {
	out, err := system.Run("osascript", "-e",
		`tell application "System Events" to get the path of every login item`)
	if err != nil {
		return nil, automationError(err)
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

// cached is what the cache file holds: what `sfltool dumpbtm` answered, and
// when.
type cached struct {
	At      time.Time
	Records []Record
}

// remembered returns what `sfltool dumpbtm` last answered, and when, in this
// launch or an earlier one.
func (p *Provider) remembered() ([]Record, time.Time) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.dumpAt.IsZero() && p.CachePath != "" {
		if data, err := os.ReadFile(p.CachePath); err == nil {
			var c cached
			if json.Unmarshal(data, &c) == nil {
				p.dump, p.dumpAt = c.Records, c.At
			}
		}
	}
	return p.dump, p.dumpAt
}

// ReadOnce runs `sfltool dumpbtm`, for which macOS asks for an administrator
// each time, and keeps its answer for the lists to come.
func (p *Provider) ReadOnce() error {
	out, err := system.Run("sfltool", "dumpbtm")
	if err != nil {
		return err
	}
	dump, at := ParseBTM(out, os.Getuid()), time.Now()
	p.mu.Lock()
	p.dump, p.dumpAt = dump, at
	p.mu.Unlock()
	if p.CachePath != "" {
		if data, err := json.Marshal(cached{at, dump}); err == nil {
			if os.MkdirAll(filepath.Dir(p.CachePath), 0o700) == nil {
				_ = os.WriteFile(p.CachePath, data, 0o600)
			}
		}
	}
	return nil
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
		return automationError(err)
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
	return automationError(err)
}

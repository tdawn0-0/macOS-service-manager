package loginitem

import (
	"encoding/hex"
	"fmt"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"time"

	"howett.net/plist"

	"macos-service-manager/internal/system"
)

// The database's files are named BackgroundItems-v<version>.btm, and from
// version 18 BackgroundItems-v<version>-<user>.btm for each user's records.
var databaseRE = regexp.MustCompile(`^BackgroundItems-v(\d+)(?:-(.+))?\.btm$`)

// The values of a record's type, and the bit of its disposition that enables
// it, as `sfltool dumpbtm` prints them: "app (0x2)", "[enabled, ...] (0xb)".
const (
	recordTypeApp       = 0x2
	recordTypeLoginItem = 0x4
	dispositionEnabled  = 0x1
)

// generatedUID returns the identifier macOS knows the user by.
func generatedUID() (string, error) {
	name, err := system.Run("id", "-un")
	if err != nil {
		return "", err
	}
	out, err := system.Run("dscl", ".", "-read", "/Users/"+strings.TrimSpace(name), "GeneratedUID")
	if err != nil {
		return "", err
	}
	fields := strings.Fields(out)
	if len(fields) != 2 {
		return "", fmt.Errorf("unexpected GeneratedUID: %q", out)
	}
	return fields[1], nil
}

// databaseFile returns the newest file holding the user's records.
func databaseFile(dir, user string) (string, error) {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return "", err
	}
	best, bestVersion, bestOwn := "", -1, false
	for _, e := range entries {
		m := databaseRE.FindStringSubmatch(e.Name())
		if m == nil || (m[2] != "" && m[2] != user) {
			continue
		}
		version, _ := strconv.Atoi(m[1])
		own := m[2] == user
		// A version's own file holds the records; its shared one, from
		// version 18, only says whose files there are.
		if version > bestVersion || (version == bestVersion && own && !bestOwn) {
			best, bestVersion, bestOwn = e.Name(), version, own
		}
	}
	if best == "" {
		return "", fmt.Errorf("no database in %s", dir)
	}
	return filepath.Join(dir, best), nil
}

// readDatabase reads the user's records from the database.
func (p *Provider) readDatabase() ([]Record, error) {
	user, err := p.UserID()
	if err != nil {
		return nil, err
	}
	path, err := databaseFile(p.Dir, user)
	if err != nil {
		return nil, err
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	return parseDatabase(data, user)
}

// archive reads the objects of a keyed archive, in which each refers to the
// others by their place.
type archive struct{ objects []any }

// at follows references to a value; the archive's null is nil.
func (a archive) at(v any) any {
	for range 16 {
		uid, ok := v.(plist.UID)
		if !ok {
			break
		}
		if int(uid) >= len(a.objects) {
			return nil
		}
		v = a.objects[uid]
	}
	if s, ok := v.(string); ok && s == "$null" {
		return nil
	}
	return v
}

func (a archive) dict(v any) map[string]any {
	m, _ := a.at(v).(map[string]any)
	return m
}

func (a archive) str(v any) string {
	s, _ := a.at(v).(string)
	return s
}

// class returns the name of the class of an archived object.
func (a archive) class(o map[string]any) string {
	return a.str(a.dict(o["$class"])["$classname"])
}

// list returns the objects of an archived array.
func (a archive) list(v any) []any {
	items, _ := a.dict(v)["NS.objects"].([]any)
	return items
}

func integer(v any) (int, bool) {
	switch n := v.(type) {
	case uint64:
		return int(n), true
	case int64:
		return int(n), true
	case int:
		return n, true
	}
	return 0, false
}

// parseDatabase reads the records of a database file, failing on anything
// it does not know, rather than list less than there is without saying so.
func parseDatabase(data []byte, user string) ([]Record, error) {
	var root map[string]any
	if _, err := plist.Unmarshal(data, &root); err != nil {
		return nil, fmt.Errorf("the database is not a property list: %w", err)
	}
	objects, _ := root["$objects"].([]any)
	top, _ := root["$top"].(map[string]any)
	a := archive{objects}

	var items any
	switch {
	case top["userStore"] != nil:
		items = a.dict(top["userStore"])["records"]
	case top["store"] != nil:
		byUser := a.dict(a.dict(top["store"])["itemsByUserIdentifier"])
		keys, _ := byUser["NS.keys"].([]any)
		values, _ := byUser["NS.objects"].([]any)
		for i, k := range keys {
			if a.str(k) == user && i < len(values) {
				items = values[i]
			}
		}
		if items == nil {
			return nil, nil // the user has no records yet
		}
	default:
		return nil, fmt.Errorf("the database has an unknown format")
	}
	if a.dict(items) == nil {
		return nil, fmt.Errorf("the database has an unknown format")
	}
	var records []Record
	for _, item := range a.list(items) {
		r, err := a.record(a.dict(item))
		if err != nil {
			return nil, err
		}
		records = append(records, r)
	}
	return records, nil
}

// record reads an archived ItemRecord.
func (a archive) record(o map[string]any) (Record, error) {
	if class := a.class(o); class != "ItemRecord" {
		return Record{}, fmt.Errorf("the database holds an unknown record: %q", class)
	}
	kind, ok := integer(a.at(o["type"]))
	if !ok {
		return Record{}, fmt.Errorf("the database has a record without a type")
	}
	disposition, ok := integer(a.at(o["disposition"]))
	if !ok {
		return Record{}, fmt.Errorf("the database has a record without a disposition")
	}
	r := Record{
		Name:       a.str(o["name"]),
		Developer:  a.str(o["developerName"]),
		Team:       a.str(o["teamIdentifier"]),
		Identifier: a.str(o["identifier"]),
		Bundle:     a.str(o["bundleIdentifier"]),
		Parent:     a.str(o["container"]),
		Enabled:    disposition&dispositionEnabled != 0,
	}
	switch kind {
	case recordTypeApp:
		r.Type = typeApp
	case recordTypeLoginItem:
		r.Type = typeLoginItem
	default:
		r.Type = fmt.Sprintf("0x%x", kind)
	}
	if id, ok := a.dict(o["uuid"])["NS.uuidbytes"].([]byte); ok {
		r.UUID = hex.EncodeToString(id)
	}
	if r.UUID == "" {
		return Record{}, fmt.Errorf("the database has a record without an identifier")
	}
	r.URL = a.path(a.dict(o["url"]), r.Type == typeLoginItem)
	if t, ok := a.dict(o["lastUseDate"])["NS.time"].(float64); ok {
		r.LastUse = time.Unix(978307200+int64(t), 0).Format("2006-01-02 15:04")
	}
	return r, nil
}

// path returns the path of an archived NSURL, decoded. An app's is a whole
// URL. A helper's is relative to the bundle of its app, as
// "Contents/Library/LoginItems/My%20Helper.app" on a base of "file:///".
func (a archive) path(o map[string]any, relative bool) string {
	raw := a.str(o["NS.relative"])
	if raw == "" {
		return ""
	}
	u, err := url.Parse(raw)
	if err != nil || (u.Scheme != "" && u.Scheme != "file") {
		return raw
	}
	path := strings.TrimSuffix(u.Path, "/")
	if relative {
		path = strings.TrimPrefix(path, "/")
	}
	return path
}

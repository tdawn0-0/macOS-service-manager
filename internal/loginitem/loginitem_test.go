package loginitem

import (
	"errors"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	"howett.net/plist"

	"macos-service-manager/internal/launchd"
	"macos-service-manager/internal/service"
	"macos-service-manager/internal/system"
)

// dump is what `sfltool dumpbtm` prints, cut down.
const dump = `========================
 Records for UID -2 : FFFFEEEE-DDDD-CCCC-BBBB-AAAAFFFFFFFE
========================

 Items:

 #1:
                 UUID: AAAAAAAA-0000-0000-0000-000000000001
                 Name: bash
                 Type: legacy daemon (0x10010)
          Disposition: [enabled, allowed, notified] (0xb)
           Identifier: 16.com.local.regionkext
                  URL: /Library/LaunchDaemons/com.local.regionkext.plist

========================
 Records for UID 501 : A057A343-8A99-493B-8B4A-2E692F9D491A
========================

 Items:

 #1:
                 UUID: 11111111-0000-0000-0000-000000000001
                 Name: CleanShot X
       Developer Name: (null)
      Team Identifier: 8LHDM4TDZ8
                 Type: app (0x2)
                Flags: [  ] (0)
          Disposition: [enabled, allowed, notified] (0xb)
           Identifier: 2.pl.maketheweb.cleanshotx
                  URL: /Applications/CleanShot X.app
    Bundle Identifier: pl.maketheweb.cleanshotx
             Last Use: 2026-11-01 10:00:00+08:00

 #2:
                 UUID: 11111111-0000-0000-0000-000000000002
                 Name: Affinity
                 Type: app (0x2)
          Disposition: [disabled, allowed, notified] (0xa)
           Identifier: 2.com.canva.affinity
                  URL: /Applications/Affinity.app

 #3:
                 UUID: 11111111-0000-0000-0000-000000000003
                 Name: WeatherMenu
       Developer Name: Some Dev
                 Type: login item (0x4)
          Disposition: [disabled, disallowed, notified] (0x8)
           Identifier: 4.com.example.WeatherMenu
                  URL: Contents/Library/LoginItems/WeatherMenu.app
    Parent Identifier: 2.com.example.Weather

 #4:
                 UUID: 11111111-0000-0000-0000-000000000004
                 Name: sh
                 Type: legacy agent (0x10008)
          Disposition: [enabled, allowed, notified] (0xb)
                  URL: file:///Users/x/Library/LaunchAgents/a.plist
`

func TestParseBTMKeepsTheUsersRecords(t *testing.T) {
	records := ParseBTM(dump, 501)
	if len(records) != 4 {
		t.Fatalf("want the 4 records of UID 501, got %d: %+v", len(records), records)
	}
	cleanshot := records[0]
	if cleanshot.Name != "CleanShot X" || cleanshot.Type != "app" || !cleanshot.Enabled ||
		cleanshot.URL != "/Applications/CleanShot X.app" || cleanshot.Developer != "" ||
		cleanshot.Bundle != "pl.maketheweb.cleanshotx" || cleanshot.Team != "8LHDM4TDZ8" {
		t.Errorf("record 1 = %+v", cleanshot)
	}
	if records[1].Enabled || records[2].Enabled {
		t.Error("a disabled disposition is not enabled")
	}
	if records[2].Type != "login item" || records[2].Parent != "2.com.example.Weather" {
		t.Errorf("record 3 = %+v", records[2])
	}
	if got := ParseBTM(dump, 999); len(got) != 0 {
		t.Errorf("another user has records: %+v", got)
	}
}

func TestItems(t *testing.T) {
	st := launchd.State{Jobs: map[string]launchd.Job{
		"application.pl.maketheweb.cleanshotx.1.2": {PID: 42},
		"com.example.WeatherMenu":                  {PID: 43},
	}}
	records := ParseBTM(dump, 501)
	records = append(records, Record{Type: typeApp, Identifier: "2.com.example.Weather", Name: "Weather", URL: "/Applications/Weather.app"})
	items := itemsOf(records, st)
	var names []string
	for _, it := range items {
		names = append(names, it.Name)
	}
	// Enabled apps and every login item; not apps that are only known, and not agents.
	if !slices.Equal(names, []string{"CleanShot X", "WeatherMenu"}) {
		t.Fatalf("items = %v", names)
	}
	app, helper := items[0], items[1]
	if app.Autostart != service.AutostartOn || helper.Autostart != service.AutostartOff {
		t.Errorf("autostart: %v, %v", app.Autostart, helper.Autostart)
	}
	if app.State != service.StateRunning || app.PID != 42 || helper.State != service.StateRunning || helper.PID != 43 {
		t.Errorf("processes: %+v, %+v", app, helper)
	}
	if helper.Path != "/Applications/Weather.app/Contents/Library/LoginItems/WeatherMenu.app" || helper.Owner != "Weather" {
		t.Errorf("a helper is within its app: %+v", helper)
	}
	p := New()
	if !slices.Equal(p.Actions(app), []service.Action{service.Remove}) {
		t.Errorf("an app can be removed: %v", p.Actions(app))
	}
	if len(p.Actions(helper)) != 0 {
		t.Errorf("a helper belongs to its app: %v", p.Actions(helper))
	}
	if p.Confirm(app, service.Remove) == "" {
		t.Error("removing asks first")
	}
}

// keyedArchive builds a database file as macOS writes them.
type keyedArchive struct{ objects []any }

func (k *keyedArchive) add(v any) plist.UID {
	k.objects = append(k.objects, v)
	return plist.UID(len(k.objects) - 1)
}

func (k *keyedArchive) class(name string) plist.UID {
	return k.add(map[string]any{"$classname": name, "$classes": []string{name, "NSObject"}})
}

func (k *keyedArchive) record(kind, disposition int, name, identifier, url, container string, id byte) plist.UID {
	uuid := make([]byte, 16)
	uuid[15] = id
	r := map[string]any{
		"$class":           k.class("ItemRecord"),
		"type":             kind,
		"disposition":      disposition,
		"name":             k.add(name),
		"identifier":       k.add(identifier),
		"developerName":    plist.UID(0), // $null
		"teamIdentifier":   k.add("TEAM"),
		"bundleIdentifier": k.add(strings.SplitN(identifier, ".", 2)[1]),
		"container":        plist.UID(0),
		"uuid":             k.add(map[string]any{"$class": k.class("NSUUID"), "NS.uuidbytes": uuid}),
		"url":              k.add(map[string]any{"$class": k.class("NSURL"), "NS.base": plist.UID(0), "NS.relative": k.add(url)}),
		"lastUseDate":      k.add(map[string]any{"$class": k.class("NSDate"), "NS.time": 1000.0}),
	}
	if container != "" {
		r["container"] = k.add(container)
	}
	return k.add(r)
}

func (k *keyedArchive) array(items ...plist.UID) plist.UID {
	objects := make([]any, len(items))
	for i, it := range items {
		objects[i] = it
	}
	return k.add(map[string]any{"$class": k.class("NSMutableArray"), "NS.objects": objects})
}

func (k *keyedArchive) encode(t *testing.T, top map[string]any) []byte {
	t.Helper()
	data, err := plist.Marshal(map[string]any{
		"$version": 100000, "$archiver": "NSKeyedArchiver", "$top": top, "$objects": k.objects,
	}, plist.BinaryFormat)
	if err != nil {
		t.Fatal(err)
	}
	return data
}

func sampleRecords(k *keyedArchive) plist.UID {
	return k.array(
		k.record(0x2, 0xb, "CleanShot X", "2.pl.maketheweb.cleanshotx", "file:///Applications/CleanShot%20X.app/", "", 1),
		k.record(0x2, 0x2, "Weather", "2.com.apple.weather", "file:///System/Applications/Weather.app/", "", 2),
		k.record(0x4, 0xb, "WeatherMenu", "4.com.apple.weather.menu", "Contents/Library/LoginItems/Weather%20Menu.app", "2.com.apple.weather", 3),
		k.record(0x10008, 0xb, "sh", "8.com.example.agent", "file:///x/a.plist", "", 4),
	)
}

// The per-user file of version 18 and later.
func userStoreFile(t *testing.T) []byte {
	t.Helper()
	k := &keyedArchive{}
	k.add("$null")
	records := sampleRecords(k)
	store := k.add(map[string]any{"$class": k.class("BTMUserStore"), "records": records, "userIdentifier": k.add("USER")})
	return k.encode(t, map[string]any{"userStore": store})
}

func TestParseDatabase(t *testing.T) {
	records, err := parseDatabase(userStoreFile(t), "USER")
	if err != nil {
		t.Fatal(err)
	}
	if len(records) != 4 {
		t.Fatalf("records = %+v", records)
	}
	app, weather, helper := records[0], records[1], records[2]
	if app.Name != "CleanShot X" || app.URL != "/Applications/CleanShot X.app" || !app.Enabled || app.Type != typeApp ||
		app.Developer != "" || app.Team != "TEAM" || app.Bundle != "pl.maketheweb.cleanshotx" || app.UUID == "" {
		t.Errorf("app = %+v", app)
	}
	if weather.Enabled {
		t.Errorf("a disposition without its first bit is not enabled: %+v", weather)
	}
	if helper.Type != typeLoginItem || helper.URL != "Contents/Library/LoginItems/Weather Menu.app" || helper.Parent != "2.com.apple.weather" {
		t.Errorf("helper = %+v", helper)
	}
	if want := time.Unix(978307200+1000, 0).Format("2006-01-02 15:04"); app.LastUse != want {
		t.Errorf("last use = %q, want %q", app.LastUse, want)
	}
	items := itemsOf(records, launchd.State{})
	if len(items) != 2 || items[1].Path != "/System/Applications/Weather.app/Contents/Library/LoginItems/Weather Menu.app" {
		t.Errorf("items = %+v", items)
	}
}

// The one file of versions before 18, with the records of every user.
func TestParseDatabaseOfSeveralUsers(t *testing.T) {
	k := &keyedArchive{}
	k.add("$null")
	records := sampleRecords(k)
	other := k.array()
	byUser := k.add(map[string]any{
		"$class":     k.class("NSMutableDictionary"),
		"NS.keys":    []any{k.add("OTHER"), k.add("USER")},
		"NS.objects": []any{other, records},
	})
	store := k.add(map[string]any{"$class": k.class("Storage"), "itemsByUserIdentifier": byUser})
	data := k.encode(t, map[string]any{"version": 16, "store": store})
	if got, err := parseDatabase(data, "USER"); err != nil || len(got) != 4 {
		t.Errorf("USER: %+v, %v", got, err)
	}
	if got, err := parseDatabase(data, "OTHER"); err != nil || len(got) != 0 {
		t.Errorf("OTHER: %+v, %v", got, err)
	}
	if got, err := parseDatabase(data, "NOBODY"); err != nil || len(got) != 0 {
		t.Errorf("NOBODY: %+v, %v", got, err)
	}
}

func TestParseDatabaseFailsOnAFormatItDoesNotKnow(t *testing.T) {
	if _, err := parseDatabase([]byte("not a plist"), "USER"); err == nil {
		t.Error("garbage should fail")
	}
	k := &keyedArchive{}
	k.add("$null")
	unknown := k.encode(t, map[string]any{"somethingNew": k.add("x")})
	if _, err := parseDatabase(unknown, "USER"); err == nil {
		t.Error("an unknown top should fail")
	}
	k = &keyedArchive{}
	k.add("$null")
	renamed := k.add(map[string]any{"$class": k.class("ItemRecord"), "kind": 2})
	store := k.add(map[string]any{"$class": k.class("BTMUserStore"), "records": k.array(renamed)})
	if _, err := parseDatabase(k.encode(t, map[string]any{"userStore": store}), "USER"); err == nil {
		t.Error("a record that lost its fields should fail, not vanish")
	}
}

func TestDatabaseFileChoosesTheNewestOfTheUser(t *testing.T) {
	dir := t.TempDir()
	for _, name := range []string{
		"BackgroundItems-v16.btm", "BackgroundItems-v18.btm", "BackgroundItems-v18-USER.btm",
		"BackgroundItems-v19-OTHER.btm", "BackgroundItems-v9-USER.btm", "other.txt",
	} {
		os.WriteFile(filepath.Join(dir, name), nil, 0o644)
	}
	got, err := databaseFile(dir, "USER")
	if err != nil || filepath.Base(got) != "BackgroundItems-v18-USER.btm" {
		t.Errorf("got %q, %v", got, err)
	}
	if _, err := databaseFile(t.TempDir(), "USER"); err == nil {
		t.Error("no file should fail")
	}
}

// fallbackProvider has a database that cannot be read, and a stubbed macOS
// that counts the prompts.
func fallbackProvider(t *testing.T, dumpErr error) (*Provider, *int) {
	t.Helper()
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "BackgroundItems-v99-USER.btm"), []byte("a new format"), 0o644); err != nil {
		t.Fatal(err)
	}
	old := system.Run
	t.Cleanup(func() { system.Run = old })
	dumps := 0
	system.Run = func(name string, args ...string) (string, error) {
		switch name {
		case "sfltool":
			dumps++
			return strings.ReplaceAll(dump, "UID 501", "UID "+itoa(os.Getuid())), dumpErr
		case "osascript":
			return "/Applications/CleanShot X.app, /Applications/New.app\n", nil
		}
		return "", nil
	}
	return &Provider{Dir: dir, CachePath: filepath.Join(t.TempDir(), "cache.json"), UserID: func() (string, error) { return "USER", nil }}, &dumps
}

func itoa(n int) string {
	var b []byte
	for ; n > 0; n /= 10 {
		b = append([]byte{byte('0' + n%10)}, b...)
	}
	return string(b)
}

func TestTheDatabaseAsksForNothing(t *testing.T) {
	p, dumps := fallbackProvider(t, nil)
	if err := os.WriteFile(filepath.Join(p.Dir, "BackgroundItems-v100-USER.btm"), userStoreFile(t), 0o644); err != nil {
		t.Fatal(err)
	}
	items, err := p.List()
	if err != nil {
		t.Fatal(err)
	}
	if len(items) != 2 || *dumps != 0 {
		t.Errorf("items = %+v; sfltool ran %d times", items, *dumps)
	}
}

func TestFallbackAsksOnceAndRemembers(t *testing.T) {
	p, dumps := fallbackProvider(t, nil)
	names := func(items []service.Item) (got []string) {
		for _, it := range items {
			got = append(got, it.Name)
		}
		return got
	}
	items, err := p.List()
	if err != nil {
		t.Fatal(err)
	}
	// CleanShot X with what the dump knows of it, New from System Events.
	if got := names(items); !slices.Equal(got, []string{"CleanShot X", "WeatherMenu", "New"}) {
		t.Errorf("items = %v", got)
	}
	if items[0].ID != "login-item:11111111-0000-0000-0000-000000000001" {
		t.Errorf("an app the dump holds keeps its record: %+v", items[0])
	}
	for range 3 {
		if _, err := p.List(); err != nil {
			t.Fatal(err)
		}
	}
	if *dumps != 1 {
		t.Errorf("sfltool ran %d times in a launch, want 1", *dumps)
	}

	// The next launch: the database is as it was, so it asks nothing.
	next := &Provider{Dir: p.Dir, CachePath: p.CachePath, UserID: p.UserID}
	if _, err := next.List(); err != nil {
		t.Fatal(err)
	}
	if *dumps != 1 {
		t.Errorf("sfltool ran again with an unchanged database: %d", *dumps)
	}

	// The database changed: the next launch asks once more.
	later := time.Now().Add(time.Hour)
	if err := os.Chtimes(filepath.Join(p.Dir, "BackgroundItems-v99-USER.btm"), later, later); err != nil {
		t.Fatal(err)
	}
	if _, err := next.List(); err != nil {
		t.Fatal(err)
	}
	if *dumps != 2 {
		t.Errorf("sfltool ran %d times after the database changed, want 2", *dumps)
	}
	if _, err := next.List(); err != nil {
		t.Fatal(err)
	}
	if *dumps != 2 {
		t.Errorf("and then no more: %d", *dumps)
	}
}

func TestACanceledPromptIsNotRepeated(t *testing.T) {
	p, dumps := fallbackProvider(t, errors.New("User canceled"))
	for range 3 {
		items, err := p.List()
		if err == nil || !strings.Contains(err.Error(), "background helpers are not listed") {
			t.Errorf("err = %v", err)
		}
		// The apps are still there.
		if len(items) != 2 {
			t.Errorf("items = %+v", items)
		}
	}
	if *dumps != 1 {
		t.Errorf("sfltool ran %d times, want 1", *dumps)
	}
}

func TestAddAndRemoveScripts(t *testing.T) {
	old := system.Run
	defer func() { system.Run = old }()
	var got [][]string
	system.Run = func(name string, args ...string) (string, error) {
		got = append(got, append([]string{name}, args...))
		return "1\n", nil
	}
	p := New()
	if err := p.Add("/System/Applications/Calculator.app"); err != nil {
		t.Fatal(err)
	}
	if err := p.Do(service.Item{Path: `/Applications/My "App".app`}, service.Remove); err != nil {
		t.Fatal(err)
	}
	const path = `"/Applications/My \"App\".app"`
	want := [][]string{
		{"osascript", "-e", `tell application "System Events" to make login item at end with properties {path:"/System/Applications/Calculator.app", hidden:false}`},
		{"osascript", "-e", `tell application "System Events"
	set found to count of (every login item whose path is ` + path + `)
	if found > 0 then delete (every login item whose path is ` + path + `)
	return found
end tell`},
	}
	if !slices.EqualFunc(got, want, slices.Equal) {
		t.Errorf("got %q\nwant %q", got, want)
	}
	if err := p.Add("/etc/hosts"); err == nil {
		t.Error("only apps can be added")
	}
}

func TestRemovingAnAppThatRegisteredItselfSaysSo(t *testing.T) {
	old := system.Run
	defer func() { system.Run = old }()
	system.Run = func(string, ...string) (string, error) { return "0\n", nil }
	err := New().Do(service.Item{Name: "Vitals", Path: "/Applications/Vitals.app"}, service.Remove)
	if err == nil || !strings.Contains(err.Error(), "System Settings") {
		t.Errorf("err = %v", err)
	}
}

func TestDetailUsesTheLastList(t *testing.T) {
	old := system.Run
	defer func() { system.Run = old }()
	runs := 0
	system.Run = func(string, ...string) (string, error) {
		runs++
		return dump, nil
	}
	p := New()
	p.records = ParseBTM(dump, 501)
	items := itemsOf(p.records, launchd.State{})
	d, err := p.Detail(items[0])
	if err != nil {
		t.Fatal(err)
	}
	if runs != 0 {
		t.Errorf("Detail ran %d commands; it should use the last List", runs)
	}
	rows := map[string]string{}
	for _, r := range d.Rows {
		rows[r.Key] = r.Value
	}
	if rows["Path"] != "/Applications/CleanShot X.app" || rows["Kind"] != "App that opens at login" || rows["Enabled"] != "Yes" {
		t.Errorf("rows = %+v", d.Rows)
	}
	if _, err := p.Detail(service.Item{ID: "login-item:gone", Name: "gone"}); err == nil {
		t.Error("an item the list does not hold should fail")
	}
}

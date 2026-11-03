package loginitem

import (
	"slices"
	"testing"

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
	items := itemsOf(ParseBTM(dump, 501))
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

func TestAddAndRemoveScripts(t *testing.T) {
	old := system.Run
	defer func() { system.Run = old }()
	var got [][]string
	system.Run = func(name string, args ...string) (string, error) {
		got = append(got, append([]string{name}, args...))
		return "", nil
	}
	p := New()
	if err := p.Add("/System/Applications/Calculator.app"); err != nil {
		t.Fatal(err)
	}
	if err := p.Do(service.Item{Path: `/Applications/My "App".app`}, service.Remove); err != nil {
		t.Fatal(err)
	}
	want := [][]string{
		{"osascript", "-e", `tell application "System Events" to make login item at end with properties {path:"/System/Applications/Calculator.app", hidden:false}`},
		{"osascript", "-e", `tell application "System Events" to delete (every login item whose path is "/Applications/My \"App\".app")`},
	}
	if !slices.EqualFunc(got, want, slices.Equal) {
		t.Errorf("got %q\nwant %q", got, want)
	}
	if err := p.Add("/etc/hosts"); err == nil {
		t.Error("only apps can be added")
	}
}

func TestDetailUsesTheLastList(t *testing.T) {
	old := system.Run
	defer func() { system.Run = old }()
	dumps := 0
	system.Run = func(string, ...string) (string, error) {
		dumps++
		return dump, nil
	}
	p := New()
	// ParseBTM reads the records of the user it is run as.
	p.records = ParseBTM(dump, 501)
	items := itemsOf(p.records)
	d, err := p.Detail(items[0])
	if err != nil {
		t.Fatal(err)
	}
	if dumps != 0 {
		t.Errorf("Detail dumped the database %d times; it should use the last List", dumps)
	}
	if len(d.Rows) == 0 || d.Rows[0].Value != "CleanShot X" {
		t.Errorf("rows = %+v", d.Rows)
	}
	if _, err := p.Detail(service.Item{ID: "login-item:gone", Name: "gone"}); err == nil {
		t.Error("an item the list does not hold should fail")
	}
}

package service

import "testing"

func TestItemStatesAndMatches(t *testing.T) {
	for state, want := range map[State][2]bool{
		StateNone:      {false, false},
		StateStopped:   {false, false},
		StateLoaded:    {false, false},
		StateRunning:   {true, true},
		StateScheduled: {true, true},
		StateFailed:    {true, false},
	} {
		it := Item{State: state}
		if it.Active() != want[0] || it.Up() != want[1] {
			t.Errorf("%v: Active %v, Up %v, want %v", state, it.Active(), it.Up(), want)
		}
	}
	it := Item{Name: "Redis", Owner: "jyeu", Path: "/opt/homebrew/opt/redis/sh.brew.redis.plist"}
	for query, want := range map[string]bool{"": true, "  ": true, "red": true, "JYEU": true, "homebrew": true, "nginx": false} {
		if got := it.Matches(query); got != want {
			t.Errorf("Matches(%q) = %v, want %v", query, got, want)
		}
	}
}

func TestPrimaryAndToggle(t *testing.T) {
	actions := []Action{Enable, Start, Restart}
	if got := Primary(actions); got != Start {
		t.Errorf("Primary = %q, want start", got)
	}
	if got := Toggle(actions); got != Enable {
		t.Errorf("Toggle = %q, want enable", got)
	}
	if Primary([]Action{Disable, Remove}) != "" || Toggle([]Action{Start}) != "" {
		t.Error("an item with only the toggle has no primary action, one without it no toggle")
	}
	if !Has(actions, Start) || Has(actions, Stop) {
		t.Error("Has")
	}
}

func TestPastReadsAsEnglish(t *testing.T) {
	for a, want := range map[Action]string{
		Start: "started", Stop: "stopped", Restart: "restarted",
		Load: "loaded", Unload: "unloaded", Enable: "enabled", Disable: "disabled", Remove: "removed",
	} {
		if got := a.Past(); got != want {
			t.Errorf("%s.Past() = %q, want %q", a, got, want)
		}
		if a.Label() == string(a) {
			t.Errorf("%s has no label", a)
		}
	}
}

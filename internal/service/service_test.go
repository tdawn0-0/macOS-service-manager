package service

import "testing"

func TestItemActiveAndMatches(t *testing.T) {
	for state, want := range map[State]bool{
		StateNone: false, StateStopped: false, StateLoaded: false,
		StateRunning: true, StateScheduled: true, StateFailed: true,
	} {
		if got := (Item{State: state}).Active(); got != want {
			t.Errorf("Active(%v) = %v, want %v", state, got, want)
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
	actions := []Action{Enable, Run, Start}
	if got := Primary(actions); got != Run {
		t.Errorf("Primary = %q, want run", got)
	}
	if got := Toggle(actions); got != Enable {
		t.Errorf("Toggle = %q, want enable", got)
	}
	if Primary([]Action{Disable, Remove}) != "" || Toggle([]Action{Run}) != "" {
		t.Error("an item with only the toggle has no primary action, one without it no toggle")
	}
	if !Has(actions, Start) || Has(actions, Stop) {
		t.Error("Has")
	}
}

func TestPastReadsAsEnglish(t *testing.T) {
	for a, want := range map[Action]string{
		Run: "ran", Start: "started", Stop: "stopped", Restart: "restarted", Kill: "killed",
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

// Package service is the model every source of startup items shares: a
// Homebrew service, a launch agent or daemon, a login item.
package service

import "strings"

// Source is where an item comes from.
type Source string

const (
	Brew        Source = "brew"
	UserAgent   Source = "user-agent"   // ~/Library/LaunchAgents
	SystemAgent Source = "system-agent" // /Library/LaunchAgents
	Daemon      Source = "daemon"       // /Library/LaunchDaemons
	LoginItem   Source = "login-item"   // System Settings › General › Login Items
)

// Sources lists the sources in the order the interface shows them.
var Sources = []Source{Brew, LoginItem, UserAgent, SystemAgent, Daemon}

// Title is what the interface calls a source.
func (s Source) Title() string {
	switch s {
	case Brew:
		return "Homebrew"
	case UserAgent:
		return "User Agents"
	case SystemAgent:
		return "System Agents"
	case Daemon:
		return "Daemons"
	case LoginItem:
		return "Login Items"
	}
	return string(s)
}

// State is what an item is doing.
type State int

const (
	StateNone      State = iota // the source knows of no process: a login item that is not open
	StateStopped                // not running, and not loaded where that applies
	StateLoaded                 // registered with launchd, with no process now
	StateRunning                // a process runs
	StateScheduled              // set to run at times
	StateFailed                 // ran and failed
)

func (s State) String() string {
	switch s {
	case StateStopped:
		return "Stopped"
	case StateLoaded:
		return "Idle"
	case StateRunning:
		return "Running"
	case StateScheduled:
		return "Scheduled"
	case StateFailed:
		return "Error"
	}
	return ""
}

// Autostart says whether an item starts by itself, at login or at boot.
type Autostart int

const (
	AutostartUnknown Autostart = iota // the source cannot tell, or has no such thing
	AutostartOn
	AutostartOff
)

// Item is one startup item.
type Item struct {
	// ID identifies the item among all sources.
	ID     string
	Source Source
	Name   string
	State  State
	// PID is the process ID, 0 for none.
	PID       int
	Autostart Autostart
	// Owner is the user, developer or domain the item belongs to, if known.
	Owner string
	// Path is the file that defines the item: a property list or an app.
	Path string
	// Kind says what the item is, as "Background helper", if the source
	// holds more than one kind.
	Kind string
	// Admin says that changing the item takes an administrator: macOS asks
	// the user to authenticate.
	Admin bool
}

// Active reports whether the item runs, is scheduled to, or failed while
// running: the states in which stopping it makes sense.
func (i Item) Active() bool {
	switch i.State {
	case StateRunning, StateScheduled, StateFailed:
		return true
	}
	return false
}

// Up reports whether the item runs, or is set to run at its times, as the
// Running filter shows.
func (i Item) Up() bool { return i.State == StateRunning || i.State == StateScheduled }

// Matches reports whether the item's name, owner or path holds the query,
// ignoring case.
func (i Item) Matches(query string) bool {
	query = strings.ToLower(strings.TrimSpace(query))
	if query == "" {
		return true
	}
	for _, s := range []string{i.Name, i.Owner, i.Path} {
		if strings.Contains(strings.ToLower(s), query) {
			return true
		}
	}
	return false
}

// Action is something done to an item.
type Action string

const (
	Start   Action = "start"   // start now, leaving the autostart as it is
	Stop    Action = "stop"    // stop now, leaving the autostart as it is
	Restart Action = "restart" // stop and start again
	Load    Action = "load"    // register with launchd
	Unload  Action = "unload"  // unregister from launchd
	Enable  Action = "enable"  // let it start by itself
	Disable Action = "disable" // keep it from starting by itself
	Remove  Action = "remove"  // delete it
)

// Label is the word on the button of an action.
func (a Action) Label() string {
	switch a {
	case Start:
		return "Start"
	case Stop:
		return "Stop"
	case Restart:
		return "Restart"
	case Load:
		return "Load"
	case Unload:
		return "Unload"
	case Enable:
		return "Enable"
	case Disable:
		return "Disable"
	case Remove:
		return "Remove"
	}
	return string(a)
}

// Past is what an action did, for a message.
func (a Action) Past() string {
	switch a {
	case Start:
		return "started"
	case Stop:
		return "stopped"
	case Restart:
		return "restarted"
	case Load:
		return "loaded"
	case Unload:
		return "unloaded"
	case Enable:
		return "enabled"
	case Disable:
		return "disabled"
	case Remove:
		return "removed"
	}
	return string(a)
}

// Row is one labeled value of an item's details, under the heading of its
// group.
type Row struct{ Group, Key, Value string }

// LogFile is a file an item writes its output to.
type LogFile struct{ Label, Path string }

// Detail is what an item shows beyond the list: its values and its logs.
type Detail struct {
	Rows []Row
	Logs []LogFile
}

// Provider is a source of items.
type Provider interface {
	Source() Source
	// List returns the items of the source. It may return items and an error
	// together: what it could read, and why that is not everything.
	List() ([]Item, error)
	// Detail reads an item's values and log files.
	Detail(Item) (Detail, error)
	// Actions returns the actions that apply to the item now, the main one
	// first. It reads nothing, so it can run for every frame.
	Actions(Item) []Action
	// Confirm returns what to ask the user before an action that has
	// consequences they may not expect, or "" if it needs no question. It
	// reads nothing, so it can run for every frame.
	Confirm(Item, Action) string
	// Do runs an action.
	Do(Item, Action) error
}

// Restriction is what macOS keeps a provider from listing.
type Restriction struct {
	// Text says what the list leaves out, and why.
	Text string
	// Settings says that System Settings can lift it.
	Settings bool
}

// Restricted is a provider that macOS may let read only part of its items.
type Restricted interface {
	// Restriction says what the last List left out, or nil if nothing.
	Restriction() *Restriction
	// NeedsAccess reports whether macOS refuses what the setting allows. It
	// asks the user nothing, and is quick enough to run every second.
	NeedsAccess() bool
	// OpenSettings opens the setting that lifts the restriction.
	OpenSettings() error
	// ReadOnce reads what is left out for the next List, after macOS asks the
	// user to authenticate. Only the user's asking may start it.
	ReadOnce() error
}

// Batcher is a provider that runs an action on several items at once, so
// that macOS asks the user to authenticate only once.
type Batcher interface {
	DoAll([]Item, Action) error
}

// Has reports whether actions holds a.
func Has(actions []Action, a Action) bool {
	for _, x := range actions {
		if x == a {
			return true
		}
	}
	return false
}

// Toggle returns the action that flips an item's autostart, or "" if the
// item has no autostart to flip.
func Toggle(actions []Action) Action {
	for _, a := range actions {
		if a == Enable || a == Disable {
			return a
		}
	}
	return ""
}

// Primary returns the main action of an item with the actions, leaving out
// the autostart toggle, or "" for none.
func Primary(actions []Action) Action {
	for _, a := range actions {
		if a != Enable && a != Disable && a != Remove {
			return a
		}
	}
	return ""
}

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
var Sources = []Source{Brew, UserAgent, SystemAgent, Daemon, LoginItem}

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

// Short is a word for an item's source in a list of several.
func (s Source) Short() string {
	switch s {
	case Brew:
		return "brew"
	case UserAgent:
		return "user agent"
	case SystemAgent:
		return "system agent"
	case Daemon:
		return "daemon"
	case LoginItem:
		return "login item"
	}
	return string(s)
}

// State is what an item is doing.
type State int

const (
	StateNone      State = iota // the source says nothing of a process: login items
	StateStopped                // not running, and not loaded where that applies
	StateLoaded                 // registered with launchd, with no process now
	StateRunning                // a process runs
	StateScheduled              // set to run at times
	StateFailed                 // ran and failed
)

func (s State) String() string {
	switch s {
	case StateStopped:
		return "stopped"
	case StateLoaded:
		return "loaded"
	case StateRunning:
		return "running"
	case StateScheduled:
		return "scheduled"
	case StateFailed:
		return "error"
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
	Run     Action = "run"     // start now, and leave the autostart as it is
	Start   Action = "start"   // start now, and register it to start by itself
	Stop    Action = "stop"    // stop now
	Restart Action = "restart" // stop and start again
	Kill    Action = "kill"    // stop now but keep it registered
	Load    Action = "load"    // register with launchd
	Unload  Action = "unload"  // unregister from launchd
	Enable  Action = "enable"  // let it start by itself
	Disable Action = "disable" // keep it from starting by itself
	Remove  Action = "remove"  // delete it
)

// Label is the word on the button of an action.
func (a Action) Label() string {
	switch a {
	case Run:
		return "Run"
	case Start:
		return "Start"
	case Stop:
		return "Stop"
	case Restart:
		return "Restart"
	case Kill:
		return "Kill"
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
	case Run:
		return "ran"
	case Start:
		return "started"
	case Stop:
		return "stopped"
	case Restart:
		return "restarted"
	case Kill:
		return "killed"
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

// Row is one labeled value of an item's details.
type Row struct{ Key, Value string }

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
	// List returns the items of the source.
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

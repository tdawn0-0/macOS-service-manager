// Package detail describes the rows of a service's detail view.
package detail

import (
	"fmt"
	"strconv"
)

// Row is one labeled value of a detail view.
type Row struct {
	Key   string
	Value string
	// LogPath marks a value that is the path of a log file.
	LogPath bool
}

// Rows collects rows, leaving out the values that are not set.
type Rows []Row

// Add appends a row with a text value.
func (r *Rows) Add(key, value string) { *r = append(*r, Row{Key: key, Value: value}) }

// AddLog appends a row whose value is the path of a log file.
func (r *Rows) AddLog(key string, value *string) {
	if value != nil {
		*r = append(*r, Row{Key: key, Value: *value, LogPath: true})
	}
}

// AddString appends a row if the value is set.
func (r *Rows) AddString(key string, value *string) {
	if value != nil {
		r.Add(key, *value)
	}
}

// AddBool appends a row if the value is set.
func (r *Rows) AddBool(key string, value *bool) {
	if value != nil {
		r.Add(key, strconv.FormatBool(*value))
	}
}

// AddInt appends a row if the value is set.
func (r *Rows) AddInt(key string, value *int) {
	if value != nil {
		r.Add(key, fmt.Sprint(*value))
	}
}

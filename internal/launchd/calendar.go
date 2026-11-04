package launchd

import (
	"fmt"
	"slices"
	"strconv"
	"strings"
)

// maxCalendarEntries bounds the start times of a calendar: launchd takes each
// combination of the fields' values as an entry of the property list.
const maxCalendarEntries = 1000

// Calendar is a schedule in the fields of cron, of which each is a number, a
// list (1,15), a range (1-5), an interval (*/10), or * or "" for any value.
type Calendar struct{ Minute, Hour, Day, Month, Weekday string }

type calendarField struct {
	key      string
	min, max int
}

// calendarFields are in the order of Calendar.values. Weekday also takes 7 for
// Sunday, as launchd does.
var calendarFields = [...]calendarField{
	{"Minute", 0, 59},
	{"Hour", 0, 23},
	{"Day", 1, 31},
	{"Month", 1, 12},
	{"Weekday", 0, 7},
}

func (c Calendar) values() [5]string {
	return [5]string{c.Minute, c.Hour, c.Day, c.Month, c.Weekday}
}

// Empty reports that no field is set: there is no calendar.
func (c Calendar) Empty() bool {
	for _, v := range c.values() {
		if strings.TrimSpace(v) != "" {
			return false
		}
	}
	return true
}

// parse returns the values a field's text stands for, or nil for any value.
func (f calendarField) parse(text string) ([]int, error) {
	name := strings.ToLower(f.key)
	text = strings.TrimSpace(text)
	if text == "" || text == "*" {
		return nil, nil
	}
	number := func(s string) (int, error) {
		n, err := strconv.Atoi(strings.TrimSpace(s))
		if err != nil {
			return 0, fmt.Errorf("the %s %q is not a number, a list, a range or an interval", name, text)
		}
		if n < f.min || n > f.max {
			return 0, fmt.Errorf("the %s %d is outside %d-%d", name, n, f.min, f.max)
		}
		return n, nil
	}
	var values []int
	for _, part := range strings.Split(text, ",") {
		part = strings.TrimSpace(part)
		if step, ok := strings.CutPrefix(part, "*/"); ok {
			n, err := strconv.Atoi(step)
			if err != nil || n < 1 {
				return nil, fmt.Errorf("the %s interval %q is not a positive number", name, part)
			}
			if n == 1 && part == text {
				return nil, nil
			}
			for v := f.min; v <= f.max; v += n {
				values = append(values, v)
			}
			continue
		}
		first, last, isRange := strings.Cut(part, "-")
		start, err := number(first)
		if err != nil {
			return nil, err
		}
		end := start
		if isRange {
			if end, err = number(last); err != nil {
				return nil, err
			}
			if end < start {
				return nil, fmt.Errorf("the %s range %q goes backwards", name, part)
			}
		}
		for v := start; v <= end; v++ {
			values = append(values, v)
		}
	}
	slices.Sort(values)
	return slices.Compact(values), nil
}

// Entries returns the calendar as launchd's StartCalendarInterval: its entries
// leave out the fields of any value. An empty calendar has none.
func (c Calendar) Entries() ([]map[string]int, error) {
	if c.Empty() {
		return nil, nil
	}
	entries := []map[string]int{{}}
	for i, text := range c.values() {
		field := calendarFields[i]
		values, err := field.parse(text)
		if err != nil {
			return nil, err
		}
		if values == nil {
			continue
		}
		if len(entries)*len(values) > maxCalendarEntries {
			return nil, fmt.Errorf("the calendar has more than %d start times: use fewer values, or an interval", maxCalendarEntries)
		}
		next := make([]map[string]int, 0, len(entries)*len(values))
		for _, entry := range entries {
			for _, v := range values {
				e := make(map[string]int, len(entry)+1)
				for k, n := range entry {
					e[k] = n
				}
				e[field.key] = v
				next = append(next, e)
			}
		}
		entries = next
	}
	// Any value in every field is every minute, which launchd's documentation
	// only describes through missing keys of an entry that has some.
	if len(entries[0]) == 0 {
		entries = entries[:0]
		for minute := 0; minute < 60; minute++ {
			entries = append(entries, map[string]int{"Minute": minute})
		}
	}
	return entries, nil
}

// formatField writes values as the text of a field, the shortest way.
func (f calendarField) formatField(values []int) string {
	if len(values) >= 3 && values[0] == f.min {
		step := values[1] - values[0]
		regular := step > 1 && values[len(values)-1]+step > f.max
		for i := 1; regular && i < len(values); i++ {
			regular = values[i]-values[i-1] == step
		}
		if regular {
			return "*/" + strconv.Itoa(step)
		}
	}
	var parts []string
	for i := 0; i < len(values); {
		j := i
		for j+1 < len(values) && values[j+1] == values[j]+1 {
			j++
		}
		switch {
		case j-i >= 2:
			parts = append(parts, fmt.Sprintf("%d-%d", values[i], values[j]))
		default:
			for k := i; k <= j; k++ {
				parts = append(parts, strconv.Itoa(values[k]))
			}
		}
		i = j + 1
	}
	return strings.Join(parts, ",")
}

// calendarFrom reads StartCalendarInterval, if the fields of a Calendar can say
// what it says: its entries must be all the combinations of their values.
func calendarFrom(value any) (Calendar, bool) {
	var raw []any
	switch v := value.(type) {
	case map[string]any:
		raw = []any{v}
	case []any:
		raw = v
	default:
		return Calendar{}, false
	}
	if len(raw) == 0 {
		return Calendar{}, false
	}
	seen := map[[5]int]bool{}
	sets := [5][]int{}
	var present [5]bool
	for n, item := range raw {
		entry, ok := item.(map[string]any)
		if !ok || len(entry) == 0 {
			return Calendar{}, false
		}
		known := 0
		key := [5]int{-1, -1, -1, -1, -1}
		for i, field := range calendarFields {
			v, has := entry[field.key]
			if n == 0 {
				present[i] = has
			}
			if has != present[i] {
				return Calendar{}, false
			}
			if !has {
				continue
			}
			known++
			number, ok := plistInt(v)
			if !ok || number < field.min || number > field.max {
				return Calendar{}, false
			}
			key[i] = number
			if !slices.Contains(sets[i], number) {
				sets[i] = append(sets[i], number)
			}
		}
		if known != len(entry) || seen[key] {
			return Calendar{}, false
		}
		seen[key] = true
	}
	product := 1
	for i := range sets {
		if present[i] {
			product *= len(sets[i])
		}
	}
	if product != len(raw) {
		return Calendar{}, false
	}
	var text [5]string
	for i, field := range calendarFields {
		if present[i] {
			slices.Sort(sets[i])
			text[i] = field.formatField(sets[i])
		}
	}
	return Calendar{Minute: text[0], Hour: text[1], Day: text[2], Month: text[3], Weekday: text[4]}, true
}

func plistInt(v any) (int, bool) {
	switch n := v.(type) {
	case int:
		return n, true
	case int64:
		return int(n), true
	case uint64:
		return int(n), true
	}
	return 0, false
}

package main

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/lobehub/lobe-mygo/pkg/action"
	"github.com/lobehub/lobe-mygo/pkg/theme"

	"devbase/internal/logs"
)

var devLogIDRE = regexp.MustCompile(`^[a-zA-Z0-9][a-zA-Z0-9._-]{0,239}$`)

func devLogTitle(jobID string) string {
	switch {
	case jobID == "proxy":
		return "HTTPS proxy"
	case jobID == "tunnel":
		return "Cloudflare tunnel"
	case strings.HasPrefix(jobID, "db-"):
		return strings.TrimPrefix(jobID, "db-")
	default:
		return jobID
	}
}

func (a *app) devLogTargets() []string {
	targets := []string{"proxy", "tunnel"}
	for _, id := range sortedKeys(a.dev.state.Instances) {
		targets = append(targets, "db-"+id)
	}
	if a.devLog.jobID != "" && !slices.Contains(targets, a.devLog.jobID) {
		targets = append(targets, a.devLog.jobID)
	}
	return targets
}

func (a *app) defaultDevLogJob() string {
	d := &a.dev
	if d.tab == devTabDatabases {
		if d.dbID != "" {
			return "db-" + d.dbID
		}
		if ids := sortedKeys(d.state.Instances); len(ids) > 0 {
			return "db-" + ids[0]
		}
	}
	return "proxy"
}

func (a *app) setDevLogTarget(jobID, title string, stream int) {
	if jobID == "" {
		jobID = a.defaultDevLogJob()
	}
	if title == "" {
		title = devLogTitle(jobID)
	}
	if stream < 0 || stream > 1 {
		stream = 0
	}
	l := &a.devLog
	if l.jobID != jobID || l.stream != stream {
		list := l.list
		list.FollowEnd = true
		*l = devLogState{
			open:   l.open,
			jobID:  jobID,
			title:  title,
			stream: stream,
			query:  l.query,
			list:   list,
		}
		return
	}
	l.title = title
}

func (a *app) openDevLog(jobID, title string) {
	stream := 0
	if jobID == a.devLog.jobID {
		stream = a.devLog.stream
	}
	a.setDevLogTarget(jobID, title, stream)
	a.devLog.open = true
	a.devLog.last = time.Time{}
}

func (a *app) toggleDevLogs() {
	if a.devLog.open {
		a.devLog.open = false
		return
	}
	a.openDevLog(a.devLog.jobID, a.devLog.title)
	a.pollDevLog()
}

func (a *app) devLogPath() string {
	l := &a.devLog
	if !devLogIDRE.MatchString(l.jobID) {
		return ""
	}
	suffix := ".err.log"
	if l.stream == 1 {
		suffix = ".log"
	}
	return filepath.Join(a.devManager().Root, "logs", l.jobID+suffix)
}

// pollDevLog tails the selected workspace job log at most once per logEvery
// while the workspace live log drawer is visible.
func (a *app) pollDevLog() {
	l := &a.devLog
	if a.post == nil || a.nav != navDevelopment || !l.open {
		return
	}
	if l.jobID == "" {
		l.jobID = a.defaultDevLogJob()
		l.title = devLogTitle(l.jobID)
	}
	path := a.devLogPath()
	if path == "" {
		l.path = ""
		l.err = "Invalid log target"
		return
	}
	if path != l.path {
		list := l.list
		list.FollowEnd = true
		*l = devLogState{
			open:   l.open,
			jobID:  l.jobID,
			title:  l.title,
			stream: l.stream,
			query:  l.query,
			path:   path,
			list:   list,
		}
	}
	if l.reading || (!l.last.IsZero() && time.Since(l.last) < logEvery*9/10) {
		return
	}
	l.reading, l.last = true, time.Now()
	size, mod := l.size, l.mod
	known := !l.missing && l.err == "" && l.lines != nil
	a.spawn(func() {
		var snap logs.Snapshot
		var err error
		if s, m, statErr := logs.Stat(path); statErr == nil && known && s == size && m.Equal(mod) {
			snap.Size, snap.ModTime = s, m
			err = errUnchanged
		} else {
			snap, err = logs.Tail(path, logBytes, logLines)
		}
		a.post(func() {
			if a.devLog.path != path {
				return
			}
			a.devLog.reading = false
			switch {
			case err == errUnchanged:
			case errors.Is(err, os.ErrNotExist):
				a.devLog.missing = true
				a.devLog.err = ""
				a.devLog.lines = []string{}
				a.devLog.truncated = false
				a.devLog.size, a.devLog.mod = 0, time.Time{}
			case err != nil:
				a.devLog.missing = false
				a.devLog.err = err.Error()
			default:
				a.devLog.missing = false
				a.devLog.err = ""
				a.devLog.lines = snap.Lines
				if a.devLog.lines == nil {
					a.devLog.lines = []string{}
				}
				a.devLog.truncated = snap.Truncated
				a.devLog.size, a.devLog.mod = snap.Size, snap.ModTime
			}
		})
	})
}

func filterDevLogLines(lines []string, query string) []string {
	q := strings.ToLower(strings.TrimSpace(query))
	if q == "" {
		return lines
	}
	out := make([]string, 0, len(lines))
	for _, line := range lines {
		if strings.Contains(strings.ToLower(line), q) {
			out = append(out, line)
		}
	}
	return out
}

func (a *app) devLogDrawer(c *ui.Context) {
	l := &a.devLog
	if !l.open {
		return
	}
	if l.jobID == "" {
		l.jobID = a.defaultDevLogJob()
		l.title = devLogTitle(l.jobID)
	}
	targets := a.devLogTargets()
	path := a.devLogPath()
	t := c.Theme()
	tk := theme.TokensOf(c)
	g := glassOf(t)
	filtered := filterDevLogLines(l.lines, l.query)

	ui.Column(c).Height(1).Shrink(0).Background(hairline(t))
	ui.Column(c).Height(210).Shrink(0).Padding(10, 16, 10).Gap(6).Background(g.sheet).Children(func() {
		ui.Row(c).Wrap().Shrink(0).Gap(8).AlignItems(ui.Center).Children(func() {
			if ui.Select(c, &l.jobID, targets).Label("Log target").Width(160).Changed() {
				l.title = devLogTitle(l.jobID)
				a.pollDevLog()
			}
			if action.Segmented(c, &l.stream, []string{"Error", "Output"}, action.SegmentedProps{Variant: action.SegmentedFilled, Size: theme.Small}).Changed() {
				a.pollDevLog()
			}
			ui.SearchField(c, &l.query).Placeholder("Filter logs").Label("Filter logs").Grow(1).MinWidth(120)
			if btn(c, "Open in Console", iconOpen, btnText).Disabled(path == "").Clicked() {
				a.openLog(path)
			}
			iconButton(c, iconClose, "Close logs", t.TextMuted).OnClick(func() {
				l.open = false
			})
		})
		if path != "" {
			meta := path
			if len(l.lines) > 0 {
				switch {
				case strings.TrimSpace(l.query) != "":
					meta = fmt.Sprintf("%s · %d/%d lines", filepath.Base(path), len(filtered), len(l.lines))
				case l.truncated:
					meta = fmt.Sprintf("%s · last %d lines, updated %s", filepath.Base(path), len(l.lines), ago(l.last))
				case !l.last.IsZero():
					meta = fmt.Sprintf("%s · updated %s", filepath.Base(path), ago(l.last))
				}
			}
			ui.Text(c, meta).FontSize(textCaption).TextColor(tk.TextTertiary).SingleLine().Tooltip(path)
		}
		switch {
		case l.err != "":
			ui.Text(c, l.err).FontSize(textSmall).TextColor(t.Danger)
		case l.lines == nil:
			ui.Text(c, "Reading…").FontSize(textSmall).TextColor(t.TextMuted)
		case l.missing:
			ui.Text(c, "No logs yet.").FontSize(textSmall).TextColor(t.TextMuted)
		case len(l.lines) == 0:
			ui.Text(c, "The log is empty.").FontSize(textSmall).TextColor(t.TextMuted)
		case len(filtered) == 0:
			ui.Text(c, "No matching log lines.").FontSize(textSmall).TextColor(t.TextMuted)
		default:
			card(c).Grow(1).MinHeight(0).Clip().Children(func() {
				ui.List(c, &l.list, len(filtered), func(i int) {
					ui.Text(c, filtered[i]).Font("Menlo").FontSize(textCaption).Selectable()
				}).Grow(1).MinHeight(0).Padding(8, 10).Gap(2)
			})
		}
	})
}

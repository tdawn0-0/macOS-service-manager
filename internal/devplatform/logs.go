package devplatform

import (
	"devbase/internal/logs"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"
)

var logID = regexp.MustCompile(`^[a-zA-Z0-9][a-zA-Z0-9._-]{0,239}$`)

// Logs bounds disk reads as well as display size, including for active files.
func (m *Manager) Logs(id string) (string, error) {
	if !logID.MatchString(id) {
		return "", fmt.Errorf("invalid job ID")
	}
	var out strings.Builder
	for _, suffix := range []string{".log", ".err.log"} {
		snapshot, e := logs.Tail(filepath.Join(m.Root, "logs", id+suffix), 64<<10, 400)
		if os.IsNotExist(e) {
			continue
		}
		if e != nil {
			return "", e
		}
		fmt.Fprintf(&out, "%s%s", id, suffix)
		if snapshot.Truncated {
			out.WriteString(" (last 400 lines / 64 KiB)")
		}
		out.WriteString(":\n" + strings.Join(snapshot.Lines, "\n") + "\n\n")
	}
	if out.Len() == 0 {
		return "No logs yet. Start the service to capture its output.", nil
	}
	return out.String(), nil
}

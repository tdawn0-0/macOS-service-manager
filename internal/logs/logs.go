// Package logs reads the end of log files.
package logs

import (
	"bytes"
	"io"
	"os"
	"strings"
	"time"
)

// Snapshot is the end of a file.
type Snapshot struct {
	Lines   []string
	Size    int64
	ModTime time.Time
	// Truncated says the file holds more before Lines.
	Truncated bool
}

// Stat returns a file's size and modification time.
func Stat(path string) (int64, time.Time, error) {
	info, err := os.Stat(path)
	if err != nil {
		return 0, time.Time{}, err
	}
	return info.Size(), info.ModTime(), nil
}

// Tail reads the last lines of a file: at most maxLines, from the last
// maxBytes of it.
func Tail(path string, maxBytes int64, maxLines int) (Snapshot, error) {
	f, err := os.Open(path)
	if err != nil {
		return Snapshot{}, err
	}
	defer f.Close()
	info, err := f.Stat()
	if err != nil {
		return Snapshot{}, err
	}
	snap := Snapshot{Size: info.Size(), ModTime: info.ModTime()}
	offset := int64(0)
	if snap.Size > maxBytes {
		offset = snap.Size - maxBytes
		snap.Truncated = true
	}
	buf := make([]byte, snap.Size-offset)
	if _, err := f.ReadAt(buf, offset); err != nil && err != io.EOF {
		return Snapshot{}, err
	}
	if offset > 0 {
		// The first line is probably cut.
		if i := bytes.IndexByte(buf, '\n'); i >= 0 {
			buf = buf[i+1:]
		}
	}
	text := strings.ReplaceAll(strings.TrimRight(string(bytes.ToValidUTF8(buf, []byte("�"))), "\n"), "\r\n", "\n")
	if text == "" {
		return snap, nil
	}
	snap.Lines = strings.Split(text, "\n")
	if len(snap.Lines) > maxLines {
		snap.Lines = snap.Lines[len(snap.Lines)-maxLines:]
		snap.Truncated = true
	}
	return snap, nil
}

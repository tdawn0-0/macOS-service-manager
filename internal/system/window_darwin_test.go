//go:build darwin

package system

import (
	"os"
	"testing"
)

// The list of windows binds and reads, whatever this machine shows.
func TestLargestWindowAsksForNothing(t *testing.T) {
	if _, found := largestWindow(int64(os.Getpid())); found {
		t.Error("a test has no window")
	}
	if !windowList.ok {
		t.Error("the list of windows did not bind")
	}
	if windowList.keyPID == 0 || windowList.keyLayer == 0 || windowList.keyBounds == 0 {
		t.Error("the keys of the list did not bind")
	}
}

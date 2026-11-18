package system

import (
	"strings"
	"testing"
	"time"
)

func TestACommandThatHangsFails(t *testing.T) {
	_, err := RunFor(50*time.Millisecond, "sleep", "30")
	if err == nil || !strings.Contains(err.Error(), "did not answer") {
		t.Errorf("err = %v", err)
	}
}

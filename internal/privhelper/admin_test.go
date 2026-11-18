//go:build darwin

package privhelper

import (
	"errors"
	"testing"
)

func TestAdminScriptQuotes(t *testing.T) {
	got := adminScript("launchctl", "bootstrap", "system", `/Library/LaunchDaemons/it's "a".plist`)
	want := `do shell script "'launchctl' 'bootstrap' 'system' '/Library/LaunchDaemons/it'\\''s \"a\".plist'" with administrator privileges`
	if got != want {
		t.Errorf("\n got %s\nwant %s", got, want)
	}
}

func TestRunAdminCanceled(t *testing.T) {
	old := runOsascript
	defer func() { runOsascript = old }()
	runOsascript = func(string) (string, error) {
		return "", errors.New("execution error: User canceled. (-128)")
	}
	if _, err := runAdmin("true"); err == nil || err.Error() != "authorization canceled" {
		t.Errorf("err = %v", err)
	}
}

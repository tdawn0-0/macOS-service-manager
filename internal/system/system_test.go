package system

import "testing"

func TestAdminScriptQuotes(t *testing.T) {
	got := AdminScript("launchctl", "bootstrap", "system", `/Library/LaunchDaemons/it's "a".plist`)
	want := `do shell script "'launchctl' 'bootstrap' 'system' '/Library/LaunchDaemons/it'\\''s \"a\".plist'" with administrator privileges`
	if got != want {
		t.Errorf("\n got %s\nwant %s", got, want)
	}
}

func TestRunAdminCanceled(t *testing.T) {
	old := Run
	defer func() { Run = old }()
	Run = func(string, ...string) (string, error) {
		return "", errString("execution error: User canceled. (-128)")
	}
	if _, err := runAdmin("true"); err == nil || err.Error() != "authorization canceled" {
		t.Errorf("err = %v", err)
	}
}

type errString string

func (e errString) Error() string { return string(e) }

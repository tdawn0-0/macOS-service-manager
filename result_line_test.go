package main

import "testing"

func TestResultLine(t *testing.T) {
	for _, tc := range []struct{ in, want string }{
		{"Done", "Done"},
		{"Proxy published.\nError: peer is not the Devbase build; reinstall the helper in Settings", "Error: peer is not the Devbase build; reinstall the helper in Settings"},
		{"First\nsecond", "First …"},
		{"", ""},
	} {
		if got := resultLine(tc.in); got != tc.want {
			t.Fatalf("%q: got %q want %q", tc.in, got, tc.want)
		}
	}
}

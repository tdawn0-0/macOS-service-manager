package main

import (
	"context"
	"errors"
	"strings"
	"testing"

	"devbase/internal/devplatform"
	"devbase/internal/privhelper"
)

// A helper that refuses this build is never a silent failure: the user is
// asked to reinstall it, and the failed task says why.
func TestUIHelperRejectionPromptsReinstall(t *testing.T) {
	a := designFixture(t)
	a.spawn = func(f func()) { f() }
	a.post = func(f func()) { f() }
	for _, wrapped := range []error{privhelper.ErrPeerRejected, errors.New("apply proxy: " + privhelper.PeerRejectedMessage)} {
		a.ask = askState{}
		a.helperProblem(wrapped, nil)
		if !a.ask.open || a.ask.button != "Reinstall Helper" || !strings.Contains(a.ask.message, "different Devbase build") {
			t.Fatalf("no reinstall prompt for %v: %+v", wrapped, a.ask)
		}
	}
	a.ask = askState{}
	if a.helperProblem(errors.New("port busy"), nil) || a.ask.open {
		t.Fatal("unrelated error opened the helper prompt")
	}
	a.helperProblem(privhelper.ErrNotInstalled, nil)
	if a.ask.button != "Install Helper" {
		t.Fatal("missing helper keeps its own prompt", a.ask)
	}
}

func TestUITaskFailedByHelperRejectionPromptsReinstall(t *testing.T) {
	a := designFixture(t)
	a.spawn = func(f func()) { f() }
	a.post = func(f func()) { f() }
	a.dev.managerFactory = func() *devplatform.Manager {
		m := devplatform.New(t.TempDir())
		m.Run = func(context.Context, string, ...string) (string, error) { return "", errors.New("not found") }
		return m
	}
	a.devTask(func(m *devplatform.Manager) (string, error) {
		return "Proxy configuration published.", privhelper.ErrPeerRejected
	})
	if !a.ask.open || a.ask.button != "Reinstall Helper" {
		t.Fatal("failed task did not ask for a helper reinstall", a.ask, a.dev.output)
	}
	if !strings.Contains(a.dev.output, "Error:") || !strings.Contains(a.dev.output, "reinstall the helper") {
		t.Fatal("the error is not in the result", a.dev.output)
	}
}

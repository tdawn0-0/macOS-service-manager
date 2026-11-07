package devplatform

import (
	"context"
	"errors"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

func TestDatabaseRejectsForeignListenerWithoutStartingJobs(t *testing.T) {
	m := testManager(t)
	l, e := net.Listen("tcp4", "127.0.0.1:0")
	if e != nil {
		t.Fatal(e)
	}
	defer l.Close()
	i := Instance{ID: "port-conflict", Kind: "redis", Package: "redis@7.4.7-arm64", Port: l.Addr().(*net.TCPAddr).Port}
	m.Run = func(_ context.Context, name string, args ...string) (string, error) {
		if name == "launchctl" && len(args) > 0 && args[0] == "print" {
			return "", errors.New("not loaded")
		}
		t.Fatal("a conflicting database started a command")
		return "", nil
	}
	if e = m.CreateInstance(context.Background(), i); e == nil || !strings.Contains(e.Error(), "unavailable") {
		t.Fatal("created a conflicting instance", e)
	}
	s, _ := m.Read()
	if e = m.startInstance(context.Background(), &s, i); e == nil || !strings.Contains(e.Error(), "unavailable") {
		t.Fatal("foreign listener was mistaken for database readiness", e)
	}
	if len(s.Instances) > 0 {
		t.Fatal("conflicting instance was persisted")
	}
}

func TestDatabaseForeignListenerAfterPreflightDoesNotHideExitedJob(t *testing.T) {
	m := testManager(t)
	i := Instance{ID: "raced", Kind: "redis", Port: integrationPort(t)}
	pkg := Package{Name: "redis", Version: "test", Arch: runtime.GOARCH, Binaries: map[string]string{"redis-server": "server"}}
	i.Package = pkg.ID()
	if err := os.MkdirAll(m.packageDir(pkg), 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(m.packageDir(pkg), "server"), []byte("#!/bin/sh\nexit 1\n"), 0700); err != nil {
		t.Fatal(err)
	}
	s := emptyState()
	s.Packages[pkg.ID()] = pkg
	registered := false
	var external net.Listener
	t.Cleanup(func() {
		if external != nil {
			external.Close()
		}
	})
	m.Run = func(_ context.Context, _ string, args ...string) (string, error) {
		switch args[0] {
		case "print":
			if !registered {
				return "", errors.New("Could not find service")
			}
			return "job = {\n\tstate = exited\n\tlast exit code = 1\n}\n", nil
		case "bootstrap":
			var err error
			external, err = net.Listen("tcp4", net.JoinHostPort("127.0.0.1", fmt.Sprint(i.Port)))
			if err != nil {
				t.Fatal(err)
			}
			registered = true
			return "", nil
		case "bootout":
			registered = false
			return "", nil
		}
		t.Fatal("unexpected operation", args)
		return "", nil
	}
	if err := m.startInstance(context.Background(), &s, i); err == nil || !strings.Contains(err.Error(), "exited") {
		t.Fatal("foreign listener masked failed database", err)
	}
	conn, err := net.Dial("tcp", net.JoinHostPort("127.0.0.1", fmt.Sprint(i.Port)))
	if err != nil {
		t.Fatal("cleanup stopped external listener", err)
	}
	conn.Close()
}

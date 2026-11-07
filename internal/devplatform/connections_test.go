package devplatform

import (
	"context"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestConnectionURIsExcludePasswordsAndSelectWorkingDefaults(t *testing.T) {
	m := testManager(t)
	for _, kind := range []string{"mysql", "postgresql", "redis", "mongodb"} {
		t.Run(kind, func(t *testing.T) {
			i := Instance{ID: kind, Kind: kind, Port: 54321}
			if e := m.withState(context.Background(), func(s *State) error { s.Instances[i.ID] = i; return nil }); e != nil {
				t.Fatal(e)
			}
			dir := filepath.Join(m.Root, "instances", i.ID)
			if e := os.MkdirAll(dir, 0700); e != nil {
				t.Fatal(e)
			}
			if e := os.WriteFile(filepath.Join(dir, "password"), []byte("private-test-password"), 0600); e != nil {
				t.Fatal(e)
			}
			info, e := m.ConnectionInfo(i.ID)
			if e != nil {
				t.Fatal(e)
			}
			u, e := url.Parse(info.URI)
			if e != nil || u.Hostname() != "127.0.0.1" || u.Port() != "54321" || u.User.Username() != info.Username {
				t.Fatal("invalid client URI", info.URI, e)
			}
			if _, set := u.User.Password(); set {
				t.Fatal("URI contains a stored password")
			}
			if kind == "postgresql" && u.Path != "/postgres" {
				t.Fatal("default PostgreSQL database does not exist")
			}
			if kind == "mongodb" && u.Query().Get("authSource") != "admin" {
				t.Fatal("MongoDB client authenticates against the wrong database")
			}
			text, e := m.Connection(i.ID)
			if e != nil || strings.Contains(text, "private-test-password") {
				t.Fatal("connection details leaked a credential", e)
			}
			password, e := m.DatabasePassword(i.ID)
			if e != nil || password != "private-test-password" {
				t.Fatal("explicit password retrieval failed", e)
			}
		})
	}
}

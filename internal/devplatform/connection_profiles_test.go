package devplatform

import (
	"context"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func profileManager(t *testing.T, kind string) *Manager {
	t.Helper()
	m := testManager(t)
	if err := m.withState(context.Background(), func(s *State) error {
		s.Instances["local"] = Instance{ID: "local", Kind: kind, Port: 54321}
		return nil
	}); err != nil {
		t.Fatal(err)
	}
	return m
}

func TestConnectionProfiles(t *testing.T) {
	t.Run("uri", func(t *testing.T) {
		for _, kind := range []string{"mysql", "postgresql", "mongodb", "redis"} {
			t.Run(kind, func(t *testing.T) {
				m := profileManager(t, kind)
				p := ConnectionProfile{InstanceID: "local", Name: "App", Database: "app db", Username: "user:name", MongoAuthSource: "app db"}
				if kind == "redis" {
					p.Database, p.Username, p.MongoAuthSource, p.RedisDB = "", "", "", 15
				}
				if kind != "mongodb" {
					p.MongoAuthSource = ""
				}
				p, err := m.SaveConnectionProfile(context.Background(), p)
				if err != nil {
					t.Fatal(err)
				}
				info, err := m.ProfileConnectionInfo("local", p.ID)
				if err != nil {
					t.Fatal(err)
				}
				u, err := url.Parse(info.URI)
				if err != nil || u.Port() != "54321" || strings.Contains(info.URI, "password") {
					t.Fatal(info, err)
				}
				if _, ok := u.User.Password(); ok {
					t.Fatal("password in URI")
				}
				if kind == "mongodb" && (u.Query().Get("authSource") != "app db" || u.Path != "/app db") {
					t.Fatal(u)
				}
				if kind == "redis" && (u.Path != "/15" || u.User.Username() != "default") {
					t.Fatal(u)
				}
				if kind == "mysql" || kind == "postgresql" {
					if u.Path != "/app db" || u.User.Username() != "user:name" {
						t.Fatal(u)
					}
				}
			})
		}
	})
	t.Run("validation", func(t *testing.T) {
		m := profileManager(t, "redis")
		for _, db := range []int{-1, 16} {
			if _, err := m.SaveConnectionProfile(context.Background(), ConnectionProfile{InstanceID: "local", Name: "bad", RedisDB: db}); err == nil {
				t.Fatal("accepted Redis db", db)
			}
		}
		if _, err := m.SaveConnectionProfile(context.Background(), ConnectionProfile{InstanceID: "local", Name: "bad", Username: "other"}); err == nil {
			t.Fatal("accepted unsupported Redis user")
		}
		if _, err := m.SaveConnectionProfile(context.Background(), ConnectionProfile{InstanceID: "missing", Name: "bad"}); err == nil {
			t.Fatal("accepted missing instance")
		}
	})
	t.Run("lifecycle", func(t *testing.T) {
		m := profileManager(t, "postgresql")
		p, err := m.SaveConnectionProfile(context.Background(), ConnectionProfile{InstanceID: "local", Name: "App", Database: "app", Username: "alice"})
		if err != nil {
			t.Fatal(err)
		}
		if err := m.withState(context.Background(), func(s *State) error { i := s.Instances["local"]; i.Port = 54322; s.Instances["local"] = i; return nil }); err != nil {
			t.Fatal(err)
		}
		info, err := m.ProfileConnectionInfo("local", p.ID)
		if err != nil || !strings.Contains(info.URI, ":54322/") {
			t.Fatal(info, err)
		}
		if err := m.DeleteConnectionProfile(context.Background(), "other", p.ID); err == nil {
			t.Fatal("cross-instance deletion")
		}
		if err := m.DeleteConnectionProfile(context.Background(), "local", p.ID); err != nil {
			t.Fatal(err)
		}
		if _, err := m.ProfileConnectionInfo("local", p.ID); err == nil {
			t.Fatal("deleted profile resolved")
		}
		p, err = m.SaveConnectionProfile(context.Background(), ConnectionProfile{InstanceID: "local", Name: "App", Database: "app", Username: "alice"})
		if err != nil {
			t.Fatal(err)
		}
		if err := m.withState(context.Background(), func(s *State) error { delete(s.Instances, "local"); removeInstanceProfiles(s, "local"); return nil }); err != nil {
			t.Fatal(err)
		}
		if _, err := m.ProfileConnectionInfo("local", p.ID); err == nil {
			t.Fatal("orphan resolved")
		}
	})
	t.Run("remove", func(t *testing.T) {
		m, i, _ := deletionFixture(t)
		p, err := m.SaveConnectionProfile(context.Background(), ConnectionProfile{InstanceID: i.ID, Name: "Redis client", RedisDB: 3})
		if err != nil {
			t.Fatal(err)
		}
		if err := m.InstanceAction(context.Background(), i.ID, "remove"); err != nil {
			t.Fatal(err)
		}
		state, err := m.Read()
		if err != nil || state.ConnectionProfiles[p.ID].ID != "" {
			t.Fatal("remove retained profile", err)
		}
	})
	t.Run("permanent-delete", func(t *testing.T) {
		m, i, _ := deletionFixture(t)
		p, err := m.SaveConnectionProfile(context.Background(), ConnectionProfile{InstanceID: i.ID, Name: "Redis client", RedisDB: 3})
		if err != nil {
			t.Fatal(err)
		}
		preview := deletionPreview(t, m, i.ID)
		if err := m.DeleteInstanceData(context.Background(), i.ID, preview.Fingerprint, "wrong"); err == nil {
			t.Fatal("wrong confirmation accepted")
		}
		state, err := m.Read()
		if err != nil || state.ConnectionProfiles[p.ID].ID != p.ID {
			t.Fatal("failed deletion lost profile", err)
		}
		if err := m.DeleteInstanceData(context.Background(), i.ID, preview.Fingerprint, i.ID); err != nil {
			t.Fatal(err)
		}
		state, err = m.Read()
		if err != nil || state.ConnectionProfiles[p.ID].ID != "" {
			t.Fatal("committed deletion retained profile", err)
		}
	})
	t.Run("persistence", func(t *testing.T) {
		m := profileManager(t, "mongodb")
		p, err := m.SaveConnectionProfile(context.Background(), ConnectionProfile{InstanceID: "local", Name: "Private", Database: "business", Username: "alice", MongoAuthSource: "business"})
		if err != nil {
			t.Fatal(err)
		}
		b, err := os.ReadFile(filepath.Join(m.Root, "state.json"))
		if err != nil || strings.Contains(string(b), "business-password-sentinel") {
			t.Fatal("secret persisted", err)
		}
		s, err := m.Read()
		if err != nil || s.ConnectionProfiles[p.ID].MongoAuthSource != "business" {
			t.Fatal(s, err)
		}
	})
}

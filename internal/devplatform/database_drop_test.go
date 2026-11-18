package devplatform

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestIsProtectedDatabaseAndUser(t *testing.T) {
	for _, db := range []string{"", "postgres", "POSTGRES", "template0", "template1", "mysql", "sys", "information_schema", "performance_schema", "admin", "local", "config"} {
		if !IsProtectedDatabase(db) {
			t.Errorf("IsProtectedDatabase(%q) = false, want true", db)
		}
	}
	for _, db := range []string{"app", "orders", "postgres_app", "my_db"} {
		if IsProtectedDatabase(db) {
			t.Errorf("IsProtectedDatabase(%q) = true, want false", db)
		}
	}
	for _, user := range []string{"", "devmanager", "DEVMANAGER", "root", "default", "mysql.sys", "mysql.session", "mysql.infoschema", "pg_monitor", "pg_read_all_data", "PG_STAT", "devmanager@admin"} {
		if !IsProtectedDatabaseUser(user) {
			t.Errorf("IsProtectedDatabaseUser(%q) = false, want true", user)
		}
	}
	for _, user := range []string{"alice", "app_user", "alice@app", "pguser"} {
		if IsProtectedDatabaseUser(user) {
			t.Errorf("IsProtectedDatabaseUser(%q) = true, want false", user)
		}
	}
}

func dropToolFixture(t *testing.T, kind, tool, script string) (*Manager, string) {
	t.Helper()
	m := testManager(t)
	recordDir := t.TempDir()
	p := Package{Name: kind + "-drop-fixture", Version: "1.0.0", Arch: "universal", Binaries: map[string]string{tool: tool}}
	if err := os.MkdirAll(m.packageDir(p), 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(m.packageDir(p), tool), []byte(fmt.Sprintf(script, recordDir)), 0700); err != nil {
		t.Fatal(err)
	}
	instDir := filepath.Join(m.Root, "instances", "db")
	if err := os.MkdirAll(instDir, 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(instDir, "password"), []byte("drop-admin-pw"), 0600); err != nil {
		t.Fatal(err)
	}
	if kind == "mysql" {
		if err := m.mysqlConfig(Instance{ID: "db", Kind: "mysql", Port: 54321}, instDir, "drop-admin-pw"); err != nil {
			t.Fatal(err)
		}
	}
	if err := m.withState(context.Background(), func(s *State) error {
		s.Packages[p.ID()] = p
		s.Instances["db"] = Instance{ID: "db", Kind: kind, Package: p.ID(), Port: 54321}
		return nil
	}); err != nil {
		t.Fatal(err)
	}
	return m, recordDir
}

func TestDropDatabaseAndUserProtectionAndExecution(t *testing.T) {
	const psqlScript = "#!/usr/bin/python3\nimport json, os, sys\nd = %q\nopen(os.path.join(d, 'calls.jsonl'), 'a').write(json.dumps({'argv': sys.argv[1:], 'pgpass': os.environ.get('PGPASSWORD', ''), 'pghost': os.environ.get('PGHOST', '')}) + '\\n')\n"
	const mysqlScript = "#!/usr/bin/python3\nimport json, os, sys\nd = %q\nsql = sys.stdin.read()\nopen(os.path.join(d, 'calls.jsonl'), 'a').write(json.dumps({'argv': sys.argv[1:], 'sql': sql}) + '\\n')\nif 'SELECT host FROM mysql.user' in sql:\n    sys.stdout.write('localhost\\n127.0.0.1\\n')\n"

	t.Run("rejects-system-databases-and-admin-users", func(t *testing.T) {
		for _, kind := range []string{"postgresql", "mysql", "mongodb", "redis"} {
			m := testManager(t)
			if err := m.withState(context.Background(), func(s *State) error {
				s.Instances["db"] = Instance{ID: "db", Kind: kind, Port: 54321}
				return nil
			}); err != nil {
				t.Fatal(err)
			}
			for _, sysDB := range []string{"", "postgres", "template0", "template1", "mysql", "sys", "information_schema", "performance_schema", "admin", "local", "config", "POSTGRES"} {
				if err := m.DropDatabase(context.Background(), "db", sysDB); err == nil {
					t.Errorf("%s: DropDatabase accepted protected %q", kind, sysDB)
				}
			}
			for _, sysUser := range []string{"", "devmanager", "root", "default", "mysql.sys", "mysql.session", "mysql.infoschema", "pg_monitor", "pg_read_all_data", "devmanager@admin"} {
				if err := m.DropUser(context.Background(), "db", sysUser, "app"); err == nil {
					t.Errorf("%s: DropUser accepted protected %q", kind, sysUser)
				}
			}
		}
		m := testManager(t)
		if err := m.withState(context.Background(), func(s *State) error {
			s.Instances["cache"] = Instance{ID: "cache", Kind: "redis", Port: 6379}
			return nil
		}); err != nil {
			t.Fatal(err)
		}
		if err := m.DropDatabase(context.Background(), "cache", "app"); err == nil {
			t.Fatal("redis accepted DropDatabase")
		}
		if err := m.DropUser(context.Background(), "cache", "alice", ""); err == nil {
			t.Fatal("redis accepted DropUser")
		}
	})

	t.Run("postgresql-drop-db-and-user", func(t *testing.T) {
		m, recordDir := dropToolFixture(t, "postgresql", "psql", psqlScript)
		t.Setenv("PGHOST", "192.0.2.50")
		if err := m.DropDatabase(context.Background(), "db", "orders"); err != nil {
			t.Fatal(err)
		}
		if err := m.DropUser(context.Background(), "db", "alice", "orders"); err != nil {
			t.Fatal(err)
		}
		raw, err := os.ReadFile(filepath.Join(recordDir, "calls.jsonl"))
		if err != nil {
			t.Fatal(err)
		}
		lines := strings.Split(strings.TrimSpace(string(raw)), "\n")
		if len(lines) != 2 {
			t.Fatalf("expected 2 psql calls, got %d: %s", len(lines), raw)
		}
		for i, wantSQL := range []string{`DROP DATABASE "orders"`, `DROP ROLE "alice"`} {
			var rec struct {
				Argv   []string `json:"argv"`
				Pgpass string   `json:"pgpass"`
				Pghost string   `json:"pghost"`
			}
			if err := json.Unmarshal([]byte(lines[i]), &rec); err != nil {
				t.Fatal(err)
			}
			if rec.Pgpass != "drop-admin-pw" || rec.Pghost != "" {
				t.Fatalf("env isolation failed: %+v", rec)
			}
			joined := strings.Join(rec.Argv, " ")
			if strings.Contains(joined, "drop-admin-pw") || !strings.Contains(joined, wantSQL) {
				t.Fatalf("call %d argv = %v, want %q", i, rec.Argv, wantSQL)
			}
		}
	})

	t.Run("postgresql-drop-user-reassigns-dependencies", func(t *testing.T) {
		const depScript = "#!/usr/bin/python3\nimport json, os, sys\nd = %q\nopen(os.path.join(d, 'calls.jsonl'), 'a').write(json.dumps(sys.argv[1:]) + '\\n')\nargs = ' '.join(sys.argv[1:])\nif 'DROP ROLE' in args and not os.path.exists(os.path.join(d, 'cleaned')):\n    sys.stderr.write('ERROR: role \"alice\" cannot be dropped because some objects depend on it\\n')\n    sys.exit(1)\nif 'REASSIGN OWNED' in args:\n    open(os.path.join(d, 'cleaned'), 'w').write('1')\n"
		m, recordDir := dropToolFixture(t, "postgresql", "psql", depScript)
		if err := m.DropUser(context.Background(), "db", "alice", "orders"); err != nil {
			t.Fatal(err)
		}
		raw, err := os.ReadFile(filepath.Join(recordDir, "calls.jsonl"))
		if err != nil {
			t.Fatal(err)
		}
		lines := strings.Split(strings.TrimSpace(string(raw)), "\n")
		if len(lines) != 3 {
			t.Fatalf("expected 3 psql calls (drop, cleanup, retry), got %d: %s", len(lines), raw)
		}
	})

	t.Run("mysql-drop-db-and-user", func(t *testing.T) {
		m, recordDir := dropToolFixture(t, "mysql", "mysql", mysqlScript)
		if err := m.DropDatabase(context.Background(), "db", "orders"); err != nil {
			t.Fatal(err)
		}
		if err := m.DropUser(context.Background(), "db", "alice", "orders"); err != nil {
			t.Fatal(err)
		}
		raw, err := os.ReadFile(filepath.Join(recordDir, "calls.jsonl"))
		if err != nil {
			t.Fatal(err)
		}
		lines := strings.Split(strings.TrimSpace(string(raw)), "\n")
		if len(lines) != 3 {
			t.Fatalf("expected 3 mysql calls (drop db, select host, drop user), got %d: %s", len(lines), raw)
		}
		var call0, call2 struct {
			Argv []string `json:"argv"`
			SQL  string   `json:"sql"`
		}
		_ = json.Unmarshal([]byte(lines[0]), &call0)
		_ = json.Unmarshal([]byte(lines[2]), &call2)
		if call0.SQL != "DROP DATABASE `orders`" {
			t.Fatalf("unexpected drop db SQL: %q", call0.SQL)
		}
		if call2.SQL != "DROP USER 'alice'@'localhost', 'alice'@'127.0.0.1'" {
			t.Fatalf("unexpected drop user SQL: %q", call2.SQL)
		}
	})

	t.Run("respects-instance-lease", func(t *testing.T) {
		m, _ := dropToolFixture(t, "postgresql", "psql", psqlScript)
		release, err := m.resourceLeases(context.Background(), instanceResourceKeys([]string{"db"})...)
		if err != nil {
			t.Fatal(err)
		}
		defer release()
		var busy *ResourceBusyError
		if err := m.DropDatabase(context.Background(), "db", "orders"); !errors.As(err, &busy) {
			t.Fatalf("DropDatabase ignored instance lease: %v", err)
		}
		if err := m.DropUser(context.Background(), "db", "alice", "orders"); !errors.As(err, &busy) {
			t.Fatalf("DropUser ignored instance lease: %v", err)
		}
	})
}

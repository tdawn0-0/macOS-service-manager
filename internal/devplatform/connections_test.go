package devplatform

import (
	"context"
	"encoding/json"
	"fmt"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
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
			overridden := info.WithDatabase(kind, "custom_db")
			if kind == "redis" {
				overridden = info.WithDatabase(kind, "5")
			}
			envLine := overridden.EnvAssignment()
			if !strings.HasPrefix(envLine, "DATABASE_URL=") || strings.Contains(envLine, "private-test-password") {
				t.Fatalf("unexpected .env assignment: %q", envLine)
			}
			if kind == "postgresql" || kind == "mysql" {
				jdbc, err := overridden.JDBCURL(kind)
				if err != nil || !strings.HasPrefix(jdbc, "jdbc:"+kind+"://127.0.0.1:54321/custom_db?user=") || strings.Contains(jdbc, "private-test-password") {
					t.Fatalf("unexpected JDBC URL: %q err=%v", jdbc, err)
				}
			} else if _, err := overridden.JDBCURL(kind); err == nil {
				t.Fatalf("expected JDBC URL error for %s", kind)
			}
		})
	}
}

func TestEnvAssignmentForKind(t *testing.T) {
	info := ConnectionInfo{URI: "redis://default@127.0.0.1:6379/2"}
	if got := info.EnvAssignment(); got != "DATABASE_URL=redis://default@127.0.0.1:6379/2" {
		t.Fatalf("EnvAssignment changed: %q", got)
	}
	cases := map[string]string{
		"postgresql": "DATABASE_URL=postgresql://devmanager@127.0.0.1:5432/app",
		"mysql":      "DATABASE_URL=mysql://root@127.0.0.1:3306/app",
		"redis":      "REDIS_URL=redis://default@127.0.0.1:6379/2",
		"mongodb":    "MONGODB_URI=mongodb://devmanager@127.0.0.1:27017/app?authSource=admin",
	}
	for kind, want := range cases {
		uri := strings.SplitN(want, "=", 2)[1]
		got := (ConnectionInfo{URI: uri}).EnvAssignmentForKind(kind)
		if got != want {
			t.Errorf("%s: got %q want %q", kind, got, want)
		}
	}
}

func TestOpenDatabaseCLI(t *testing.T) {
	const secret = "cli-secret-token-xyz"
	for _, tc := range []struct {
		kind    string
		tool    string
		profile *ConnectionProfile
		dbOver  string
		wantArg []string
		wantEnv string
	}{
		{
			kind:    "postgresql",
			tool:    "psql",
			dbOver:  "orders",
			wantArg: []string{"-h", "127.0.0.1", "-p", "55432", "-U", "devmanager", "-d", "orders"},
			wantEnv: "PGPASSWORD",
		},
		{
			kind:    "postgresql",
			tool:    "psql",
			profile: &ConnectionProfile{ID: "app", InstanceID: "db", Name: "App", Database: "orders", Username: "alice"},
			wantArg: []string{"-h", "127.0.0.1", "-p", "55432", "-U", "alice", "-d", "orders"},
		},
		{
			kind:    "mysql",
			tool:    "mysql",
			dbOver:  "orders",
			wantArg: []string{"--defaults-file=", "orders"},
		},
		{
			kind:    "mysql",
			tool:    "mysql",
			profile: &ConnectionProfile{ID: "app", InstanceID: "db", Name: "App", Database: "orders", Username: "alice"},
			wantArg: []string{"--no-defaults", "--host=127.0.0.1", "--port=55432", "--protocol=tcp", "--user=alice", "-p", "orders"},
		},
		{
			kind:    "redis",
			tool:    "redis-cli",
			dbOver:  "4",
			wantArg: []string{"-h", "127.0.0.1", "-p", "55432", "-n", "4"},
			wantEnv: "REDISCLI_AUTH",
		},
		{
			kind:    "redis",
			tool:    "redis-cli",
			profile: &ConnectionProfile{ID: "app", InstanceID: "db", Name: "App", RedisDB: 9},
			wantArg: []string{"-h", "127.0.0.1", "-p", "55432", "-n", "9"},
			wantEnv: "REDISCLI_AUTH",
		},
	} {
		name := tc.kind + "-admin"
		if tc.profile != nil {
			name = tc.kind + "-profile"
		}
		t.Run(name, func(t *testing.T) {
			m := testManager(t)
			recordDir := t.TempDir()
			pkg := Package{Name: tc.kind + "-cli-fixture", Version: "1.0.0", Arch: "universal", Binaries: map[string]string{tc.tool: tc.tool}}
			if err := os.MkdirAll(m.packageDir(pkg), 0700); err != nil {
				t.Fatal(err)
			}
			recorder := fmt.Sprintf("#!/usr/bin/python3\nimport json, os, sys\nd = %q\nopen(os.path.join(d, 'argv.json'), 'w').write(json.dumps(sys.argv[1:]))\nopen(os.path.join(d, 'env.json'), 'w').write(json.dumps(dict(os.environ)))\n", recordDir)
			if err := os.WriteFile(filepath.Join(m.packageDir(pkg), tc.tool), []byte(recorder), 0700); err != nil {
				t.Fatal(err)
			}
			instDir := filepath.Join(m.Root, "instances", "db")
			if err := os.MkdirAll(instDir, 0700); err != nil {
				t.Fatal(err)
			}
			if err := os.WriteFile(filepath.Join(instDir, "password"), []byte(secret), 0600); err != nil {
				t.Fatal(err)
			}
			if tc.kind == "mysql" {
				if err := m.mysqlConfig(Instance{ID: "db", Kind: "mysql", Port: 55432}, instDir, secret); err != nil {
					t.Fatal(err)
				}
			}
			if err := m.withState(context.Background(), func(s *State) error {
				s.Packages[pkg.ID()] = pkg
				s.Instances["db"] = Instance{ID: "db", Kind: tc.kind, Package: pkg.ID(), Port: 55432}
				if tc.profile != nil {
					s.ConnectionProfiles[tc.profile.ID] = *tc.profile
				}
				return nil
			}); err != nil {
				t.Fatal(err)
			}

			var runCalls [][]string
			m.Run = func(_ context.Context, program string, args ...string) (string, error) {
				runCalls = append(runCalls, append([]string{program}, args...))
				return "", nil
			}
			profileID := ""
			if tc.profile != nil {
				profileID = tc.profile.ID
			}
			if err := m.OpenDatabaseCLI(context.Background(), "db", profileID, tc.dbOver); err != nil {
				t.Fatal(err)
			}
			scriptPath := filepath.Join(instDir, "open-cli.command")
			if !reflect.DeepEqual(runCalls, [][]string{{"/usr/bin/open", "-a", "Terminal", scriptPath}}) {
				t.Fatalf("unexpected open call: %v", runCalls)
			}
			info, err := os.Stat(scriptPath)
			if err != nil || info.Mode().Perm() != 0700 {
				t.Fatalf("script mode = %v err = %v", info.Mode().Perm(), err)
			}
			scriptBytes, err := os.ReadFile(scriptPath)
			if err != nil {
				t.Fatal(err)
			}
			if strings.Contains(string(scriptBytes), secret) {
				t.Fatal("launcher script embedded plaintext password")
			}

			t.Setenv("PGHOST", "192.0.2.99")
			t.Setenv("PGPASSFILE", "/tmp/untrusted")
			t.Setenv("MYSQL_PWD", "ambient-mysql")
			if out, err := exec.Command(scriptPath).CombinedOutput(); err != nil {
				t.Fatalf("script failed: %v %s", err, out)
			}

			var argv []string
			argvBytes, err := os.ReadFile(filepath.Join(recordDir, "argv.json"))
			if err != nil {
				t.Fatal(err)
			}
			if err := json.Unmarshal(argvBytes, &argv); err != nil {
				t.Fatal(err)
			}
			for _, arg := range argv {
				if strings.Contains(arg, secret) {
					t.Fatalf("argv leaked secret: %v", argv)
				}
			}
			if tc.kind == "mysql" && tc.profile == nil {
				want := []string{"--defaults-file=" + filepath.Join(instDir, "client.cnf"), "orders"}
				if !reflect.DeepEqual(argv, want) {
					t.Fatalf("argv = %v want %v", argv, want)
				}
			} else if !reflect.DeepEqual(argv, tc.wantArg) {
				t.Fatalf("argv = %v want %v", argv, tc.wantArg)
			}

			var env map[string]string
			envBytes, err := os.ReadFile(filepath.Join(recordDir, "env.json"))
			if err != nil {
				t.Fatal(err)
			}
			if err := json.Unmarshal(envBytes, &env); err != nil {
				t.Fatal(err)
			}
			if tc.wantEnv != "" && env[tc.wantEnv] != secret {
				t.Fatalf("env[%s] = %q, want %q", tc.wantEnv, env[tc.wantEnv], secret)
			}
			if tc.profile != nil && tc.kind == "postgresql" {
				if _, ok := env["PGPASSWORD"]; ok {
					t.Fatal("profile CLI set admin PGPASSWORD")
				}
			}
			for _, stripped := range []string{"PGHOST", "PGPASSFILE", "MYSQL_PWD"} {
				if _, exists := env[stripped]; exists {
					t.Fatalf("ambient %s was not cleared", stripped)
				}
			}
		})
	}

	t.Run("refuses-mongodb-and-invalid-ids", func(t *testing.T) {
		m := testManager(t)
		if err := m.withState(context.Background(), func(s *State) error {
			s.Instances["mongo"] = Instance{ID: "mongo", Kind: "mongodb", Port: 27017}
			return nil
		}); err != nil {
			t.Fatal(err)
		}
		if err := m.OpenDatabaseCLI(context.Background(), "mongo", "", ""); err == nil {
			t.Fatal("expected mongodb refusal")
		}
		if err := m.OpenDatabaseCLI(context.Background(), "../bad", "", ""); err == nil {
			t.Fatal("expected invalid ID refusal")
		}
	})
}

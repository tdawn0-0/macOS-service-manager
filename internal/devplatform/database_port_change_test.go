package devplatform

import (
	"bytes"
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
)

func databasePortFixture(t *testing.T, kind string) (*Manager, Instance, string) {
	t.Helper()
	m := testManager(t)
	i := Instance{ID: "database", Kind: kind, Package: kind + "@test-arm64", Port: 54321, Persistent: true}
	if err := m.withState(context.Background(), func(s *State) error { s.Instances[i.ID] = i; return nil }); err != nil {
		t.Fatal(err)
	}
	dir := filepath.Join(m.Root, "instances", i.ID)
	if err := os.MkdirAll(dir, 0700); err != nil {
		t.Fatal(err)
	}
	for name, value := range map[string]string{"password": "keep-this-credential", "data-sentinel": "42", "valkey.conf": "bind 127.0.0.1\nport 54321\nrequirepass keep-this-credential\nappendonly yes\n", "client.cnf": "[client]\npassword=\"keep-this-credential\"\nport=54321\nprotocol=tcp\n"} {
		if err := os.WriteFile(filepath.Join(dir, name), []byte(value), 0600); err != nil {
			t.Fatal(err)
		}
	}
	m.Run = func(context.Context, string, ...string) (string, error) {
		return "", errors.New("Could not find service")
	}
	return m, i, dir
}
func TestDatabasePortChangeKeepsEveryEngineDataAndCredentials(t *testing.T) {
	for _, kind := range []string{"mysql", "postgresql", "mongodb", "redis", "valkey"} {
		t.Run(kind, func(t *testing.T) {
			m, i, dir := databasePortFixture(t, kind)
			port := integrationPort(t)
			if port == i.Port {
				port = integrationPort(t)
			}
			if err := m.ChangeInstancePort(context.Background(), i, port); err != nil {
				t.Fatal(err)
			}
			s, _ := m.Read()
			if s.Instances[i.ID].Port != port || s.Instances[i.ID].Package != i.Package {
				t.Fatal("port changed version or identity")
			}
			for name, want := range map[string]string{"password": "keep-this-credential", "data-sentinel": "42"} {
				got, err := os.ReadFile(filepath.Join(dir, name))
				if err != nil || string(got) != want {
					t.Fatal("port change modified data", name, err)
				}
			}
			if kind == "redis" || kind == "valkey" {
				b, _ := os.ReadFile(filepath.Join(dir, "valkey.conf"))
				if !strings.Contains(string(b), "port "+strconv.Itoa(port)+"\n") || !strings.Contains(string(b), "requirepass keep-this-credential") {
					t.Fatal("Redis configuration was recreated incorrectly")
				}
			}
			if kind == "mysql" {
				b, _ := os.ReadFile(filepath.Join(dir, "client.cnf"))
				if !strings.Contains(string(b), "port="+strconv.Itoa(port)+"\n") || !strings.Contains(string(b), "password=\"keep-this-credential\"") {
					t.Fatal("MySQL client configuration lost credentials or port")
				}
			}
			if err := m.ChangeInstancePort(context.Background(), i, integrationPort(t)); err == nil {
				t.Fatal("stale instance record overwrote a newer port")
			}
		})
	}
}
func TestDatabasePortChangeRefusesRunningInstanceAndUnknownObservation(t *testing.T) {
	for _, kind := range []string{"database", "unknown"} {
		t.Run(kind, func(t *testing.T) {
			m, i, dir := databasePortFixture(t, "redis")
			before, _ := os.ReadFile(filepath.Join(dir, "valkey.conf"))
			m.Run = func(_ context.Context, _ string, args ...string) (string, error) {
				if kind == "unknown" {
					return "", errors.New("observation failed")
				}
				target := args[len(args)-1]
				if kind == "database" && strings.Contains(target, "db-") {
					return "state = running", nil
				}
				return "", errors.New("Could not find service")
			}
			if err := m.ChangeInstancePort(context.Background(), i, integrationPort(t)); err == nil {
				t.Fatal("unverified resource changed port")
			}
			after, _ := os.ReadFile(filepath.Join(dir, "valkey.conf"))
			if !bytes.Equal(before, after) {
				t.Fatal("blocked port change modified configuration")
			}
		})
	}
}
func TestDatabasePortChangeStateFailureRestoresManagedConfiguration(t *testing.T) {
	for _, kind := range []string{"redis", "mysql"} {
		t.Run(kind, func(t *testing.T) {
			m, i, dir := databasePortFixture(t, kind)
			name := "valkey.conf"
			if kind == "mysql" {
				name = "client.cnf"
			}
			before, _ := os.ReadFile(filepath.Join(dir, name))
			path, backup := filepath.Join(m.Root, "state.json"), filepath.Join(m.Root, "state.backup")
			t.Cleanup(func() { _ = os.Remove(path); _ = os.Rename(backup, path) })
			injected := false
			m.Run = func(context.Context, string, ...string) (string, error) {
				if !injected {
					if err := os.Rename(path, backup); err != nil {
						t.Fatal(err)
					}
					if err := os.Mkdir(path, 0700); err != nil {
						t.Fatal(err)
					}
					injected = true
				}
				return "", errors.New("Could not find service")
			}
			if err := m.ChangeInstancePort(context.Background(), i, integrationPort(t)); err == nil {
				t.Fatal("state write failure accepted")
			}
			after, _ := os.ReadFile(filepath.Join(dir, name))
			if !bytes.Equal(before, after) {
				t.Fatal("managed port not rolled back")
			}
		})
	}
}

func TestManagedRedisPortRejectsAmbiguousOrAlteredConfiguration(t *testing.T) {
	for _, config := range []string{"port 54321\nport 54321\n", "port 12345\n", "# port 54321\n"} {
		if _, err := replaceManagedRedisPort([]byte(config), 54321, 54322); err == nil {
			t.Fatal("ambiguous managed configuration rewritten")
		}
	}
}

func TestDatabasePortChangeCancellationRestoresFiles(t *testing.T) {
	for _, cancelled := range []bool{false, true} {
		t.Run(strconv.FormatBool(cancelled), func(t *testing.T) {
			m, i, dir := databasePortFixture(t, "redis")
			pkg, err := Resolve(context.Background(), "redis", "7.4.7")
			if err != nil {
				t.Fatal(err)
			}
			i.Package = pkg.ID()
			if err = m.withState(context.Background(), func(s *State) error {
				s.Instances[i.ID] = i
				s.Packages[pkg.ID()] = pkg
				return nil
			}); err != nil {
				t.Fatal(err)
			}
			before, _ := os.ReadFile(filepath.Join(dir, "valkey.conf"))
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			if cancelled {
				m.Event = func(message string) {
					if strings.HasSuffix(message, "valkey.conf") {
						cancel()
					}
				}
			}
			port := integrationPort(t)
			err = m.ChangeInstancePort(ctx, i, port)
			if cancelled {
				if !errors.Is(err, context.Canceled) {
					t.Fatal(err)
				}
				after, _ := os.ReadFile(filepath.Join(dir, "valkey.conf"))
				if !bytes.Equal(before, after) {
					t.Fatal("cancelled port change left modified endpoint files")
				}
			} else {
				if err != nil {
					t.Fatal(err)
				}
				state, err := m.Read()
				if err != nil || state.Instances[i.ID].Port != port {
					t.Fatal("saved port mismatch", err)
				}
			}
		})
	}
}
func TestDatabasePortCrashChild(t *testing.T) {
	root := os.Getenv("MSM_DB_PORT_CRASH_ROOT")
	if root == "" {
		return
	}
	m := New(root)
	s, err := m.Read()
	if err != nil {
		t.Fatal(err)
	}
	port, err := strconv.Atoi(os.Getenv("MSM_DB_PORT_CRASH_NEW"))
	if err != nil {
		t.Fatal(err)
	}
	m.Event = func(message string) {
		if strings.HasSuffix(message, "valkey.conf") {
			os.Exit(23)
		}
	}
	if err = m.ChangeInstancePort(context.Background(), s.Instances["database"], port); err != nil {
		t.Fatal(err)
	}
	t.Fatal("crash point not reached")
}
func TestDatabasePortRepairAfterProcessExitPreservesSavedStateAndData(t *testing.T) {
	m, i, dir := databasePortFixture(t, "redis")
	port := integrationPort(t)
	c := exec.Command(os.Args[0], "-test.run=^TestDatabasePortCrashChild$")
	c.Env = append(os.Environ(), "MSM_DB_PORT_CRASH_ROOT="+m.Root, "MSM_DB_PORT_CRASH_NEW="+strconv.Itoa(port))
	out, err := c.CombinedOutput()
	var exited *exec.ExitError
	if !errors.As(err, &exited) || exited.ExitCode() != 23 {
		t.Fatalf("crash helper: %v %s", err, out)
	}
	m = New(m.Root)
	s, err := m.Read()
	if err != nil || s.Instances[i.ID].Port != i.Port {
		t.Fatal("crash published partial state", err)
	}
	config, _ := os.ReadFile(filepath.Join(dir, "valkey.conf"))
	actual, err := managedConfigPort(config, "redis")
	if err != nil || actual != port {
		t.Fatal("crash did not interrupt after config save", err)
	}
	if err = m.RepairInstancePort(context.Background(), i); err != nil {
		t.Fatal(err)
	}
	config, _ = os.ReadFile(filepath.Join(dir, "valkey.conf"))
	actual, err = managedConfigPort(config, "redis")
	if err != nil || actual != i.Port {
		t.Fatal("repair did not restore saved endpoint", err)
	}
	data, _ := os.ReadFile(filepath.Join(dir, "data-sentinel"))
	password, _ := os.ReadFile(filepath.Join(dir, "password"))
	if string(data) != "42" || string(password) != "keep-this-credential" {
		t.Fatal("endpoint repair modified data or credentials")
	}
}

func TestManagedMySQLPortRejectsAmbiguousOrAlteredConfiguration(t *testing.T) {
	for _, config := range []string{"[client]\nport=54321\nport=54321\n", "[mysql]\nport=54321\n", "[client]\nport=12345\n"} {
		if _, err := replaceManagedMySQLPort([]byte(config), 54321, 54322); err == nil {
			t.Fatal("ambiguous client configuration rewritten")
		}
	}
}

package devplatform

import (
	"context"
	"errors"
	"fmt"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"syscall"
)

func (m *Manager) initializeInstanceData(ctx context.Context, s *State, i Instance, dir string) error {
	if e := m.verifyDataLocation(i); e != nil {
		return e
	}
	if i.DataDirectory != "" && (i.Kind == "redis" || i.Kind == "valkey") {
		if e := os.MkdirAll(m.DatabaseDataDirectory(i), 0700); e != nil {
			return e
		}
	}
	password := randomID() + randomID()
	if e := atomicWrite(filepath.Join(dir, "password"), []byte(password), 0600); e != nil {
		return e
	}
	if i.Kind == "postgresql" {
		bin, e := m.Binary(s, i.Package, "initdb")
		if e != nil {
			return e
		}
		if _, e = m.runInitializationTool(ctx, bin, "-D", m.DatabaseDataDirectory(i), "-U", "devmanager", "--locale=C", "--encoding=UTF8", "--auth-host=scram-sha-256", "--auth-local=scram-sha-256", "--pwfile="+filepath.Join(dir, "password")); e != nil {
			return e
		}
	} else if i.Kind == "mysql" || i.Kind == "mongodb" {
		if e := m.createOther(ctx, s, i, dir, password); e != nil {
			return e
		}
	} else {
		binary := "valkey-server"
		if i.Kind == "redis" {
			binary = "redis-server"
		}
		if _, e := m.Binary(s, i.Package, binary); e != nil {
			return e
		}
		config := fmt.Sprintf("bind 127.0.0.1\nport %d\ndir %q\nrequirepass %s\ndaemonize no\n", i.Port, m.DatabaseDataDirectory(i), password)
		if i.Persistent {
			config += "appendonly yes\n"
		} else {
			config += "save \"\"\nappendonly no\n"
		}
		if e := atomicWrite(filepath.Join(dir, "valkey.conf"), []byte(config), 0600); e != nil {
			return e
		}
	}

	return nil
}
func (m *Manager) startInstance(ctx context.Context, s *State, i Instance) error {
	if e := m.verifyDataLocation(i); e != nil {
		return e
	}
	if out, e := m.Run(ctx, "launchctl", "print", domain()+"/"+m.label(m.databaseJobID(i.ID))); e != nil || !strings.Contains(out, "state = running") {
		if e := freeDatabasePort(i.Port); e != nil {
			return e
		}
	}
	dir := m.instanceDirectory(i.ID)
	args, e := m.instanceStartArguments(s, i)
	if e != nil {
		return e
	}
	var env map[string]string
	if i.Kind == "postgresql" {
		env = map[string]string{"LANG": "C", "LC_ALL": "C"}
	}
	fresh, e := m.startJob(ctx, m.databaseJobID(i.ID), args, dir, env, false)
	if e != nil {
		return e
	}
	if e = m.waitService(ctx, m.databaseJobID(i.ID), processProbe{ID: i.ID, Health: fmt.Sprintf("tcp://127.0.0.1:%d", i.Port)}); e != nil {
		if fresh {
			_ = m.stopJob(context.Background(), m.databaseJobID(i.ID))
		}
		return e
	}
	return nil
}
func (m *Manager) instanceStartArguments(s *State, i Instance) ([]string, error) {
	dir := m.instanceDirectory(i.ID)
	var args []string
	if i.Kind == "postgresql" {
		bin, e := m.Binary(s, i.Package, "postgres")
		if e != nil {
			return nil, e
		}
		args = []string{bin, "-D", m.DatabaseDataDirectory(i), "-h", "127.0.0.1", "-p", fmt.Sprint(i.Port), "-k", ""}
	} else if i.Kind == "mysql" {
		bin, e := m.Binary(s, i.Package, "mysqld")
		if e != nil {
			return nil, e
		}
		args = []string{bin, "--no-defaults", "--basedir=" + m.packageDir(s.Packages[i.Package]), "--datadir=" + m.DatabaseDataDirectory(i), "--bind-address=127.0.0.1", "--port=" + fmt.Sprint(i.Port), "--mysqlx=OFF", "--socket=" + filepath.Join("/tmp", "msm-"+i.ID+"-"+randomID()+".sock")}
	} else if i.Kind == "mongodb" {
		bin, e := m.Binary(s, i.Package, "mongod")
		if e != nil {
			return nil, e
		}
		args = []string{bin, "--dbpath", m.DatabaseDataDirectory(i), "--bind_ip", "127.0.0.1", "--port", fmt.Sprint(i.Port), "--auth"}
	} else {
		binary := "valkey-server"
		if i.Kind == "redis" {
			binary = "redis-server"
		}
		bin, e := m.Binary(s, i.Package, binary)
		if e != nil {
			return nil, e
		}
		args = []string{bin, filepath.Join(dir, "valkey.conf")}
	}
	return args, nil
}
func freeDatabasePort(port int) error {
	l, e := net.Listen("tcp4", fmt.Sprintf("127.0.0.1:%d", port))
	if e != nil {
		return &PortUnavailableError{Port: port, Role: "database", Err: e}
	}
	return l.Close()
}
func (m *Manager) instanceAction(ctx context.Context, id, action string) error {
	if e := validID(id); e != nil {
		return e
	}
	if action != "start" && action != "stop" && action != "restart" && action != "remove" {
		return errors.New("unknown instance action")
	}
	if e := ctx.Err(); e != nil {
		return e
	}
	if e := m.init(); e != nil {
		return e
	}
	unlock, e := m.resourceLeases(ctx, resourceKey{"database instance", id})
	if e != nil {
		return e
	}
	defer unlock()
	s, e := m.Read()
	if e != nil {
		return e
	}
	i, ok := s.Instances[id]
	if !ok {
		return errors.New("instance not found")
	}
	if action == "start" || action == "restart" {
		release, e := m.packageLease(ctx, i.Package, false)
		if e != nil {
			return e
		}
		defer release()
		s, e = m.Read()
		if e != nil {
			return e
		}
		if current, exists := s.Instances[id]; !exists || current != i {
			return errors.New("database settings changed; retry with the current instance")
		}
	}
	if e = m.checkInstanceAction(ctx, s, id, action); e != nil {
		return e
	}
	if action != "start" {
		m.emit("Stopping database " + id)
		if e = m.stopJob(ctx, "db-"+id); e != nil {
			return e
		}
		if e = m.verifyShutdownTarget(ctx, domain()+"/"+m.label("db-"+id)); e != nil {
			return e
		}
	}
	switch action {
	case "start", "restart":
		m.emit("Starting database " + id)
		if e = ctx.Err(); e != nil {
			return e
		}
		return m.startInstance(ctx, &s, i)
	case "remove":
		return m.withState(ctx, func(latest *State) error {
			if current, exists := latest.Instances[id]; !exists || current != i {
				return errors.New("database settings changed before removal; refresh and retry")
			}
			if e := m.checkInstanceAction(ctx, *latest, id, action); e != nil {
				return e
			}
			if e := os.Remove(filepath.Join(m.Root, "jobs", m.label("db-"+id)+".plist")); e != nil && !errors.Is(e, os.ErrNotExist) {
				return e
			}
			delete(latest.Instances, id)
			removeInstanceProfiles(latest, id)
			return nil
		})
	}
	return nil
}

func (m *Manager) checkInstanceAction(ctx context.Context, s State, id, action string) error {
	if action == "start" || action == "restart" {
		if e := m.verifyDataLocation(s.Instances[id]); e != nil {
			return e
		}
	}
	return ctx.Err()
}
func cleanDatabaseEnv(extra ...string) []string {
	ambient := os.Environ()
	filtered := make([]string, 0, len(ambient)+len(extra))
	for _, entry := range ambient {
		if strings.HasPrefix(entry, "PG") || strings.HasPrefix(entry, "MYSQL_") {
			continue
		}
		filtered = append(filtered, entry)
	}
	return append(filtered, extra...)
}

func validDatabasePassword(password string) error {
	if password == "" {
		return errors.New("password is required")
	}
	if len(password) > 256 {
		return errors.New("password exceeds maximum length")
	}
	for _, r := range password {
		if r < 0x20 || r == 0x7f {
			return errors.New("password cannot contain newline or control characters")
		}
	}
	return nil
}

func (m *Manager) databaseCommand(ctx context.Context, s *State, id, tool string, args ...string) (string, error) {
	i, ok := s.Instances[id]
	if !ok || i.Kind != "postgresql" {
		return "", errors.New("PostgreSQL instance not found")
	}
	bin, e := m.Binary(s, i.Package, tool)
	if e != nil {
		return "", e
	}
	password, e := os.ReadFile(filepath.Join(m.Root, "instances", id, "password"))
	if e != nil {
		return "", e
	}
	base := []string{"-w", "-h", "127.0.0.1", "-p", fmt.Sprint(i.Port), "-U", "devmanager"}
	c := exec.CommandContext(ctx, bin, append(base, args...)...)
	c.Env = cleanDatabaseEnv("PGPASSWORD=" + string(password))
	b, e := c.CombinedOutput()
	if e != nil {
		return "", fmt.Errorf("%s failed: %w: %s", tool, e, b)
	}
	return string(b), nil
}
func (m *Manager) createDatabase(ctx context.Context, id, name string) error {
	if e := validID(name); e != nil {
		return e
	}
	return m.withDatabaseOperation(ctx, id, func(s *State) error {
		if i, ok := s.Instances[id]; ok && i.Kind != "postgresql" {
			return m.otherCreateDatabase(ctx, s, i, name)
		}
		_, e := m.databaseCommand(ctx, s, id, "createdb", "--", name)
		return e
	})
}
func (m *Manager) backup(ctx context.Context, id, name string) (string, error) {
	if e := validID(id); e != nil {
		return "", e
	}
	if e := validID(name); e != nil {
		return "", e
	}
	var path string
	e := m.withDatabaseOperation(ctx, id, func(s *State) error {
		path = filepath.Join(m.Root, "backups", id+"-"+name+"-"+randomID()+".dump")
		partial := path + ".partial"
		m.operationArtifact(partial)
		f, e := os.OpenFile(partial, os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0600)
		if e != nil {
			return e
		}
		f.Close()
		if i, ok := s.Instances[id]; ok && i.Kind != "postgresql" {
			e = m.otherBackup(ctx, s, i, name, partial)
		} else {
			_, e = m.databaseCommand(ctx, s, id, "pg_dump", "-Fc", "-f", partial, "--", name)
		}
		if e == nil {
			e = ctx.Err()
		}
		if e != nil {
			if !m.retainDatabaseWorkOutput() {
				os.Remove(partial)
			}
			return e
		}
		f, e = os.OpenFile(partial, os.O_RDWR|syscall.O_NOFOLLOW, 0)
		if e != nil {
			return e
		}
		e = f.Sync()
		f.Close()
		if e != nil {
			return e
		}
		if e = os.Rename(partial, path); e != nil {
			return e
		}
		if e = syncExportDirectory(filepath.Dir(path)); e != nil {
			return e
		}
		m.operationArtifact(path)
		return nil
	})
	if e != nil {
		return "", e
	}
	return path, nil
}
func (m *Manager) restore(ctx context.Context, id, name, path string) error {
	if e := validID(id); e != nil {
		return e
	}
	if e := validID(name); e != nil {
		return e
	}
	return m.withDatabaseOperation(ctx, id, func(s *State) error {
		if i, ok := s.Instances[id]; ok && i.Kind != "postgresql" {
			return m.otherRestore(ctx, s, i, name, path)
		}
		if _, e := m.databaseCommand(ctx, s, id, "createdb", "--", name); e != nil {
			return e
		}
		_, e := m.databaseCommand(ctx, s, id, "pg_restore", "--exit-on-error", "--single-transaction", "-d", name, path)
		return e
	})
}

// CreateUser creates a role without putting its password in argv or logs.
func (m *Manager) createUser(ctx context.Context, id, name, password, database string) error {
	if e := validID(name); e != nil {
		return e
	}
	if e := validID(database); e != nil {
		return e
	}
	if e := validDatabasePassword(password); e != nil {
		return e
	}
	return m.withDatabaseOperation(ctx, id, func(s *State) error {
		i, ok := s.Instances[id]
		if ok && i.Kind != "postgresql" {
			return m.otherCreateUser(ctx, s, i, name, password, database)
		}
		if !ok {
			return errors.New("database instance not found")
		}
		bin, e := m.Binary(s, i.Package, "psql")
		if e != nil {
			return e
		}
		admin, e := os.ReadFile(filepath.Join(m.Root, "instances", id, "password"))
		if e != nil {
			return e
		}
		identifier := func(v string) string { return `"` + strings.ReplaceAll(v, `"`, `""`) + `"` }
		literal := func(v string) string { return `'` + strings.ReplaceAll(v, `'`, `''`) + `'` }
		role := identifier(name)
		sql := "BEGIN; CREATE ROLE " + role + " LOGIN PASSWORD " + literal(password) + "; GRANT ALL PRIVILEGES ON DATABASE " + identifier(database) + " TO " + role + "; GRANT USAGE, CREATE ON SCHEMA public TO " + role + "; GRANT ALL PRIVILEGES ON ALL TABLES IN SCHEMA public TO " + role + "; GRANT ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA public TO " + role + "; ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL PRIVILEGES ON TABLES TO " + role + "; ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL PRIVILEGES ON SEQUENCES TO " + role + "; COMMIT;"
		c := exec.CommandContext(ctx, bin, "-X", "-w", "-h", "127.0.0.1", "-p", fmt.Sprint(i.Port), "-U", "devmanager", "-d", database, "--set", "ON_ERROR_STOP=1")
		c.Env = cleanDatabaseEnv("PGPASSWORD=" + string(admin))
		c.Stdin = strings.NewReader(sql)
		if e = c.Run(); e != nil {
			return fmt.Errorf("create user failed: %w", e)
		}
		return nil
	})
}

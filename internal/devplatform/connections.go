package devplatform

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net"
	"net/url"
	"os"
	"path/filepath"
	"strconv"
	"strings"
)

type ConnectionInfo struct {
	URI            string
	Username       string
	CredentialFile string
}

// WithDatabase returns a copy of ConnectionInfo targeting db if db is valid for kind.
func (c ConnectionInfo) WithDatabase(kind, db string) ConnectionInfo {
	db = strings.TrimSpace(db)
	if db == "" {
		return c
	}
	if kind == "redis" {
		if _, err := redisDB(db); err != nil {
			return c
		}
	} else if validID(db) != nil {
		return c
	}
	u, err := url.Parse(c.URI)
	if err != nil {
		return c
	}
	u.Path = "/" + db
	c.URI = u.String()
	return c
}

// EnvAssignment formats the password-free connection URI as a .env variable assignment.
func (c ConnectionInfo) EnvAssignment() string {
	return "DATABASE_URL=" + c.URI
}

// EnvAssignmentForKind formats the password-free connection URI as an
// engine-appropriate .env variable assignment.
func (c ConnectionInfo) EnvAssignmentForKind(kind string) string {
	switch kind {
	case "redis":
		return "REDIS_URL=" + c.URI
	case "mongodb":
		return "MONGODB_URI=" + c.URI
	default:
		return c.EnvAssignment()
	}
}

// JDBCURL formats the password-free connection URI as a JDBC URL for PostgreSQL or MySQL.
func (c ConnectionInfo) JDBCURL(kind string) (string, error) {
	if kind != "postgresql" && kind != "mysql" {
		return "", errors.New("JDBC URLs are supported only for PostgreSQL and MySQL")
	}
	u, err := url.Parse(c.URI)
	if err != nil || u.Host == "" {
		return "", errors.New("invalid database connection URI")
	}
	db := strings.TrimPrefix(u.Path, "/")
	out := fmt.Sprintf("jdbc:%s://%s/%s", kind, u.Host, db)
	user := ""
	if u.User != nil {
		user = u.User.Username()
	}
	if user != "" {
		q := url.Values{}
		q.Set("user", user)
		out += "?" + q.Encode()
	}
	return out, nil
}

func (m *Manager) ConnectionInfo(id string) (ConnectionInfo, error) {
	s, e := m.Read()
	if e != nil {
		return ConnectionInfo{}, e
	}
	i, ok := s.Instances[id]
	if !ok {
		return ConnectionInfo{}, errors.New("database instance not found")
	}
	if e := validID(i.ID); e != nil {
		return ConnectionInfo{}, e
	}
	if i.Port < 1024 || i.Port > 65535 {
		return ConnectionInfo{}, errors.New("invalid database connection port")
	}
	scheme, user, path := "", "", "/"
	query := url.Values{}
	switch i.Kind {
	case "postgresql":
		scheme, user, path = "postgresql", "devmanager", "/postgres"
	case "mysql":
		scheme, user = "mysql", "root"
	case "mongodb":
		scheme, user, path = "mongodb", "devmanager", "/admin"
		query.Set("authSource", "admin")
		query.Set("directConnection", "true")
	case "redis":
		scheme, user, path = "redis", "default", "/0"
	default:
		return ConnectionInfo{}, errors.New("unsupported database engine")
	}
	uri := url.URL{Scheme: scheme, User: url.User(user), Host: net.JoinHostPort("127.0.0.1", strconv.Itoa(i.Port)), Path: path, RawQuery: query.Encode()}
	return ConnectionInfo{URI: uri.String(), Username: user, CredentialFile: filepath.Join(m.Root, "instances", i.ID, "password")}, nil
}
func (m *Manager) DatabasePassword(id string) (string, error) {
	info, e := m.ConnectionInfo(id)
	if e != nil {
		return "", e
	}
	f, e := os.Open(info.CredentialFile)
	if e != nil {
		return "", e
	}
	defer f.Close()
	b, e := io.ReadAll(io.LimitReader(f, 4097))
	if e != nil {
		return "", e
	}
	if len(b) == 0 || len(b) > 4096 {
		return "", errors.New("invalid database credential file")
	}
	return string(b), nil
}
func (m *Manager) Connection(id string) (string, error) {
	info, e := m.ConnectionInfo(id)
	if e != nil {
		return "", e
	}
	return fmt.Sprintf("%s\nUsername: %s\nPassword file: %s", info.URI, info.Username, info.CredentialFile), nil
}

func requireRegularNonEmptyFile(path, label string) error {
	info, err := os.Stat(path)
	if err != nil || !info.Mode().IsRegular() || info.Size() == 0 {
		if err != nil {
			return fmt.Errorf("%s unavailable: %w", label, err)
		}
		return fmt.Errorf("%s unavailable", label)
	}
	return nil
}

// DatabaseCLICommand builds the POSIX shell launcher script for an instance's
// interactive CLI without embedding plaintext credentials.
func (m *Manager) DatabaseCLICommand(id, profileID, dbOverride string) (string, error) {
	if err := validID(id); err != nil {
		return "", err
	}
	if profileID != "" {
		if err := validID(profileID); err != nil {
			return "", err
		}
	}
	s, err := m.Read()
	if err != nil {
		return "", err
	}
	i, ok := s.Instances[id]
	if !ok {
		return "", errors.New("database instance not found")
	}
	if i.Port < 1024 || i.Port > 65535 {
		return "", errors.New("invalid database connection port")
	}
	var profile ConnectionProfile
	if profileID != "" {
		profile, ok = s.ConnectionProfiles[profileID]
		if !ok || profile.InstanceID != id {
			return "", errors.New("connection profile not found")
		}
		if err := validateConnectionProfile(profile, i.Kind); err != nil {
			return "", err
		}
	}

	tool := ""
	switch i.Kind {
	case "postgresql":
		tool = "psql"
	case "mysql":
		tool = "mysql"
	case "redis":
		tool = "redis-cli"
	default:
		return "", errors.New("Terminal CLI is supported for PostgreSQL, MySQL, and Redis instances only")
	}
	bin, err := m.Binary(&s, i.Package, tool)
	if err != nil {
		return "", err
	}

	port := strconv.Itoa(i.Port)
	dbOverride = strings.TrimSpace(dbOverride)
	var b strings.Builder
	b.WriteString("#!/bin/sh\nset -eu\nunset PGHOST PGPORT PGUSER PGDATABASE PGOPTIONS PGPASSFILE PGPASSWORD MYSQL_PWD MYSQL_HOST MYSQL_TCP_PORT REDISCLI_AUTH\n")

	switch i.Kind {
	case "postgresql":
		if profileID == "" {
			passwordFile := filepath.Join(m.instanceDirectory(id), "password")
			if err := requireRegularNonEmptyFile(passwordFile, "database credential file"); err != nil {
				return "", err
			}
			db := "postgres"
			if dbOverride != "" && validID(dbOverride) == nil {
				db = dbOverride
			}
			b.WriteString("PGPASSWORD=\"$(cat " + shellQuote(passwordFile) + ")\"\n")
			b.WriteString("export PGPASSWORD\n")
			b.WriteString("exec " + shellQuote(bin) + " -h 127.0.0.1 -p " + shellQuote(port) + " -U devmanager -d " + shellQuote(db) + "\n")
		} else {
			b.WriteString("exec " + shellQuote(bin) + " -h 127.0.0.1 -p " + shellQuote(port) + " -U " + shellQuote(profile.Username) + " -d " + shellQuote(profile.Database) + "\n")
		}
	case "mysql":
		if profileID == "" {
			clientCnf := filepath.Join(m.instanceDirectory(id), "client.cnf")
			if err := requireRegularNonEmptyFile(clientCnf, "MySQL client configuration"); err != nil {
				return "", err
			}
			b.WriteString("exec " + shellQuote(bin) + " " + shellQuote("--defaults-file="+clientCnf))
			if dbOverride != "" && validID(dbOverride) == nil {
				b.WriteString(" " + shellQuote(dbOverride))
			}
			b.WriteString("\n")
		} else {
			b.WriteString("exec " + shellQuote(bin) + " --no-defaults --host=127.0.0.1 " + shellQuote("--port="+port) + " --protocol=tcp " + shellQuote("--user="+profile.Username) + " -p " + shellQuote(profile.Database) + "\n")
		}
	case "redis":
		passwordFile := filepath.Join(m.instanceDirectory(id), "password")
		if err := requireRegularNonEmptyFile(passwordFile, "database credential file"); err != nil {
			return "", err
		}
		db := "0"
		if profileID != "" {
			db = strconv.Itoa(profile.RedisDB)
		} else if dbOverride != "" {
			if _, err := redisDB(dbOverride); err == nil {
				db = dbOverride
			}
		}
		b.WriteString("REDISCLI_AUTH=\"$(cat " + shellQuote(passwordFile) + ")\"\n")
		b.WriteString("export REDISCLI_AUTH\n")
		b.WriteString("exec " + shellQuote(bin) + " -h 127.0.0.1 -p " + shellQuote(port) + " -n " + shellQuote(db) + "\n")
	}
	return b.String(), nil
}

// OpenDatabaseCLI writes a 0700 launcher script for the instance's interactive
// CLI and opens it in macOS Terminal without placing secrets in argv or logs.
func (m *Manager) OpenDatabaseCLI(ctx context.Context, id, profileID, dbOverride string) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	script, err := m.DatabaseCLICommand(id, profileID, dbOverride)
	if err != nil {
		return err
	}
	scriptPath := filepath.Join(m.instanceDirectory(id), "open-cli.command")
	if err := atomicWrite(scriptPath, []byte(script), 0700); err != nil {
		return err
	}
	_, err = m.Run(ctx, "/usr/bin/open", "-a", "Terminal", scriptPath)
	return err
}

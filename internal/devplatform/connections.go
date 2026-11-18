package devplatform

import (
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

package devplatform

import (
	"context"
	"errors"
	"fmt"
	"strings"

	"go.mongodb.org/mongo-driver/v2/bson"
)

// IsProtectedDatabase reports whether name is empty or a reserved system
// database across managed engines.
func IsProtectedDatabase(name string) bool {
	switch strings.ToLower(strings.TrimSpace(name)) {
	case "", "postgres", "template0", "template1", "mysql", "sys", "information_schema", "performance_schema", "admin", "local", "config":
		return true
	}
	return false
}

// IsProtectedDatabaseUser reports whether name is empty or a built-in/admin
// database account that must never be dropped.
func IsProtectedDatabaseUser(name string) bool {
	trimmed := strings.TrimSpace(name)
	if trimmed == "" {
		return true
	}
	user, _, _ := strings.Cut(trimmed, "@")
	lower := strings.ToLower(strings.TrimSpace(user))
	if lower == "" || strings.HasPrefix(lower, "pg_") {
		return true
	}
	switch lower {
	case "devmanager", "root", "default", "mysql.sys", "mysql.session", "mysql.infoschema":
		return true
	}
	return false
}

func pgIdentifier(v string) string {
	return `"` + strings.ReplaceAll(v, `"`, `""`) + `"`
}

// DropDatabase drops a non-system database on a running PostgreSQL, MySQL, or
// MongoDB instance under its instance lease.
func (m *Manager) DropDatabase(ctx context.Context, id, name string) error {
	if err := validID(id); err != nil {
		return err
	}
	name = strings.TrimSpace(name)
	if name == "" {
		return errors.New("database name is required")
	}
	if IsProtectedDatabase(name) {
		return fmt.Errorf("cannot drop protected system database %q", name)
	}
	if err := validID(name); err != nil {
		return err
	}
	return m.withDatabaseOperation(ctx, id, func(s *State) error {
		i, ok := s.Instances[id]
		if !ok {
			return errors.New("database instance not found")
		}
		switch i.Kind {
		case "postgresql":
			_, err := m.databaseCommand(ctx, s, id, "psql", "-X", "-d", "postgres", "--set", "ON_ERROR_STOP=1", "-c", "DROP DATABASE "+pgIdentifier(name))
			return err
		case "mysql":
			_, err := m.mysqlSQL(ctx, s, i, "DROP DATABASE "+sqlIdentifier(name))
			return err
		case "mongodb":
			client, err := m.mongoClient(ctx, i, true)
			if err != nil {
				return err
			}
			defer client.Disconnect(context.Background())
			return client.Database(name).Drop(ctx)
		case "redis":
			return errors.New("Redis uses numbered databases; database drop is not available")
		default:
			return errors.New("unsupported database engine")
		}
	})
}

// DropUser drops a non-admin database user/role on a running PostgreSQL,
// MySQL, or MongoDB instance under its instance lease.
func (m *Manager) DropUser(ctx context.Context, id, username, database string) error {
	if err := validID(id); err != nil {
		return err
	}
	username = strings.TrimSpace(username)
	database = strings.TrimSpace(database)
	if username == "" {
		return errors.New("username is required")
	}
	if IsProtectedDatabaseUser(username) {
		return fmt.Errorf("cannot drop protected built-in user %q", username)
	}
	if database != "" {
		if err := validID(database); err != nil {
			return err
		}
	}
	if u, db, hasAt := strings.Cut(username, "@"); hasAt {
		if err := validID(u); err != nil {
			return err
		}
		if db == "" {
			return validID(username)
		}
		if err := validID(db); err != nil {
			return err
		}
	} else if err := validID(username); err != nil {
		return err
	}
	return m.withDatabaseOperation(ctx, id, func(s *State) error {
		i, ok := s.Instances[id]
		if !ok {
			return errors.New("database instance not found")
		}
		switch i.Kind {
		case "postgresql":
			if strings.Contains(username, "@") {
				return validID(username)
			}
			role := pgIdentifier(username)
			_, err := m.databaseCommand(ctx, s, id, "psql", "-X", "-d", "postgres", "--set", "ON_ERROR_STOP=1", "-c", "DROP ROLE "+role)
			if err != nil && database != "" && strings.Contains(err.Error(), "cannot be dropped") {
				if _, cleanErr := m.databaseCommand(ctx, s, id, "psql", "-X", "-d", database, "--set", "ON_ERROR_STOP=1", "-c", "REASSIGN OWNED BY "+role+" TO devmanager; DROP OWNED BY "+role); cleanErr == nil {
					_, retryErr := m.databaseCommand(ctx, s, id, "psql", "-X", "-d", "postgres", "--set", "ON_ERROR_STOP=1", "-c", "DROP ROLE "+role)
					return retryErr
				}
			}
			return err
		case "mysql":
			if strings.Contains(username, "@") {
				return validID(username)
			}
			hostsRaw, err := m.mysqlSQL(ctx, s, i, "SELECT host FROM mysql.user WHERE user = "+sqlLiteral(username)+";")
			if err != nil {
				return err
			}
			hosts := splitCatalogLines(hostsRaw)
			if len(hosts) == 0 {
				_, err = m.mysqlSQL(ctx, s, i, "DROP USER "+sqlLiteral(username)+"@'localhost'")
				return err
			}
			targets := make([]string, 0, len(hosts))
			for _, host := range hosts {
				targets = append(targets, sqlLiteral(username)+"@"+sqlLiteral(host))
			}
			_, err = m.mysqlSQL(ctx, s, i, "DROP USER "+strings.Join(targets, ", "))
			return err
		case "mongodb":
			user := username
			db := database
			if u, d, hasAt := strings.Cut(username, "@"); hasAt {
				user = u
				if db == "" {
					db = d
				}
			}
			if db == "" {
				db = "admin"
			}
			if err := validID(user); err != nil {
				return err
			}
			if err := validID(db); err != nil {
				return err
			}
			client, err := m.mongoClient(ctx, i, true)
			if err != nil {
				return err
			}
			defer client.Disconnect(context.Background())
			return client.Database(db).RunCommand(ctx, bson.D{{Key: "dropUser", Value: user}}).Err()
		case "redis":
			return errors.New("Redis uses a single default user; user drop is not available")
		default:
			return errors.New("unsupported database engine")
		}
	})
}

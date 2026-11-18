package devplatform

import (
	"context"
	"errors"
	"sort"
	"strconv"
	"strings"
	"time"

	"go.mongodb.org/mongo-driver/v2/bson"
)

// InstanceCatalog lists databases and users currently present in a running instance.
type InstanceCatalog struct {
	Databases []string
	Users     []string
}

// InspectInstanceCatalog queries a running instance for its databases and users.
func (m *Manager) InspectInstanceCatalog(ctx context.Context, id string) (InstanceCatalog, error) {
	if err := validID(id); err != nil {
		return InstanceCatalog{}, err
	}
	s, err := m.Read()
	if err != nil {
		return InstanceCatalog{}, err
	}
	i, ok := s.Instances[id]
	if !ok {
		return InstanceCatalog{}, errors.New("database instance not found")
	}
	qctx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()
	if m.InspectCatalog != nil {
		return m.InspectCatalog(qctx, i)
	}
	switch i.Kind {
	case "postgresql":
		dbsRaw, err := m.databaseCommand(qctx, &s, id, "psql", "-X", "-d", "postgres", "-Atc", "SELECT datname FROM pg_database WHERE datistemplate = false ORDER BY datname")
		if err != nil {
			return InstanceCatalog{}, err
		}
		usersRaw, err := m.databaseCommand(qctx, &s, id, "psql", "-X", "-d", "postgres", "-Atc", "SELECT rolname FROM pg_roles WHERE rolcanlogin = true AND rolname NOT LIKE 'pg_%' ORDER BY rolname")
		if err != nil {
			return InstanceCatalog{}, err
		}
		return InstanceCatalog{
			Databases: splitCatalogLines(dbsRaw),
			Users:     splitCatalogLines(usersRaw),
		}, nil
	case "mysql":
		dbsRaw, err := m.mysqlSQL(qctx, &s, i, "SELECT schema_name FROM information_schema.schemata WHERE schema_name NOT IN ('information_schema', 'performance_schema', 'mysql', 'sys') ORDER BY schema_name;")
		if err != nil {
			return InstanceCatalog{}, err
		}
		usersRaw, err := m.mysqlSQL(qctx, &s, i, "SELECT DISTINCT user FROM mysql.user WHERE user NOT IN ('mysql.infoschema', 'mysql.session', 'mysql.sys') ORDER BY user;")
		if err != nil {
			return InstanceCatalog{}, err
		}
		return InstanceCatalog{
			Databases: splitCatalogLines(dbsRaw),
			Users:     splitCatalogLines(usersRaw),
		}, nil
	case "mongodb":
		client, err := m.mongoClient(qctx, i, true)
		if err != nil {
			return InstanceCatalog{}, err
		}
		defer client.Disconnect(context.Background())
		names, err := client.ListDatabaseNames(qctx, bson.D{})
		if err != nil {
			return InstanceCatalog{}, err
		}
		var dbs []string
		for _, name := range names {
			if name == "local" || name == "config" {
				continue
			}
			dbs = append(dbs, name)
		}
		sort.Strings(dbs)

		var res struct {
			Users []struct {
				User string `bson:"user"`
				DB   string `bson:"db"`
			} `bson:"users"`
		}
		if err := client.Database("admin").RunCommand(qctx, bson.D{{Key: "usersInfo", Value: bson.D{{Key: "forAllDBs", Value: true}}}}).Decode(&res); err != nil {
			return InstanceCatalog{}, err
		}
		seenUsers := map[string]bool{}
		var users []string
		for _, u := range res.Users {
			if u.User == "" {
				continue
			}
			label := u.User
			if u.DB != "" && u.DB != "admin" {
				label = u.User + "@" + u.DB
			}
			if !seenUsers[label] {
				seenUsers[label] = true
				users = append(users, label)
			}
		}
		sort.Strings(users)
		return InstanceCatalog{Databases: dbs, Users: users}, nil
	case "redis":
		var infoText string
		err := m.withRedisSession(qctx, i, 0, func(call func(args ...[]byte) (any, error)) error {
			raw, err := call([]byte("INFO"), []byte("keyspace"))
			if err != nil {
				return err
			}
			switch v := raw.(type) {
			case []byte:
				infoText = string(v)
			case string:
				infoText = v
			}
			return nil
		})
		if err != nil {
			return InstanceCatalog{}, err
		}
		return InstanceCatalog{
			Databases: parseRedisKeyspaceDatabases(infoText),
			Users:     []string{"default"},
		}, nil
	default:
		return InstanceCatalog{}, errors.New("unsupported database engine")
	}
}

func splitCatalogLines(raw string) []string {
	seen := map[string]bool{}
	var out []string
	for _, line := range strings.Split(raw, "\n") {
		v := strings.TrimSpace(line)
		if v == "" || seen[v] {
			continue
		}
		seen[v] = true
		out = append(out, v)
	}
	return out
}

func parseRedisKeyspaceDatabases(info string) []string {
	seen := map[int]bool{}
	var nums []int
	for _, line := range strings.Split(info, "\n") {
		line = strings.TrimSpace(line)
		if !strings.HasPrefix(line, "db") {
			continue
		}
		prefix, _, ok := strings.Cut(line, ":")
		if !ok {
			continue
		}
		n, err := strconv.Atoi(strings.TrimPrefix(prefix, "db"))
		if err != nil || n < 0 || n > 15 || seen[n] {
			continue
		}
		seen[n] = true
		nums = append(nums, n)
	}
	if len(nums) == 0 {
		return []string{"0"}
	}
	sort.Ints(nums)
	out := make([]string, len(nums))
	for i, n := range nums {
		out[i] = strconv.Itoa(n)
	}
	return out
}

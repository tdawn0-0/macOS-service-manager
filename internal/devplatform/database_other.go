package devplatform

import (
	"context"
	"errors"
	"fmt"
	"go.mongodb.org/mongo-driver/v2/bson"
	"go.mongodb.org/mongo-driver/v2/mongo"
	"go.mongodb.org/mongo-driver/v2/mongo/options"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"
)

func (m *Manager) mongoClient(ctx context.Context, i Instance, authenticate bool) (*mongo.Client, error) {
	uri := fmt.Sprintf("mongodb://127.0.0.1:%d/?directConnection=true", i.Port)
	if authenticate {
		pw, e := os.ReadFile(filepath.Join(m.instanceDirectory(i.ID), "password"))
		if e != nil {
			return nil, e
		}
		uri = fmt.Sprintf("mongodb://devmanager:%s@127.0.0.1:%d/?authSource=admin&directConnection=true", url.QueryEscape(string(pw)), i.Port)
	}
	return mongo.Connect(options.Client().ApplyURI(uri).SetServerSelectionTimeout(3 * time.Second))
}
func (m *Manager) createOther(ctx context.Context, s *State, i Instance, dir, password string) error {
	switch i.Kind {
	case "mongodb":
		if _, e := m.Binary(s, i.Package, "mongod"); e != nil {
			return e
		}
		if e := os.MkdirAll(filepath.Join(dir, "data"), 0700); e != nil {
			return e
		}
		if e := m.startInstance(ctx, s, i); e != nil {
			return e
		}
		defer m.stopJob(context.Background(), m.databaseJobID(i.ID))
		client, e := m.mongoClient(ctx, i, false)
		if e != nil {
			return e
		}
		defer client.Disconnect(context.Background())
		return client.Database("admin").RunCommand(ctx, bson.D{{Key: "createUser", Value: "devmanager"}, {Key: "pwd", Value: password}, {Key: "roles", Value: bson.A{bson.D{{Key: "role", Value: "root"}, {Key: "db", Value: "admin"}}}}}).Err()
	case "mysql":
		bin, e := m.Binary(s, i.Package, "mysqld")
		if e != nil {
			return e
		}
		pkg := s.Packages[i.Package]
		if _, e = m.runInitializationTool(ctx, bin, "--no-defaults", "--initialize-insecure", "--basedir="+m.packageDir(pkg), "--datadir="+filepath.Join(dir, "data")); e != nil {
			return e
		}
		if e = m.mysqlConfig(i, dir, ""); e != nil {
			return e
		}
		if e = m.startInstance(ctx, s, i); e != nil {
			return e
		}
		defer m.stopJob(context.Background(), m.databaseJobID(i.ID))
		if _, e = m.mysqlSQL(ctx, s, i, "ALTER USER 'root'@'localhost' IDENTIFIED BY "+sqlLiteral(password)); e != nil {
			return e
		}
		return m.mysqlConfig(i, dir, password)
	}
	return errors.New("unsupported engine")
}
func (m *Manager) mysqlConfig(i Instance, dir, password string) error {
	return atomicWrite(filepath.Join(dir, "client.cnf"), []byte(fmt.Sprintf("[client]\nuser=root\npassword=%q\nhost=127.0.0.1\nport=%d\nprotocol=tcp\n", password, i.Port)), 0600)
}
func sqlLiteral(v string) string {
	return "'" + strings.ReplaceAll(strings.ReplaceAll(v, "\\", "\\\\"), "'", "''") + "'"
}
func sqlIdentifier(v string) string { return "`" + strings.ReplaceAll(v, "`", "``") + "`" }
func (m *Manager) mysqlSQL(ctx context.Context, s *State, i Instance, sql string) (string, error) {
	bin, e := m.Binary(s, i.Package, "mysql")
	if e != nil {
		return "", e
	}
	c := exec.CommandContext(ctx, bin, "--defaults-file="+filepath.Join(m.instanceDirectory(i.ID), "client.cnf"), "--batch", "--skip-column-names")
	c.Stdin = strings.NewReader(sql)
	out, e := c.CombinedOutput()
	if e != nil {
		return "", fmt.Errorf("MySQL command failed: %w", e)
	}
	return string(out), nil
}
func (m *Manager) otherCreateDatabase(ctx context.Context, s *State, i Instance, name string) error {
	switch i.Kind {
	case "mysql":
		_, e := m.mysqlSQL(ctx, s, i, "CREATE DATABASE "+sqlIdentifier(name))
		return e
	case "mongodb":
		client, e := m.mongoClient(ctx, i, true)
		if e != nil {
			return e
		}
		defer client.Disconnect(context.Background())
		return client.Database(name).CreateCollection(ctx, "__devmanager")
	}
	return errors.New("this engine uses numbered databases; no database creation is needed")
}
func (m *Manager) otherCreateUser(ctx context.Context, s *State, i Instance, name, password, database string) error {
	switch i.Kind {
	case "mysql":
		_, e := m.mysqlSQL(ctx, s, i, "CREATE USER "+sqlLiteral(name)+"@'localhost' IDENTIFIED BY "+sqlLiteral(password)+"; GRANT ALL ON "+sqlIdentifier(database)+".* TO "+sqlLiteral(name)+"@'localhost'")
		return e
	case "mongodb":
		client, e := m.mongoClient(ctx, i, true)
		if e != nil {
			return e
		}
		defer client.Disconnect(context.Background())
		return client.Database(database).RunCommand(ctx, bson.D{{Key: "createUser", Value: name}, {Key: "pwd", Value: password}, {Key: "roles", Value: bson.A{"readWrite"}}}).Err()
	}
	return errors.New("user management is not available for this engine")
}

func (m *Manager) otherBackup(ctx context.Context, s *State, i Instance, name, path string) error {
	if i.Kind == "redis" || i.Kind == "valkey" {
		return m.redisBackup(ctx, i, name, path)
	}
	if i.Kind == "mysql" {
		bin, e := m.Binary(s, i.Package, "mysqldump")
		if e != nil {
			return e
		}
		f, e := os.OpenFile(path, os.O_WRONLY|os.O_TRUNC, 0600)
		if e != nil {
			return e
		}
		defer f.Close()
		c := exec.CommandContext(ctx, bin, "--defaults-file="+filepath.Join(m.instanceDirectory(i.ID), "client.cnf"), "--single-transaction", "--set-gtid-purged=OFF", name)
		c.Stdout = f
		return c.Run()
	}
	if i.Kind == "mongodb" {
		return m.mongoBackup(ctx, i, name, path)
	}
	return errors.New("backup for this engine is not implemented")
}
func (m *Manager) otherRestore(ctx context.Context, s *State, i Instance, name, path string) error {
	if i.Kind == "redis" || i.Kind == "valkey" {
		return m.redisRestore(ctx, i, name, path)
	}
	if i.Kind == "mysql" {
		if e := m.otherCreateDatabase(ctx, s, i, name); e != nil {
			return e
		}
		b, e := os.ReadFile(path)
		if e != nil {
			return e
		}
		_, e = m.mysqlSQL(ctx, s, i, "USE "+sqlIdentifier(name)+";\n"+string(b))
		return e
	}
	if i.Kind == "mongodb" {
		return m.mongoRestore(ctx, i, name, path)
	}
	return errors.New("restore for this engine is not implemented")
}

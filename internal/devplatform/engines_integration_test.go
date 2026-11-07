package devplatform

import (
	"context"
	"fmt"
	"go.mongodb.org/mongo-driver/v2/bson"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
	"time"
)

func integrationPort(t *testing.T) int {
	t.Helper()
	l, e := net.Listen("tcp4", "127.0.0.1:0")
	if e != nil {
		t.Fatal(e)
	}
	port := l.Addr().(*net.TCPAddr).Port
	l.Close()
	return port
}
func TestAdditionalEnginesIntegration(t *testing.T) {
	root := os.Getenv("MSM_SIX_INTEGRATION_ROOT")
	if root == "" {
		t.Skip("set MSM_SIX_INTEGRATION_ROOT to an isolated directory with installed packages")
	}
	m := New(root)
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Minute)
	defer cancel()
	state, e := m.Read()
	if e != nil {
		t.Fatal(e)
	}
	for _, kind := range []string{"mysql", "mongodb", "redis"} {
		t.Run(kind, func(t *testing.T) {
			version := AvailableVersions(kind)[0]
			id := "verify-" + kind + "-" + randomID()
			i := Instance{ID: id, Kind: kind, Package: kind + "@" + version + "-" + runtime.GOARCH, Port: integrationPort(t), Persistent: true}
			if e := m.CreateInstance(ctx, i); e != nil {
				t.Fatal(e)
			}
			defer func() {
				if e := m.InstanceAction(context.Background(), id, "remove"); e != nil {
					t.Error(e)
				}
			}()
			if e = m.InstanceAction(ctx, id, "start"); e != nil {
				t.Fatal(e)
			}
			if kind == "redis" {
				bin, e := m.Binary(&state, i.Package, "redis-cli")
				if e != nil {
					t.Fatal(e)
				}
				password, _ := os.ReadFile(filepath.Join(root, "instances", id, "password"))
				command := func(args ...string) string {
					c := exec.CommandContext(ctx, bin, append([]string{"-p", fmt.Sprint(i.Port)}, args...)...)
					c.Env = append(os.Environ(), "REDISCLI_AUTH="+string(password))
					b, e := c.CombinedOutput()
					if e != nil {
						t.Fatal(e, string(b))
					}
					return strings.TrimSpace(string(b))
				}
				if command("PING") != "PONG" {
					t.Fatal("not authenticated")
				}
				if command("SET", "verification", "persisted") != "OK" {
					t.Fatal("write failed")
				}
				if e = m.InstanceAction(ctx, id, "stop"); e != nil {
					t.Fatal(e)
				}
				if e = m.InstanceAction(ctx, id, "start"); e != nil {
					t.Fatal(e)
				}
				if command("GET", "verification") != "persisted" {
					t.Fatal("data lost on restart")
				}
				if command("LPUSH", "items", "one", "two") != "2" {
					t.Fatal("list write failed")
				}
				backup, e := m.Backup(ctx, id, "0")
				if e != nil {
					t.Fatal(e)
				}
				defer os.Remove(backup)
				if e = m.Restore(ctx, id, "1", backup); e != nil {
					t.Fatal(e)
				}
				result, e := m.redisCommand(ctx, i, 1, []byte("GET"), []byte("verification"))
				if e != nil || string(result.([]byte)) != "persisted" {
					t.Fatal(result, e)
				}
				result, e = m.redisCommand(ctx, i, 1, []byte("LRANGE"), []byte("items"), []byte("0"), []byte("-1"))
				if e != nil || len(result.([]any)) != 2 {
					t.Fatal(result, e)
				}
				if e = m.Restore(ctx, id, "1", backup); e == nil {
					t.Fatal("overwrote nonempty Redis database")
				}

				return
			}
			if e = m.CreateDatabase(ctx, id, "source_db"); e != nil {
				t.Fatal(e)
			}
			if e = m.CreateUser(ctx, id, "app_user", "verify-secret-password", "source_db"); e != nil {
				t.Fatal(e)
			}
			if kind == "mysql" {
				if _, e = m.mysqlSQL(ctx, &state, i, "CREATE TABLE source_db.sample(n int); INSERT INTO source_db.sample VALUES(42)"); e != nil {
					t.Fatal(e)
				}
			} else {
				client, e := m.mongoClient(ctx, i, true)
				if e != nil {
					t.Fatal(e)
				}
				defer client.Disconnect(context.Background())
				db := client.Database("source_db")
				if e = db.RunCommand(ctx, bson.D{{Key: "create", Value: "sample"}, {Key: "validator", Value: bson.D{{Key: "n", Value: bson.D{{Key: "$type", Value: "int"}}}}}}).Err(); e != nil {
					t.Fatal(e)
				}
				if e = db.RunCommand(ctx, bson.D{{Key: "createIndexes", Value: "sample"}, {Key: "indexes", Value: bson.A{bson.D{{Key: "key", Value: bson.D{{Key: "n", Value: 1}}}, {Key: "name", Value: "unique_n"}, {Key: "unique", Value: true}}}}}).Err(); e != nil {
					t.Fatal(e)
				}
				if e = db.RunCommand(ctx, bson.D{{Key: "create", Value: "sample_view"}, {Key: "viewOn", Value: "sample"}, {Key: "pipeline", Value: bson.A{bson.D{{Key: "$match", Value: bson.D{{Key: "n", Value: 42}}}}}}}).Err(); e != nil {
					t.Fatal(e)
				}
				if _, e = client.Database("source_db").Collection("sample").InsertOne(ctx, bson.D{{Key: "n", Value: 42}}); e != nil {
					t.Fatal(e)
				}
			}
			backup, e := m.Backup(ctx, id, "source_db")
			if e != nil {
				t.Fatal(e)
			}
			defer os.Remove(backup)
			if e = m.Restore(ctx, id, "restored_db", backup); e != nil {
				t.Fatal(e)
			}
			if kind == "mysql" {
				out, e := m.mysqlSQL(ctx, &state, i, "SELECT n FROM restored_db.sample")
				if e != nil || strings.TrimSpace(out) != "42" {
					t.Fatal(out, e)
				}
			} else {
				client, e := m.mongoClient(ctx, i, true)
				if e != nil {
					t.Fatal(e)
				}
				defer client.Disconnect(context.Background())
				n, e := client.Database("restored_db").Collection("sample").CountDocuments(ctx, bson.D{{Key: "n", Value: 42}})
				if e != nil || n != 1 {
					t.Fatal(n, e)
				}
				db := client.Database("restored_db")
				if _, e = db.Collection("sample").InsertOne(ctx, bson.D{{Key: "n", Value: 42}}); e == nil {
					t.Fatal("unique index was lost during restore")
				}
				if _, e = db.Collection("sample").InsertOne(ctx, bson.D{{Key: "n", Value: "invalid-type"}}); e == nil {
					t.Fatal("collection validator was lost during restore")
				}
				if n, e = db.Collection("sample_view").CountDocuments(ctx, bson.D{}); e != nil || n != 1 {
					t.Fatal("view definition was lost during restore", n, e)
				}
			}
		})
	}
}

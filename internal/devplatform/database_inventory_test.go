package devplatform

import (
	"bufio"
	"context"
	"fmt"
	"io"
	"net"
	"os"
	"path/filepath"
	"reflect"
	"strconv"
	"strings"
	"testing"
)

func TestParseRedisKeyspaceDatabases(t *testing.T) {
	info := "# Keyspace\r\ndb10:keys=3,expires=0\r\ndb0:keys=12,expires=1\r\ndb2:keys=1,expires=0\r\n"
	got := parseRedisKeyspaceDatabases(info)
	want := []string{"0", "2", "10"}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("got %v, want %v", got, want)
	}
	if got := parseRedisKeyspaceDatabases("# Keyspace\r\n"); !reflect.DeepEqual(got, []string{"0"}) {
		t.Fatalf("empty keyspace got %v", got)
	}
}

func TestInspectInstanceCatalogRedisSession(t *testing.T) {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer ln.Close()
	port := ln.Addr().(*net.TCPAddr).Port

	go func() {
		conn, err := ln.Accept()
		if err != nil {
			return
		}
		defer conn.Close()
		r := bufio.NewReader(conn)
		for {
			argcLine, err := r.ReadString('\n')
			if err != nil {
				return
			}
			argc, err := strconv.Atoi(strings.TrimSpace(strings.TrimPrefix(argcLine, "*")))
			if err != nil || argc <= 0 {
				return
			}
			args := make([]string, argc)
			for i := 0; i < argc; i++ {
				lenLine, err := r.ReadString('\n')
				if err != nil {
					return
				}
				n, err := strconv.Atoi(strings.TrimSpace(strings.TrimPrefix(lenLine, "$")))
				if err != nil || n < 0 {
					return
				}
				buf := make([]byte, n+2)
				if _, err := io.ReadFull(r, buf); err != nil {
					return
				}
				args[i] = string(buf[:n])
			}
			if strings.EqualFold(args[0], "AUTH") || strings.EqualFold(args[0], "SELECT") {
				_, _ = conn.Write([]byte("+OK\r\n"))
			} else if strings.EqualFold(args[0], "INFO") {
				payload := "# Keyspace\r\ndb0:keys=4,expires=0\r\ndb3:keys=2,expires=0\r\n"
				_, _ = fmt.Fprintf(conn, "$%d\r\n%s\r\n", len(payload), payload)
			}
		}
	}()

	m := testManager(t)
	inst := Instance{ID: "cache", Kind: "redis", Port: port}
	if err := m.withState(context.Background(), func(s *State) error {
		s.Instances[inst.ID] = inst
		return nil
	}); err != nil {
		t.Fatal(err)
	}
	dir := filepath.Join(m.Root, "instances", inst.ID)
	if err := os.MkdirAll(dir, 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "password"), []byte("secret-redis-pw"), 0o600); err != nil {
		t.Fatal(err)
	}

	cat, err := m.InspectInstanceCatalog(context.Background(), inst.ID)
	if err != nil {
		t.Fatal(err)
	}
	if !reflect.DeepEqual(cat.Databases, []string{"0", "3"}) || !reflect.DeepEqual(cat.Users, []string{"default"}) {
		t.Fatalf("unexpected Redis catalog: %+v", cat)
	}
	if _, err := m.InspectInstanceCatalog(context.Background(), "missing"); err == nil {
		t.Fatal("expected error for missing instance")
	}
	if _, err := m.InspectInstanceCatalog(context.Background(), "../bad"); err == nil {
		t.Fatal("expected error for invalid instance ID")
	}
}

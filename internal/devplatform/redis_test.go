package devplatform

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"
)

type mockRedisServer struct {
	ln         net.Listener
	mu         sync.Mutex
	dbs        map[int]map[string]mockRedisVal
	keysCalls  int
	scanPages  [][]string
	scanIndex  int
	failOnEval bool
	evalCalls  int
	evalCounts []int
	closed     bool
}

type mockRedisVal struct {
	dump []byte
	ttl  int64
}

func startMockRedisServer(t *testing.T) *mockRedisServer {
	t.Helper()
	ln, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	s := &mockRedisServer{
		ln:  ln,
		dbs: make(map[int]map[string]mockRedisVal),
	}
	go s.serve()
	t.Cleanup(func() {
		s.mu.Lock()
		s.closed = true
		s.ln.Close()
		s.mu.Unlock()
	})
	return s
}

func (s *mockRedisServer) port() int {
	return s.ln.Addr().(*net.TCPAddr).Port
}

func (s *mockRedisServer) serve() {
	for {
		conn, err := s.ln.Accept()
		if err != nil {
			return
		}
		go s.handleConn(conn)
	}
}

func (s *mockRedisServer) handleConn(conn net.Conn) {
	defer conn.Close()
	r := bufio.NewReader(conn)
	currentDB := 0

	for {
		req, err := redisRead(r, 0)
		if err != nil {
			return
		}
		args, ok := req.([]any)
		if !ok || len(args) == 0 {
			_ = sendRESPError(conn, "ERR invalid command")
			return
		}
		cmdBytes, ok := args[0].([]byte)
		if !ok {
			_ = sendRESPError(conn, "ERR invalid command name")
			return
		}
		cmd := strings.ToUpper(string(cmdBytes))

		switch cmd {
		case "AUTH":
			_ = sendRESPStatus(conn, "OK")
		case "SELECT":
			if len(args) < 2 {
				_ = sendRESPError(conn, "ERR wrong number of arguments for 'select' command")
				return
			}
			dbNum, _ := strconv.Atoi(string(args[1].([]byte)))
			currentDB = dbNum
			_ = sendRESPStatus(conn, "OK")
		case "KEYS":
			s.mu.Lock()
			s.keysCalls++
			s.mu.Unlock()
			_ = sendRESPError(conn, "ERR KEYS command is disabled")
		case "SCAN":
			// args: SCAN <cursor> COUNT <count>
			s.mu.Lock()
			var keys []string
			var nextCursor string
			if len(s.scanPages) > 0 {
				if s.scanIndex < len(s.scanPages) {
					keys = s.scanPages[s.scanIndex]
					s.scanIndex++
					if s.scanIndex < len(s.scanPages) {
						nextCursor = strconv.Itoa(s.scanIndex)
					} else {
						nextCursor = "0"
					}
				} else {
					nextCursor = "0"
				}
			} else {
				// default scan from currentDB
				dbMap := s.dbs[currentDB]
				for k := range dbMap {
					keys = append(keys, k)
				}
				nextCursor = "0"
			}
			s.mu.Unlock()

			// RESP array: [nextCursor, [keys...]]
			var respBuf bytes.Buffer
			respBuf.WriteString("*2\r\n")
			// cursor bulk string
			fmt.Fprintf(&respBuf, "$%d\r\n%s\r\n", len(nextCursor), nextCursor)
			// keys array
			fmt.Fprintf(&respBuf, "*%d\r\n", len(keys))
			for _, k := range keys {
				fmt.Fprintf(&respBuf, "$%d\r\n%s\r\n", len(k), k)
			}
			if _, err := conn.Write(respBuf.Bytes()); err != nil {
				return
			}
		case "DBSIZE":
			s.mu.Lock()
			count := int64(len(s.dbs[currentDB]))
			s.mu.Unlock()
			fmt.Fprintf(conn, ":%d\r\n", count)
		case "FLUSHDB":
			s.mu.Lock()
			delete(s.dbs, currentDB)
			s.mu.Unlock()
			_ = sendRESPStatus(conn, "OK")
		case "EVAL":
			// args: EVAL <script> <numkeys> [KEYS...] [ARGV...]
			if len(args) < 3 {
				_ = sendRESPError(conn, "ERR wrong number of arguments for 'eval' command")
				return
			}
			script := string(args[1].([]byte))
			numkeys, _ := strconv.Atoi(string(args[2].([]byte)))

			s.mu.Lock()
			s.evalCalls++
			if s.failOnEval {
				s.mu.Unlock()
				_ = sendRESPError(conn, "ERR mock eval failure")
				return
			}

			if strings.Contains(script, "DUMP") {
				// Dump batch script: KEYS are keys
				keysToDump := args[3 : 3+numkeys]
				var triples []any
				dbMap := s.dbs[currentDB]
				for _, kb := range keysToDump {
					k := string(kb.([]byte))
					val, exists := dbMap[k]
					if !exists {
						// Mock default value if not preset
						val = mockRedisVal{dump: []byte("dump-" + k), ttl: 3600000}
					}
					triples = append(triples, []byte(k), val.dump, val.ttl)
				}
				s.mu.Unlock()

				var respBuf bytes.Buffer
				fmt.Fprintf(&respBuf, "*%d\r\n", len(triples))
				for _, tr := range triples {
					switch v := tr.(type) {
					case []byte:
						fmt.Fprintf(&respBuf, "$%d\r\n", len(v))
						respBuf.Write(v)
						respBuf.WriteString("\r\n")
					case int64:
						fmt.Fprintf(&respBuf, ":%d\r\n", v)
					}
				}
				if _, err := conn.Write(respBuf.Bytes()); err != nil {
					return
				}
			} else if strings.Contains(script, "RESTORE") {
				// Restore batch script: ARGV has key, ttl, val triples
				argv := args[3+numkeys:]
				count := len(argv) / 3
				s.evalCounts = append(s.evalCounts, count)
				if s.dbs[currentDB] == nil {
					s.dbs[currentDB] = make(map[string]mockRedisVal)
				}
				for n := 0; n < len(argv); n += 3 {
					k := string(argv[n].([]byte))
					ttl, _ := strconv.ParseInt(string(argv[n+1].([]byte)), 10, 64)
					val := argv[n+2].([]byte)
					s.dbs[currentDB][k] = mockRedisVal{dump: val, ttl: ttl}
				}
				s.mu.Unlock()
				fmt.Fprintf(conn, ":%d\r\n", count)
			} else {
				s.mu.Unlock()
				_ = sendRESPError(conn, "ERR unknown script")
			}
		default:
			_ = sendRESPError(conn, fmt.Sprintf("ERR unknown command '%s'", cmd))
		}
	}
}

func sendRESPStatus(w io.Writer, msg string) error {
	_, err := fmt.Fprintf(w, "+%s\r\n", msg)
	return err
}

func sendRESPError(w io.Writer, msg string) error {
	_, err := fmt.Fprintf(w, "-%s\r\n", msg)
	return err
}

func setupMockRedisInstance(t *testing.T, s *mockRedisServer) (*Manager, Instance) {
	t.Helper()
	root := t.TempDir()
	instID := "test-redis-inst"
	instDir := filepath.Join(root, "instances", instID)
	if err := os.MkdirAll(instDir, 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(instDir, "password"), []byte("mock-password"), 0600); err != nil {
		t.Fatal(err)
	}
	m := &Manager{Root: root}
	inst := Instance{
		ID:   instID,
		Kind: "redis",
		Port: s.port(),
	}
	return m, inst
}

func TestRedisBackupUsesCursorScanAndDeduplicatesKeys(t *testing.T) {
	mock := startMockRedisServer(t)

	// Set up 3 SCAN pages with 1,200 keys total including a duplicate across pages 1 and 2
	var page1, page2, page3 []string
	for i := range 400 {
		page1 = append(page1, fmt.Sprintf("key-%04d", i))
	}
	// duplicate key-0000 on page 2
	page2 = append(page2, "key-0000")
	for i := 400; i < 800; i++ {
		page2 = append(page2, fmt.Sprintf("key-%04d", i))
	}
	for i := 800; i < 1200; i++ {
		page3 = append(page3, fmt.Sprintf("key-%04d", i))
	}

	mock.mu.Lock()
	mock.scanPages = [][]string{page1, page2, page3}
	mock.mu.Unlock()

	m, inst := setupMockRedisInstance(t, mock)

	backupPath := filepath.Join(t.TempDir(), "backup.json")
	ctx := context.Background()

	if err := m.redisBackup(ctx, inst, "0", backupPath); err != nil {
		t.Fatalf("redisBackup failed: %v", err)
	}

	mock.mu.Lock()
	keysCalls := mock.keysCalls
	mock.mu.Unlock()

	if keysCalls > 0 {
		t.Fatalf("expected 0 KEYS calls, got %d", keysCalls)
	}

	data, err := os.ReadFile(backupPath)
	if err != nil {
		t.Fatal(err)
	}

	var parsed struct {
		Schema  int          `json:"schema"`
		Created int64        `json:"created"`
		Entries []redisEntry `json:"entries"`
	}
	if err := json.Unmarshal(data, &parsed); err != nil {
		t.Fatalf("invalid JSON backup: %v", err)
	}

	if parsed.Schema != 1 {
		t.Fatalf("expected schema 1, got %d", parsed.Schema)
	}
	if parsed.Created <= 0 {
		t.Fatalf("expected created > 0, got %d", parsed.Created)
	}

	// 1,200 unique keys expected
	if len(parsed.Entries) != 1200 {
		t.Fatalf("expected 1,200 entries without duplicates, got %d", len(parsed.Entries))
	}

	seen := make(map[string]bool)
	for _, entry := range parsed.Entries {
		k := string(entry.Key)
		if seen[k] {
			t.Fatalf("duplicate key in backup: %s", k)
		}
		seen[k] = true
	}
}

func TestRedisRestoreBatchesEntriesSkipsExpiredAndRollsBackOnFailure(t *testing.T) {
	mock := startMockRedisServer(t)
	m, inst := setupMockRedisInstance(t, mock)

	// 1. Prepare a backup file with 1,200 entries, including some expired TTLs
	backupPath := filepath.Join(t.TempDir(), "backup.json")
	created := time.Now().Add(-10 * time.Second).UnixMilli() // 10s ago

	var entries []redisEntry
	for i := range 1200 {
		var ttl int64
		if i == 5 {
			// Expired: TTL 5000ms, elapsed 10000ms -> ttl-elapsed < 0
			ttl = 5000
		} else if i%2 == 0 {
			// Persistent
			ttl = -1
		} else {
			// Future TTL: 60000ms -> remaining ~50000ms
			ttl = 60000
		}
		entries = append(entries, redisEntry{
			Key:   []byte(fmt.Sprintf("restore-key-%04d", i)),
			Value: []byte(fmt.Sprintf("val-%04d", i)),
			TTL:   ttl,
		})
	}

	rawBackup, err := json.Marshal(redisBackup{
		Schema:  1,
		Created: created,
		Entries: entries,
	})
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(backupPath, rawBackup, 0600); err != nil {
		t.Fatal(err)
	}

	// 2. Test restore on empty DB
	ctx := context.Background()
	if err := m.redisRestore(ctx, inst, "0", backupPath); err != nil {
		t.Fatalf("redisRestore failed on empty target: %v", err)
	}

	mock.mu.Lock()
	evalCounts := append([]int(nil), mock.evalCounts...)
	db0Count := len(mock.dbs[0])
	_, key5Exists := mock.dbs[0]["restore-key-0005"]
	mock.mu.Unlock()

	// Verify batched in <= 500 keys per call: 1,200 - 1 expired = 1,199 entries -> 500, 500, 299
	if len(evalCounts) != 3 {
		t.Fatalf("expected 3 eval batches, got %d (%v)", len(evalCounts), evalCounts)
	}
	for _, c := range evalCounts {
		if c > 500 {
			t.Fatalf("expected batch size <= 500, got %d", c)
		}
	}
	if db0Count != 1199 {
		t.Fatalf("expected 1199 restored keys (1 expired skipped), got %d", db0Count)
	}
	if key5Exists {
		t.Fatal("expired key restore-key-0005 should have been skipped")
	}

	// 3. Test non-empty target rejection
	if err := m.redisRestore(ctx, inst, "0", backupPath); err == nil || !strings.Contains(err.Error(), "restore target must be empty") {
		t.Fatalf("expected 'restore target must be empty' error on non-empty DB, got %v", err)
	}

	// 4. Test failure rollback
	// Target DB 1 is empty
	mock.mu.Lock()
	mock.failOnEval = true
	mock.mu.Unlock()

	err = m.redisRestore(ctx, inst, "1", backupPath)
	if err == nil {
		t.Fatal("expected failure when eval fails")
	}

	mock.mu.Lock()
	db1Count := len(mock.dbs[1])
	mock.mu.Unlock()

	if db1Count != 0 {
		t.Fatalf("expected DB 1 to be rolled back to 0 keys via FLUSHDB, got %d", db1Count)
	}
}

package devplatform

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"
)

func mysqlRestoreFixture(t *testing.T) (*Manager, *State, Instance) {
	t.Helper()
	m := testManager(t)
	p := Package{Name: "mysql-restore-fixture", Version: "test", Arch: "universal", Binaries: map[string]string{"mysql": "mysql"}}
	if e := os.MkdirAll(m.packageDir(p), 0700); e != nil {
		t.Fatal(e)
	}
	script := fmt.Sprintf(`#!/usr/bin/python3
import sys,pathlib,hashlib,json,os
root=pathlib.Path(%q)
if not sys.argv[1].startswith('--defaults-file=') or sys.argv[2:]!=['--batch','--skip-column-names']: sys.exit(10)
(root/'child-pid').write_text(str(os.getpid()))
if (root/'wait').exists():
 (root/'waiting').write_text('ready')
 import time
 while True: time.sleep(.05)
h=hashlib.sha256(); n=0; first=b''
while True:
 chunk=sys.stdin.buffer.read(8192)
 if not chunk: break
 if n==0: first=chunk
 h.update(chunk); n+=len(chunk)
 (root/'first-chunk').write_text('consumed')
with (root/'calls').open('a') as f: f.write(json.dumps(dict(size=n,sha=h.hexdigest(),prefix=first.split(b'\n',1)[0].decode()))+'\n')
if (root/'fail').exists(): sys.exit(7)
if (root/'noisy').exists():
 for _ in range(1024): sys.stdout.buffer.write(b'x'*32768);sys.stderr.buffer.write(b'y'*32768)
`, m.Root)
	if e := os.WriteFile(filepath.Join(m.packageDir(p), "mysql"), []byte(script), 0700); e != nil {
		t.Fatal(e)
	}
	s := emptyState()
	s.Packages[p.ID()] = p
	i := Instance{ID: "fixture", Kind: "mysql", Package: p.ID(), Port: 12345}
	return m, &s, i
}
func TestMySQLRestoreRejectsUnreadableSourceBeforeCreate(t *testing.T) {
	for _, kind := range []string{"missing", "directory"} {
		t.Run(kind, func(t *testing.T) {
			m, s, i := mysqlRestoreFixture(t)
			path := filepath.Join(t.TempDir(), "missing")
			if kind == "directory" {
				path = t.TempDir()
			}
			if e := m.otherRestore(context.Background(), s, i, "restored", path); e == nil {
				t.Fatal("accepted invalid input")
			}
			if _, e := os.Stat(filepath.Join(m.Root, "calls")); !errors.Is(e, os.ErrNotExist) {
				t.Fatal("created database before source validation", e)
			}
		})
	}
}

type mysqlRestoreCall struct {
	Size        int64
	SHA, Prefix string
}

func mysqlCalls(t *testing.T, m *Manager) []mysqlRestoreCall {
	t.Helper()
	b, e := os.ReadFile(filepath.Join(m.Root, "calls"))
	if e != nil {
		t.Fatal(e)
	}
	var rows []mysqlRestoreCall
	for _, line := range strings.Split(strings.TrimSpace(string(b)), "\n") {
		var c mysqlRestoreCall
		if e = json.Unmarshal([]byte(line), &c); e != nil {
			t.Fatal(e)
		}
		rows = append(rows, c)
	}
	return rows
}
func TestMySQLRestoreInputAndTarget(t *testing.T) {
	m, s, i := mysqlRestoreFixture(t)
	dump := []byte("CREATE TABLE sample(n int);\nINSERT INTO sample VALUES(42);\n")
	path := filepath.Join(t.TempDir(), "dump.sql")
	if e := os.WriteFile(path, dump, 0600); e != nil {
		t.Fatal(e)
	}
	if e := m.otherRestore(context.Background(), s, i, "quoted`database", path); e != nil {
		t.Fatal(e)
	}
	calls := mysqlCalls(t, m)
	if len(calls) != 2 || calls[0].Prefix != "CREATE DATABASE `quoted``database`" {
		t.Fatal("wrong operation order")
	}
	input := append([]byte("USE `quoted``database`;\n"), dump...)
	sum := sha256.Sum256(input)
	if calls[1].Size != int64(len(input)) || calls[1].SHA != hex.EncodeToString(sum[:]) || calls[1].Prefix != "USE `quoted``database`;" {
		t.Fatal("restore input changed")
	}
}

type gatedDumpReader struct {
	ctx   context.Context
	root  string
	left  int64
	first bool
	max   int
}

func (r *gatedDumpReader) Read(p []byte) (int, error) {
	if len(p) > r.max {
		r.max = len(p)
	}
	if r.left == 0 {
		return 0, io.EOF
	}
	if r.first {
		deadline := time.NewTimer(5 * time.Second)
		defer deadline.Stop()
		tick := time.NewTicker(5 * time.Millisecond)
		defer tick.Stop()
		for {
			if _, e := os.Stat(filepath.Join(r.root, "first-chunk")); e == nil {
				break
			}
			select {
			case <-r.ctx.Done():
				return 0, r.ctx.Err()
			case <-deadline.C:
				return 0, errors.New("child did not consume first chunk")
			case <-tick.C:
			}
		}
	}
	n := min(len(p), 8192, int(r.left))
	for k := range p[:n] {
		p[k] = 'x'
	}
	r.first = true
	r.left -= int64(n)
	return n, nil
}
func waitMySQLFile(t *testing.T, m *Manager, name string) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for time.Now().Before(deadline) {
		if _, e := os.Stat(filepath.Join(m.Root, name)); e == nil {
			return
		}
		time.Sleep(5 * time.Millisecond)
	}
	t.Fatal("child did not reach", name)
}
func TestMySQLRestoreStreamingAndCancellation(t *testing.T) {
	t.Run("streaming-and-output", func(t *testing.T) {
		m, s, i := mysqlRestoreFixture(t)
		if e := os.WriteFile(filepath.Join(m.Root, "noisy"), nil, 0600); e != nil {
			t.Fatal(e)
		}
		ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
		defer cancel()
		const size = 32 << 20
		r := &gatedDumpReader{ctx: ctx, root: m.Root, left: size}
		if e := m.mysqlRestoreInput(ctx, s, i, "target", r); e != nil {
			t.Fatal(e)
		}
		rows := mysqlCalls(t, m)
		h := sha256.New()
		io.WriteString(h, "USE `target`;\n")
		if _, e := io.CopyN(h, strings.NewReader(strings.Repeat("x", 8192)), 8192); e != nil {
			t.Fatal(e)
		}
		// Hash the remaining data in fixed-size blocks, never construct the dump.
		block := []byte(strings.Repeat("x", 8192))
		for n := 8192; n < size; n += len(block) {
			h.Write(block)
		}
		if len(rows) != 1 || rows[0].Size != size+int64(len("USE `target`;\n")) || rows[0].SHA != hex.EncodeToString(h.Sum(nil)) || r.max > 64<<10 || r.left != 0 {
			t.Fatal("not streamed completely with bounded reads")
		}
	})
	t.Run("cancel-child", func(t *testing.T) {
		m, s, i := mysqlRestoreFixture(t)
		if e := os.WriteFile(filepath.Join(m.Root, "wait"), nil, 0600); e != nil {
			t.Fatal(e)
		}
		ctx, cancel := context.WithCancel(context.Background())
		defer cancel()
		done := make(chan error, 1)
		go func() { done <- m.mysqlRestoreInput(ctx, s, i, "target", strings.NewReader("fixture")) }()
		waitMySQLFile(t, m, "waiting")
		cancel()
		select {
		case e := <-done:
			if e == nil {
				t.Fatal("cancel succeeded")
			}
		case <-time.After(5 * time.Second):
			t.Fatal("child not reaped")
		}
		pidBytes, e := os.ReadFile(filepath.Join(m.Root, "child-pid"))
		if e != nil {
			t.Fatal(e)
		}
		pid, e := strconv.Atoi(string(pidBytes))
		if e != nil {
			t.Fatal(e)
		}
		if e = syscall.Kill(pid, 0); !errors.Is(e, syscall.ESRCH) {
			t.Fatal("cancelled child still exists", e)
		}
	})
	t.Run("already-cancelled", func(t *testing.T) {
		m, s, i := mysqlRestoreFixture(t)
		ctx, cancel := context.WithCancel(context.Background())
		cancel()
		if e := m.mysqlRestoreInput(ctx, s, i, "target", strings.NewReader("fixture")); e == nil {
			t.Fatal("cancelled command succeeded")
		}
	})
}

type brokenDumpReader struct{}

func (brokenDumpReader) Read([]byte) (int, error) { return 0, errors.New("fixture source failure") }
func TestMySQLRestoreFailures(t *testing.T) {
	for _, failure := range []string{"reader", "exit"} {
		t.Run(failure, func(t *testing.T) {
			m, s, i := mysqlRestoreFixture(t)
			var r io.Reader = strings.NewReader("fixture")
			if failure == "reader" {
				r = brokenDumpReader{}
			} else if e := os.WriteFile(filepath.Join(m.Root, "fail"), nil, 0600); e != nil {
				t.Fatal(e)
			}
			if e := m.mysqlRestoreInput(context.Background(), s, i, "target", r); e == nil {
				t.Fatal("restore failure hidden")
			}
		})
	}
}

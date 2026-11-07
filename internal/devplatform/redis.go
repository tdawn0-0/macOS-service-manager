package devplatform

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"
)

func redisDB(name string) (int, error) {
	n, e := strconv.Atoi(name)
	if e != nil || n < 0 || n > 15 {
		return 0, errors.New("Redis database number must be between 0 and 15")
	}
	return n, nil
}
func redisWrite(w io.Writer, args [][]byte) error {
	if _, e := fmt.Fprintf(w, "*%d\r\n", len(args)); e != nil {
		return e
	}
	for _, arg := range args {
		if _, e := fmt.Fprintf(w, "$%d\r\n", len(arg)); e != nil {
			return e
		}
		if _, e := w.Write(arg); e != nil {
			return e
		}
		if _, e := io.WriteString(w, "\r\n"); e != nil {
			return e
		}
	}
	return nil
}
func redisRead(r *bufio.Reader, depth int) (any, error) {
	if depth > 8 {
		return nil, errors.New("Redis response nesting limit")
	}
	line, e := r.ReadString('\n')
	if e != nil {
		return nil, e
	}
	if len(line) < 3 || !strings.HasSuffix(line, "\r\n") {
		return nil, errors.New("invalid Redis response")
	}
	text := line[1 : len(line)-2]
	switch line[0] {
	case '+':
		return text, nil
	case '-':
		return nil, errors.New(text)
	case ':':
		return strconv.ParseInt(text, 10, 64)
	case '$':
		n, e := strconv.Atoi(text)
		if e != nil || n < -1 || n > 256<<20 {
			return nil, errors.New("invalid Redis bulk length")
		}
		if n == -1 {
			return nil, nil
		}
		b := make([]byte, n+2)
		if _, e = io.ReadFull(r, b); e != nil {
			return nil, e
		}
		if string(b[n:]) != "\r\n" {
			return nil, errors.New("invalid Redis bulk terminator")
		}
		return b[:n], nil
	case '*':
		n, e := strconv.Atoi(text)
		if e != nil || n < -1 || n > 300000 {
			return nil, errors.New("invalid Redis array length")
		}
		if n == -1 {
			return nil, nil
		}
		out := make([]any, n)
		for k := range out {
			out[k], e = redisRead(r, depth+1)
			if e != nil {
				return nil, e
			}
		}
		return out, nil
	}
	return nil, errors.New("unknown Redis response")
}
func (m *Manager) redisCommand(ctx context.Context, i Instance, db int, args ...[]byte) (any, error) {
	password, e := os.ReadFile(filepath.Join(m.Root, "instances", i.ID, "password"))
	if e != nil {
		return nil, e
	}
	conn, e := (&net.Dialer{Timeout: 3 * time.Second}).DialContext(ctx, "tcp", fmt.Sprintf("127.0.0.1:%d", i.Port))
	if e != nil {
		return nil, e
	}
	defer conn.Close()
	deadline := time.Now().Add(30 * time.Second)
	if d, ok := ctx.Deadline(); ok && d.Before(deadline) {
		deadline = d
	}
	conn.SetDeadline(deadline)
	stop := context.AfterFunc(ctx, func() { conn.Close() })
	defer stop()
	r := bufio.NewReader(io.LimitReader(conn, 512<<20))
	command := func(a ...[]byte) (any, error) {
		if e := redisWrite(conn, a); e != nil {
			return nil, e
		}
		return redisRead(r, 0)
	}
	if _, e = command([]byte("AUTH"), password); e != nil {
		return nil, errors.New("Redis authentication failed")
	}
	if _, e = command([]byte("SELECT"), []byte(strconv.Itoa(db))); e != nil {
		return nil, e
	}
	return command(args...)
}

type redisEntry struct {
	Key   []byte `json:"key"`
	Value []byte `json:"value"`
	TTL   int64  `json:"ttl"`
}
type redisBackup struct {
	Schema  int          `json:"schema"`
	Created int64        `json:"created"`
	Entries []redisEntry `json:"entries"`
}

const redisSnapshotScript = `local out={}; for _,key in ipairs(redis.call('KEYS','*')) do local value=redis.call('DUMP',key); if value then table.insert(out,key); table.insert(out,value); table.insert(out,redis.call('PTTL',key)); end; end; return out`

func (m *Manager) redisBackup(ctx context.Context, i Instance, name, path string) error {
	db, e := redisDB(name)
	if e != nil {
		return e
	}
	response, e := m.redisCommand(ctx, i, db, []byte("EVAL"), []byte(redisSnapshotScript), []byte("0"))
	if e != nil {
		return e
	}
	rows, ok := response.([]any)
	if !ok || len(rows)%3 != 0 {
		return errors.New("invalid Redis snapshot")
	}
	backup := redisBackup{Schema: 1, Created: time.Now().UnixMilli()}
	for n := 0; n < len(rows); n += 3 {
		key, kok := rows[n].([]byte)
		value, vok := rows[n+1].([]byte)
		ttl, tok := rows[n+2].(int64)
		if !kok || !vok || !tok {
			return errors.New("invalid Redis snapshot entry")
		}
		backup.Entries = append(backup.Entries, redisEntry{key, value, ttl})
	}
	return writeJSON(path, backup)
}

const redisRestoreScript = `if redis.call('DBSIZE')~=0 then return redis.error_reply('restore target must be empty') end; local restored={}; for i=1,#ARGV,3 do local result=redis.pcall('RESTORE',ARGV[i],ARGV[i+1],ARGV[i+2]); if type(result)=='table' and result.err then for _,key in ipairs(restored) do redis.call('DEL',key) end; return redis.error_reply(result.err) end; table.insert(restored,ARGV[i]); end; return #restored`

func (m *Manager) redisRestore(ctx context.Context, i Instance, name, path string) error {
	db, e := redisDB(name)
	if e != nil {
		return e
	}
	f, e := os.Open(path)
	if e != nil {
		return e
	}
	defer f.Close()
	var backup redisBackup
	if e = json.NewDecoder(io.LimitReader(f, 512<<20)).Decode(&backup); e != nil {
		return e
	}
	if backup.Schema != 1 || backup.Created <= 0 || len(backup.Entries) > 100000 {
		return errors.New("unsupported Redis backup")
	}
	elapsed := time.Now().UnixMilli() - backup.Created
	if elapsed < 0 {
		elapsed = 0
	}
	args := [][]byte{[]byte("EVAL"), []byte(redisRestoreScript), []byte("0")}
	for _, entry := range backup.Entries {
		ttl := int64(0)
		if entry.TTL >= 0 {
			ttl = entry.TTL - elapsed
			if ttl <= 0 {
				continue
			}
		}
		args = append(args, entry.Key, []byte(strconv.FormatInt(ttl, 10)), entry.Value)
	}
	_, e = m.redisCommand(ctx, i, db, args...)
	return e
}

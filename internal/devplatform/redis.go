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
func (m *Manager) withRedisSession(ctx context.Context, i Instance, db int, fn func(call func(args ...[]byte) (any, error)) error) error {
	password, e := os.ReadFile(filepath.Join(m.Root, "instances", i.ID, "password"))
	if e != nil {
		return e
	}
	conn, e := (&net.Dialer{Timeout: 3 * time.Second}).DialContext(ctx, "tcp", fmt.Sprintf("127.0.0.1:%d", i.Port))
	if e != nil {
		return e
	}
	defer conn.Close()
	stop := context.AfterFunc(ctx, func() { conn.Close() })
	defer stop()
	r := bufio.NewReader(conn)
	call := func(args ...[]byte) (any, error) {
		if err := ctx.Err(); err != nil {
			return nil, err
		}
		deadline := time.Now().Add(30 * time.Second)
		if d, ok := ctx.Deadline(); ok && d.Before(deadline) {
			deadline = d
		}
		conn.SetDeadline(deadline)
		if e := redisWrite(conn, args); e != nil {
			return nil, e
		}
		return redisRead(r, 0)
	}
	if _, e = call([]byte("AUTH"), password); e != nil {
		return errors.New("Redis authentication failed")
	}
	if _, e = call([]byte("SELECT"), []byte(strconv.Itoa(db))); e != nil {
		return e
	}
	return fn(call)
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

const redisDumpBatchScript = `local out={}
for i=1,#KEYS do
  local value=redis.call('DUMP',KEYS[i])
  if value then
    table.insert(out,KEYS[i])
    table.insert(out,value)
    table.insert(out,redis.call('PTTL',KEYS[i]))
  end
end
return out`

func (m *Manager) redisBackup(ctx context.Context, i Instance, name, path string) error {
	db, e := redisDB(name)
	if e != nil {
		return e
	}
	f, e := os.OpenFile(path, os.O_WRONLY|os.O_CREATE|os.O_TRUNC, 0600)
	if e != nil {
		return e
	}
	defer f.Close()
	w := bufio.NewWriter(f)
	defer w.Flush()

	header := fmt.Sprintf(`{"schema":1,"created":%d,"entries":[`, time.Now().UnixMilli())
	if _, e = io.WriteString(w, header); e != nil {
		return e
	}

	seen := make(map[string]struct{})
	first := true

	sessionErr := m.withRedisSession(ctx, i, db, func(call func(args ...[]byte) (any, error)) error {
		cursor := []byte("0")
		for {
			res, err := call([]byte("SCAN"), cursor, []byte("COUNT"), []byte("500"))
			if err != nil {
				return err
			}
			scanParts, ok := res.([]any)
			if !ok || len(scanParts) != 2 {
				return errors.New("invalid SCAN response")
			}
			nextCursorBytes, ok := scanParts[0].([]byte)
			if !ok {
				return errors.New("invalid SCAN cursor")
			}
			cursor = nextCursorBytes

			rawKeys, ok := scanParts[1].([]any)
			if !ok {
				return errors.New("invalid SCAN keys array")
			}

			var batch [][]byte
			for _, rk := range rawKeys {
				kb, ok := rk.([]byte)
				if !ok {
					continue
				}
				ks := string(kb)
				if _, exists := seen[ks]; exists {
					continue
				}
				seen[ks] = struct{}{}
				batch = append(batch, kb)
			}

			if len(batch) > 0 {
				evalArgs := make([][]byte, 0, 3+len(batch))
				evalArgs = append(evalArgs, []byte("EVAL"), []byte(redisDumpBatchScript), []byte(strconv.Itoa(len(batch))))
				evalArgs = append(evalArgs, batch...)

				dumpRes, err := call(evalArgs...)
				if err != nil {
					return err
				}
				triples, ok := dumpRes.([]any)
				if !ok || len(triples)%3 != 0 {
					return errors.New("invalid DUMP batch response")
				}

				for n := 0; n < len(triples); n += 3 {
					key, kok := triples[n].([]byte)
					val, vok := triples[n+1].([]byte)
					ttl, tok := triples[n+2].(int64)
					if !kok || !vok || !tok {
						return errors.New("invalid DUMP batch entry")
					}
					entryData, err := json.Marshal(redisEntry{Key: key, Value: val, TTL: ttl})
					if err != nil {
						return err
					}
					if !first {
						if _, err := w.WriteString(","); err != nil {
							return err
						}
					}
					first = false
					if _, err := w.Write(entryData); err != nil {
						return err
					}
				}
			}

			if string(cursor) == "0" {
				break
			}
		}
		return nil
	})

	if sessionErr != nil {
		return sessionErr
	}

	if _, e = io.WriteString(w, "]}\n"); e != nil {
		return e
	}
	return w.Flush()
}

const redisRestoreBatchScript = `local restored={}
for i=1,#ARGV,3 do
  local result=redis.pcall('RESTORE',ARGV[i],ARGV[i+1],ARGV[i+2])
  if type(result)=='table' and result.err then
    return redis.error_reply(result.err)
  end
  table.insert(restored,ARGV[i])
end
return #restored`

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

	dec := json.NewDecoder(f)
	t, e := dec.Token()
	if e != nil || t != json.Delim('{') {
		return errors.New("unsupported Redis backup")
	}

	var schema int
	var created int64
	var entriesFound bool

	sessionErr := m.withRedisSession(ctx, i, db, func(call func(args ...[]byte) (any, error)) error {
		dbsize, err := call([]byte("DBSIZE"))
		if err != nil {
			return err
		}
		size, ok := dbsize.(int64)
		if !ok || size != 0 {
			return errors.New("restore target must be empty")
		}

		rollback := func(origErr error) error {
			_, _ = call([]byte("FLUSHDB"))
			return origErr
		}

		for dec.More() {
			token, err := dec.Token()
			if err != nil {
				return rollback(err)
			}
			prop, ok := token.(string)
			if !ok {
				return rollback(errors.New("unsupported Redis backup"))
			}
			switch prop {
			case "schema":
				if err := dec.Decode(&schema); err != nil {
					return rollback(err)
				}
			case "created":
				if err := dec.Decode(&created); err != nil {
					return rollback(err)
				}
			case "entries":
				entriesFound = true
				if schema != 1 || created <= 0 {
					return rollback(errors.New("unsupported Redis backup"))
				}
				elapsed := time.Now().UnixMilli() - created
				if elapsed < 0 {
					elapsed = 0
				}
				arrToken, err := dec.Token()
				if err != nil || arrToken != json.Delim('[') {
					return rollback(errors.New("unsupported Redis backup"))
				}
				var batchArgs [][]byte
				for dec.More() {
					var entry redisEntry
					if err := dec.Decode(&entry); err != nil {
						return rollback(err)
					}
					ttl := int64(0)
					if entry.TTL >= 0 {
						ttl = entry.TTL - elapsed
						if ttl <= 0 {
							continue
						}
					}
					batchArgs = append(batchArgs, entry.Key, []byte(strconv.FormatInt(ttl, 10)), entry.Value)
					if len(batchArgs) >= 500*3 {
						evalArgs := make([][]byte, 0, 3+len(batchArgs))
						evalArgs = append(evalArgs, []byte("EVAL"), []byte(redisRestoreBatchScript), []byte("0"))
						evalArgs = append(evalArgs, batchArgs...)
						if _, err := call(evalArgs...); err != nil {
							return rollback(err)
						}
						batchArgs = batchArgs[:0]
					}
				}
				closeToken, err := dec.Token()
				if err != nil || closeToken != json.Delim(']') {
					return rollback(errors.New("unsupported Redis backup"))
				}
				if len(batchArgs) > 0 {
					evalArgs := make([][]byte, 0, 3+len(batchArgs))
					evalArgs = append(evalArgs, []byte("EVAL"), []byte(redisRestoreBatchScript), []byte("0"))
					evalArgs = append(evalArgs, batchArgs...)
					if _, err := call(evalArgs...); err != nil {
						return rollback(err)
					}
				}
			default:
				var dummy any
				if err := dec.Decode(&dummy); err != nil {
					return rollback(err)
				}
			}
		}

		closeDelim, err := dec.Token()
		if err != nil || closeDelim != json.Delim('}') {
			return rollback(errors.New("unsupported Redis backup"))
		}
		if !entriesFound || schema != 1 || created <= 0 {
			return rollback(errors.New("unsupported Redis backup"))
		}
		return nil
	})

	return sessionErr
}

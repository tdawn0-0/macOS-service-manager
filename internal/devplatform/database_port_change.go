package devplatform

import (
	"context"
	"errors"
	"fmt"
	"path/filepath"
	"strconv"
	"strings"
)

func replaceManagedRedisPort(config []byte, from, to int) ([]byte, error) {
	lines := strings.Split(string(config), "\n")
	count := 0
	for n, line := range lines {
		fields := strings.Fields(line)
		if len(fields) == 0 || fields[0] != "port" {
			continue
		}
		count++
		if len(fields) != 2 || fields[1] != strconv.Itoa(from) {
			return nil, errors.New("managed Redis port configuration changed; the existing file was kept")
		}
		lines[n] = fmt.Sprintf("port %d", to)
	}
	if count != 1 {
		return nil, errors.New("managed Redis configuration needs exactly one matching port directive")
	}
	return []byte(strings.Join(lines, "\n")), nil
}

func replaceManagedMySQLPort(config []byte, from, to int) ([]byte, error) {
	lines := strings.Split(string(config), "\n")
	client := false
	count := 0
	for n, line := range lines {
		trimmed := strings.TrimSpace(line)
		if strings.HasPrefix(trimmed, "[") {
			client = trimmed == "[client]"
			continue
		}
		key, value, ok := strings.Cut(trimmed, "=")
		if !ok || strings.TrimSpace(key) != "port" {
			continue
		}
		if !client || strings.TrimSpace(value) != strconv.Itoa(from) {
			return nil, errors.New("managed MySQL client port configuration changed; the existing file was kept")
		}
		count++
		lines[n] = fmt.Sprintf("port=%d", to)
	}
	if count != 1 {
		return nil, errors.New("managed MySQL client configuration needs exactly one matching port")
	}
	return []byte(strings.Join(lines, "\n")), nil
}

// Reconfigure a stopped instance without recreating data or credentials. The
// expected record protects against another GUI/CLI changing it before saving.
func (m *Manager) ChangeInstancePort(ctx context.Context, expected Instance, port int) error {
	return m.changeInstancePort(ctx, expected, port, false)
}

// Repair converges configuration on the saved state after an interrupted change.
// It never changes the instance's saved port, credentials or data.
func (m *Manager) RepairInstancePort(ctx context.Context, expected Instance) error {
	return m.changeInstancePort(ctx, expected, expected.Port, true)
}
func (m *Manager) changeInstancePort(ctx context.Context, expected Instance, port int, repair bool) error {
	if err := validID(expected.ID); err != nil {
		return err
	}
	if port < 1024 || port > 65535 {
		return errors.New("database port must be 1024–65535")
	}
	unlock, lockErr := m.resourceLeases(ctx, resourceKey{"database instance", expected.ID}, resourceKey{"TCP port", strconv.Itoa(port)})
	if lockErr != nil {
		return lockErr
	}
	defer unlock()
	return m.withStateTransaction(ctx, func(s *State) (func() error, func(), error) {
		i, ok := s.Instances[expected.ID]
		if !ok {
			return nil, nil, errors.New("instance not found")
		}
		if e := m.verifyDataLocation(i); e != nil {
			return nil, nil, e
		}
		if i != expected {
			return nil, nil, errors.New("database settings changed; select the instance again before changing its port")
		}
		if i.Kind != "redis" && i.Kind != "postgresql" && i.Kind != "mysql" && i.Kind != "mongodb" {
			return nil, nil, errors.New("unsupported database engine")
		}
		if port == i.Port && !repair {
			return nil, nil, nil
		}
		if err := m.verifyShutdownTarget(ctx, domain()+"/"+m.label("db-"+i.ID)); err != nil {
			return nil, nil, fmt.Errorf("stop the database before changing its port: %w", err)
		}
		for _, other := range s.Instances {
			if other.ID != i.ID && other.Port == port {
				return nil, nil, errors.New("another configured database already uses this port")
			}
		}
		if !repair {
			if err := freeDatabasePort(port); err != nil {
				return nil, nil, err
			}
		}
		type fileUpdate struct {
			path          string
			before, after []byte
		}
		var updates []fileUpdate
		if i.Kind == "redis" || i.Kind == "mysql" {
			dir := filepath.Join(m.Root, "instances", i.ID)
			name := "valkey.conf"
			replace := replaceManagedRedisPort
			if i.Kind == "mysql" {
				name = "client.cnf"
				replace = replaceManagedMySQLPort
			}
			previous, err := readRegularFile(dir, name, 1<<20)
			if err != nil {
				return nil, nil, err
			}
			from := i.Port
			if repair {
				from, err = managedConfigPort(previous, i.Kind)
				if err != nil {
					return nil, nil, err
				}
			}
			updated, err := replace(previous, from, port)
			if err != nil {
				return nil, nil, err
			}
			path := filepath.Join(dir, name)
			updates = append(updates, fileUpdate{path, previous, updated})
		}
		applied := 0
		undo := func() error {
			var result error
			for n := applied - 1; n >= 0; n-- {
				result = errors.Join(result, atomicWrite(updates[n].path, updates[n].before, 0600))
			}
			return result
		}
		for _, update := range updates {
			if err := ctx.Err(); err != nil {
				return undo, nil, err
			}
			if err := atomicWrite(update.path, update.after, 0600); err != nil {
				return undo, nil, err
			}
			applied++
			m.emit("Saved database port settings: " + filepath.Base(update.path))
		}

		i.Port = port
		s.Instances[i.ID] = i
		return undo, nil, nil
	})
}

func managedConfigPort(config []byte, kind string) (int, error) {
	client := kind != "mysql"
	port, count := 0, 0
	for _, line := range strings.Split(string(config), "\n") {
		line = strings.TrimSpace(line)
		if kind == "mysql" && strings.HasPrefix(line, "[") {
			client = line == "[client]"
			continue
		}
		var value string
		if kind == "mysql" {
			key, tail, ok := strings.Cut(line, "=")
			if !ok || strings.TrimSpace(key) != "port" {
				continue
			}
			if !client {
				return 0, errors.New("managed client configuration contains another port directive")
			}
			value = strings.TrimSpace(tail)
		} else {
			fields := strings.Fields(line)
			if len(fields) == 0 || fields[0] != "port" {
				continue
			}
			if len(fields) != 2 {
				return 0, errors.New("invalid managed port directive")
			}
			value = fields[1]
		}
		n, err := strconv.Atoi(value)
		if err != nil || n < 1024 || n > 65535 {
			return 0, errors.New("invalid managed database port")
		}
		port = n
		count++
	}
	if count != 1 {
		return 0, errors.New("managed configuration needs exactly one database port")
	}
	return port, nil
}

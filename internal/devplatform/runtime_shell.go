package devplatform

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

func (m *Manager) zshStartupFiles() ([]string, error) {
	home, err := m.home()
	if err != nil {
		return nil, err
	}
	// An explicit fixture home must never escape to the real user's ZDOTDIR.
	if dir := os.Getenv("ZDOTDIR"); m.Home == "" && dir != "" {
		if !filepath.IsAbs(dir) {
			return nil, errors.New("ZDOTDIR must be an absolute path")
		}
		home = dir
	}
	// zshenv covers -c; zprofile restores PATH after macOS path_helper for
	// -lc; zshrc keeps interactive startup and existing integrations working.
	return []string{filepath.Join(home, ".zshenv"), filepath.Join(home, ".zprofile"), filepath.Join(home, ".zshrc")}, nil
}

func updateShellBlock(data []byte, begin, end, block string, enable bool) ([]byte, error) {
	b := bytes.Clone(data)
	start, stop := bytes.Index(b, []byte(begin)), bytes.Index(b, []byte(end))
	if bytes.Count(b, []byte(begin)) > 1 || bytes.Count(b, []byte(end)) > 1 {
		return nil, errors.New("duplicate runtime shell blocks")
	}
	if (start < 0) != (stop < 0) || stop >= 0 && stop < start {
		return nil, errors.New("damaged runtime shell block")
	}
	if start >= 0 {
		finish := stop + len(end)
		if start > 0 && b[start-1] != '\n' || finish < len(b) && b[finish] != '\n' {
			return nil, errors.New("runtime shell markers must be on separate lines")
		}
		if finish < len(b) {
			finish++
		}
		if enable {
			// Upgrade in place, preserving the user's ordering and surrounding text.
			return append(append(b[:start:start], block...), b[finish:]...), nil
		}
		if begin == nodeBlockBegin && start > 0 && b[start-1] == '\n' && (start == 1 || b[start-2] == '\n') {
			start--
		}
		return append(b[:start:start], b[finish:]...), nil
	}
	if !enable {
		return b, nil
	}
	if len(b) > 0 && b[len(b)-1] != '\n' {
		b = append(b, '\n')
	}
	if begin == nodeBlockBegin && len(b) > 0 {
		b = append(b, '\n')
	}
	return append(b, block...), nil
}

func (m *Manager) configureZshShell(ctx context.Context, begin, end, block string, enable bool) (string, error) {
	paths, err := m.zshStartupFiles()
	if err != nil {
		return "", err
	}
	label := strings.Join(paths, ", ")
	type change struct {
		config nodeShellConfiguration
		data   []byte
	}
	var changes []change
	seen := map[string]bool{}
	// Validate every file before changing any; preserve symlinks and permissions.
	for _, path := range paths {
		if err := ctx.Err(); err != nil {
			return label, err
		}
		config, err := readNodeShellConfiguration(path)
		if err != nil {
			return label, fmt.Errorf("%s: %w", path, err)
		}
		if seen[config.target] {
			continue
		}
		seen[config.target] = true
		data, err := updateShellBlock(config.data, begin, end, block, enable)
		if err != nil {
			return label, fmt.Errorf("%s: %w; review this file", path, err)
		}
		if !bytes.Equal(config.data, data) {
			changes = append(changes, change{config, data})
		}
	}
	for i, change := range changes {
		err := ctx.Err()
		if err == nil {
			err = change.config.commit(change.data)
		}
		if err == nil {
			continue
		}
		// Roll back earlier writes only while their content and targets still match.
		for j := i - 1; j >= 0; j-- {
			old := changes[j]
			current, rollbackErr := readNodeShellConfiguration(old.config.rc)
			if rollbackErr == nil {
				if current.target != old.config.target || !bytes.Equal(current.data, old.data) {
					rollbackErr = errors.New("configuration changed during rollback")
				} else {
					rollbackErr = current.commit(old.config.data)
				}
			}
			if rollbackErr != nil {
				err = errors.Join(err, fmt.Errorf("restore %s: %w", old.config.rc, rollbackErr))
			}
		}
		return label, err
	}
	return label, nil
}

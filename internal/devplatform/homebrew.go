package devplatform

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"regexp"
	"strings"
)

const homebrewProgram = "/opt/homebrew/bin/brew"

type commandRunner func(context.Context, string, ...string) (string, error)

type homebrewFormula struct {
	Name     string `json:"name"`
	FullName string `json:"full_name"`
	License  string `json:"license"`
	Revision int    `json:"revision"`
	Versions struct {
		Stable string `json:"stable"`
	} `json:"versions"`
	SourceChecksum struct {
		SHA256 string `json:"sha256"`
	} `json:"ruby_source_checksum"`
}

var postgresBrewVersion = regexp.MustCompile(`^(\d+)(?:\.\d+)?(?:_\d+)?$`)

var redisBrewVersion = regexp.MustCompile(`^8\.2(?:\.\d+)?(?:_\d+)?$`)

func homebrewFormulaFor(name, version string) (string, error) {
	version = strings.TrimPrefix(version, "v")
	switch name {
	case "postgresql":
		if match := postgresBrewVersion.FindStringSubmatch(version); match != nil {
			return "postgresql@" + match[1], nil
		}
		return "", errors.New("choose a PostgreSQL major version (for example 17) or an exact Homebrew version")
	case "redis":
		if redisBrewVersion.MatchString(version) {
			return "redis@8.2", nil
		}
		return "", errors.New("choose Redis 8.2 or an exact 8.2.x Homebrew version")
	}
	return "", errors.New("Homebrew packages are only supported for PostgreSQL and Redis")
}

func homebrewBinaries(name string) map[string]string {
	names := []string{"postgres", "initdb", "pg_dump", "pg_restore", "psql", "createdb"}
	if name == "redis" {
		names = []string{"redis-server", "redis-cli"}
	}
	bins := map[string]string{}
	for _, bin := range names {
		bins[bin] = "bin/" + bin
	}
	return bins
}

var redisExactVersion = regexp.MustCompile(`^8\.2\.\d+$`)

func validHomebrewPackage(p Package) bool {
	formula, e := homebrewFormulaFor(p.Name, p.Version)
	if e != nil || p.Arch != "arm64" || p.Strip != 0 {
		return false
	}
	if p.Formula != formula {
		return false
	}
	// Persist exact keg versions, never a moving major/minor channel or opt link.
	base := strings.Split(p.Version, "_")[0]
	switch p.Name {
	case "postgresql":
		if strings.Count(base, ".") != 1 {
			return false
		}
	case "redis":
		if !redisExactVersion.MatchString(base) {
			return false
		}
	default:
		return false
	}
	return reflect.DeepEqual(p.Binaries, homebrewBinaries(p.Name))
}

func brewRun(ctx context.Context, run commandRunner, args ...string) (string, error) {
	// Avoid implicit updates and cleanup of other keg versions used by projects.
	env := []string{"HOMEBREW_NO_AUTO_UPDATE=1", "HOMEBREW_NO_INSTALL_CLEANUP=1", "HOMEBREW_NO_INSTALLED_DEPENDENTS_CHECK=1", homebrewProgram}
	out, err := run(ctx, "/usr/bin/env", append(env, args...)...)
	if err != nil {
		return "", fmt.Errorf("Homebrew %s: %w. Homebrew packages require Apple Silicon Homebrew at %s", args[0], err, homebrewProgram)
	}
	return out, nil
}

func resolveHomebrew(ctx context.Context, name, requested string, run commandRunner) (Package, error) {
	formula, err := homebrewFormulaFor(name, requested)
	if err != nil {
		return Package{}, err
	}
	out, err := brewRun(ctx, run, "info", "--json=v2", "--formula", formula)
	if err != nil {
		return Package{}, err
	}
	var result struct {
		Formulae []homebrewFormula `json:"formulae"`
	}
	if err = json.Unmarshal([]byte(out), &result); err != nil || len(result.Formulae) != 1 {
		return Package{}, errors.New("invalid Homebrew formula metadata")
	}
	f := result.Formulae[0]
	if f.FullName != formula || f.Name != filepath.Base(f.FullName) || f.Revision < 0 {
		return Package{}, errors.New("Homebrew returned a different formula or tap")
	}
	version := f.Versions.Stable
	if f.Revision > 0 {
		version += fmt.Sprintf("_%d", f.Revision)
	}
	requested = strings.TrimPrefix(requested, "v")
	channel := strings.TrimPrefix(strings.TrimPrefix(formula, "postgresql@"), "redis@")
	if requested != channel && requested != version && requested != f.Versions.Stable {
		return Package{}, fmt.Errorf("Homebrew currently provides %s %s, not %s; review the available version", formula, version, requested)
	}
	if !strings.HasPrefix(f.Versions.Stable, channel+".") {
		return Package{}, errors.New("Homebrew formula does not match the requested release series")
	}
	p := Package{Name: name, Version: version, Arch: "arm64", Format: "homebrew", Formula: f.FullName, SHA256: f.SourceChecksum.SHA256, License: f.License, Binaries: homebrewBinaries(name)}
	p.URL = "https://formulae.brew.sh/formula/" + formula
	return p, p.Validate()
}

// ResolvePackage uses the same runner as installation, including GUI/CLI test
// fixtures. Preview only reads metadata; adding a tap is a separate user action.
func (m *Manager) ResolvePackage(ctx context.Context, name, version string) (Package, error) {
	if name == "postgresql" || name == "redis" {
		return resolveHomebrew(ctx, name, version, m.Run)
	}
	return Resolve(ctx, name, version)
}

func (m *Manager) checkHomebrewReceipt(p Package) error {
	var receipt struct {
		Source struct {
			Tap      string `json:"tap"`
			Versions struct {
				Stable string `json:"stable"`
			} `json:"versions"`
		} `json:"source"`
	}
	b, err := os.ReadFile(filepath.Join(m.packageDir(p), "INSTALL_RECEIPT.json"))
	if err != nil {
		return fmt.Errorf("Homebrew keg %s is missing; install it again in Software: %w", p.ID(), err)
	}
	if json.Unmarshal(b, &receipt) != nil {
		return errors.New("invalid Homebrew installation receipt")
	}
	tap := "homebrew/core"
	if receipt.Source.Tap != tap || receipt.Source.Versions.Stable != strings.Split(p.Version, "_")[0] {
		return fmt.Errorf("Homebrew keg does not match %s %s; existing software from a different tap will not be adopted", p.Formula, p.Version)
	}
	return nil
}

func (m *Manager) installHomebrew(ctx context.Context, p Package) error {
	unlock, err := m.packageLease(ctx, p.ID(), true)
	if err != nil {
		return err
	}
	defer unlock()
	state, err := m.Read()
	if err != nil {
		return err
	}
	registered, exists := state.Packages[p.ID()]
	if exists && !reflect.DeepEqual(registered, p) {
		return errors.New("installed package source differs; review the existing version")
	}
	if !exists {
		current, err := m.ResolvePackage(ctx, p.Name, p.Version)
		if err != nil {
			return err
		}
		if !reflect.DeepEqual(current, p) {
			return errors.New("Homebrew formula changed since preview; review the package again")
		}
	}
	if err = m.checkHomebrewReceipt(p); err != nil {
		// Never replace a conflicting tap's installation or an existing keg.
		if _, statErr := os.Lstat(m.packageDir(p)); !errors.Is(statErr, os.ErrNotExist) {
			return err
		}
		current, resolveErr := m.ResolvePackage(ctx, p.Name, p.Version)
		if resolveErr != nil {
			return resolveErr
		}
		if !reflect.DeepEqual(current, p) {
			return errors.New("Homebrew formula changed since preview; review the package again")
		}
		var releases []func()
		defer func() {
			for _, release := range releases {
				release()
			}
		}()
		for _, installed := range state.Packages {
			if installed.Formula == p.Formula {
				if installed.ID() != p.ID() {
					release, e := m.packageLease(ctx, installed.ID(), true)
					if e != nil {
						return e
					}
					releases = append(releases, release)
				}
				if err = m.requirePackageIdle(ctx, &state, installed); err != nil {
					return err
				}
			}
		}
		m.emit("Installing with Homebrew: " + p.Formula)
		if _, err = brewRun(ctx, m.Run, "install", "--formula", "--force-bottle", p.Formula); err != nil {
			return err
		}
	}
	state.Packages[p.ID()] = p
	for name := range p.Binaries {
		if _, err = m.Binary(&state, p.ID(), name); err != nil {
			return err
		}
	}
	return m.withState(ctx, func(s *State) error {
		if current, exists := s.Packages[p.ID()]; exists && !reflect.DeepEqual(current, p) {
			return errors.New("package registration changed during Homebrew installation")
		}
		s.Packages[p.ID()] = p
		m.emit("Registered Homebrew keg " + p.ID())
		return nil
	})
}

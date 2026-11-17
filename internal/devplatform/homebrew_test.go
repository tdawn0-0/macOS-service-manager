package devplatform

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

func brewMetadata() string {
	f := homebrewFormula{SourceChecksum: struct {
		SHA256 string `json:"sha256"`
	}{strings.Repeat("a", 64)}, License: "test license"}
	f.Name, f.FullName, f.Versions.Stable, f.Revision = "postgresql@17", "postgresql@17", "17.11", 1
	b, _ := json.Marshal(map[string]any{"formulae": []homebrewFormula{f}})
	return string(b)
}

func brewFixture(t *testing.T) (*Manager, Package, *[][]string) {
	t.Helper()
	m := testManager(t)
	m.brewCellar = t.TempDir()
	calls := [][]string{}
	m.Run = func(ctx context.Context, program string, args ...string) (string, error) {
		if err := ctx.Err(); err != nil {
			return "", err
		}
		if program == "launchctl" {
			return "", errors.New("Could not find service")
		}
		if program != "/usr/bin/env" || len(args) < 5 || args[3] != homebrewProgram {
			t.Fatalf("unexpected command %s %v", program, args)
		}
		command := append([]string{}, args[4:]...)
		calls = append(calls, command)
		if command[0] == "info" {
			return brewMetadata(), nil
		}
		if command[0] == "install" {
			p, err := resolveHomebrew(ctx, "postgresql", "17", m.Run)
			if err != nil {
				t.Fatal(err)
			}
			writeBrewKeg(t, m, p, "homebrew/core")
			return "installed", nil
		}
		t.Fatalf("unexpected Homebrew mutation %v", command)
		return "", nil
	}
	p, err := m.ResolvePackage(context.Background(), "postgresql", "17")
	if err != nil {
		t.Fatal(err)
	}
	return m, p, &calls
}

func writeBrewKeg(t *testing.T, m *Manager, p Package, tap string) {
	t.Helper()
	for _, bin := range p.Binaries {
		path := filepath.Join(m.packageDir(p), bin)
		if err := os.MkdirAll(filepath.Dir(path), 0755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte("#!/bin/sh\nexit 0\n"), 0755); err != nil {
			t.Fatal(err)
		}
	}
	receipt := map[string]any{"source": map[string]any{"tap": tap, "versions": map[string]string{"stable": strings.Split(p.Version, "_")[0]}}}
	if err := writeJSON(filepath.Join(m.packageDir(p), "INSTALL_RECEIPT.json"), receipt); err != nil {
		t.Fatal(err)
	}
}

func TestHomebrewInstallSelectsTapAndExactKegWithoutGlobalServiceChanges(t *testing.T) {
	for _, name := range []string{"postgresql"} {
		t.Run(name, func(t *testing.T) {
			m, p, calls := brewFixture(t)
			ctx := context.Background()
			info, err := m.PackageDetails(ctx, p)
			if err != nil || info.Installed || info.Location != m.packageDir(p) || len(*calls) != 1 {
				t.Fatal("preview mutated the installation", info, *calls, err)
			}
			if err = m.Install(ctx, p); err != nil {
				t.Fatal(err)
			}
			s, err := m.Read()
			if err != nil {
				t.Fatal(err)
			}
			binName := "postgres"
			bin, err := m.Binary(&s, p.ID(), binName)
			if err != nil || bin != filepath.Join(m.packageDir(p), p.Binaries[binName]) || strings.Contains(bin, "/opt/") {
				t.Fatal("runtime did not bind to the exact keg", bin, err)
			}
			check, err := m.VerifyPackage(ctx, p.ID())
			if err != nil || check.Status != "Homebrew managed" || check.Files != len(p.Binaries) {
				t.Fatal(check, err)
			}
			if err = m.Uninstall(ctx, p.ID()); err != nil {
				t.Fatal(err)
			}
			if _, err = os.Stat(bin); err != nil {
				t.Fatal("workspace removal deleted shared Homebrew software", err)
			}
			for _, call := range *calls {
				if call[0] != "info" && !reflect.DeepEqual(call, []string{"install", "--formula", "--force-bottle", p.Formula}) {
					t.Fatal("unexpected global service/link/uninstall mutation", call)
				}
			}
		})
	}
}

func TestHomebrewRejectsForeignTapAndFormulaDrift(t *testing.T) {
	m, p, calls := brewFixture(t)
	ctx := context.Background()
	writeBrewKeg(t, m, p, "other/tap")
	if err := m.Install(ctx, p); err == nil || !strings.Contains(err.Error(), "different tap") {
		t.Fatal("adopted foreign tap", err)
	}
	for _, call := range *calls {
		if call[0] != "info" {
			t.Fatal("tried to overwrite conflicting installation", *calls)
		}
	}
	if err := os.RemoveAll(m.packageDir(p)); err != nil {
		t.Fatal(err)
	}
	p.SHA256 = strings.Repeat("b", 64)
	if err := m.Install(ctx, p); err == nil || !strings.Contains(err.Error(), "changed since preview") {
		t.Fatal("installed formula changed after preview", err)
	}
	for _, call := range *calls {
		if call[0] != "info" {
			t.Fatal("mutated Homebrew on a failed source check", call)
		}
	}
	s, _ := m.Read()
	if len(s.Packages) != 0 {
		t.Fatal("failed installation was registered")
	}
}

func TestHomebrewFailureAndCancellationNeverFallBackToArchive(t *testing.T) {
	for _, cancel := range []bool{false, true} {
		m, p, _ := brewFixture(t)
		m.Run = func(context.Context, string, ...string) (string, error) { return "", errors.New("brew unavailable") }
		ctx, stop := context.WithCancel(context.Background())
		if cancel {
			stop()
		}
		err := m.Install(ctx, p)
		stop()
		if err == nil {
			t.Fatal("installation ignored failure")
		}
		s, _ := m.Read()
		if len(s.Packages) != 0 {
			t.Fatal("failed install was registered")
		}
		files, _ := os.ReadDir(filepath.Join(m.Root, "downloads"))
		if len(files) != 0 {
			t.Fatal("Homebrew failure fell back to archive download")
		}
	}
}

func TestHomebrewDoesNotFollowUpdatedOptLinkOrAcceptMissingKeg(t *testing.T) {
	m, p, _ := brewFixture(t)
	ctx := context.Background()
	writeBrewKeg(t, m, p, "homebrew/core")
	if err := m.Install(ctx, p); err != nil {
		t.Fatal(err)
	}
	if err := os.Rename(m.packageDir(p), m.packageDir(p)+"-new"); err != nil {
		t.Fatal(err)
	}
	s, _ := m.Read()
	if _, err := m.Binary(&s, p.ID(), "postgres"); err == nil {
		t.Fatal("missing locked keg silently used another version")
	}
	info, err := m.PackageDetails(ctx, p)
	if err != nil || info.Integrity != "Needs repair" {
		t.Fatal(info, err)
	}
	if err := m.RepairPackage(ctx, p.ID()); err == nil || !strings.Contains(err.Error(), "brew reinstall") {
		t.Fatal("archive repair was offered for Homebrew software", err)
	}
}

func TestHomebrewResolutionRejectsUnavailableExactVersionAndWrongTap(t *testing.T) {
	m, _, _ := brewFixture(t)
	if _, err := m.ResolvePackage(context.Background(), "postgresql", "17.10"); err == nil {
		t.Fatal("silently changed an exact requested version")
	}
	m.Run = func(context.Context, string, ...string) (string, error) {
		return strings.ReplaceAll(brewMetadata(), "postgresql@17", "other/tap/postgresql@17"), nil
	}
	if _, err := m.ResolvePackage(context.Background(), "postgresql", "17"); err == nil {
		t.Fatal("resolved foreign PostgreSQL tap")
	}
}

func TestHomebrewInstallRefusesVersionUsedByAnotherCommand(t *testing.T) {
	m, p, calls := brewFixture(t)
	old := p
	old.Version = "17.10"
	ctx := context.Background()
	if err := m.withState(ctx, func(s *State) error {
		s.Packages[old.ID()] = old
		return nil
	}); err != nil {
		t.Fatal(err)
	}
	release, err := m.packageLease(ctx, old.ID(), false)
	if err != nil {
		t.Fatal(err)
	}
	defer release()
	var busy *PackageBusyError
	if err = m.Install(ctx, p); !errors.As(err, &busy) || busy.PackageID != old.ID() {
		t.Fatal("Homebrew install ignored a command using another keg version", err)
	}
	for _, call := range *calls {
		if call[0] != "info" {
			t.Fatal("changed software while another version was in use", call)
		}
	}
}

func TestHomebrewDescriptorsRejectRemovedSourcesAndForeignBinaryPaths(t *testing.T) {
	_, p, _ := brewFixture(t)
	for _, change := range []func(*Package){
		func(p *Package) { p.Formula = ""; p.Format = "dmg.postgres" },
		func(p *Package) { p.Formula = "other/tap/postgresql@17" },
		func(p *Package) { p.Version = "17" },
		func(p *Package) { p.Binaries = map[string]string{"postgres": "/opt/homebrew/bin/postgres"} },
	} {
		bad := p
		change(&bad)
		if err := bad.Validate(); err == nil {
			t.Fatal("accepted invalid Homebrew descriptor", bad)
		}
	}
}

func TestHomebrewDiagnosticsDoNotOfferArchiveRepair(t *testing.T) {
	m, p, _ := brewFixture(t)
	ctx := context.Background()
	writeBrewKeg(t, m, p, "homebrew/core")
	if err := m.Install(ctx, p); err != nil {
		t.Fatal(err)
	}
	s, _ := m.Read()
	for _, damaged := range []bool{false, true} {
		if damaged {
			if err := os.Remove(filepath.Join(m.packageDir(p), p.Binaries["postgres"])); err != nil {
				t.Fatal(err)
			}
		}
		report, err := m.diagnose(ctx, s, diagnosticProbes{})
		if err != nil {
			t.Fatal(err)
		}
		check := findDiagnostic(t, report, "Package", p.ID())
		want := "OK"
		if damaged {
			want = "Failed"
		}
		if check.Status != want || strings.Contains(check.Remedy, "archive") || (damaged && !strings.Contains(check.Remedy, "Homebrew")) {
			t.Fatal(check)
		}
	}
}

// Read-only verification against the developer's Homebrew. Never installs,
// upgrades, starts or removes shared system software.
func TestHomebrewLiveMetadataAndExistingKeg(t *testing.T) {
	if os.Getenv("MSM_HOMEBREW_VERIFY") != "1" {
		t.Skip("set MSM_HOMEBREW_VERIFY=1 for read-only Homebrew metadata/keg verification")
	}
	m := testManager(t)
	for _, name := range []string{"postgresql"} {
		p, err := m.ResolvePackage(context.Background(), name, "17")
		if err != nil {
			t.Fatal(err)
		}
		t.Log(p.Formula, p.Version, p.SHA256)
		if name == "postgresql" {
			s := emptyState()
			s.Packages[p.ID()] = p
			bin, err := m.Binary(&s, p.ID(), "postgres")
			if err != nil {
				t.Fatal(err)
			}
			out, err := m.Run(context.Background(), bin, "--version")
			if err != nil || !strings.Contains(out, strings.Split(p.Version, "_")[0]) {
				t.Fatal(out, err)
			}
			t.Log(strings.TrimSpace(out))
		}
	}
}

func redisBrewMetadata(fullName string) string {
	f := homebrewFormula{Name: "redis@8.2", FullName: fullName, License: "AGPL-3.0-only AND BSD-2-Clause"}
	f.Versions.Stable = "8.2.11"
	f.SourceChecksum.SHA256 = strings.Repeat("b", 64)
	b, _ := json.Marshal(map[string]any{"formulae": []homebrewFormula{f}})
	return string(b)
}

func TestHomebrewFormulaForRedis(t *testing.T) {
	for _, version := range []string{"8.2", "8.2.11", "v8.2.11", "8.2.11_1"} {
		if formula, err := homebrewFormulaFor("redis", version); err != nil || formula != "redis@8.2" {
			t.Fatal(version, formula, err)
		}
	}
	for _, version := range []string{"7.4.7", "8", "8.10", "redis"} {
		if _, err := homebrewFormulaFor("redis", version); err == nil {
			t.Fatal("accepted", version)
		}
	}
}

func TestResolveRedisHomebrew(t *testing.T) {
	run := func(metadata string) commandRunner {
		return func(ctx context.Context, program string, args ...string) (string, error) {
			if program != "/usr/bin/env" || len(args) < 5 || args[3] != homebrewProgram || args[4] != "info" {
				t.Fatalf("unexpected command %s %v", program, args)
			}
			return metadata, nil
		}
	}
	p, err := resolveHomebrew(context.Background(), "redis", "8.2", run(redisBrewMetadata("redis@8.2")))
	if err != nil {
		t.Fatal(err)
	}
	if p.Name != "redis" || p.Version != "8.2.11" || p.Formula != "redis@8.2" || p.Format != "homebrew" || p.License == "" ||
		!reflect.DeepEqual(p.Binaries, map[string]string{"redis-server": "bin/redis-server", "redis-cli": "bin/redis-cli"}) {
		t.Fatalf("%+v", p)
	}
	if err = p.Validate(); err != nil {
		t.Fatal(err)
	}
	if _, err = resolveHomebrew(context.Background(), "redis", "8.2", run(redisBrewMetadata("other/tap/redis@8.2"))); err == nil {
		t.Fatal("accepted a formula from a different tap")
	}
}

func TestRedisRequiresHomebrewPackage(t *testing.T) {
	p := Package{Name: "redis", Version: "8.2.11", Arch: "arm64", URL: "https://example.com/redis.zip", SHA256: strings.Repeat("a", 64), Format: "zip", Binaries: map[string]string{"redis-server": "bin/redis-server"}}
	if err := p.Validate(); err == nil {
		t.Fatal("zip-format Redis package accepted")
	}
}

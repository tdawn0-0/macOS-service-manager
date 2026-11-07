package devplatform

import (
	"archive/zip"
	"bytes"
	"context"
	"crypto/sha256"
	"debug/macho"
	"encoding/binary"
	"encoding/hex"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func testMachO(cpu macho.Cpu) []byte {
	b := make([]byte, 32)
	binary.LittleEndian.PutUint32(b, macho.Magic64)
	binary.LittleEndian.PutUint32(b[4:], uint32(cpu))
	binary.LittleEndian.PutUint32(b[12:], uint32(macho.TypeExec))
	return b
}
func TestManagedExecutableRequiresAppleSiliconSlice(t *testing.T) {
	for _, cpu := range []macho.Cpu{macho.CpuArm64, macho.CpuAmd64} {
		t.Run(cpu.String(), func(t *testing.T) {
			path := filepath.Join(t.TempDir(), "binary")
			if e := os.WriteFile(path, testMachO(cpu), 0700); e != nil {
				t.Fatal(e)
			}
			e := requireAppleSiliconExecutable(path)
			if (e == nil) != (cpu == macho.CpuArm64) {
				t.Fatal(cpu, e)
			}
		})
	}
	for _, arm := range []bool{false, true} {
		t.Run(map[bool]string{false: "fat-intel-only", true: "fat-arm64"}[arm], func(t *testing.T) {
			cpu := macho.CpuAmd64
			if arm {
				cpu = macho.CpuArm64
			}
			b := make([]byte, 28)
			binary.BigEndian.PutUint32(b, macho.MagicFat)
			binary.BigEndian.PutUint32(b[4:], 1)
			binary.BigEndian.PutUint32(b[8:], uint32(cpu))
			binary.BigEndian.PutUint32(b[16:], 28)
			binary.BigEndian.PutUint32(b[20:], 32)
			b = append(b, testMachO(cpu)...)
			path := filepath.Join(t.TempDir(), "fat")
			if e := os.WriteFile(path, b, 0700); e != nil {
				t.Fatal(e)
			}
			e := requireAppleSiliconExecutable(path)
			if (e == nil) != arm {
				t.Fatal(arm, e)
			}
		})
	}
	path := filepath.Join(t.TempDir(), "script")
	_ = os.WriteFile(path, []byte("#!/bin/sh\nexit 0\n"), 0700)
	if e := requireAppleSiliconExecutable(path); e != nil {
		t.Fatal("script compatibility", e)
	}
}
func TestIntelManifestRejectedBeforeInstallAndSavedBinaryUse(t *testing.T) {
	m := testManager(t)
	p, _ := packageArchive(t, "1.0.0")
	p.Arch = "amd64"
	if e := m.Install(context.Background(), p); e == nil || !strings.Contains(e.Error(), "Apple Silicon") {
		t.Fatal("installed Intel manifest", e)
	}
	if _, e := m.PackageDetails(context.Background(), p); e == nil {
		t.Fatal("preview accepted Intel package")
	}
	state := emptyState()
	state.Packages[p.ID()] = p
	if _, e := m.Binary(&state, p.ID(), "fixture"); e == nil || !strings.Contains(e.Error(), "Apple Silicon") {
		t.Fatal("saved Intel package reached execution", e)
	}
	files, _ := os.ReadDir(filepath.Join(m.Root, "packages"))
	if len(files) != 0 {
		t.Fatal("rejected package left installation", files)
	}
}
func TestCatalogOffersOnlyAppleSiliconCompatiblePackages(t *testing.T) {
	for _, name := range []string{"mysql", "redis", "mongodb"} {
		p, e := Resolve(context.Background(), name, AvailableVersions(name)[0])
		if e != nil {
			t.Fatal(name, e)
		}
		if p.Arch != "arm64" && p.Arch != "universal" {
			t.Fatal(p)
		}
		for _, s := range []string{"x86_64", "x64", "amd64"} {
			if strings.Contains(p.URL, s) {
				t.Fatal("Intel download", p.URL)
			}
		}
	}
}

func TestInstallRejectsIntelBinaryEvenWithCompatibleManifest(t *testing.T) {
	for _, arch := range []string{"arm64", "universal"} {
		t.Run(arch, func(t *testing.T) {
			m := testManager(t)
			var archive bytes.Buffer
			z := zip.NewWriter(&archive)
			h := &zip.FileHeader{Name: "bin/intel", Method: zip.Deflate}
			h.SetMode(0755)
			w, e := z.CreateHeader(h)
			if e != nil {
				t.Fatal(e)
			}
			if _, e = w.Write(testMachO(macho.CpuAmd64)); e != nil {
				t.Fatal(e)
			}
			if e = z.Close(); e != nil {
				t.Fatal(e)
			}
			sum := sha256.Sum256(archive.Bytes())
			p := Package{Name: "incompatible", Version: "1.0.0", Arch: arch, URL: "https://unused.invalid/intel.zip", SHA256: hex.EncodeToString(sum[:]), Format: "zip", Binaries: map[string]string{"intel": "bin/intel"}}
			if e = os.WriteFile(filepath.Join(m.Root, "downloads", p.SHA256), archive.Bytes(), 0600); e != nil {
				t.Fatal(e)
			}
			if e = m.Install(context.Background(), p); e == nil || !strings.Contains(e.Error(), "Apple Silicon") {
				t.Fatal("installed Intel executable", e)
			}
			state, e := m.Read()
			if e != nil || len(state.Packages) != 0 {
				t.Fatal("registered incompatible package", state, e)
			}
			entries, _ := os.ReadDir(filepath.Join(m.Root, "packages"))
			if len(entries) != 0 {
				t.Fatal("left staged Intel binary", entries)
			}
		})
	}
}

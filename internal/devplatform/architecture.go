package devplatform

import (
	"debug/macho"
	"encoding/binary"
	"errors"
	"fmt"
	"os"
	"runtime"
)

func requireAppleSiliconHost() error {
	if runtime.GOOS != "darwin" || runtime.GOARCH != "arm64" {
		return errors.New("managed development software requires macOS on Apple Silicon (arm64)")
	}
	return nil
}

// Upstream universal archives remain usable without changing saved package IDs.
// Every managed Mach-O entry point must contain an arm64 slice. Scripts retain
// their normal interpreter handling; no Intel-only executable is accepted.
func requireAppleSiliconExecutable(path string) error {
	f, e := os.Open(path)
	if e != nil {
		return e
	}
	defer f.Close()
	var header [4]byte
	n, e := f.Read(header[:])
	if e != nil || n < 4 {
		return nil
	}
	magic := binary.BigEndian.Uint32(header[:])
	switch magic {
	case 0xfeedface, 0xcefaedfe, 0xfeedfacf, 0xcffaedfe, 0xcafebabe, 0xbebafeca, 0xcafebabf, 0xbfbafeca:
	default:
		return nil
	}
	if thin, e := macho.Open(path); e == nil {
		defer thin.Close()
		if thin.Cpu == macho.CpuArm64 {
			return nil
		}
		return fmt.Errorf("%s is Intel-only or another unsupported architecture; an Apple Silicon executable is required", path)
	}
	fat, e := macho.OpenFat(path)
	if e != nil {
		return fmt.Errorf("cannot verify Apple Silicon architecture in %s: %w", path, e)
	}
	defer fat.Close()
	for _, arch := range fat.Arches {
		if arch.Cpu == macho.CpuArm64 {
			return nil
		}
	}
	return fmt.Errorf("%s has no Apple Silicon (arm64) executable", path)
}

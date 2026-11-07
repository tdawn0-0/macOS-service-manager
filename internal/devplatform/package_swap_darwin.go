//go:build darwin

package devplatform

import (
	"errors"
	"github.com/ebitengine/purego"
)

// RENAME_SWAP keeps the installation path present throughout replacement.
// Both directories are staged on the same volume. No non-atomic fallback is
// used when the filesystem cannot exchange names.
func exchangePackageDirectories(a, b string) error {
	lib, err := purego.Dlopen("/usr/lib/libSystem.B.dylib", purego.RTLD_NOW)
	if err != nil {
		return err
	}
	defer purego.Dlclose(lib)
	var rename func(string, string, uint32) int32
	purego.RegisterLibFunc(&rename, lib, "renamex_np")
	if rename(a, b, 2) != 0 {
		return errors.New("filesystem could not atomically exchange the package directories; the original installation was kept")
	}
	return nil
}

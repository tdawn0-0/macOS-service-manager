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
	return renamePackageDirectories(a, b, 2, "filesystem could not atomically exchange the package directories; the original installation was kept")
}

// RENAME_EXCL refuses even an empty destination created after the caller's
// existence check. Ordinary rename would silently replace that directory.
func publishNewPackageDirectory(a, b string) error {
	return renamePackageDirectories(a, b, 4, "installation destination already exists or cannot be published exclusively; existing files were kept: "+b)
}

func renamePackageDirectories(a, b string, flags uint32, failure string) error {
	lib, err := purego.Dlopen("/usr/lib/libSystem.B.dylib", purego.RTLD_NOW)
	if err != nil {
		return err
	}
	defer purego.Dlclose(lib)
	var rename func(string, string, uint32) int32
	purego.RegisterLibFunc(&rename, lib, "renamex_np")
	if rename(a, b, flags) != 0 {
		return errors.New(failure)
	}
	return nil
}

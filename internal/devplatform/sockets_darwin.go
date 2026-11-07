//go:build darwin

package devplatform

import (
	"fmt"
	"github.com/ebitengine/purego"
	"net"
	"os"
	"unsafe"
)

// launchd creates the privileged socket before dropping to the configured user.
// The Go server only receives the already-bound descriptor.
func activatedListener(name string) (net.Listener, error) {
	lib, e := purego.Dlopen("/usr/lib/libSystem.B.dylib", purego.RTLD_NOW)
	if e != nil {
		return nil, e
	}
	defer purego.Dlclose(lib)
	var activate func(string, **int32, *uintptr) int32
	var free func(unsafe.Pointer)
	purego.RegisterLibFunc(&activate, lib, "launch_activate_socket")
	purego.RegisterLibFunc(&free, lib, "free")
	var fds *int32
	var count uintptr
	if code := activate(name, &fds, &count); code != 0 {
		return nil, fmt.Errorf("activate %s: errno %d", name, code)
	}
	if fds == nil || count == 0 {
		return nil, fmt.Errorf("no socket for %s", name)
	}
	defer free(unsafe.Pointer(fds))
	descriptors := unsafe.Slice(fds, int(count))
	var files []*os.File
	for _, fd := range descriptors {
		files = append(files, os.NewFile(uintptr(fd), name))
	}
	defer func() {
		for _, f := range files {
			f.Close()
		}
	}()
	if len(files) != 1 {
		return nil, fmt.Errorf("expected one IPv4 socket, got %d", len(files))
	}
	return net.FileListener(files[0])
}

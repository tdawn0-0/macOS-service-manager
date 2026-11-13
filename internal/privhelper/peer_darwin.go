//go:build darwin

package privhelper

import (
	"fmt"
	"net"
	"os"
	"sync"
	"unsafe"

	"github.com/ebitengine/purego"
)

var (
	libSystemOnce sync.Once
	libSystemErr  error
	activateSock  func(name *byte, fds **int32, cnt *uintptr) int32
	freeMem       func(ptr unsafe.Pointer)
	getPeerEID    func(fd int32, euid *uint32, egid *uint32) int32
)

func initLibSystem() error {
	libSystemOnce.Do(func() {
		lib, err := purego.Dlopen("/usr/lib/libSystem.B.dylib", purego.RTLD_LAZY)
		if err != nil {
			libSystemErr = err
			return
		}
		purego.RegisterLibFunc(&activateSock, lib, "launch_activate_socket")
		purego.RegisterLibFunc(&freeMem, lib, "free")
		purego.RegisterLibFunc(&getPeerEID, lib, "getpeereid")
	})
	return libSystemErr
}

// activatedUnixListener claims the launchd-managed Unix domain socket named by
// name and configures it not to unlink the socket file on close.
func activatedUnixListener(name string) (*net.UnixListener, error) {
	if err := initLibSystem(); err != nil {
		return nil, err
	}
	cname := append([]byte(name), 0)
	var fds *int32
	var cnt uintptr
	if rc := activateSock(&cname[0], &fds, &cnt); rc != 0 {
		return nil, fmt.Errorf("launch_activate_socket(%q) returned errno %d", name, rc)
	}
	if cnt == 0 || fds == nil {
		if fds != nil {
			freeMem(unsafe.Pointer(fds))
		}
		return nil, fmt.Errorf("launch_activate_socket(%q) returned no file descriptors", name)
	}
	fd := *fds
	freeMem(unsafe.Pointer(fds))

	file := os.NewFile(uintptr(fd), name)
	if file == nil {
		return nil, fmt.Errorf("failed to wrap launchd socket descriptor %d", fd)
	}
	ln, err := net.FileListener(file)
	_ = file.Close()
	if err != nil {
		return nil, err
	}
	ul, ok := ln.(*net.UnixListener)
	if !ok {
		_ = ln.Close()
		return nil, fmt.Errorf("activated socket %q is %T, want *net.UnixListener", name, ln)
	}
	// Preserve /var/run/xyz.jyeu.devbase.helper.sock so launchd can re-activate
	// the helper on demand after idle shutdown.
	ul.SetUnlinkOnClose(false)
	return ul, nil
}

// peerEffectiveUID returns the effective UID of the connected Unix domain socket peer.
func peerEffectiveUID(conn net.Conn) (int, error) {
	if err := initLibSystem(); err != nil {
		return -1, err
	}
	uc, ok := conn.(*net.UnixConn)
	if !ok {
		return -1, fmt.Errorf("unexpected connection type %T", conn)
	}
	raw, err := uc.SyscallConn()
	if err != nil {
		return -1, err
	}
	var euid, egid uint32
	var rc int32
	ctrlErr := raw.Control(func(fd uintptr) {
		rc = getPeerEID(int32(fd), &euid, &egid)
	})
	if ctrlErr != nil {
		return -1, ctrlErr
	}
	if rc != 0 {
		return -1, fmt.Errorf("getpeereid failed")
	}
	return int(euid), nil
}

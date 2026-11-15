//go:build darwin

package privhelper

import (
	"fmt"
	"net"
	"sync"
	"unsafe"

	"github.com/ebitengine/purego"
)

const (
	solLocal       = 0     // SOL_LOCAL
	localPeerToken = 0x006 // LOCAL_PEERTOKEN
	auditTokenLen  = 32
)

var (
	secOnce sync.Once
	secErr  error

	secCodeCopySelf                  func(flags uint32, out *uintptr) int32
	secCodeCopyDesignatedRequirement func(code uintptr, flags uint32, out *uintptr) int32
	secCodeCopyGuestWithAttributes   func(host uintptr, attrs uintptr, flags uint32, out *uintptr) int32
	secCodeCheckValidity             func(code uintptr, flags uint32, req uintptr) int32

	cfRelease          func(obj uintptr)
	cfDataCreate       func(alloc uintptr, bytes *byte, length int) uintptr
	cfDictionaryCreate func(alloc uintptr, keys *uintptr, values *uintptr, n int, keyCB uintptr, valueCB uintptr) uintptr

	cfTypeDictKeyCallBacks   uintptr
	cfTypeDictValueCallBacks uintptr
	secGuestAttributeAudit   uintptr // the CFStringRef value
	secLib                   uintptr

	getSockOpt func(fd int32, level int32, name int32, val unsafe.Pointer, vlen *uint32) int32

	selfReqOnce sync.Once
	selfReq     uintptr
	selfReqErr  error
)

// derefSymbol reads the pointer-sized value stored at a data symbol's address.
func derefSymbol(addr uintptr) uintptr {
	p := *(*unsafe.Pointer)(unsafe.Pointer(&addr))
	return *(*uintptr)(p)
}

func initSecurity() error {
	secOnce.Do(func() {
		sec, err := purego.Dlopen("/System/Library/Frameworks/Security.framework/Security", purego.RTLD_LAZY|purego.RTLD_GLOBAL)
		if err != nil {
			secErr = err
			return
		}
		cf, err := purego.Dlopen("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation", purego.RTLD_LAZY|purego.RTLD_GLOBAL)
		if err != nil {
			secErr = err
			return
		}
		sys, err := purego.Dlopen("/usr/lib/libSystem.B.dylib", purego.RTLD_LAZY)
		if err != nil {
			secErr = err
			return
		}
		secLib = sec
		purego.RegisterLibFunc(&secCodeCopySelf, sec, "SecCodeCopySelf")
		purego.RegisterLibFunc(&secCodeCopyDesignatedRequirement, sec, "SecCodeCopyDesignatedRequirement")
		purego.RegisterLibFunc(&secCodeCopyGuestWithAttributes, sec, "SecCodeCopyGuestWithAttributes")
		purego.RegisterLibFunc(&secCodeCheckValidity, sec, "SecCodeCheckValidity")
		purego.RegisterLibFunc(&cfRelease, cf, "CFRelease")
		purego.RegisterLibFunc(&cfDataCreate, cf, "CFDataCreate")
		purego.RegisterLibFunc(&cfDictionaryCreate, cf, "CFDictionaryCreate")
		purego.RegisterLibFunc(&getSockOpt, sys, "getsockopt")

		var e error
		if cfTypeDictKeyCallBacks, e = purego.Dlsym(cf, "kCFTypeDictionaryKeyCallBacks"); e != nil {
			secErr = e
			return
		}
		if cfTypeDictValueCallBacks, e = purego.Dlsym(cf, "kCFTypeDictionaryValueCallBacks"); e != nil {
			secErr = e
			return
		}
		sym, e := purego.Dlsym(sec, "kSecGuestAttributeAudit")
		if e != nil {
			secErr = e
			return
		}
		secGuestAttributeAudit = derefSymbol(sym)
		if secGuestAttributeAudit == 0 {
			secErr = fmt.Errorf("kSecGuestAttributeAudit is nil")
		}
	})
	return secErr
}

// selfRequirement returns this process's designated requirement. It is
// computed once and intentionally never released.
func selfRequirement() (uintptr, error) {
	if err := initSecurity(); err != nil {
		return 0, err
	}
	selfReqOnce.Do(func() {
		var self uintptr
		if rc := secCodeCopySelf(0, &self); rc != 0 || self == 0 {
			selfReqErr = fmt.Errorf("SecCodeCopySelf failed: OSStatus %d", rc)
			return
		}
		defer cfRelease(self)
		var req uintptr
		if rc := secCodeCopyDesignatedRequirement(self, 0, &req); rc != 0 || req == 0 {
			selfReqErr = fmt.Errorf("SecCodeCopyDesignatedRequirement failed: OSStatus %d", rc)
			return
		}
		selfReq = req
	})
	return selfReq, selfReqErr
}

// codeSatisfies looks up the guest code object identified by the single
// attribute key/value (CF objects) and checks it against requirement.
func codeSatisfies(key uintptr, value uintptr, requirement uintptr) error {
	if err := initSecurity(); err != nil {
		return err
	}
	if key == 0 || value == 0 || requirement == 0 {
		return fmt.Errorf("missing code identity inputs")
	}
	keys := [1]uintptr{key}
	values := [1]uintptr{value}
	attrs := cfDictionaryCreate(0, &keys[0], &values[0], 1, cfTypeDictKeyCallBacks, cfTypeDictValueCallBacks)
	if attrs == 0 {
		return fmt.Errorf("CFDictionaryCreate failed")
	}
	defer cfRelease(attrs)
	var guest uintptr
	if rc := secCodeCopyGuestWithAttributes(0, attrs, 0, &guest); rc != 0 || guest == 0 {
		return fmt.Errorf("SecCodeCopyGuestWithAttributes failed: OSStatus %d", rc)
	}
	defer cfRelease(guest)
	if rc := secCodeCheckValidity(guest, 0, requirement); rc != 0 {
		return fmt.Errorf("peer code does not satisfy requirement: OSStatus %d", rc)
	}
	return nil
}

// peerAuditToken returns the audit token of the connected Unix socket peer.
func peerAuditToken(conn net.Conn) ([32]byte, error) {
	var token [32]byte
	if err := initSecurity(); err != nil {
		return token, err
	}
	uc, ok := conn.(*net.UnixConn)
	if !ok {
		return token, fmt.Errorf("unexpected connection type %T", conn)
	}
	raw, err := uc.SyscallConn()
	if err != nil {
		return token, err
	}
	n := uint32(auditTokenLen)
	var rc int32
	if cerr := raw.Control(func(fd uintptr) {
		rc = getSockOpt(int32(fd), solLocal, localPeerToken, unsafe.Pointer(&token[0]), &n)
	}); cerr != nil {
		return token, cerr
	}
	if rc != 0 {
		return token, fmt.Errorf("getsockopt LOCAL_PEERTOKEN failed")
	}
	if n != auditTokenLen {
		return token, fmt.Errorf("peer token length %d, want %d", n, auditTokenLen)
	}
	return token, nil
}

// peerRunsHelperCode reports nil only when the peer runs code that is validly
// signed and satisfies this process's own designated requirement.
func peerRunsHelperCode(conn net.Conn) error {
	token, err := peerAuditToken(conn)
	if err != nil {
		return err
	}
	req, err := selfRequirement()
	if err != nil {
		return err
	}
	data := cfDataCreate(0, &token[0], auditTokenLen)
	if data == 0 {
		return fmt.Errorf("CFDataCreate failed")
	}
	defer cfRelease(data)
	return codeSatisfies(secGuestAttributeAudit, data, req)
}

//go:build darwin

package system

import (
	"strconv"
	"strings"
	"sync"
	"time"
	"unsafe"

	"github.com/ebitengine/purego"
)

// minSettingsSide tells System Settings' window from the small picture of it
// that Stage Manager keeps at the side of the screen, which has the same owner.
const minSettingsSide = 300

var windowList struct {
	once                              sync.Once
	ok                                bool
	copyInfo, rectOf                  uintptr
	count, at, value, number, release uintptr
	keyPID, keyLayer, keyBounds       uintptr
}

// loadWindowList binds what reads the list of windows, which tells where a
// window is without any permission: only a window's title takes one.
func loadWindowList() {
	const frameworks = "/System/Library/Frameworks/"
	graphics, err := purego.Dlopen(frameworks+"CoreGraphics.framework/CoreGraphics", purego.RTLD_LAZY)
	if err != nil {
		return
	}
	foundation, err := purego.Dlopen(frameworks+"CoreFoundation.framework/CoreFoundation", purego.RTLD_LAZY)
	if err != nil {
		return
	}
	failed := false
	symbol := func(lib uintptr, name string) uintptr {
		p, err := purego.Dlsym(lib, name)
		failed = failed || err != nil
		return p
	}
	// A constant of the framework is a variable holding the string.
	constant := func(name string) uintptr {
		p := symbol(graphics, name)
		if p == 0 {
			return 0
		}
		return **(**uintptr)(unsafe.Pointer(&p))
	}
	l := &windowList
	l.copyInfo = symbol(graphics, "CGWindowListCopyWindowInfo")
	l.rectOf = symbol(graphics, "CGRectMakeWithDictionaryRepresentation")
	l.count = symbol(foundation, "CFArrayGetCount")
	l.at = symbol(foundation, "CFArrayGetValueAtIndex")
	l.value = symbol(foundation, "CFDictionaryGetValue")
	l.number = symbol(foundation, "CFNumberGetValue")
	l.release = symbol(foundation, "CFRelease")
	l.keyPID = constant("kCGWindowOwnerPID")
	l.keyLayer = constant("kCGWindowLayer")
	l.keyBounds = constant("kCGWindowBounds")
	l.ok = !failed
}

func call(fn uintptr, args ...uintptr) uintptr {
	r, _, _ := purego.SyscallN(fn, args...)
	return r
}

func numberOf(dict, key uintptr) int64 {
	const sint64Type = 4
	var n int64
	if v := call(windowList.value, dict, key); v != 0 {
		call(windowList.number, v, sint64Type, uintptr(unsafe.Pointer(&n)))
	}
	return n
}

// largestWindow returns the largest normal window that a process shows.
func largestWindow(pid int64) (Rect, bool) {
	windowList.once.Do(loadWindowList)
	l := &windowList
	if !l.ok {
		return Rect{}, false
	}
	const onScreenOnly, excludeDesktopElements = 1 << 0, 1 << 4
	list := call(l.copyInfo, onScreenOnly|excludeDesktopElements, 0)
	if list == 0 {
		return Rect{}, false
	}
	defer call(l.release, list)
	var best Rect
	found := false
	for i, n := uintptr(0), call(l.count, list); i < n; i++ {
		info := call(l.at, list, i)
		if numberOf(info, l.keyPID) != pid || numberOf(info, l.keyLayer) != 0 {
			continue
		}
		bounds := call(l.value, info, l.keyBounds)
		if bounds == 0 {
			continue
		}
		var r struct{ x, y, width, height float64 }
		if call(l.rectOf, bounds, uintptr(unsafe.Pointer(&r)))&0xff == 0 {
			continue
		}
		if r.width < minSettingsSide || r.height < minSettingsSide {
			continue
		}
		if !found || int(r.width)*int(r.height) > best.Width*best.Height {
			best = Rect{int(r.x), int(r.y), int(r.width), int(r.height)}
			found = true
		}
	}
	return best, found
}

var settingsProcess struct {
	sync.Mutex
	pid     int64
	checked time.Time
}

// settingsPID returns the process of System Settings, looked for once a second
// at most: this runs for every frame that follows its window.
func settingsPID() int64 {
	p := &settingsProcess
	p.Lock()
	defer p.Unlock()
	if time.Since(p.checked) < time.Second {
		return p.pid
	}
	p.checked = time.Now()
	p.pid = 0
	// The process keeps its name in every language; its window's owner does not.
	if out, err := Run("pgrep", "-x", "System Settings"); err == nil {
		if pid, err := strconv.ParseInt(strings.TrimSpace(out), 10, 64); err == nil {
			p.pid = pid
		}
	}
	return p.pid
}

// SettingsWindow returns where the window of System Settings is, if it shows.
func SettingsWindow() (Rect, bool) {
	pid := settingsPID()
	if pid == 0 {
		return Rect{}, false
	}
	return largestWindow(pid)
}

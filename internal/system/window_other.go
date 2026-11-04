//go:build !darwin

package system

// SettingsWindow is about System Settings, which only macOS has.
func SettingsWindow() (Rect, bool) { return Rect{}, false }

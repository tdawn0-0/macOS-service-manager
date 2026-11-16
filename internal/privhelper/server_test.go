package privhelper

import (
	"path/filepath"
	"testing"
)

// The default hosts path must pass the helper's own symlink checks: /etc is a
// symlink to /private/etc on macOS, so the helper has to name the real file.
func TestDefaultHostsPathPassesSymlinkChecks(t *testing.T) {
	path := NewServer(ServerConfig{}).cfg.HostsPath
	if _, err := validateCleanTargetFile(path, []string{filepath.Dir(path)}, ""); err != nil {
		t.Fatalf("default hosts path %q is refused: %v", path, err)
	}
}

//go:build darwin

package privhelper

import (
	"context"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"testing"
	"time"
)

// InstallTestHelper starts an isolated in-process privhelper server over a
// short temporary Unix socket and registers it as privhelper.Default() for tb.
func InstallTestHelper(tb testing.TB, cfg ServerConfig) *Client {
	tb.Helper()
	sockDir, err := os.MkdirTemp("/tmp", "msm-ph-")
	if err != nil {
		tb.Fatalf("MkdirTemp: %v", err)
	}
	sockPath := filepath.Join(sockDir, "h.sock")
	ln, err := net.Listen("unix", sockPath)
	if err != nil {
		_ = os.RemoveAll(sockDir)
		tb.Fatalf("net.Listen unix %s: %v", sockPath, err)
	}
	if ul, ok := ln.(*net.UnixListener); ok {
		ul.SetUnlinkOnClose(false)
	}

	if cfg.OwnerUID == 0 {
		cfg.OwnerUID = os.Geteuid()
	}

	helperDir := tb.TempDir()
	stagedExe := filepath.Join(helperDir, "devbase-helper")
	if err := os.WriteFile(stagedExe, []byte("bin"), 0755); err != nil {
		tb.Fatal(err)
	}
	plistPath := filepath.Join(tb.TempDir(), Label+".plist")
	plistBytes, err := buildHelperPlist(stagedExe, cfg.OwnerUID, sockPath)
	if err != nil {
		tb.Fatal(err)
	}
	if err := os.WriteFile(plistPath, plistBytes, 0644); err != nil {
		tb.Fatal(err)
	}

	srv := NewServer(cfg)
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() { done <- srv.Serve(ctx, ln) }()

	client := NewClient(ClientConfig{
		SocketPath: sockPath,
		PlistPath:  plistPath,
		HelperDir:  helperDir,
		Expected: BinaryInfo{
			Executable: "/Applications/Devbase.app/Contents/MacOS/Devbase",
			OwnerUID:   cfg.OwnerUID,
		},
	})
	restore := SetDefaultForTest(client)

	tb.Cleanup(func() {
		restore()
		cancel()
		_ = ln.Close()
		select {
		case <-done:
		case <-time.After(3 * time.Second):
			panic(fmt.Sprintf("test helper %s did not shut down cleanly", sockPath))
		}
		_ = os.RemoveAll(sockDir)
	})
	return client
}

//go:build !darwin

package devplatform

import (
	"errors"
	"net"
)

func activatedListener(string) (net.Listener, error) {
	return nil, errors.New("launchd socket activation requires macOS")
}

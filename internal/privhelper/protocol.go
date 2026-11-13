// Package privhelper implements Devbase's on-demand launchd-managed
// privileged helper protocol, server, and client.
package privhelper

import "errors"

// ErrNotInstalled reports that privileged operations cannot run because the
// Devbase privileged helper is missing or outdated.
var ErrNotInstalled = errors.New("privileged helper is not installed or needs an update; install the helper in Devbase first")

const (
	// Label is the launchd job label for the Devbase privileged helper.
	Label = "xyz.jyeu.devbase.helper"
	// PlistPath is the system LaunchDaemon property list path for the helper.
	PlistPath = "/Library/LaunchDaemons/xyz.jyeu.devbase.helper.plist"
	// HelperDir is the root-owned directory holding the staged helper bundle or binary.
	HelperDir = "/Library/PrivilegedHelperTools/xyz.jyeu.devbase.helper"
	// SocketPath is the launchd-managed Unix domain socket path.
	SocketPath = "/var/run/xyz.jyeu.devbase.helper.sock"
	// SocketName is the Mach/launchd Sockets dictionary key.
	SocketName = "helper"
	// ProtocolVersion is the helper HTTP-over-Unix protocol version.
	ProtocolVersion = 1
)

// HelperState describes the passive installation state of the privileged helper.
type HelperState string

const (
	// StateNotInstalled reports that the helper plist or binary is not staged.
	StateNotInstalled HelperState = "not-installed"
	// StateOutdated reports that the staged helper version or owner differs from the running app.
	StateOutdated HelperState = "outdated"
	// StateInstalled reports that the helper plist and staged artifact match the running app.
	StateInstalled HelperState = "installed"
)

// HelperStatus describes the passive state read without launching the helper or prompting for a password.
type HelperStatus struct {
	State            HelperState
	InstalledVersion string
	ExpectedVersion  string
	Message          string
}

// StatusResponse is returned by GET /v1/status.
type StatusResponse struct {
	ProtocolVersion int    `json:"protocolVersion"`
	Version         string `json:"version"`
	OwnerUID        int    `json:"ownerUid"`
}

// HostsSyncRequest is sent to POST /v1/hosts/sync.
type HostsSyncRequest struct {
	ExpectedOld string `json:"expectedOld"`
	NewContent  string `json:"newContent"`
}

// ProxySocketInstallRequest is sent to POST /v1/proxy-socket/install.
type ProxySocketInstallRequest struct {
	Plist   []byte `json:"plist"`
	JobPath string `json:"jobPath"`
	Target  string `json:"target"`
}

// ProxySocketStopRequest is sent to POST /v1/proxy-socket/stop.
type ProxySocketStopRequest struct {
	JobPath string `json:"jobPath"`
	Target  string `json:"target"`
}

// ProxySocketKickstartRequest is sent to POST /v1/proxy-socket/kickstart.
type ProxySocketKickstartRequest struct {
	Target string `json:"target"`
}

// ProxySocketPrintRequest is sent to POST /v1/proxy-socket/print.
type ProxySocketPrintRequest struct {
	Target string `json:"target"`
}

// ProxySocketPrintResponse is returned by POST /v1/proxy-socket/print.
type ProxySocketPrintResponse struct {
	Output   []byte `json:"output,omitempty"`
	ExitCode int    `json:"exitCode"`
}

// PortInspectRequest is sent to POST /v1/ports/inspect.
type PortInspectRequest struct {
	Port int `json:"port"`
}

// PortInspectResponse is returned by POST /v1/ports/inspect.
type PortInspectResponse struct {
	Output []byte `json:"output,omitempty"`
}

// LaunchdControlRequest is sent to POST /v1/launchd/control.
type LaunchdControlRequest struct {
	Commands [][]string `json:"commands"`
}

// LaunchdSaveRequest is sent to POST /v1/launchd/save.
type LaunchdSaveRequest struct {
	Plist        []byte `json:"plist"`
	Path         string `json:"path"`
	Domain       string `json:"domain"`
	Target       string `json:"target"`
	ExistingPath string `json:"existingPath,omitempty"`
	Load         bool   `json:"load"`
}

// LaunchdSaveResponse is returned by POST /v1/launchd/save.
type LaunchdSaveResponse struct {
	Saved bool   `json:"saved"`
	Error string `json:"error,omitempty"`
}

// LaunchdRemoveRequest is sent to POST /v1/launchd/remove.
type LaunchdRemoveRequest struct {
	Path     string `json:"path"`
	Target   string `json:"target"`
	TrashDir string `json:"trashDir"`
}

// ErrorResponse is returned by helper endpoints on failure.
type ErrorResponse struct {
	Error string `json:"error"`
}

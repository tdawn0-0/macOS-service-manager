package devplatform

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net"
	"os"
	"sort"
	"strconv"
	"strings"
	"syscall"
	"time"
	"unicode"
)

type PortUnavailableError struct {
	Port int
	Role string
	Err  error
}

func (e *PortUnavailableError) Error() string {
	return fmt.Sprintf("%s port %d is unavailable; inspect the listener or choose another local port: %v", e.Role, e.Port, e.Err)
}
func (e *PortUnavailableError) Unwrap() error { return e.Err }

type PortListener struct {
	PID, UID      int
	Name, Address string
}
type PortInspection struct {
	Port      int
	Status    string // Available, In use, Unknown
	CheckedAt time.Time
	Listeners []PortListener
	Truncated bool
	Detail    string
}

func visiblePortText(value string) string {
	var b strings.Builder
	for _, r := range value {
		if unicode.IsControl(r) || unicode.IsSpace(r) {
			r = ' '
		}
		b.WriteRune(r)
		if b.Len() > 240 {
			break
		}
	}
	return strings.TrimSpace(b.String())
}
func parsePortListeners(output string) []PortListener {
	current := PortListener{UID: -1}
	seen := map[string]bool{}
	var results []PortListener
listenerFields:
	for _, raw := range strings.Split(output, "\x00") {
		field := strings.TrimLeft(raw, "\r\n")
		if field == "" {
			continue
		}
		value := field[1:]
		switch field[0] {
		case 'p':
			current = PortListener{UID: -1}
			current.PID, _ = strconv.Atoi(value)
		case 'c':
			current.Name = visiblePortText(value)
		case 'u':
			if uid, err := strconv.Atoi(value); err == nil {
				current.UID = uid
			}
		case 'n':
			current.Address = visiblePortText(value)
			if current.PID < 1 || current.Address == "" {
				continue
			}
			key := strconv.Itoa(current.PID) + ":" + current.Address
			if !seen[key] {
				results = append(results, current)
				seen[key] = true
				if len(results) >= 65 {
					break listenerFields
				}
			}
		}
	}
	sort.Slice(results, func(i, j int) bool {
		if results[i].PID != results[j].PID {
			return results[i].PID < results[j].PID
		}
		return results[i].Address < results[j].Address
	})
	return results
}

func isPermissionBindError(err error) bool {
	return errors.Is(err, os.ErrPermission) || errors.Is(err, syscall.EACCES)
}

func (m *Manager) bindLoopbackPort(port int) (io.Closer, error) {
	if m != nil && m.listenLoopback != nil {
		return m.listenLoopback(port)
	}
	return net.Listen("tcp4", fmt.Sprintf("127.0.0.1:%d", port))
}

func (m *Manager) connectLoopbackPort(ctx context.Context, port int) (io.Closer, error) {
	if m != nil && m.dialLoopback != nil {
		return m.dialLoopback(ctx, port)
	}
	probeCtx, cancel := context.WithTimeout(ctx, 500*time.Millisecond)
	defer cancel()
	return (&net.Dialer{}).DialContext(probeCtx, "tcp4", fmt.Sprintf("127.0.0.1:%d", port))
}

// parseNetstatListeners extracts LISTEN rows from macOS `netstat -anv -p tcp`
// output for a target port so root-owned listeners hidden from unprivileged lsof
// still report PID, process name, and endpoint.
func parseNetstatListeners(output string, port int) []PortListener {
	portText := strconv.Itoa(port)
	seen := map[string]bool{}
	var results []PortListener
	for _, line := range strings.Split(output, "\n") {
		fields := strings.Fields(line)
		if len(fields) < 11 {
			continue
		}
		if fields[0] != "tcp4" && fields[0] != "tcp6" && fields[0] != "tcp46" {
			continue
		}
		if fields[5] != "LISTEN" {
			continue
		}
		localAddr := fields[3]
		dot := strings.LastIndexByte(localAddr, '.')
		if dot <= 0 || localAddr[dot+1:] != portText {
			continue
		}
		host := localAddr[:dot]
		addr := host + ":" + portText
		if strings.Contains(host, ":") {
			addr = "[" + host + "]:" + portText
		}
		pid := 0
		var rawName string
		for idx := len(fields) - 1; idx >= 10; idx-- {
			colon := strings.LastIndexByte(fields[idx], ':')
			if colon < 0 {
				continue
			}
			parsedPID, err := strconv.Atoi(fields[idx][colon+1:])
			if err != nil || parsedPID < 1 {
				continue
			}
			pid = parsedPID
			var nameParts []string
			if idx > 10 {
				nameParts = append(nameParts, fields[10:idx]...)
			}
			if prefix := fields[idx][:colon]; prefix != "" {
				nameParts = append(nameParts, prefix)
			}
			rawName = strings.Join(nameParts, " ")
			break
		}
		if pid < 1 {
			continue
		}
		name := visiblePortText(rawName)
		if name == "" {
			name = "unknown"
		}
		key := strconv.Itoa(pid) + ":" + addr
		if seen[key] {
			continue
		}
		results = append(results, PortListener{PID: pid, UID: -1, Name: name, Address: addr})
		seen[key] = true
		if len(results) >= 65 {
			break
		}
	}
	sort.Slice(results, func(i, j int) bool {
		if results[i].PID != results[j].PID {
			return results[i].PID < results[j].PID
		}
		return results[i].Address < results[j].Address
	})
	return results
}

// Port status comes from binding IPv4 loopback (or probing loopback connect for
// privileged ports < 1024 when bind fails with permission denied), never from a
// PID or lsof exit code. Process names and endpoints are a bounded, read-only
// observation; full command lines, environment and process-control operations
// are not requested.
func (m *Manager) InspectPort(ctx context.Context, port int) (PortInspection, error) {
	r := PortInspection{Port: port, CheckedAt: time.Now().UTC(), Status: "Unknown"}
	if port < 1 || port > 65535 {
		return r, errors.New("port must be 1–65535")
	}
	if err := ctx.Err(); err != nil {
		return r, err
	}
	l, err := m.bindLoopbackPort(port)
	if err == nil {
		_ = l.Close()
		r.Status = "Available"
		r.Detail = "IPv4 loopback was available at this check. Saving a suggestion does not reserve it."
		return r, nil
	}
	if isPermissionBindError(err) {
		conn, dialErr := m.connectLoopbackPort(ctx, port)
		if ctxErr := ctx.Err(); ctxErr != nil {
			if conn != nil {
				_ = conn.Close()
			}
			return r, ctxErr
		}
		switch {
		case dialErr == nil:
			_ = conn.Close()
			err = syscall.EADDRINUSE
		case errors.Is(dialErr, syscall.ECONNREFUSED):
			r.Status = "Available"
			r.Detail = "IPv4 loopback had no active listener. Privileged ports below 1024 use launchd authorization when applying the proxy."
			return r, nil
		default:
			r.Detail = "Could not verify IPv4 loopback availability: " + dialErr.Error()
			return r, nil
		}
	}
	if !errors.Is(err, syscall.EADDRINUSE) {
		r.Detail = "Could not test IPv4 loopback availability: " + err.Error()
		return r, nil
	}
	r.Status = "In use"
	query, done := context.WithTimeout(ctx, 2*time.Second)
	defer done()
	output, lookupErr := m.Run(query, "/usr/sbin/lsof", "-nP", "-a", "-iTCP:"+strconv.Itoa(port), "-sTCP:LISTEN", "+c0", "-F0pcun")
	if err := ctx.Err(); err != nil {
		return r, err
	}
	if lookupErr == nil {
		r.Listeners = parsePortListeners(output)
	}
	if len(r.Listeners) == 0 {
		netstatOut, netstatErr := m.Run(query, "/usr/sbin/netstat", "-anv", "-p", "tcp")
		if err := ctx.Err(); err != nil {
			return r, err
		}
		if netstatErr == nil {
			r.Listeners = parseNetstatListeners(netstatOut, port)
		}
	}
	if len(r.Listeners) > 64 {
		r.Listeners = r.Listeners[:64]
		r.Truncated = true
	}
	if len(r.Listeners) == 0 {
		r.Detail = "IPv4 loopback is in use. Listener details were unavailable or the process changed during this check."
	} else {
		r.Detail = "Listeners reported for this TCP port. Names and PIDs do not prove app ownership. Stop an external service in its own app, or choose another port."
	}
	return r, nil
}

func (m *Manager) AvailableLocalPort(ctx context.Context) (int, error) {
	if err := ctx.Err(); err != nil {
		return 0, err
	}
	s, err := m.Read()
	if err != nil {
		return 0, err
	}
	reserved := map[int]bool{}
	for _, i := range s.Instances {
		reserved[i.Port] = true
	}
	reserved[s.ProxyListenerPort] = true
	for _, rule := range s.ProxyRules {
		reserved[rule.LocalPort] = true
	}
	if err := ctx.Err(); err != nil {
		return 0, err
	}
	port, err := freeInstancePort(0, reserved)
	if ctx.Err() != nil {
		return 0, ctx.Err()
	}
	return port, err
}

// Check only newly requested ports, keeping an existing app listener intact.
// launchd can bind privileged ports even when this process lacks that privilege.
func (m *Manager) preflightProxyPorts(ctx context.Context, sites, old []proxyRoute, loaded bool) error {
	before := map[int]bool{}
	for _, site := range old {
		before[site.Port] = true
	}
	for _, port := range proxyListenerPorts(sites) {
		if loaded && before[port] {
			continue
		}
		l, err := m.bindLoopbackPort(port)
		if err == nil {
			_ = l.Close()
			continue
		}
		if port < 1024 && isPermissionBindError(err) {
			conn, dialErr := m.connectLoopbackPort(ctx, port)
			if dialErr == nil {
				_ = conn.Close()
				return &PortUnavailableError{Port: port, Role: "HTTPS", Err: syscall.EADDRINUSE}
			}
			if ctxErr := ctx.Err(); ctxErr != nil {
				return ctxErr
			}
			continue
		}
		return &PortUnavailableError{Port: port, Role: "HTTPS", Err: err}
	}
	return nil
}

func freeInstancePort(preferred int, reserved map[int]bool) (int, error) {
	if preferred != 0 && (preferred < 1024 || preferred > 65535) {
		return 0, errors.New("locked database port must be 1024–65535")
	}
	if preferred != 0 && !reserved[preferred] {
		if l, e := net.Listen("tcp4", fmt.Sprintf("127.0.0.1:%d", preferred)); e == nil {
			l.Close()
			return preferred, nil
		}
	}
	for n := 0; n < 32; n++ {
		l, e := net.Listen("tcp4", "127.0.0.1:0")
		if e != nil {
			return 0, e
		}
		port := l.Addr().(*net.TCPAddr).Port
		l.Close()
		if !reserved[port] {
			return port, nil
		}
	}
	return 0, errors.New("could not select a free database port")
}

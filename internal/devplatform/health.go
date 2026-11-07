package devplatform

import (
	"context"
	"errors"
	"fmt"
	"net"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"
)

func (s Service) readinessTimeout() time.Duration {
	n := s.ReadinessTimeoutSeconds
	if n == 0 {
		n = 30
	}
	return time.Duration(n) * time.Second
}
func (s Service) restartThrottle() int {
	if s.RestartThrottleSeconds == 0 {
		return 10
	}
	return s.RestartThrottleSeconds
}
func validateHealth(address string) error {
	if address == "" || address == "process" {
		return nil
	}
	u, err := url.Parse(address)
	if err != nil || u.Hostname() == "" || u.User != nil || u.Fragment != "" {
		return errors.New("readiness check must be process, tcp://host:port or http(s)://host/path")
	}
	if u.Scheme == "tcp" {
		_, port, err := net.SplitHostPort(u.Host)
		n, _ := strconv.Atoi(port)
		if err != nil || n < 1 || n > 65535 || u.Path != "" || u.RawQuery != "" {
			return errors.New("TCP readiness requires a host and port from 1–65535")
		}
	} else if u.Scheme == "http" || u.Scheme == "https" {
		if _, err := http.NewRequest(http.MethodGet, address, nil); err != nil {
			return errors.New("invalid HTTP readiness address")
		}
		if u.Port() != "" {
			n, err := strconv.Atoi(u.Port())
			if err != nil || n < 1 || n > 65535 {
				return errors.New("invalid HTTP readiness port")
			}
		}
	} else {
		return errors.New("readiness check must be process, tcp://host:port or http(s)://host/path")
	}
	return nil
}

// probeHealth makes one bounded attempt. It does not imply that the process
// which should own that address is alive; service callers also check launchd.
func probeHealth(ctx context.Context, address string) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	if err := validateHealth(address); err != nil {
		return err
	}
	if address == "" || address == "process" {
		return nil
	}
	if strings.HasPrefix(address, "tcp://") {
		c, err := (&net.Dialer{Timeout: time.Second}).DialContext(ctx, "tcp", strings.TrimPrefix(address, "tcp://"))
		if c != nil {
			c.Close()
		}
		return err
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, address, nil)
	if err != nil {
		return errors.New("invalid readiness request")
	}
	res, err := (&http.Client{Timeout: time.Second}).Do(req)
	if res != nil {
		res.Body.Close()
	}
	if err != nil {
		return errors.New("HTTP readiness request did not complete")
	}
	if res.StatusCode >= 400 {
		return fmt.Errorf("HTTP %d", res.StatusCode)
	}
	return nil
}

func waitHealth(ctx context.Context, address string) error {
	if err := validateHealth(address); err != nil {
		return err
	}
	ctx, cancel := context.WithTimeout(ctx, 30*time.Second)
	defer cancel()
	for {
		if err := probeHealth(ctx, address); err == nil {
			return nil
		}
		select {
		case <-ctx.Done():
			return fmt.Errorf("readiness check did not pass: %w", ctx.Err())
		case <-time.After(200 * time.Millisecond):
		}
	}
}

type jobState struct {
	Running      bool
	PID          int
	Runs         int
	LastExitCode *int
	LastSignal   string
	ExitReason   bool
}

func (s jobState) finished() bool {
	return !s.Running && s.PID == 0 && s.Runs > 0 && (s.LastExitCode != nil || s.LastSignal != "" || s.ExitReason)
}

func (s jobState) successfulExit() bool {
	return s.finished() && s.LastExitCode != nil && *s.LastExitCode == 0 && s.LastSignal == "" && !s.ExitReason
}

func parseJobState(output string) jobState {
	s := jobState{}
	propertyIndent := -1
	for _, line := range strings.Split(output, "\n") {
		key, value, ok := strings.Cut(strings.TrimSpace(line), " = ")
		if !ok {
			continue
		}
		switch key {
		case "state", "pid", "runs", "last exit code", "last terminating signal", "last exit reason":
		default:
			continue
		}
		indent := len(line) - len(strings.TrimLeft(line, " \t"))
		if propertyIndent < 0 {
			propertyIndent = indent
		}
		// launchctl also prints nested resource coalitions with their own
		// "state = active". Those are not the managed process state.
		if indent != propertyIndent {
			continue
		}
		switch key {
		case "state":
			s.Running = value == "running"
		case "pid":
			s.PID, _ = strconv.Atoi(value)
		case "runs":
			s.Runs, _ = strconv.Atoi(value)
		case "last exit code":
			if n, err := strconv.Atoi(value); err == nil {
				s.LastExitCode = &n
			}
		case "last terminating signal":
			// Only retain the numeric part, never arbitrary launchctl text.
			for _, field := range strings.Fields(value) {
				if n, err := strconv.Atoi(strings.Trim(field, "():,")); err == nil && n > 0 {
					s.LastSignal = strconv.Itoa(n)
					break
				}
			}
		case "last exit reason":
			s.ExitReason = value != ""
		}
	}
	return s
}
func (m *Manager) readJobState(ctx context.Context, id string) (jobState, error) {
	ctx, cancel := context.WithTimeout(ctx, time.Second)
	defer cancel()
	output, err := m.Run(ctx, "launchctl", "print", domain()+"/"+m.label(id))
	if err != nil && ctx.Err() != nil {
		err = ctx.Err()
	}
	return parseJobState(output), err
}

// Launch registration alone is not readiness. Wait for a stable main process
// and the configured check. Fast-exiting commands must fail even when another
// process happens to answer the readiness port.
func (m *Manager) waitService(ctx context.Context, job string, svc Service) error {
	ctx, cancel := context.WithTimeout(ctx, svc.readinessTimeout())
	defer cancel()
	var since time.Time
	pid := 0
	for {
		if err := ctx.Err(); err != nil {
			return fmt.Errorf("service %s did not become ready: %w", svc.ID, err)
		}
		state, err := m.readJobState(ctx, job)
		if err == nil && state.Running && state.PID > 0 {
			if pid != state.PID || since.IsZero() {
				pid, since = state.PID, time.Now()
			}
			if time.Since(since) >= 200*time.Millisecond && probeHealth(ctx, svc.Health) == nil {
				after, err := m.readJobState(ctx, job)
				if err == nil && after.Running && after.PID == pid {
					return nil
				}
			}
		} else {
			pid, since = 0, time.Time{}
			if err == nil && !svc.Restart && state.LastExitCode != nil {
				return fmt.Errorf("service %s exited with code %d before becoming ready; read its logs", svc.ID, *state.LastExitCode)
			}
			if err == nil && !svc.Restart && state.LastSignal != "" {
				return fmt.Errorf("service %s terminated by signal %s before becoming ready; read its logs", svc.ID, state.LastSignal)
			}
		}
		select {
		case <-ctx.Done():
			return fmt.Errorf("service %s did not become ready: %w", svc.ID, ctx.Err())
		case <-time.After(100 * time.Millisecond):
		}
	}
}

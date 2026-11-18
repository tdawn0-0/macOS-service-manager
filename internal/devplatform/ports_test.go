package devplatform

import (
	"context"
	"errors"
	"io"
	"net"
	"os"
	"path/filepath"
	"reflect"
	"strconv"
	"strings"
	"syscall"
	"testing"
)

func TestInspectPortShowsRealListenerWithReadOnlyArguments(t *testing.T) {
	m := testManager(t)
	l, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer l.Close()
	port := l.Addr().(*net.TCPAddr).Port
	realRun := m.Run
	calls := 0
	m.Run = func(ctx context.Context, name string, args ...string) (string, error) {
		calls++
		if name != "/usr/sbin/lsof" || !reflect.DeepEqual(args, []string{"-nP", "-a", "-iTCP:" + strconv.Itoa(port), "-sTCP:LISTEN", "+c0", "-F0pcun"}) {
			t.Fatal("inspection requested more than listener metadata", name, args)
		}
		return realRun(ctx, name, args...)
	}
	r, err := m.InspectPort(context.Background(), port)
	if err != nil || r.Status != "In use" || calls != 1 {
		t.Fatal(r, err)
	}
	found := false
	for _, listener := range r.Listeners {
		if listener.PID == os.Getpid() && listener.UID == os.Getuid() && listener.Name != "" {
			found = true
		}
	}
	if !found {
		t.Fatal("actual listener process was not reported", r)
	}
	if err = l.Close(); err != nil {
		t.Fatal(err)
	}
	r, err = m.InspectPort(context.Background(), port)
	if err != nil || r.Status != "Available" || calls != 1 {
		t.Fatal("available port trusted process lookup instead of bind", r, err)
	}
}
func TestInspectPortKeepsOccupiedStatusWhenOwnerCannotBeObserved(t *testing.T) {
	m := testManager(t)
	l, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer l.Close()
	m.Run = func(context.Context, string, ...string) (string, error) { return "", errors.New("lookup unavailable") }
	r, err := m.InspectPort(context.Background(), l.Addr().(*net.TCPAddr).Port)
	if err != nil || r.Status != "In use" || len(r.Listeners) != 0 || !strings.Contains(r.Detail, "unavailable") {
		t.Fatal("unobserved owner was inferred", r, err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if _, err = m.InspectPort(ctx, r.Port); !errors.Is(err, context.Canceled) {
		t.Fatal(err)
	}
}
func TestPortListenerMachineFieldsDoNotConfuseNamesUIDsAndEndpoints(t *testing.T) {
	got := parsePortListeners("p123\x00cserver\nname\x00u0\x00\nf9\x00n*:8443\x00\nf10\x00n*:8443\x00\np456\x00c开发服务\x00u501\x00\nf7\x00n[::1]:8443\x00\n")
	if len(got) != 2 || got[0].UID != 0 || got[0].Name != "server name" || got[1].PID != 456 || got[1].Address != "[::1]:8443" {
		t.Fatal(got)
	}
	if result := parsePortListeners("pbad\x00cunknown\x00n*:443\x00"); len(result) != 0 {
		t.Fatal("invalid process ID accepted", result)
	}
}
func TestSocketProxyConflictFailsBeforeStoppingOrAuthorizing(t *testing.T) {
	m := testManager(t)
	l, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer l.Close()
	m.Run = func(_ context.Context, name string, args ...string) (string, error) {
		if filepath.Base(name) == "launchctl" && len(args) > 0 && args[0] == "print" {
			return "", errors.New("Could not find service")
		}
		t.Fatal("conflict modified a process", name, args)
		return "", nil
	}
	before := []byte("last-good-routes")
	path := filepath.Join(m.Root, "proxy", "rules.json")
	if err = os.WriteFile(path, before, 0600); err != nil {
		t.Fatal(err)
	}
	site := proxyRoute{Domain: "conflict.devbase.localhost", LocalPort: 3000, Port: l.Addr().(*net.TCPAddr).Port}
	err = m.startSocketProxy(context.Background(), []proxyRoute{site}, nil)
	var conflict *PortUnavailableError
	if !errors.As(err, &conflict) || conflict.Port != site.Port {
		t.Fatal(err)
	}
	after, err := os.ReadFile(path)
	if err != nil || string(after) != string(before) {
		t.Fatal("preflight damaged prior routing", err)
	}
}

func TestSuggestedPortIsAvailableWithoutChangingStoredState(t *testing.T) {
	m := testManager(t)
	if err := m.withState(context.Background(), func(s *State) error { s.Instances["reserved"] = Instance{Port: 5432}; return nil }); err != nil {
		t.Fatal(err)
	}
	before, _ := os.ReadFile(filepath.Join(m.Root, "state.json"))
	port, err := m.AvailableLocalPort(context.Background())
	if err != nil || port < 1024 || port == 5432 {
		t.Fatal(port, err)
	}
	l, err := net.Listen("tcp4", "127.0.0.1:"+strconv.Itoa(port))
	if err != nil {
		t.Fatal("suggestion not free", err)
	}
	l.Close()
	after, _ := os.ReadFile(filepath.Join(m.Root, "state.json"))
	if string(after) != string(before) {
		t.Fatal("suggesting a port rewrote settings")
	}
}

func TestInspectPrivilegedPortAvailableAndOccupiedViaConnectProbe(t *testing.T) {
	m := testManager(t)
	m.listenLoopback = func(port int) (io.Closer, error) {
		if port != 443 {
			t.Fatalf("unexpected port %d", port)
		}
		return nil, os.ErrPermission
	}

	t.Run("free", func(t *testing.T) {
		m.dialLoopback = func(context.Context, int) (io.Closer, error) {
			return nil, syscall.ECONNREFUSED
		}
		m.Run = func(context.Context, string, ...string) (string, error) {
			t.Fatal("available privileged port should not run listener lookups")
			return "", nil
		}
		r, err := m.InspectPort(context.Background(), 443)
		if err != nil || r.Status != "Available" || len(r.Listeners) != 0 {
			t.Fatalf("expected available port 443, got %+v err=%v", r, err)
		}
	})

	t.Run("occupied-by-root-daemon", func(t *testing.T) {
		m.dialLoopback = func(context.Context, int) (io.Closer, error) {
			client, server := net.Pipe()
			_ = server.Close()
			return client, nil
		}
		var calls []string
		m.Run = func(_ context.Context, name string, args ...string) (string, error) {
			calls = append(calls, name)
			switch name {
			case "/usr/sbin/lsof":
				return "", errors.New("exit status 1")
			case "/bin/launchctl":
				return "", errors.New("Could not find service") // the app's proxy job is not loaded
			case "/usr/sbin/netstat":
				return strings.Join([]string{
					"Active Internet connections (including servers)",
					"Proto Recv-Q Send-Q  Local Address          Foreign Address        (state)          rxbytes      txbytes  rhiwat  shiwat          process:pid    state  options           gencnt    flags   flags1 usecnt rtncnt fltrs",
					"tcp4       0      0  127.0.0.1.443          *.*                    LISTEN                 0            0  131072  131072          launchd:1      00180 00000006 0000000000000ddc 00000000 00000800      1      0 000000",
					"tcp4       0      0  *.443                  *.*                    LISTEN                 0            0  131072  131072 Google Chrome fo:94161  00100 00000006 0000000000686ff8 00000000 00000800      1      0 000000",
					"tcp4       0      0  127.0.0.1.8443         *.*                    LISTEN                 0            0  131072  131072           caddy:777    00100 00000006 0000000000686ff9 00000000 00000800      1      0 000000",
				}, "\n"), nil
			default:
				t.Fatalf("unexpected lookup command: %s %v", name, args)
				return "", nil
			}
		}
		r, err := m.InspectPort(context.Background(), 443)
		if err != nil || r.Status != "In use" {
			t.Fatalf("expected occupied port 443, got %+v err=%v", r, err)
		}
		if !reflect.DeepEqual(calls, []string{"/usr/sbin/lsof", "/usr/sbin/netstat", "/bin/launchctl"}) {
			t.Fatalf("unexpected lookup sequence: %v", calls)
		}
		want := []PortListener{
			{PID: 1, UID: -1, Name: "launchd", Address: "127.0.0.1:443"},
			{PID: 94161, UID: -1, Name: "Google Chrome fo", Address: "*:443"},
		}
		if !reflect.DeepEqual(r.Listeners, want) {
			t.Fatalf("got listeners %+v, want %+v", r.Listeners, want)
		}
	})

	t.Run("probe-timeout-keeps-unknown", func(t *testing.T) {
		m.dialLoopback = func(context.Context, int) (io.Closer, error) {
			return nil, context.DeadlineExceeded
		}
		m.Run = func(context.Context, string, ...string) (string, error) {
			t.Fatal("timed-out probe should not run listener lookups")
			return "", nil
		}
		r, err := m.InspectPort(context.Background(), 443)
		if err != nil || r.Status != "Unknown" {
			t.Fatalf("expected unknown port 443 on probe timeout, got %+v err=%v", r, err)
		}
	})
}

func TestPrivilegedSocketProxyPreflightDetectsOccupiedPortBeforeAuthorizing(t *testing.T) {
	m := testManager(t)
	m.listenLoopback = func(int) (io.Closer, error) {
		return nil, syscall.EACCES
	}
	m.Run = func(_ context.Context, name string, args ...string) (string, error) {
		if filepath.Base(name) == "launchctl" && len(args) > 0 && args[0] == "print" {
			return "", errors.New("Could not find service")
		}
		t.Fatal("conflict modified a process", name, args)
		return "", nil
	}
	before := []byte("last-good-routes")
	path := filepath.Join(m.Root, "proxy", "rules.json")
	if err := os.WriteFile(path, before, 0600); err != nil {
		t.Fatal(err)
	}
	m.dialLoopback = func(context.Context, int) (io.Closer, error) {
		client, server := net.Pipe()
		_ = server.Close()
		return client, nil
	}
	err := m.startSocketProxy(context.Background(), []proxyRoute{{Domain: "conflict.devbase.localhost", LocalPort: 3000, Port: 443}}, nil)
	var conflict *PortUnavailableError
	if !errors.As(err, &conflict) || conflict.Port != 443 {
		t.Fatalf("expected PortUnavailableError for 443, got %v", err)
	}
	after, err := os.ReadFile(path)
	if err != nil || string(after) != string(before) {
		t.Fatalf("preflight damaged prior routing: %v", err)
	}

	m.dialLoopback = func(context.Context, int) (io.Closer, error) {
		return nil, syscall.ECONNREFUSED
	}
	if err := m.preflightProxyPorts(context.Background(), []proxyRoute{{Domain: "ok.devbase.localhost", LocalPort: 3000, Port: 443}}, nil, false); err != nil {
		t.Fatalf("expected free privileged port preflight to pass, got %v", err)
	}
}

func TestInspectPortRecognizesOwnProxyListener(t *testing.T) {
	for _, tc := range []struct {
		name      string
		savedPort func(port int) int
		jobPID    func() int
		own       bool
	}{
		{"own job process", func(p int) int { return p }, os.Getpid, true},
		{"another process", func(p int) int { return p }, func() int { return os.Getpid() + 100000 }, false},
		{"port is not the saved proxy port", func(p int) int { return p + 1 }, os.Getpid, false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			m := testManager(t)
			l, err := net.Listen("tcp4", "127.0.0.1:0")
			if err != nil {
				t.Fatal(err)
			}
			defer l.Close()
			port := l.Addr().(*net.TCPAddr).Port
			if err = m.withState(context.Background(), func(s *State) error {
				s.ProxyListenerPort = tc.savedPort(port)
				return nil
			}); err != nil {
				t.Fatal(err)
			}
			realRun := m.Run
			m.Run = func(ctx context.Context, name string, args ...string) (string, error) {
				if name == "/bin/launchctl" {
					return "{\n\tstate = running\n\tpid = " + strconv.Itoa(tc.jobPID()) + "\n}", nil
				}
				return realRun(ctx, name, args...)
			}
			r, err := m.InspectPort(context.Background(), port)
			if err != nil {
				t.Fatal(err)
			}
			if tc.own && r.Status != "Devbase proxy" {
				t.Fatal("own proxy was reported as foreign", r)
			}
			if !tc.own && r.Status != "In use" {
				t.Fatal("foreign listener was reported as ours", r)
			}
		})
	}
}

func TestParsePortListenersIgnoresLeadingLsofWarnings(t *testing.T) {
	raw := "lsof: WARNING: can't stat() smbfs file system /Volumes/.timemachine/host/share\n      Output information may be incomplete.\n      assuming \"dev=36000006\" from mount table\np866\x00cDevbase\x00u501\x00\nf8\x00n127.0.0.1:443\x00\n"
	got := parsePortListeners(raw)
	want := []PortListener{{PID: 866, UID: 501, Name: "Devbase", Address: "127.0.0.1:443"}}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("got %+v, want %+v", got, want)
	}
	if empty := parsePortListeners("lsof: WARNING: only warning\n"); len(empty) != 0 {
		t.Fatalf("expected no listeners from warning-only output, got %+v", empty)
	}
}

func TestListeningUserPortsFiltersDeduplicatesAndExcludes(t *testing.T) {
	uid := os.Getuid()
	uidStr := strconv.Itoa(uid)

	t.Run("mock-filter-dedup-sort", func(t *testing.T) {
		m := testManager(t)
		if err := m.withState(context.Background(), func(s *State) error {
			s.ProxyListenerPort = 8443
			s.Instances["pg"] = Instance{ID: "pg", Kind: "postgresql", Package: "postgresql@17", Port: 5432}
			return nil
		}); err != nil {
			t.Fatal(err)
		}
		m.Run = func(_ context.Context, name string, args ...string) (string, error) {
			wantArgs := []string{"-w", "-nP", "-a", "-u", uidStr, "-iTCP", "-sTCP:LISTEN", "+c0", "-F0pcun"}
			if name != "/usr/sbin/lsof" || !reflect.DeepEqual(args, wantArgs) {
				t.Fatalf("unexpected lsof call: %s %v", name, args)
			}
			return strings.Join([]string{
				"p101\x00crootd\x00u0\x00\nf4\x00n127.0.0.1:9000\x00",
				"p202\x00cpriv\x00u" + uidStr + "\x00\nf5\x00n127.0.0.1:443\x00",
				"p203\x00cproxy\x00u" + uidStr + "\x00\nf6\x00n127.0.0.1:8443\x00",
				"p204\x00cpostgres\x00u" + uidStr + "\x00\nf7\x00n127.0.0.1:5432\x00",
				"p301\x00cvite\x00u" + uidStr + "\x00\nf8\x00n127.0.0.1:5173\x00\nf9\x00n[::1]:5173\x00",
				"p302\x00cnode\x00u" + uidStr + "\x00\nf10\x00n*:3000\x00",
			}, "\n") + "\n", nil
		}
		got, err := m.ListeningUserPorts(context.Background())
		if err != nil {
			t.Fatal(err)
		}
		want := []ListeningPort{
			{Port: 3000, Process: "node"},
			{Port: 5173, Process: "vite"},
		}
		if !reflect.DeepEqual(got, want) {
			t.Fatalf("got %+v, want %+v", got, want)
		}
	})

	t.Run("empty-exit-1", func(t *testing.T) {
		m := testManager(t)
		m.Run = func(context.Context, string, ...string) (string, error) {
			return "", errors.New("exit status 1")
		}
		got, err := m.ListeningUserPorts(context.Background())
		if err != nil || len(got) != 0 {
			t.Fatalf("expected empty slice and nil err on exit status 1, got %+v err=%v", got, err)
		}
	})

	t.Run("real-listener", func(t *testing.T) {
		m := testManager(t)
		l, err := net.Listen("tcp4", "127.0.0.1:0")
		if err != nil {
			t.Fatal(err)
		}
		defer l.Close()
		port := l.Addr().(*net.TCPAddr).Port
		got, err := m.ListeningUserPorts(context.Background())
		if err != nil {
			t.Fatal(err)
		}
		found := false
		for _, p := range got {
			if p.Port == port && p.Process != "" {
				found = true
			}
		}
		if !found {
			t.Fatalf("real listener on port %d missing from %+v", port, got)
		}
	})
}

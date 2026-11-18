package devcli

import (
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"slices"
	"strings"

	"devbase/internal/brew"
	"devbase/internal/devplatform"
	"devbase/internal/launchd"
	"devbase/internal/loginitem"
	"devbase/internal/privhelper"
	"devbase/internal/service"
	"devbase/internal/system"
)

// CLIVersion is the Devbase CLI version reported by version and schema commands.
const CLIVersion = "0.1.0"

// UsageError marks invalid flags, missing arguments, or safety guard rejections (exit code 2).
type UsageError struct {
	Err error
}

func (e *UsageError) Error() string { return e.Err.Error() }
func (e *UsageError) Unwrap() error { return e.Err }
func (e *UsageError) ExitCode() int { return 2 }

func usageErrorf(format string, args ...any) error {
	return &UsageError{Err: fmt.Errorf(format, args...)}
}

// Env holds injectable runtime dependencies for public CLI execution and isolated tests.
type Env struct {
	In               io.Reader
	Out              io.Writer
	Err              io.Writer
	JSON             bool
	ManagerFactory   func() *devplatform.Manager
	ServiceProviders func() map[service.Source]service.Provider
	LoginAdder       interface{ Add(string) error }
	HelperClient     func() *privhelper.Client
	LoginAgentDir    string
	UserAgentDir     string
	CLIBinDir        string
	AppPath          string
	LoginProgram     string
	LoginRunner      func(...string) error
}

// DefaultEnv constructs the production CLI runtime environment.
func DefaultEnv(in io.Reader, out, errOut io.Writer) *Env {
	if in == nil {
		in = strings.NewReader("")
	}
	if out == nil {
		out = io.Discard
	}
	if errOut == nil {
		errOut = io.Discard
	}
	login := loginitem.New()
	loginDir := launchd.UserAgents().Dir
	userAgentDir := launchd.UserAgents().Dir
	if envHome := os.Getenv("MSM_DEV_HOME"); envHome != "" {
		loginDir = filepath.Join(envHome, "login-agents")
		userAgentDir = filepath.Join(envHome, "user-agents")
	}
	return &Env{
		In:  in,
		Out: out,
		Err: errOut,
		ManagerFactory: func() *devplatform.Manager {
			m := devplatform.New(devplatform.DefaultRoot())
			m.Event = func(msg string) {
				if strings.TrimSpace(msg) != "" {
					fmt.Fprintln(errOut, msg)
				}
			}
			return m
		},
		ServiceProviders: func() map[service.Source]service.Provider {
			return map[service.Source]service.Provider{
				service.Brew:        brew.New(),
				service.LoginItem:   login,
				service.UserAgent:   launchd.UserAgents(),
				service.SystemAgent: launchd.SystemAgents(),
				service.Daemon:      launchd.Daemons(),
			}
		},
		LoginAdder:    login,
		HelperClient:  privhelper.Default,
		LoginAgentDir: loginDir,
		UserAgentDir:  userAgentDir,
		CLIBinDir:     DefaultCLIBinDir(),
		LoginRunner: func(args ...string) error {
			_, err := system.Run("launchctl", args...)
			return err
		},
	}
}

func (e *Env) manager() *devplatform.Manager {
	if e.ManagerFactory != nil {
		return e.ManagerFactory()
	}
	m := devplatform.New(devplatform.DefaultRoot())
	m.Event = func(msg string) {
		if strings.TrimSpace(msg) != "" && e.Err != nil {
			fmt.Fprintln(e.Err, msg)
		}
	}
	return m
}

func (e *Env) providers() map[service.Source]service.Provider {
	if e.ServiceProviders != nil {
		return e.ServiceProviders()
	}
	return map[service.Source]service.Provider{}
}

func (e *Env) helper() *privhelper.Client {
	if e.HelperClient != nil {
		return e.HelperClient()
	}
	return privhelper.Default()
}

func rejectSecretFlags(args []string) error {
	for _, a := range args {
		if a == "--" {
			break
		}
		switch {
		case a == "--password" || strings.HasPrefix(a, "--password="):
			return usageErrorf("passing secrets in command-line arguments is forbidden; pipe the password on stdin and use --password-stdin")
		case a == "--token" || strings.HasPrefix(a, "--token="):
			return usageErrorf("passing secrets in command-line arguments is forbidden; pipe the token on stdin and use --token-stdin")
		case a == "--api-token" || strings.HasPrefix(a, "--api-token="):
			return usageErrorf("passing secrets in command-line arguments is forbidden; pipe the API token on stdin and use --token-stdin")
		}
	}
	return nil
}

func stripGlobalFlags(args []string) ([]string, bool, error) {
	if err := rejectSecretFlags(args); err != nil {
		return nil, false, err
	}
	cleaned := make([]string, 0, len(args))
	jsonOut := false
	afterDoubleDash := false
	for _, a := range args {
		if a == "--" {
			afterDoubleDash = true
			cleaned = append(cleaned, a)
			continue
		}
		if !afterDoubleDash {
			if a == "--json" || a == "--json=true" {
				jsonOut = true
				continue
			}
			if a == "--json=false" {
				jsonOut = false
				continue
			}
		}
		cleaned = append(cleaned, a)
	}
	return cleaned, jsonOut, nil
}

func readSecretStdin(r io.Reader, label string) (string, error) {
	if r == nil {
		return "", usageErrorf("%s must be provided on stdin", label)
	}
	b, err := io.ReadAll(io.LimitReader(r, 8193))
	if err != nil {
		return "", err
	}
	if len(b) > 8192 {
		return "", usageErrorf("%s on stdin exceeds 8192 bytes", label)
	}
	secret := strings.TrimRight(string(b), "\r\n")
	if strings.TrimSpace(secret) == "" {
		return "", usageErrorf("%s on stdin cannot be empty", label)
	}
	return secret, nil
}

func readJSONStdin(r io.Reader, limit int64) ([]byte, error) {
	if r == nil {
		return nil, usageErrorf("JSON input must be provided on stdin")
	}
	b, err := io.ReadAll(io.LimitReader(r, limit+1))
	if err != nil {
		return nil, err
	}
	if int64(len(b)) > limit {
		return nil, usageErrorf("JSON stdin exceeds %d bytes", limit)
	}
	if len(strings.TrimSpace(string(b))) == 0 {
		return nil, usageErrorf("JSON stdin cannot be empty")
	}
	return b, nil
}

func writeJSONValue(w io.Writer, v any) error {
	enc := json.NewEncoder(w)
	enc.SetIndent("", "  ")
	enc.SetEscapeHTML(false)
	return enc.Encode(v)
}

func writeResult(e *Env, payload any, human func(io.Writer)) error {
	if e.JSON {
		return writeJSONValue(e.Out, payload)
	}
	if human != nil {
		human(e.Out)
	}
	return nil
}

func writeActionError(e *Env, err error) {
	if err == nil || e.Err == nil {
		return
	}
	code := 1
	var ec interface{ ExitCode() int }
	if errors.As(err, &ec) {
		code = ec.ExitCode()
	}
	if e.JSON {
		_ = writeJSONValue(e.Err, map[string]any{
			"error":    err.Error(),
			"exitCode": code,
		})
		return
	}
	fmt.Fprintf(e.Err, "Error: %s\n", err.Error())
}

func newFlagSet(name string) *flag.FlagSet {
	fs := flag.NewFlagSet(name, flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	return fs
}

type boolFlag interface {
	IsBoolFlag() bool
}

// parseFlags allows flags to appear before or after positional arguments while preserving "--".
func parseFlags(fs *flag.FlagSet, args []string) ([]string, error) {
	boolNames := map[string]bool{}
	fs.VisitAll(func(f *flag.Flag) {
		if bf, ok := f.Value.(boolFlag); ok && bf.IsBoolFlag() {
			boolNames[f.Name] = true
		}
	})
	var flagArgs []string
	var positionals []string
	for i := 0; i < len(args); i++ {
		a := args[i]
		if a == "--" {
			positionals = append(positionals, args[i+1:]...)
			break
		}
		if strings.HasPrefix(a, "-") && a != "-" {
			name := strings.TrimPrefix(a, "-")
			name = strings.TrimPrefix(name, "-")
			if eq := strings.IndexByte(name, '='); eq >= 0 {
				flagArgs = append(flagArgs, a)
				continue
			}
			flagArgs = append(flagArgs, a)
			if name == "h" || name == "help" || boolNames[name] {
				continue
			}
			if fs.Lookup(name) != nil {
				if i+1 >= len(args) {
					return nil, usageErrorf("flag needs an argument: %s", a)
				}
				flagArgs = append(flagArgs, args[i+1])
				i++
			}
			continue
		}
		positionals = append(positionals, a)
	}
	if err := fs.Parse(flagArgs); err != nil {
		if errors.Is(err, flag.ErrHelp) {
			return nil, flag.ErrHelp
		}
		return nil, &UsageError{Err: err}
	}
	positionals = append(positionals, fs.Args()...)
	return positionals, nil
}

type stringSliceFlag []string

func (s *stringSliceFlag) String() string { return strings.Join(*s, ", ") }
func (s *stringSliceFlag) Set(v string) error {
	*s = append(*s, v)
	return nil
}

func hasHelpFlag(args []string) bool {
	for _, a := range args {
		if a == "--" {
			return false
		}
		if a == "--help" || a == "-h" {
			return true
		}
	}
	return false
}

func sortedMapKeys[T any](m map[string]T) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	slices.Sort(keys)
	return keys
}

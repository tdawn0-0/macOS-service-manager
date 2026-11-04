package launchd

import (
	"bytes"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strconv"
	"strings"

	"howett.net/plist"

	"macos-service-manager/internal/service"
	"macos-service-manager/internal/system"
)

// Spec is the part of a launch agent's property list that the editor
// changes. The keys it does not know stay as they were.
type Spec struct {
	Label string
	// Existing says the job has a file already, whose label and name stay as
	// they are, even if a new job could not have them.
	Existing bool
	// Args is ProgramArguments: the program, then its arguments.
	Args       []string
	WorkingDir string
	RunAtLoad  bool
	KeepAlive  bool
	// KeepAliveCustom says the file has KeepAlive conditions, which the
	// editor leaves alone; KeepAlive is true for them.
	KeepAliveCustom bool
	// RestartOnCrash starts the job again when it exits with a failure, for
	// which launchd runs it at load as well.
	RestartOnCrash bool
	// StartInterval runs the job every this many seconds; 0 is never.
	StartInterval int
	// Calendar runs the job at the times it says.
	Calendar Calendar
	// CalendarCustom says the file has a calendar that a Calendar cannot say,
	// which the editor leaves alone.
	CalendarCustom bool
	// StartOnMount runs the job whenever a filesystem is mounted.
	StartOnMount bool
	// QueueDirs keeps the job alive while any of these directories has files.
	QueueDirs []string
	// UserName, GroupName and RootDir say who the job runs as, and where it is
	// chrooted to. launchd only applies them in the system domain: to daemons.
	UserName, GroupName, RootDir string
	Env                          map[string]string
	StdOut                       string
	StdErr                       string
}

var labelRE = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9._-]*$`)

// Validate checks that the spec makes a launch agent that launchd accepts.
func (s Spec) Validate() error {
	if !labelRE.MatchString(s.Label) && (!s.Existing || s.Label == "") {
		return fmt.Errorf("the label may hold letters, digits, dots, dashes and underscores, as com.example.job")
	}
	if len(s.Args) == 0 {
		return fmt.Errorf("give the program to run")
	}
	// launchd looks in its PATH for a program without a path, which a file that
	// works may rely on. What this form writes says where the program is.
	if !filepath.IsAbs(s.Args[0]) && !s.Existing {
		return fmt.Errorf("the program %q must be an absolute path", s.Args[0])
	}
	if s.GroupName != "" && s.UserName == "" {
		return fmt.Errorf("a group needs a user to run as")
	}
	for name, path := range map[string]string{"working directory": s.WorkingDir, "standard output": s.StdOut, "standard error": s.StdErr, "root directory": s.RootDir} {
		if path != "" && !filepath.IsAbs(path) {
			return fmt.Errorf("the %s %q must be an absolute path", name, path)
		}
	}
	for _, dir := range s.QueueDirs {
		if !filepath.IsAbs(dir) {
			return fmt.Errorf("the queue directory %q must be an absolute path", dir)
		}
	}
	if s.StartInterval < 0 {
		return fmt.Errorf("the interval cannot be negative")
	}
	if _, err := s.Calendar.Entries(); err != nil {
		return err
	}
	for k := range s.Env {
		if k == "" || strings.Contains(k, "=") {
			return fmt.Errorf("the environment variable name %q is not valid", k)
		}
	}
	return nil
}

// LoadSpec reads the editable part of the property list at path.
func LoadSpec(path string) (Spec, error) {
	f, err := readFile(path)
	if err != nil {
		return Spec{}, err
	}
	s := Spec{Label: f.Label, Existing: true, Args: f.ProgramArguments, Env: f.Environment}
	// launchd runs Program, if there is one, whatever the first of the arguments
	// says: what its process calls itself.
	if f.Program != nil {
		s.Args = append([]string{*f.Program}, s.Args[min(1, len(s.Args)):]...)
	}
	if f.WorkingDirectory != nil {
		s.WorkingDir = *f.WorkingDirectory
	}
	if f.RunAtLoad != nil {
		s.RunAtLoad = *f.RunAtLoad
	}
	switch k := f.KeepAlive.(type) {
	case bool:
		s.KeepAlive = k
	case nil:
	case map[string]any:
		if success, ok := k["SuccessfulExit"].(bool); ok && !success && len(k) == 1 {
			s.RestartOnCrash = true
		} else {
			s.KeepAlive, s.KeepAliveCustom = true, true
		}
	default:
		s.KeepAlive, s.KeepAliveCustom = true, true
	}
	if f.StartInterval != nil {
		s.StartInterval = *f.StartInterval
	}
	if f.StartCalendarInterval != nil {
		// The form holds one way of being scheduled: with an interval as well,
		// the calendar stays as it is.
		if cal, ok := calendarFrom(f.StartCalendarInterval); ok && f.StartInterval == nil {
			s.Calendar = cal
		} else {
			s.CalendarCustom = true
		}
	}
	if f.StartOnMount != nil {
		s.StartOnMount = *f.StartOnMount
	}
	s.QueueDirs = f.QueueDirectories
	if f.UserName != nil {
		s.UserName = *f.UserName
	}
	if f.GroupName != nil {
		s.GroupName = *f.GroupName
	}
	if f.RootDirectory != nil {
		s.RootDir = *f.RootDirectory
	}
	if f.StandardOutPath != nil {
		s.StdOut = *f.StandardOutPath
	}
	if f.StandardErrorPath != nil {
		s.StdErr = *f.StandardErrorPath
	}
	return s, nil
}

// Marshal writes the spec into a property list. Given the current file, it
// keeps the keys the spec does not hold.
func (s Spec) Marshal(current []byte) ([]byte, error) {
	dict := map[string]any{}
	if current != nil {
		if _, err := plist.Unmarshal(current, &dict); err != nil {
			return nil, fmt.Errorf("the current file is not a property list: %w", err)
		}
	}
	set := func(key string, ok bool, v any) {
		if ok {
			dict[key] = v
		} else {
			delete(dict, key)
		}
	}
	// False is what launchd takes a missing key for: a file only says so if it
	// did already.
	setBool := func(key string, on bool) {
		if _, had := dict[key]; on || had {
			dict[key] = on
		}
	}
	dict["Label"] = s.Label
	setProgram(dict, s.Args)
	setBool("RunAtLoad", s.RunAtLoad || s.RestartOnCrash)
	switch {
	case s.KeepAliveCustom:
	case s.RestartOnCrash && !s.KeepAlive:
		dict["KeepAlive"] = map[string]bool{"SuccessfulExit": false}
	default:
		setBool("KeepAlive", s.KeepAlive)
	}
	if !s.CalendarCustom {
		entries, err := s.Calendar.Entries()
		if err != nil {
			return nil, err
		}
		switch len(entries) {
		case 0:
			delete(dict, "StartCalendarInterval")
		case 1:
			dict["StartCalendarInterval"] = entries[0]
		default:
			dict["StartCalendarInterval"] = entries
		}
	}
	setBool("StartOnMount", s.StartOnMount)
	set("QueueDirectories", len(s.QueueDirs) > 0, s.QueueDirs)
	set("UserName", s.UserName != "", s.UserName)
	set("GroupName", s.GroupName != "", s.GroupName)
	set("RootDirectory", s.RootDir != "", s.RootDir)
	set("WorkingDirectory", s.WorkingDir != "", s.WorkingDir)
	set("StartInterval", s.StartInterval > 0, s.StartInterval)
	set("EnvironmentVariables", len(s.Env) > 0, s.Env)
	set("StandardOutPath", s.StdOut != "", s.StdOut)
	set("StandardErrorPath", s.StdErr != "", s.StdErr)
	var buf bytes.Buffer
	enc := plist.NewEncoderForFormat(&buf, plist.XMLFormat)
	enc.Indent("\t")
	if err := enc.Encode(dict); err != nil {
		return nil, err
	}
	return buf.Bytes(), nil
}

// setProgram writes the program and its arguments. A file with a Program runs it
// whatever ProgramArguments begins with, so it has to change with the program,
// and what it says already stays: the first argument can differ from the path, and
// a Program alone says as much as both keys would.
func setProgram(dict map[string]any, args []string) {
	program, ok := dict["Program"].(string)
	if !ok || len(args) == 0 {
		dict["ProgramArguments"] = args
		return
	}
	current, _ := dict["ProgramArguments"].([]any)
	if len(current) == 0 && len(args) == 1 && args[0] == program {
		delete(dict, "ProgramArguments")
		return
	}
	written := slices.Clone(args)
	if args[0] == program && len(current) > 0 {
		if first, ok := current[0].(string); ok {
			written[0] = first
		}
	}
	dict["Program"] = args[0]
	dict["ProgramArguments"] = written
}

// Changes lists the keys of the property list that saving the spec changes.
func (s Spec) Changes(current []byte) ([]string, error) {
	data, err := s.Marshal(current)
	if err != nil {
		return nil, err
	}
	var before, after map[string]any
	if _, err := plist.Unmarshal(current, &before); err != nil {
		return nil, err
	}
	if _, err := plist.Unmarshal(data, &after); err != nil {
		return nil, err
	}
	keys := make([]string, 0, len(after))
	for k := range before {
		keys = append(keys, k)
	}
	for k := range after {
		if _, ok := before[k]; !ok {
			keys = append(keys, k)
		}
	}
	slices.Sort(keys)
	return slices.DeleteFunc(keys, func(k string) bool {
		was, had := before[k]
		is, has := after[k]
		return had && has && sameValue(was, is)
	}), nil
}

// sameValue compares values as a property list writes them, whichever Go types
// they were read into.
func sameValue(a, b any) bool {
	x, err := plist.Marshal(a, plist.XMLFormat)
	if err != nil {
		return false
	}
	y, err := plist.Marshal(b, plist.XMLFormat)
	return err == nil && bytes.Equal(x, y)
}

// Save writes the spec as a launch agent in dir and returns its path. To
// edit the file at existing, pass its path; to make a new one, "", which
// fails if the label is taken.
func Save(dir string, s Spec, existing string) (string, error) {
	s.Existing = existing != ""
	if err := s.Validate(); err != nil {
		return "", err
	}
	path := existing
	var current []byte
	if existing == "" {
		path = filepath.Join(dir, s.Label+".plist")
		if _, err := os.Stat(path); err == nil {
			return "", fmt.Errorf("%s already exists", filepath.Base(path))
		}
	} else {
		var err error
		if current, err = os.ReadFile(existing); err != nil {
			return "", err
		}
	}
	data, err := s.Marshal(current)
	if err != nil {
		return "", err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return "", err
	}
	tmp, err := writeChecked(filepath.Dir(path), data)
	if err != nil {
		return "", err
	}
	defer os.Remove(tmp)
	return path, os.Rename(tmp, path)
}

// writeChecked writes a property list to a new file in dir, for all to read,
// if plutil finds it valid, and returns its path.
func writeChecked(dir string, data []byte) (string, error) {
	tmp, err := os.CreateTemp(dir, ".msm-*.plist")
	if err != nil {
		return "", err
	}
	if _, err := tmp.Write(data); err != nil {
		tmp.Close()
		os.Remove(tmp.Name())
		return "", err
	}
	if err := tmp.Close(); err != nil {
		os.Remove(tmp.Name())
		return "", err
	}
	if _, err := system.Run("plutil", "-lint", tmp.Name()); err != nil {
		os.Remove(tmp.Name())
		return "", fmt.Errorf("the property list is not valid: %w", err)
	}
	if err := os.Chmod(tmp.Name(), 0o644); err != nil {
		os.Remove(tmp.Name())
		return "", err
	}
	return tmp.Name(), nil
}

// SaveAsRoot writes a job into the provider's directory, which only root can
// write to, owned as launchd demands, and loads it if asked, under one
// authentication. To edit the file at existing, pass its path, and a loaded job
// loads again; to make a new one, "", which fails if the label is taken. An error
// with a path says the job was saved but did not load.
func (p *Provider) SaveAsRoot(s Spec, existing string, load bool) (string, error) {
	s.Existing = existing != ""
	if err := s.Validate(); err != nil {
		return "", err
	}
	path := existing
	var current []byte
	if existing == "" {
		path = filepath.Join(p.Dir, s.Label+".plist")
		if _, err := os.Stat(path); err == nil {
			return "", fmt.Errorf("%s already exists", filepath.Base(path))
		}
	} else {
		info, err := os.Lstat(existing)
		if err != nil {
			return "", err
		}
		// Installing over a link would replace the link, not what it points at.
		if !info.Mode().IsRegular() {
			return "", fmt.Errorf("%s is not a regular file", filepath.Base(existing))
		}
		if current, err = os.ReadFile(existing); err != nil {
			return "", err
		}
	}
	// launchd only says "Input/output error" of a user that does not exist.
	if s.UserName != "" {
		if _, err := system.Run("id", "-u", s.UserName); err != nil {
			return "", fmt.Errorf("there is no user %q", s.UserName)
		}
	}
	if s.GroupName != "" {
		if _, err := system.Run("dscl", ".", "-read", "/Groups/"+s.GroupName, "PrimaryGroupID"); err != nil {
			return "", fmt.Errorf("there is no group %q", s.GroupName)
		}
	}
	data, err := s.Marshal(current)
	if err != nil {
		return "", err
	}
	tmp, err := writeChecked(os.TempDir(), data)
	if err != nil {
		return "", err
	}
	defer os.Remove(tmp)
	// Agents load into the user's domain, for which the user needs no root.
	inScript := load && p.Elevated
	target := p.Domain + "/" + s.Label
	script := installScript(tmp, path, p.Domain, inScript)
	if existing != "" {
		script = replaceScript(tmp, path, p.Domain, target, inScript)
	}
	if _, err := system.RunAdmin("/bin/sh", "-c", script); err != nil {
		// Root may have saved it and failed to load it.
		if inScript {
			if saved, readErr := os.ReadFile(path); readErr == nil && bytes.Equal(saved, data) {
				return path, fmt.Errorf("saved, but it did not load: %w", err)
			}
		}
		return "", err
	}
	if load && !p.Elevated {
		if existing != "" {
			// Not being loaded is fine too.
			_ = p.launchctl("bootout", target)
		}
		if err := p.launchctl("bootstrap", p.Domain, path); err != nil {
			return path, fmt.Errorf("saved, but it did not load: %w", err)
		}
	}
	return path, nil
}

// installScript is what root runs for a new job: it never replaces a job that
// appeared since the check, and launchd refuses a file that root does not own.
func installScript(tmp, path, domain string, load bool) string {
	script := "[ ! -e " + system.ShellQuote(path) + " ]" +
		" && /bin/mkdir -p " + system.ShellQuote(filepath.Dir(path)) +
		" && /usr/bin/install -S -m 644 -o root -g wheel " + system.ShellQuote(tmp) + " " + system.ShellQuote(path)
	if load {
		script += " && /bin/launchctl bootstrap " + system.ShellQuote(domain) + " " + system.ShellQuote(path)
	}
	return script
}

// replaceScript is what root runs for an edit. The job stays as it is until the
// new file is in place, which replaces the old one all at once, and a loaded job
// keeps what it was loaded with until it loads again.
func replaceScript(tmp, path, domain, target string, load bool) string {
	script := "[ -f " + system.ShellQuote(path) + " ]" +
		" && /usr/bin/install -S -m 644 -o root -g wheel " + system.ShellQuote(tmp) + " " + system.ShellQuote(path)
	if load {
		script += " && { " + unloadScript(target) + " /bin/launchctl bootstrap " + system.ShellQuote(domain) + " " + system.ShellQuote(path) + "; }"
	}
	return script
}

// unloadScript unloads a job if launchd has it loaded, and ends the script if it
// cannot: what follows must not leave behind a job that still runs.
func unloadScript(target string) string {
	quoted := system.ShellQuote(target)
	return "if /bin/launchctl print " + quoted + " >/dev/null 2>&1; then /bin/launchctl bootout " + quoted + " || exit $?; fi;"
}

// removeAsRoot unloads a job that only root can remove and moves its file to the
// user's Trash, under one authentication. Root hands the file to the user, since
// only the user can reach their Trash, whichever access macOS gave this app. So
// the Trash keeps a copy and is no undo: the file left its directory for a
// temporary one, which is where Put Back would look, and it is no longer root's.
func (p *Provider) removeAsRoot(it service.Item) error {
	dir, err := os.MkdirTemp("", "msm-removed-")
	if err != nil {
		return err
	}
	moved := filepath.Join(dir, filepath.Base(it.Path))
	script := unloadScript(p.Domain+"/"+it.Name) +
		" /bin/mv " + system.ShellQuote(it.Path) + " " + system.ShellQuote(moved) +
		" && /usr/sbin/chown " + strconv.Itoa(os.Getuid()) + " " + system.ShellQuote(moved)
	if _, err := system.RunAdmin("/bin/sh", "-c", script); err != nil {
		os.Remove(dir)
		return err
	}
	if err := system.Trash(moved); err != nil {
		return fmt.Errorf("removed, but it is at %s and not in the Trash: %w", moved, err)
	}
	os.Remove(dir)
	return nil
}

// ShellArgs returns the arguments that run a command line with the shell.
func ShellArgs(command string) []string { return []string{"/bin/sh", "-c", command} }

// ShellCommand returns the command line of arguments that ShellArgs could have
// made.
func ShellCommand(args []string) (string, bool) {
	if len(args) == 3 && (args[0] == "/bin/sh" || args[0] == "sh") && args[1] == "-c" {
		return args[2], true
	}
	return "", false
}

// ParseList splits text at its commas into its trimmed, non-empty items.
func ParseList(text string) []string {
	var items []string
	for _, item := range strings.Split(text, ",") {
		if item = strings.TrimSpace(item); item != "" {
			items = append(items, item)
		}
	}
	return items
}

// ParseLines splits text into its trimmed, non-empty lines.
func ParseLines(text string) []string {
	var lines []string
	for _, l := range strings.Split(text, "\n") {
		if l = strings.TrimSpace(l); l != "" {
			lines = append(lines, l)
		}
	}
	return lines
}

// ParseEnv reads lines of KEY=value.
func ParseEnv(text string) (map[string]string, error) {
	var env map[string]string
	for _, l := range ParseLines(text) {
		k, v, ok := strings.Cut(l, "=")
		if !ok || strings.TrimSpace(k) == "" {
			return nil, fmt.Errorf("%q is not KEY=value", l)
		}
		if env == nil {
			env = map[string]string{}
		}
		env[strings.TrimSpace(k)] = strings.TrimSpace(v)
	}
	return env, nil
}

// FormatEnv writes env as lines of KEY=value, sorted.
func FormatEnv(env map[string]string) string {
	keys := make([]string, 0, len(env))
	for k := range env {
		keys = append(keys, k)
	}
	slices.Sort(keys)
	lines := make([]string, len(keys))
	for i, k := range keys {
		lines[i] = k + "=" + env[k]
	}
	return strings.Join(lines, "\n")
}

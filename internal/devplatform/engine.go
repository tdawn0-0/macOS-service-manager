package devplatform

import (
	"context"
	"crypto/sha256"
	"errors"
	"fmt"
	"macos-service-manager/internal/launchd"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"regexp"
	"sort"
	"strings"
	"syscall"
	"time"
)

func runCommand(ctx context.Context, name string, args ...string) (string, error) {
	ctx, cancel := context.WithTimeout(ctx, 5*time.Minute)
	defer cancel()
	c := exec.CommandContext(ctx, name, args...)
	b, e := c.CombinedOutput()
	if e != nil {
		return "", fmt.Errorf("%s: %w: %s", filepath.Base(name), e, strings.TrimSpace(string(b)))
	}
	return string(b), nil
}
func (m *Manager) label(id string) string {
	sum := sha256.Sum256([]byte(filepath.Clean(m.Root)))
	return fmt.Sprintf("xyz.jyeu.msm.dev.%x.%s", sum[:4], id)
}
func domain() string { return fmt.Sprintf("gui/%d", os.Getuid()) }
func (m *Manager) loaded(ctx context.Context, id string) bool {
	_, e := m.Run(ctx, "launchctl", "print", domain()+"/"+m.label(id))
	return e == nil
}

// A loaded, finite launchd marker records intent for this login session. This
// also gives database-only projects an owner without adding a supervisor.
func (m *Manager) projectActive(ctx context.Context, p Project) bool {
	if m.loaded(ctx, "project-"+p.ID) {
		return true
	}
	// Retain resources of jobs started by versions predating project markers.
	for _, svc := range p.Services {
		if m.loaded(ctx, "p-"+p.ID+"-"+svc.ID) {
			return true
		}
	}
	return false
}
func (m *Manager) startJob(ctx context.Context, id string, args []string, dir string, env map[string]string, restart bool) (bool, error) {
	return m.startJobPolicy(ctx, id, args, dir, env, restart, 0)
}
func (m *Manager) startJobPolicy(ctx context.Context, id string, args []string, dir string, env map[string]string, restart bool, throttle int) (bool, error) {
	if !regexp.MustCompile(`^[a-zA-Z0-9][a-zA-Z0-9._-]{0,199}$`).MatchString(id) {
		return false, errors.New("invalid job identifier")
	}
	if out, e := m.Run(ctx, "launchctl", "print", domain()+"/"+m.label(id)); e == nil {
		if strings.Contains(out, "state = running") {
			return false, nil
		}
		_, e = m.Run(ctx, "launchctl", "kickstart", domain()+"/"+m.label(id))
		return e == nil, e
	}
	path := filepath.Join(m.Root, "jobs", m.label(id)+".plist")
	existing := ""
	if _, e := os.Stat(path); e == nil {
		existing = path
	}
	// Reuse the existing plist validation and atomic job writer.
	spec := launchd.Spec{Label: m.label(id), Args: args, WorkingDir: dir, Env: env, StdOut: filepath.Join(m.Root, "logs", id+".log"), StdErr: filepath.Join(m.Root, "logs", id+".err.log"), RunAtLoad: true, KeepAlive: restart}
	if throttle > 0 {
		spec.ThrottleInterval = &throttle
	}
	path, e := launchd.Save(filepath.Join(m.Root, "jobs"), spec, existing)
	if e != nil {
		return false, e
	}
	if e = os.Chmod(path, 0600); e != nil {
		return false, e
	}
	if m.initialization != nil {
		if e = m.recordInitializationJob(id, path); e != nil {
			return false, e
		}
	}
	if e = m.recordDatabaseWorkJob(id, path); e != nil {
		return false, e
	}
	if e = m.recordCommandWorkJob(id, path); e != nil {
		return false, e
	}
	_, e = m.Run(ctx, "launchctl", "bootstrap", domain(), path)
	return e == nil, e
}
func (m *Manager) stopJob(ctx context.Context, id string) error {
	ctx, cancel := context.WithTimeout(ctx, 30*time.Second)
	defer cancel()
	if e := ctx.Err(); e != nil {
		return e
	}
	target := domain() + "/" + m.label(id)
	out, e := m.Run(ctx, "launchctl", "print", target)
	if e != nil {
		if ctx.Err() != nil {
			return ctx.Err()
		}
		return nil
	}
	pid := parseJobState(out).PID
	if _, e = m.Run(ctx, "launchctl", "bootout", target); e != nil {
		return e
	}
	for {
		_, printErr := m.Run(ctx, "launchctl", "print", target)
		processGone := pid == 0 || syscall.Kill(pid, 0) == syscall.ESRCH
		if printErr != nil && processGone {
			return nil
		}
		select {
		case <-ctx.Done():
			return fmt.Errorf("waiting for %s to stop: %w", id, ctx.Err())
		case <-time.After(100 * time.Millisecond):
		}
	}
}
func (m *Manager) serviceArgs(s *State, p Project, svc Service) ([]string, map[string]string, error) {
	args := append([]string{}, svc.Command...)
	env := map[string]string{"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": os.Getenv("HOME")}
	if svc.EnvFile != "" {
		path := svc.EnvFile
		if !filepath.IsAbs(path) {
			path = filepath.Join(p.Directory, path)
		}
		values, e := readEnvironment(path)
		if e != nil {
			return nil, nil, e
		}
		for k, v := range values {
			env[k] = v
		}
	}
	for k, v := range svc.Environment {
		env[k] = v
	}
	if svc.Package != "" {
		pkg, ok := s.Packages[svc.Package]
		if !ok {
			return nil, nil, fmt.Errorf("install package %s first", svc.Package)
		}
		name := args[0]
		bin, e := m.Binary(s, svc.Package, name)
		if e != nil {
			return nil, nil, e
		}
		args[0] = bin
		dirs := []string{}
		for _, rel := range pkg.Binaries {
			dirs = append(dirs, filepath.Dir(filepath.Join(m.packageDir(pkg), rel)))
		}
		sort.Strings(dirs)
		env["PATH"] = strings.Join(dirs, ":") + ":" + env["PATH"]
	} else if !filepath.IsAbs(args[0]) {
		return nil, nil, fmt.Errorf("select a managed runtime for %s or use an absolute executable path", svc.ID)
	}
	return args, env, nil
}
func (m *Manager) up(ctx context.Context, id string) error {
	return m.withProjectLifecycle(ctx, id, true, func(s *State) error { return m.upProject(ctx, s, id) })
}
func (m *Manager) upProject(ctx context.Context, s *State, id string) (err error) {
	p, ok := s.Projects[id]
	if !ok {
		return errors.New("project not found")
	}
	if e := p.Validate(); e != nil {
		return e
	}
	if e := projectFolderError(p.Directory); e != nil {
		return e
	}
	if e := p.requireLocalSettings(ctx); e != nil {
		return e
	}
	hasSite := false
	for _, site := range p.Sites {
		hasSite = hasSite || !site.Disabled
	}
	if len(p.Services)+len(p.Instances) == 0 && !hasSite {
		return errors.New("configure at least one service or database before starting the project")
	}
	ordered, e := serviceOrder(p.Services)
	if e != nil {
		return e
	}
	var started []string
	defer func() {
		if err != nil {
			cleanup, cancel := context.WithTimeout(context.Background(), 35*time.Second)
			defer cancel()
			for n := len(started) - 1; n >= 0; n-- {
				stopErr := m.stopJob(cleanup, started[n])
				if stopErr == nil {
					stopErr = m.verifyShutdownTarget(cleanup, domain()+"/"+m.label(started[n]))
				}
				if stopErr != nil {
					err = errors.Join(err, fmt.Errorf("could not confirm cleanup of %s: %w", started[n], stopErr))
				}
			}
		}
	}()
	for _, iid := range p.Instances {
		inst, ok := s.Instances[iid]
		if !ok {
			return fmt.Errorf("instance %s not found", iid)
		}
		job := "db-" + iid
		before, observeErr := m.readJobState(ctx, job)
		if observeErr != nil && !strings.Contains(observeErr.Error(), "Could not find service") && !strings.Contains(observeErr.Error(), "Could not find specified service") {
			return fmt.Errorf("cannot confirm database %s state: %w", iid, observeErr)
		}
		was := observeErr == nil && before.Running && before.PID > 0
		if e = m.startInstance(ctx, s, inst); e != nil {
			return e
		}
		if !was {
			started = append(started, job)
		}
	}
	for _, svc := range ordered {
		args, env, e := m.serviceArgs(s, p, svc)
		if e != nil {
			return e
		}
		job := "p-" + p.ID + "-" + svc.ID
		m.emit("Starting " + svc.ID)
		if _, observeErr := m.packageJobLoaded(ctx, m.label(job)); observeErr != nil {
			return fmt.Errorf("cannot confirm service %s state: %w", svc.ID, observeErr)
		}
		fresh, e := m.startJobPolicy(ctx, job, args, filepath.Join(p.Directory, svc.Directory), env, svc.Restart, svc.restartThrottle())
		if e != nil {
			return e
		}
		if fresh {
			started = append(started, job)
		}
		if e = m.waitService(ctx, job, svc); e != nil {
			return e
		}
	}
	marker := "project-" + p.ID
	if !m.loaded(ctx, marker) {
		fresh, e := m.startJob(ctx, marker, []string{"/usr/bin/true"}, p.Directory, nil, false)
		if e != nil {
			return e
		}
		if fresh {
			started = append(started, marker)
		}
	}
	return m.ApplyProxy(ctx)
}
func (m *Manager) down(ctx context.Context, id string) error {
	return m.withProjectLifecycle(ctx, id, false, func(s *State) error { return m.downProject(ctx, s, id) })
}
func (m *Manager) downProject(ctx context.Context, s *State, id string) error {
	p, ok := s.Projects[id]
	if !ok {
		return errors.New("project not found")
	}
	order, e := serviceOrder(p.Services)
	if e != nil {
		return e
	}
	active, e := m.projectActiveKnown(ctx, p)
	if e != nil {
		return e
	}
	for n := len(order) - 1; n >= 0; n-- {
		job := "p-" + p.ID + "-" + order[n].ID
		if e = m.stopJob(ctx, job); e != nil {
			return e
		}
		if e = m.verifyShutdownTarget(ctx, domain()+"/"+m.label(job)); e != nil {
			return e
		}
	}
	if !active {
		return nil
	}
	for _, iid := range p.Instances {
		shared := false
		for other, pr := range s.Projects {
			if other == id {
				continue
			}
			for _, ref := range pr.Instances {
				if ref == iid {
					observed, observeErr := m.projectActiveKnown(ctx, pr)
					if observeErr != nil {
						return observeErr
					}
					shared = shared || observed
				}
			}
		}
		if !shared {
			if e = m.stopJob(ctx, "db-"+iid); e != nil {
				return e
			}
			if e = m.verifyShutdownTarget(ctx, domain()+"/"+m.label("db-"+iid)); e != nil {
				return e
			}
		}
	}
	if e = m.stopJob(ctx, "project-"+p.ID); e != nil {
		return e
	}
	return m.verifyShutdownTarget(ctx, domain()+"/"+m.label("project-"+p.ID))
}
func (m *Manager) restartProject(ctx context.Context, id string) error {
	return m.withProjectLifecycle(ctx, id, true, func(s *State) error {
		if e := m.downProject(ctx, s, id); e != nil {
			return e
		}
		return m.upProject(ctx, s, id)
	})
}
func (m *Manager) RemoveProject(ctx context.Context, id string) error {
	return m.runOperation(ctx, OperationRequest{Kind: "remove-project", Resource: id}, "", func(w *Manager) error {
		return w.withProjectLifecycle(ctx, id, false, func(s *State) error {
			p := s.Projects[id]
			if e := w.downProject(ctx, s, id); e != nil {
				return e
			}
			p.Autostart = false
			return w.withStateTransaction(ctx, func(latest *State) (func() error, func(), error) {
				if current, exists := latest.Projects[id]; !exists || !reflect.DeepEqual(current, s.Projects[id]) {
					return nil, nil, errors.New("project changed before removal; refresh and retry")
				}
				restoreLogin, e := w.snapshotLoginConfiguration(id)
				if e != nil {
					return nil, nil, e
				}
				before := *latest
				before.Projects = make(map[string]Project, len(latest.Projects))
				for key, value := range latest.Projects {
					before.Projects[key] = value
				}
				proxyTouched := false
				undo := func() error {
					err := restoreLogin()
					if proxyTouched {
						rollback, cancel := context.WithTimeout(context.Background(), 35*time.Second)
						defer cancel()
						err = errors.Join(err, w.startProxy(rollback, &before))
					}
					return err
				}
				if e = w.configureLogin(ctx, p); e != nil {
					return undo, nil, e
				}
				for _, svc := range p.Services {
					if e = os.Remove(filepath.Join(w.Root, "jobs", w.label("p-"+id+"-"+svc.ID)+".plist")); e != nil && !errors.Is(e, os.ErrNotExist) {
						return undo, nil, e
					}
				}
				if e = os.Remove(filepath.Join(w.Root, "jobs", w.label("project-"+id)+".plist")); e != nil && !errors.Is(e, os.ErrNotExist) {
					return undo, nil, e
				}
				delete(latest.Projects, id)
				proxyTouched = true
				return undo, nil, w.startProxy(ctx, latest)
			})
		})
	})
}
func (m *Manager) execProject(ctx context.Context, id string, args []string) (string, error) {
	if len(args) == 0 {
		return "", errors.New("command required")
	}
	s, e := m.Read()
	if e != nil {
		return "", e
	}
	p, ok := s.Projects[id]
	if !ok || len(p.Services) == 0 {
		return "", errors.New("project has no service environment")
	}
	svc := p.Services[0]
	if e := p.requireLocalSettings(ctx); e != nil {
		return "", e
	}
	if svc.Package != "" {
		packageID := svc.Package
		unlock, err := m.packageLease(ctx, svc.Package, false)
		if err != nil {
			return "", err
		}
		defer unlock()
		s, err = m.Read()
		if err != nil {
			return "", err
		}
		current, ok := s.Projects[id]
		if !ok || len(current.Services) == 0 || current.Services[0].Package != packageID {
			return "", errors.New("project environment changed before CLI execution; retry with its current runtime")
		}
		p = current
		if err := p.requireLocalSettings(ctx); err != nil {
			return "", err
		}
		svc = p.Services[0]
	}
	svc.Command = args
	argv, env, e := m.serviceArgs(&s, p, svc)
	if e != nil {
		return "", e
	}
	childCtx, cancel := context.WithCancel(ctx)
	defer cancel()
	c := exec.CommandContext(childCtx, argv[0], argv[1:]...)
	c.Dir = p.Directory
	for k, v := range env {
		c.Env = append(c.Env, k+"="+v)
	}
	if m.commandWork != nil {
		c.Cancel = func() error { return c.Process.Signal(syscall.SIGTERM) }
		c.WaitDelay = 5 * time.Second
		f, e := os.OpenFile(filepath.Join(m.commandWork.dir, "output.log"), os.O_CREATE|os.O_EXCL|os.O_WRONLY|syscall.O_NOFOLLOW, 0600)
		if e != nil {
			return "", e
		}
		writer := &commandOutputWriter{file: f, remaining: maxCommandOutput, cancel: cancel}
		c.Stdout = writer
		c.Stderr = writer
		e = c.Run()
		syncErr := f.Sync()
		closeErr := f.Close()
		if writer.exceeded {
			e = errors.New("project command output exceeds 64 MiB")
		}
		return "", errors.Join(e, syncErr, closeErr)
	}
	b, e := c.CombinedOutput()
	return string(b), e
}

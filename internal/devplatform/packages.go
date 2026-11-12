package devplatform

import (
	"archive/tar"
	"archive/zip"
	"compress/gzip"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"howett.net/plist"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"time"
)

const maxDownload int64 = 2 << 30
const maxExpanded int64 = 8 << 30

func (m *Manager) packageDir(p Package) string {
	if p.Formula != "" {
		cellar := m.brewCellar
		if cellar == "" {
			cellar = "/opt/homebrew/Cellar"
		}
		return filepath.Join(cellar, filepath.Base(p.Formula), p.Version)
	}
	if p.Name == "java" {
		folder := p.JavaFolder
		if folder == "" {
			folder = javaFolderBase(p)
		}
		return filepath.Join(m.javaDirectory(), folder)
	}
	return filepath.Join(m.Root, "packages", p.ID())
}

func (m *Manager) PackageDirectory(p Package) string { return m.packageDir(p) }
func (m *Manager) Binary(s *State, id, name string) (string, error) {
	p, ok := s.Packages[id]
	if !ok {
		return "", fmt.Errorf("package %s is not installed; install it before starting", id)
	}
	if p.Arch != "arm64" && p.Arch != "universal" {
		return "", errors.New("managed software requires Apple Silicon (arm64)")
	}
	if p.ID() != id {
		return "", errors.New("installed package identity mismatch")
	}
	if p.Name == "php" || p.Name == "php-fpm" {
		return "", errors.New("PHP is not managed by this app")
	}
	if p.Formula != "" || p.Name == "postgresql" || p.Name == "java" {
		if e := p.Validate(); e != nil {
			return "", e
		}
	}
	if p.Formula != "" {
		if e := m.checkHomebrewReceipt(p); e != nil {
			return "", e
		}
	}
	if e := m.checkJavaOwnership(p); e != nil {
		return "", e
	}
	rel, ok := p.Binaries[name]
	if !ok {
		return "", fmt.Errorf("%s does not provide %s", id, name)
	}
	path := filepath.Join(m.packageDir(p), rel)
	f, e := os.Stat(path)
	if e != nil {
		return "", e
	}
	if !f.Mode().IsRegular() || f.Mode()&0111 == 0 {
		return "", fmt.Errorf("%s is not executable", path)
	}
	if e := requireAppleSiliconHost(); e != nil {
		return "", e
	}
	if e := requireAppleSiliconExecutable(path); e != nil {
		return "", e
	}
	return path, nil
}

type stagedPackage struct {
	Dir    string
	Index  []byte
	Digest string
}

func (m *Manager) stagePackage(ctx context.Context, p Package) (*stagedPackage, error) {
	if e := p.Validate(); e != nil {
		return nil, e
	}
	if e := requireAppleSiliconHost(); e != nil {
		return nil, e
	}
	if p.MinMacOS != "" {
		out, e := m.Run(ctx, "/usr/bin/sw_vers", "-productVersion")
		if e != nil {
			return nil, e
		}
		if versionLess(strings.TrimSpace(out), p.MinMacOS) {
			return nil, fmt.Errorf("%s requires macOS %s or newer", p.ID(), p.MinMacOS)
		}
	}
	m.emit("Downloading " + p.ID())
	cache := filepath.Join(m.downloadDirectory(), p.SHA256)
	// Caches can be removed while the app is running.
	if e := os.MkdirAll(filepath.Dir(cache), 0700); e != nil {
		return nil, e
	}
	if !hasDigestContext(ctx, cache, p.SHA256) {
		last := time.Time{}
		if e := downloadProgress(ctx, p.URL, cache, p.SHA256, func(done, total int64) {
			if time.Since(last) < 250*time.Millisecond && done != total {
				return
			}
			last = time.Now()
			if total > 0 {
				m.emit(fmt.Sprintf("Downloading %s · %.1f / %.1f MiB", p.ID(), float64(done)/(1<<20), float64(total)/(1<<20)))
			} else {
				m.emit(fmt.Sprintf("Downloading %s · %.1f MiB", p.ID(), float64(done)/(1<<20)))
			}
		}); e != nil {
			return nil, e
		}
	}
	// Stage beside the destination so publication and repair remain atomic,
	// including when the application data lives on another volume.
	parent := filepath.Dir(m.packageDir(p))
	if e := os.MkdirAll(parent, 0700); e != nil {
		return nil, e
	}
	if info, e := os.Lstat(parent); e != nil || !info.IsDir() || info.Mode()&os.ModeSymlink != 0 {
		return nil, errors.New("package parent must be a plain directory")
	}
	tmp, e := os.MkdirTemp(parent, ".install-")
	if e != nil {
		return nil, e
	}
	success := false
	defer func() {
		if !success {
			os.RemoveAll(tmp)
		}
	}()
	m.emit("Verifying and extracting " + p.ID())
	if e = extractContext(ctx, cache, tmp, p.Format, p.Strip); e != nil {
		return nil, e
	}
	if e = ctx.Err(); e != nil {
		return nil, e
	}
	if p.Name == "java" {
		if p.Binaries["java"] == "Contents/Home/bin/java" {
			if e = validateJavaBundle(tmp); e != nil {
				return nil, e
			}
		} else if p.Binaries["java"] == "bin/java" {
			b, err := readJavaMetadata(filepath.Join(tmp, "release"))
			if err != nil || javaReleaseValues(b)["JAVA_VERSION"] == "" {
				return nil, errors.New("JDK archive must contain valid release metadata")
			}
		} else {
			return nil, errors.New("unsupported Java archive layout")
		}
		if e = atomicWrite(filepath.Join(tmp, javaOwnerFile), []byte(m.javaOwner(p)), 0600); e != nil {
			return nil, e
		}
	}
	for _, rel := range p.Binaries {
		path := filepath.Join(tmp, rel)
		real, e := filepath.EvalSymlinks(path)
		if e != nil {
			return nil, e
		}
		if !inside(tmp, real) {
			return nil, errors.New("binary escapes package directory")
		}
		f, e := os.Stat(real)
		if e != nil {
			return nil, e
		}
		if !f.Mode().IsRegular() {
			return nil, errors.New("binary is not a regular file")
		}
		if e = requireAppleSiliconExecutable(real); e != nil {
			return nil, e
		}
		if e = os.Chmod(real, f.Mode().Perm()|0100); e != nil {
			return nil, e
		}
	}
	if e = writeJSON(filepath.Join(tmp, ".package.json"), p); e != nil {
		return nil, e
	}
	tree, e := m.scanPackageTree(ctx, tmp, p)
	if e != nil {
		return nil, e
	}
	index, digest, e := encodePackageTree(tree)
	if e != nil {
		return nil, e
	}
	success = true
	return &stagedPackage{tmp, index, digest}, nil
}

func (m *Manager) install(ctx context.Context, p Package) error {
	if e := p.Validate(); e != nil {
		return e
	}
	if e := requireAppleSiliconHost(); e != nil {
		return e
	}
	if e := m.init(); e != nil {
		return e
	}
	if p.Formula != "" {
		return m.installHomebrew(ctx, p)
	}

	state, e := m.Read()
	if e != nil {
		return e
	}
	_, installed := state.Packages[p.ID()]
	unlock, e := m.packageLease(ctx, p.ID(), !installed)
	if e != nil {
		return e
	}
	defer func() {
		if unlock != nil {
			unlock()
		}
	}()
	state, e = m.Read()
	if e != nil {
		return e
	}
	if current, ok := state.Packages[p.ID()]; ok {
		if current.ID() != p.ID() {
			return errors.New("installed package identity mismatch")
		}
		if current.SHA256 != p.SHA256 {
			return errors.New("installed package identity has a different checksum")
		}
		m.emit("Checking installed package " + p.ID())
		report, e := m.verifyPackage(ctx, &state, current)
		if e != nil {
			return e
		}
		if report.Status == "Needs repair" {
			return errors.New("installed package is incomplete or changed; use Repair this version")
		}
		return nil
	}
	if installed { // The observed version was uninstalled before the read lease.
		unlock()
		unlock = nil
		unlock, e = m.packageLease(ctx, p.ID(), true)
		if e != nil {
			return e
		}
		state, e = m.Read()
		if e != nil {
			return e
		}
		if _, exists := state.Packages[p.ID()]; exists {
			return errors.New("package registration changed; review it again")
		}
	}
	if e = m.requirePackageIdle(ctx, &state, p); e != nil {
		return e
	}
	if p.Name == "java" {
		p.JavaFolder, e = m.nextJavaFolder(p)
		if e != nil {
			return e
		}
		m.emit("Installing Java into " + m.packageDir(p))
	}
	return m.preparePackageForPublication(ctx, p, false)

}
func versionLess(a, b string) bool {
	var aa, bb [3]int
	fmt.Sscanf(a, "%d.%d.%d", &aa[0], &aa[1], &aa[2])
	fmt.Sscanf(b, "%d.%d.%d", &bb[0], &bb[1], &bb[2])
	for i := range aa {
		if aa[i] != bb[i] {
			return aa[i] < bb[i]
		}
	}
	return false
}
func (m *Manager) Uninstall(ctx context.Context, id string) error {
	var unlock func()
	defer func() {
		if unlock != nil {
			unlock()
		}
	}()
	return m.withState(ctx, func(s *State) error {
		p, ok := s.Packages[id]
		if !ok {
			return errors.New("package not installed")
		}
		if p.ID() != id {
			return errors.New("installed package identity mismatch")
		}
		var err error
		unlock, err = m.packageLease(ctx, id, true)
		if err != nil {
			return err
		}
		for _, i := range s.Instances {
			if i.Package == id {
				return fmt.Errorf("package is used by instance %s", i.ID)
			}
		}
		// Refuse packages referenced by generated launchd jobs, including interrupted operations.
		files, _ := filepath.Glob(filepath.Join(m.Root, "jobs", "*.plist"))
		for _, file := range files {
			b, e := os.ReadFile(file)
			if e != nil {
				return e
			}
			var job map[string]any
			if _, e = plist.Unmarshal(b, &job); e != nil {
				return e
			}
			if packageValueReferences(job, m.packageDir(p)) {
				return errors.New("package is referenced by a launchd job")
			}
		}
		if p.Formula == "" {
			if p.Name == "java" {
				if e := p.Validate(); e != nil {
					return e
				}
			}
			if e := m.checkJavaOwnership(p); e != nil {
				return e
			}
			if e := os.RemoveAll(m.packageDir(p)); e != nil {
				return e
			}
		}
		if p.Name == "node" {
			if s.NodeDefault == id {
				s.NodeDefault = ""
			}
			if e := os.RemoveAll(m.nodeGlobalPrefix(p.Version)); e != nil {
				return e
			}
		}
		if s.RuntimeDefaults[p.Name] == id || p.Name == "java" && s.RuntimeDefaults["java"] == javaInstallationID(m.packageDir(p)) {
			delete(s.RuntimeDefaults, p.Name)
			if _, err := m.updateRuntimeScripts(p.Name, nil); err != nil {
				return err
			}
		}
		delete(s.Packages, id)
		delete(s.PackageIntegrity, id)
		if p.Name == "node" {
			if e := m.linkNodeDefault(*s); e != nil {
				return e
			}
		}
		if err := os.Remove(m.integrityPath(id)); err != nil && !errors.Is(err, os.ErrNotExist) {
			return err
		}
		return nil
	})
}
func hasDigest(path, want string) bool {
	return hasDigestContext(context.Background(), path, want)
}
func hasDigestContext(ctx context.Context, path, want string) bool {
	f, e := os.Open(path)
	if e != nil {
		return false
	}
	defer f.Close()
	h := sha256.New()
	_, e = io.Copy(h, contextReader{ctx, f})
	return e == nil && hex.EncodeToString(h.Sum(nil)) == want
}
func download(ctx context.Context, address, dest, want string) error {
	return downloadProgress(ctx, address, dest, want, nil)
}

type contextReader struct {
	ctx    context.Context
	reader io.Reader
}

func (r contextReader) Read(p []byte) (int, error) {
	if e := r.ctx.Err(); e != nil {
		return 0, e
	}
	return r.reader.Read(p)
}

type downloadMeter struct {
	written, total int64
	progress       func(int64, int64)
}

func (m *downloadMeter) Write(p []byte) (int, error) {
	m.written += int64(len(p))
	if m.progress != nil {
		m.progress(m.written, m.total)
	}
	return len(p), nil
}
func downloadProgress(ctx context.Context, address, dest, want string, progress func(int64, int64)) error {
	req, e := http.NewRequestWithContext(ctx, http.MethodGet, address, nil)
	if e != nil {
		return e
	}
	client := &http.Client{Timeout: 30 * time.Minute, CheckRedirect: func(r *http.Request, via []*http.Request) error {
		if r.URL.Scheme != "https" {
			return errors.New("insecure download redirect")
		}
		if len(via) > 8 {
			return errors.New("too many redirects")
		}
		return nil
	}}
	res, e := client.Do(req)
	if e != nil {
		return e
	}
	defer res.Body.Close()
	if res.StatusCode != 200 {
		return fmt.Errorf("download: %s", res.Status)
	}
	if res.ContentLength > maxDownload {
		return errors.New("download exceeds limit")
	}
	f, e := os.CreateTemp(filepath.Dir(dest), ".download-")
	if e != nil {
		return e
	}
	defer os.Remove(f.Name())
	h := sha256.New()
	n, e := io.Copy(io.MultiWriter(f, h, &downloadMeter{total: res.ContentLength, progress: progress}), io.LimitReader(contextReader{ctx, res.Body}, maxDownload+1))
	ce := f.Close()
	if e != nil {
		return e
	}
	if ce != nil {
		return ce
	}
	if n > maxDownload {
		return errors.New("download exceeds limit")
	}
	if hex.EncodeToString(h.Sum(nil)) != want {
		return errors.New("download checksum mismatch")
	}
	return os.Rename(f.Name(), dest)
}
func inside(root, path string) bool {
	canonical := func(p string) string {
		var suffix []string
		for {
			real, e := filepath.EvalSymlinks(p)
			if e == nil {
				for n := len(suffix) - 1; n >= 0; n-- {
					real = filepath.Join(real, suffix[n])
				}
				return real
			}
			parent := filepath.Dir(p)
			if parent == p {
				return p
			}
			suffix = append(suffix, filepath.Base(p))
			p = parent
		}
	}
	r, e := filepath.Rel(canonical(root), canonical(path))
	return e == nil && (r == "." || safeRelative(r))
}
func archivePath(root, name string, strip int) (string, error) {
	if strings.Contains(name, "\\") || !safeRelative(name) {
		return "", fmt.Errorf("unsafe archive path %q", name)
	}
	parts := strings.Split(filepath.ToSlash(filepath.Clean(name)), "/")
	if len(parts) <= strip {
		return "", nil
	}
	path := filepath.Join(append([]string{root}, parts[strip:]...)...)
	if !inside(root, path) {
		return "", errors.New("archive path escapes root")
	}
	return path, nil
}
func extract(src, root, format string, strip int) error {
	return extractContext(context.Background(), src, root, format, strip)
}
func extractContext(ctx context.Context, src, root, format string, strip int) error {
	if e := ctx.Err(); e != nil {
		return e
	}

	var total int64
	type link struct{ path, target string }
	var links []link
	write := func(name string, mode os.FileMode, size int64, r io.Reader, target string) error {
		if e := ctx.Err(); e != nil {
			return e
		}
		path, e := archivePath(root, name, strip)
		if e != nil {
			return e
		}
		if path == "" {
			return nil
		}
		if mode.IsDir() {
			return os.MkdirAll(path, 0755)
		}
		if mode&os.ModeSymlink != 0 {
			if filepath.IsAbs(target) || !inside(root, filepath.Join(filepath.Dir(path), target)) {
				return errors.New("unsafe archive symlink")
			}
			links = append(links, link{path, target})
			return nil
		}
		if !mode.IsRegular() {
			return errors.New("unsupported archive entry")
		}
		total += size
		if size < 0 || total > maxExpanded {
			return errors.New("expanded archive exceeds limit")
		}
		if e = os.MkdirAll(filepath.Dir(path), 0755); e != nil {
			return e
		}
		f, e := os.OpenFile(path, os.O_CREATE|os.O_EXCL|os.O_WRONLY, mode.Perm()&0777)
		if e != nil {
			return e
		}
		n, e := io.Copy(f, io.LimitReader(contextReader{ctx, r}, size+1))
		ce := f.Close()
		if e != nil {
			return e
		}
		if ce != nil {
			return ce
		}
		if n != size {
			return errors.New("archive entry size mismatch")
		}
		return nil
	}
	if format == "zip" {
		z, e := zip.OpenReader(src)
		if e != nil {
			return e
		}
		defer z.Close()
		for _, f := range z.File {
			r, e := f.Open()
			if e != nil {
				return e
			}
			target := ""
			if f.Mode()&os.ModeSymlink != 0 {
				b, e := io.ReadAll(io.LimitReader(r, 4097))
				if e != nil || len(b) > 4096 {
					r.Close()
					return errors.New("invalid symlink")
				}
				target = string(b)
			}
			e = write(f.Name, f.Mode(), int64(f.UncompressedSize64), r, target)
			r.Close()
			if e != nil {
				return e
			}
		}
	} else {
		f, e := os.Open(src)
		if e != nil {
			return e
		}
		defer f.Close()
		gz, e := gzip.NewReader(f)
		if e != nil {
			return e
		}
		defer gz.Close()
		tr := tar.NewReader(gz)
		for {
			h, e := tr.Next()
			if e == io.EOF {
				break
			}
			if e != nil {
				return e
			}
			if h.Typeflag != tar.TypeReg && h.Typeflag != tar.TypeDir && h.Typeflag != tar.TypeSymlink {
				return fmt.Errorf("unsupported tar entry %s", h.Name)
			}
			if e = write(h.Name, h.FileInfo().Mode(), h.Size, tr, h.Linkname); e != nil {
				return e
			}
		}
	}
	// Links are created last so no archive entry can write through a symlink.
	for _, l := range links {
		if e := os.MkdirAll(filepath.Dir(l.path), 0755); e != nil {
			return e
		}
		if e := os.Symlink(l.target, l.path); e != nil {
			return e
		}
	}
	for _, l := range links {
		r, e := filepath.EvalSymlinks(l.path)
		if e != nil || !inside(root, r) {
			return errors.New("invalid archive symlink chain")
		}
	}
	return nil
}

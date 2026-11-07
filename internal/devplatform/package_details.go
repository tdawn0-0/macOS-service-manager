package devplatform

import (
	"context"
	"errors"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"
)

type PackageDetails struct {
	Package       Package
	ArchiveBytes  int64
	Cached        bool
	Installed     bool
	Compatible    bool
	Compatibility string
	Location      string
	References    []string
	Integrity     string
	IntegrityNote string
}

// ResolveInstalledOrCatalog keeps already installed exact versions usable
// offline. Repository lock restoration deliberately uses Resolve instead.
func (m *Manager) ResolveInstalledOrCatalog(ctx context.Context, name, version string) (Package, error) {
	version = strings.TrimPrefix(strings.TrimSpace(version), "v")
	state, e := m.Read()
	if e != nil {
		return Package{}, e
	}
	for _, p := range state.Packages {
		if p.Name == name && p.Version == version && (p.Arch == "arm64" || p.Arch == "universal") {
			return p, p.Validate()
		}
	}
	return Resolve(ctx, name, version)
}

func (m *Manager) PackageDetails(ctx context.Context, p Package) (PackageDetails, error) {
	info := PackageDetails{Package: p, Location: m.packageDir(p)}
	if e := p.Validate(); e != nil {
		return info, e
	}
	if e := ctx.Err(); e != nil {
		return info, e
	}
	info.Compatible = requireAppleSiliconHost() == nil && (p.Arch == "arm64" || p.Arch == "universal")
	info.Compatibility = "Compatible architecture"
	if !info.Compatible {
		info.Compatibility = "This archive is for a different architecture"
	}
	if p.MinMacOS != "" {
		version, e := m.Run(ctx, "/usr/bin/sw_vers", "-productVersion")
		if e != nil {
			return info, e
		}
		if versionLess(strings.TrimSpace(version), p.MinMacOS) {
			info.Compatible = false
			info.Compatibility = "Requires macOS " + p.MinMacOS + " or newer"
		} else if info.Compatible {
			info.Compatibility = "macOS " + strings.TrimSpace(version) + " meets the package's system requirement"
		}
	}
	state, e := m.Read()
	if e != nil {
		return info, e
	}
	if installed, ok := state.Packages[p.ID()]; ok {
		if installed.SHA256 != p.SHA256 {
			return info, errors.New("installed package has a different checksum")
		}
		info.Installed = true
		info.Integrity = "Not checked"
		info.IntegrityNote = "Run Check files to compare the installed tree with its recorded index."
		if state.PackageIntegrity[p.ID()] == "" {
			info.Integrity = "Not recorded"
			info.IntegrityNote = "Repair this version from the verified archive to record an installation index."
		}
		for name := range p.Binaries {
			if _, e = m.Binary(&state, p.ID(), name); e != nil {
				info.Integrity = "Needs repair"
				info.IntegrityNote = "A required executable is missing or invalid: " + name
			}
		}
	}
	for _, project := range state.Projects {
		for _, service := range project.Services {
			if service.Package == p.ID() {
				info.References = append(info.References, "Project: "+project.Name)
				break
			}
		}
	}
	for _, instance := range state.Instances {
		if instance.Package == p.ID() {
			info.References = append(info.References, "Instance: "+instance.ID)
		}
	}
	sort.Strings(info.References)
	cache := filepath.Join(m.Root, "downloads", p.SHA256)
	if hasDigestContext(ctx, cache, p.SHA256) {
		if stat, e := os.Stat(cache); e == nil {
			info.ArchiveBytes = stat.Size()
			info.Cached = true
		}
	} else if info.Compatible && !info.Installed {
		info.ArchiveBytes, _ = packageArchiveSize(ctx, p, metadataClient())
	}
	if info.ArchiveBytes > maxDownload {
		info.Compatible = false
		info.Compatibility = "Archive exceeds the supported 2 GiB download limit"
	}
	if e = ctx.Err(); e != nil {
		return info, e
	}
	return info, nil
}

func metadataClient() *http.Client {
	client := *http.DefaultClient
	client.Timeout = 15 * time.Second
	client.CheckRedirect = func(req *http.Request, via []*http.Request) error {
		if req.URL.Scheme != "https" {
			return errors.New("package metadata redirect must use HTTPS")
		}
		if len(via) > 8 {
			return errors.New("too many package metadata redirects")
		}
		return nil
	}
	return &client
}

func packageArchiveSize(ctx context.Context, p Package, client *http.Client) (int64, error) {
	for _, method := range []string{http.MethodHead, http.MethodGet} {
		req, e := http.NewRequestWithContext(ctx, method, p.URL, nil)
		if e != nil {
			return 0, e
		}
		if method == http.MethodGet {
			req.Header.Set("Range", "bytes=0-0")
		}
		response, e := client.Do(req)
		if e != nil {
			return 0, e
		}
		response.Body.Close()
		if response.StatusCode == http.StatusPartialContent {
			if _, total, ok := strings.Cut(response.Header.Get("Content-Range"), "/"); ok {
				n, e := strconv.ParseInt(total, 10, 64)
				if e == nil && n > 0 {
					return n, nil
				}
			}
		}
		if response.StatusCode == 200 && response.ContentLength > 0 {
			return response.ContentLength, nil
		}
	}
	return 0, errors.New("archive size is unavailable until download")
}

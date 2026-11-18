package devplatform

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"regexp"
	"strings"
	"time"
)

// archiveRelease is one exact, verified upstream archive.
type archiveRelease struct{ version, url, sha256 string }

// Newest first: the first entry is the default the UI offers.
var mysqlReleases = []archiveRelease{
	{"8.4.13", "https://dev.mysql.com/get/Downloads/MySQL-8.4/mysql-8.4.13-macos15-arm64.tar.gz", "f324f8950e24d4147470bbc6a6ef0b90ef93b91d3fc74154b3d2ba085b44912a"},
	{"8.4.4", "https://cdn.mysql.com/archives/mysql-8.4/mysql-8.4.4-macos15-arm64.tar.gz", "a23a5beada1404a609ee8f2a7b14f2869444fe05b5731f94a3d2215d4f6c41b5"},
	{"8.0.45", "https://cdn.mysql.com/archives/mysql-8.0/mysql-8.0.45-macos15-arm64.tar.gz", "3b8bd89b839e663479775a99deb03ff6cde11c86a1c37c0aed7116a778e4a8bc"},
	{"8.0.41", "https://cdn.mysql.com/archives/mysql-8.0/mysql-8.0.41-macos15-arm64.tar.gz", "37ed31ff5013b6cc964b2ad071da941d5f9cb423c11c615996e216f6224c889f"},
}

var mongodbReleases = []archiveRelease{
	{"8.0.38", "https://fastdl.mongodb.org/osx/mongodb-macos-arm64-8.0.38.tgz", "5c372533187a7789b6b2f4d7856f0651557dfbed572dbe5f4c23f43578fc74df"},
	{"8.0.5", "https://fastdl.mongodb.org/osx/mongodb-macos-arm64-8.0.5.tgz", "71e2510f0455ff24e1961aa0507eb0660b300c84f6da9899052be409090fb990"},
	{"7.0.30", "https://fastdl.mongodb.org/osx/mongodb-macos-arm64-7.0.30.tgz", "6c6c6fcfc38e025d48e99c6f5892a9e6aa8012ccb08ce247787936423d634dd1"},
	{"7.0.16", "https://fastdl.mongodb.org/osx/mongodb-macos-arm64-7.0.16.tgz", "e01c5ce1ef8efbef4196797130564b8f121f6e29f20ccf70e025feba07e15739"},
}

func findRelease(releases []archiveRelease, version string) (archiveRelease, bool) {
	for _, r := range releases {
		if r.version == version {
			return r, true
		}
	}
	return archiveRelease{}, false
}

func releaseVersionList(releases []archiveRelease) []string {
	out := make([]string, 0, len(releases))
	for _, r := range releases {
		out = append(out, r.version)
	}
	return out
}

func releaseVersions(releases []archiveRelease) string {
	return strings.Join(releaseVersionList(releases), ", ")
}

func fetch(ctx context.Context, address string, v any) error {
	ctx, cancel := context.WithTimeout(ctx, 30*time.Second)
	defer cancel()
	req, e := http.NewRequestWithContext(ctx, "GET", address, nil)
	if e != nil {
		return e
	}
	r, e := metadataClient().Do(req)
	if e != nil {
		return e
	}
	defer r.Body.Close()
	if r.StatusCode != 200 {
		return fmt.Errorf("catalog: %s", r.Status)
	}
	return json.NewDecoder(io.LimitReader(r.Body, 8<<20)).Decode(v)
}

// Resolve uses upstream version metadata. Returned manifests contain exact hashes
// and are persisted with the installed package; no floating versions are stored.
func Resolve(ctx context.Context, name, version string) (Package, error) {
	ctx, cancel := context.WithTimeout(ctx, 30*time.Second)
	defer cancel()
	arch := "arm64"
	p := Package{Name: name, Version: strings.TrimPrefix(version, "v"), Arch: arch, Format: "tar.gz", Strip: 1, Binaries: map[string]string{}}
	if !nameRE.MatchString(p.Name) || !nameRE.MatchString(p.Version) {
		return p, errors.New("choose an exact package version")
	}
	switch name {
	case "java":
		return resolveJava(ctx, version)
	case "mysql":
		r, ok := findRelease(mysqlReleases, p.Version)
		if !ok {
			return p, errors.New("supported MySQL versions: " + releaseVersions(mysqlReleases))
		}
		p.SHA256 = r.sha256
		p.URL = r.url
		p.MinMacOS = "26.0"
		p.License = "GPL-2.0 and bundled component licenses"
		p.Binaries = map[string]string{"mysqld": "bin/mysqld", "mysql": "bin/mysql", "mysqldump": "bin/mysqldump"}
		return p, p.Validate()

	case "postgresql", "redis":
		return resolveHomebrew(ctx, name, version, runCommand)
	case "mongodb":
		r, ok := findRelease(mongodbReleases, p.Version)
		if !ok {
			return p, errors.New("supported MongoDB versions: " + releaseVersions(mongodbReleases))
		}
		p.SHA256 = r.sha256
		p.URL = r.url
		p.MinMacOS = "26.0"
		p.License = "SSPL-1.0"
		p.Binaries = map[string]string{"mongod": "bin/mongod", "mongos": "bin/mongos"}
		return p, p.Validate()

	case "node":
		if !exactRuntimeVersion.MatchString(p.Version) {
			return p, errors.New("choose an exact Node.js version, for example 22.14.0; resolve 22, lts or latest first")
		}
		filename := fmt.Sprintf("node-v%s-darwin-%s.tar.gz", p.Version, arch)
		base := nodeDist + "v" + p.Version + "/"
		req, e := http.NewRequestWithContext(ctx, "GET", base+"SHASUMS256.txt", nil)
		if e != nil {
			return p, e
		}
		r, e := metadataClient().Do(req)
		if e != nil {
			return p, e
		}
		defer r.Body.Close()
		if r.StatusCode != 200 {
			return p, fmt.Errorf("node catalog: %s", r.Status)
		}
		b, e := io.ReadAll(io.LimitReader(r.Body, 1<<20))
		if e != nil {
			return p, e
		}
		for _, line := range strings.Split(string(b), "\n") {
			fields := strings.Fields(line)
			if len(fields) == 2 && fields[1] == filename {
				p.URL = base + filename
				p.SHA256 = fields[0]
				p.Binaries = map[string]string{"node": "bin/node", "npm": "bin/npm", "npx": "bin/npx"}
				p.License = "MIT"
				return p, p.Validate()
			}
		}
	default:
		return p, errors.New("use an explicit verified package manifest for this software")
	}
	return p, errors.New("requested version is not available for this architecture")
}

func AvailableVersions(name string) []string {
	switch name {
	case "node":
		return []string{"22.14.0", "20.19.0"}
	case "mysql":
		return releaseVersionList(mysqlReleases)
	case "postgresql":
		return []string{"17", "18", "16"}
	case "redis":
		return []string{"8.2"}
	case "mongodb":
		return releaseVersionList(mongodbReleases)
	}
	return nil
}

var exactRuntimeVersion = regexp.MustCompile(`^[0-9]+\.[0-9]+\.[0-9]+$`)

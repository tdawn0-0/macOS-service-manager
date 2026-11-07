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
	if e := requireAppleSiliconHost(); e != nil {
		return Package{}, e
	}
	arch := "arm64"
	p := Package{Name: name, Version: strings.TrimPrefix(version, "v"), Arch: arch, Format: "tar.gz", Strip: 1, Binaries: map[string]string{}}
	if !nameRE.MatchString(p.Name) || !nameRE.MatchString(p.Version) {
		return p, errors.New("choose an exact package version")
	}
	switch name {
	case "java":
		return resolveJava(ctx, version)
	case "mysql":
		if p.Version != "8.4.4" {
			return p, errors.New("supported MySQL version: 8.4.4")
		}
		assetArch := "arm64"
		p.SHA256 = "a23a5beada1404a609ee8f2a7b14f2869444fe05b5731f94a3d2215d4f6c41b5"
		p.URL = "https://cdn.mysql.com/archives/mysql-8.4/mysql-8.4.4-macos15-" + assetArch + ".tar.gz"
		p.MinMacOS = "15.0"
		p.License = "GPL-2.0 and bundled component licenses"
		p.Binaries = map[string]string{"mysqld": "bin/mysqld", "mysql": "bin/mysql", "mysqldump": "bin/mysqldump"}
		return p, p.Validate()

	case "postgresql":
		return resolveHomebrew(ctx, name, version, runCommand)
	case "redis":
		if p.Version != "7.4.7" {
			return p, errors.New("supported Redis version: 7.4.7 (Redis Stack 7.4.0-v8 distribution)")
		}
		asset := "sonoma.arm64"
		p.SHA256 = "4b063a410120843f6e493fa52d433b17ad79b57adc751b9f8050aa81874f04da"
		p.URL = "https://packages.redis.io/redis-stack/redis-stack-server-7.4.0-v8." + asset + ".zip"
		p.Format = "zip"
		p.Strip = 0
		p.MinMacOS = "14.0"
		p.License = "RSALv2/SSPLv1 and bundled component licenses"
		p.Binaries = map[string]string{"redis-server": "bin/redis-server", "redis-cli": "bin/redis-cli"}
		return p, p.Validate()
	case "mongodb":
		if p.Version != "8.0.5" {
			return p, errors.New("supported MongoDB version: 8.0.5")
		}
		assetArch := "arm64"
		p.SHA256 = "71e2510f0455ff24e1961aa0507eb0660b300c84f6da9899052be409090fb990"
		p.URL = "https://fastdl.mongodb.org/osx/mongodb-macos-" + assetArch + "-8.0.5.tgz"
		p.MinMacOS = "13.0"
		p.License = "SSPL-1.0"
		p.Binaries = map[string]string{"mongod": "bin/mongod", "mongos": "bin/mongos"}
		return p, p.Validate()

	case "valkey":
		if p.Version != "9.1.2" {
			return p, errors.New("supported Valkey package version: 9.1.2")
		}
		p.Arch = "universal"
		p.URL = "https://github.com/alhassanaraouf/Valkey.app/releases/download/valkey-9.1.2/valkey-9.1.2-macos-universal.tar.gz"
		p.SHA256 = "befbdf6a57552e7f4c2824e2bc11dfba1d74c1e97b89955836497cca67b1e5c9"
		p.Strip = 0
		p.MinMacOS = "13.0"
		p.License = "BSD-3-Clause"
		p.Binaries = map[string]string{"valkey-server": "bin/valkey-server", "valkey-cli": "bin/valkey-cli"}
		return p, p.Validate()

	case "go":
		var releases []struct {
			Version string
			Files   []struct{ Filename, OS, Arch, SHA256 string }
		}
		if e := fetch(ctx, "https://go.dev/dl/?mode=json&include=all", &releases); e != nil {
			return p, e
		}
		for _, r := range releases {
			if strings.TrimPrefix(r.Version, "go") == p.Version {
				for _, f := range r.Files {
					if f.OS == "darwin" && f.Arch == arch && strings.HasSuffix(f.Filename, ".tar.gz") {
						p.URL = "https://go.dev/dl/" + f.Filename
						p.SHA256 = f.SHA256
						p.Binaries = map[string]string{"go": "bin/go", "gofmt": "bin/gofmt"}
						p.License = "BSD-3-Clause"
						return p, p.Validate()
					}
				}
			}
		}
	case "node":
		if !exactRuntimeVersion.MatchString(p.Version) {
			return p, errors.New("choose an exact Node.js version, for example 22.14.0; resolve 22, lts or latest first")
		}
		filename := fmt.Sprintf("node-v%s-darwin-%s.tar.gz", p.Version, arch)
		dist, e := nodeDist()
		if e != nil {
			return p, e
		}
		base := dist + "v" + p.Version + "/"
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
		return []string{"8.4.4"}
	case "postgresql":
		return []string{"17", "18", "16"}
	case "redis":
		return []string{"7.4.7"}
	case "mongodb":
		return []string{"8.0.5"}
	}
	return nil
}

var exactRuntimeVersion = regexp.MustCompile(`^[0-9]+\.[0-9]+\.[0-9]+$`)

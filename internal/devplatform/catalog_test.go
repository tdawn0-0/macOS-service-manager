package devplatform

import (
	"context"
	"regexp"
	"strconv"
	"strings"
	"testing"
)

var hex64 = regexp.MustCompile(`^[0-9a-f]{64}$`)

func TestCatalogEveryListedVersionResolves(t *testing.T) {
	for _, name := range []string{"mysql", "redis", "mongodb"} {
		seen := map[string]string{}
		for _, v := range AvailableVersions(name) {
			p, err := Resolve(context.Background(), name, v)
			if err != nil {
				t.Fatalf("%s %s: %v", name, v, err)
			}
			if err := p.Validate(); err != nil {
				t.Fatalf("%s %s validate: %v", name, v, err)
			}
			if p.Version != v {
				t.Fatalf("%s %s: version %q", name, v, p.Version)
			}
			// Redis ships inside the Redis Stack archive, whose file name carries the
			// distribution version rather than the Redis version.
			if !strings.HasPrefix(p.URL, "https://") || (name != "redis" && !strings.Contains(p.URL, v)) {
				t.Fatalf("%s %s: url %q", name, v, p.URL)
			}
			if !hex64.MatchString(p.SHA256) {
				t.Fatalf("%s %s: sha256 %q", name, v, p.SHA256)
			}
			if other, dup := seen[p.SHA256]; dup {
				t.Fatalf("%s: versions %s and %s share a SHA-256", name, other, v)
			}
			seen[p.SHA256] = v
		}
	}
}

func TestCatalogKeepsExistingPins(t *testing.T) {
	cases := []struct{ name, version, url, sha string }{
		{"mysql", "8.4.4", "https://cdn.mysql.com/archives/mysql-8.4/mysql-8.4.4-macos15-arm64.tar.gz", "a23a5beada1404a609ee8f2a7b14f2869444fe05b5731f94a3d2215d4f6c41b5"},
		{"mongodb", "8.0.5", "https://fastdl.mongodb.org/osx/mongodb-macos-arm64-8.0.5.tgz", "71e2510f0455ff24e1961aa0507eb0660b300c84f6da9899052be409090fb990"},
	}
	for _, c := range cases {
		p, err := Resolve(context.Background(), c.name, c.version)
		if err != nil {
			t.Fatal(err)
		}
		if p.URL != c.url || p.SHA256 != c.sha {
			t.Fatalf("%s %s pin changed: %s %s", c.name, c.version, p.URL, p.SHA256)
		}
	}
}

func catalogVersionParts(t *testing.T, v string) [3]int {
	t.Helper()
	parts := strings.Split(v, ".")
	var out [3]int
	if len(parts) != 3 {
		t.Fatalf("version %q is not x.y.z", v)
	}
	for i, s := range parts {
		n, err := strconv.Atoi(s)
		if err != nil {
			t.Fatal(err)
		}
		out[i] = n
	}
	return out
}

func TestCatalogNewestFirst(t *testing.T) {
	if got := AvailableVersions("mysql")[0]; got != "8.4.13" {
		t.Fatalf("mysql default %s", got)
	}
	if got := AvailableVersions("mongodb")[0]; got != "8.0.38" {
		t.Fatalf("mongodb default %s", got)
	}
	for _, name := range []string{"mysql", "mongodb"} {
		vs := AvailableVersions(name)
		for i := 1; i < len(vs); i++ {
			a, b := catalogVersionParts(t, vs[i-1]), catalogVersionParts(t, vs[i])
			if !(a[0] > b[0] || a[0] == b[0] && (a[1] > b[1] || a[1] == b[1] && a[2] > b[2])) {
				t.Fatalf("%s not strictly descending: %v", name, vs)
			}
		}
	}
}

func TestCatalogRejectsUnlistedVersion(t *testing.T) {
	for name, v := range map[string]string{"mysql": "8.4.5", "mongodb": "8.0.6"} {
		_, err := Resolve(context.Background(), name, v)
		if err == nil || !strings.Contains(err.Error(), AvailableVersions(name)[0]) {
			t.Fatalf("%s %s: %v", name, v, err)
		}
	}
}

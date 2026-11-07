package devplatform

import (
	"context"
	"errors"
	"fmt"
	"net/url"
	"strings"
)

type javaRelease struct {
	Binaries []struct {
		Architecture, OS string
		ImageType        string `json:"image_type"`
		Package          struct{ Link, Checksum string }
	} `json:"binaries"`
	Version struct{ Semver string } `json:"version_data"`
}

func javaReleases(ctx context.Context, major string) ([]javaRelease, error) {
	if !nameRE.MatchString(major) {
		return nil, errors.New("invalid Java version")
	}
	var rows []javaRelease
	err := fetch(ctx, "https://api.adoptium.net/v3/assets/feature_releases/"+url.PathEscape(major)+"/ga?architecture=aarch64&image_type=jdk&os=mac&vendor=eclipse&page_size=20", &rows)
	return rows, err
}
func resolveJava(ctx context.Context, version string) (Package, error) {
	major := strings.Split(version, ".")[0]
	rows, err := javaReleases(ctx, major)
	if err != nil {
		return Package{}, err
	}
	for _, r := range rows {
		exact := strings.ReplaceAll(r.Version.Semver, "+", "_")
		if exact != version {
			continue
		}
		for _, b := range r.Binaries {
			if b.Architecture != "aarch64" || b.OS != "mac" || b.ImageType != "jdk" {
				continue
			}
			p := Package{Name: "java", Version: exact, Arch: "arm64", URL: b.Package.Link, SHA256: b.Package.Checksum, Format: "tar.gz", Strip: 1, License: "GPL-2.0 with Classpath Exception", Binaries: map[string]string{"java": "Contents/Home/bin/java", "javac": "Contents/Home/bin/javac"}}
			return p, p.Validate()
		}
	}
	return Package{}, errors.New("exact Temurin JDK version is unavailable; refresh versions")
}
func (m *Manager) RuntimeVersions(ctx context.Context, name string) ([]string, error) {
	var versions []string
	switch name {
	case "java":
		for _, major := range []string{"25", "21", "17"} {
			rows, err := javaReleases(ctx, major)
			if err != nil {
				return nil, err
			}
			for _, r := range rows {
				for _, b := range r.Binaries {
					if b.Architecture == "aarch64" && b.OS == "mac" && b.ImageType == "jdk" && validDigest(b.Package.Checksum) && nameRE.MatchString(strings.ReplaceAll(r.Version.Semver, "+", "_")) {
						versions = append(versions, strings.ReplaceAll(r.Version.Semver, "+", "_"))
						break
					}
				}
			}
		}
	case "go":
		var rows []struct {
			Version string
			Stable  bool
			Files   []struct{ OS, Arch, Kind string }
		}
		if err := fetch(ctx, "https://go.dev/dl/?mode=json&include=all", &rows); err != nil {
			return nil, err
		}
		for _, r := range rows {
			if !r.Stable {
				continue
			}
			for _, f := range r.Files {
				if f.OS == "darwin" && f.Arch == "arm64" && f.Kind == "archive" {
					versions = append(versions, strings.TrimPrefix(r.Version, "go"))
					break
				}
			}
		}
	default:
		return nil, fmt.Errorf("unsupported runtime %s", name)
	}
	return versions, nil
}

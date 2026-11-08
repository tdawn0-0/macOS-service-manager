package devplatform

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"path/filepath"
	"strings"
)

// IDEA uses the .xz representation of this feed; the same JSON is available
// uncompressed so the application does not need an additional decompressor.
const JavaCatalogURL = "https://download.jetbrains.com/jdk/feed/v1/jdks.json"

type javaFeedProduct struct {
	Vendor, Product, Flavour string
	Version                  string `json:"jdk_version"`
	VendorVersion            string `json:"jdk_vendor_version"`
	Filter, Listed, Preview  json.RawMessage
	Packages                 []javaFeedPackage
}
type javaFeedPackage struct {
	OS, Arch, Version, URL, SHA256 string
	Type                           string `json:"package_type"`
	Root                           string `json:"package_root_prefix"`
	Home                           string `json:"package_to_java_home_prefix"`
	Folder                         string `json:"install_folder_name"`
	Filter                         json.RawMessage
}

// IDE build ranges concern the IDE's parser version, not the JDK's platform.
// Platform constraints are checked separately; unknown predicates fail closed.
func javaFeedPredicate(raw json.RawMessage, absent bool) bool {
	if len(raw) == 0 || string(raw) == "null" {
		return absent
	}
	var value bool
	if json.Unmarshal(raw, &value) == nil {
		return value
	}
	var f struct {
		Type  string
		Value bool
		Arch  string
		Items []json.RawMessage
		Item  json.RawMessage
	}
	if json.Unmarshal(raw, &f) != nil {
		return false
	}
	switch f.Type {
	case "const":
		return f.Value
	case "supports_arch":
		return f.Arch == "" || f.Arch == "aarch64"
	case "build_number_range":
		return true
	case "and":
		if len(f.Items) == 0 {
			return false
		}
		for _, r := range f.Items {
			if !javaFeedPredicate(r, false) {
				return false
			}
		}
		return true
	case "or":
		for _, r := range f.Items {
			if javaFeedPredicate(r, false) {
				return true
			}
		}
		return false
	case "not":
		// Negation of unknown predicates must not turn them into accepted entries.
		var known struct{ Type string }
		var b bool
		if json.Unmarshal(f.Item, &b) != nil {
			if json.Unmarshal(f.Item, &known) != nil {
				return false
			}
			switch known.Type {
			case "const", "supports_arch", "build_number_range":
			default:
				return false
			}
		}
		return !javaFeedPredicate(f.Item, false)
	}
	return false
}
func javaCatalog(ctx context.Context) ([]Package, error) {
	var feed struct {
		JDKs []javaFeedProduct `json:"jdks"`
	}
	if err := fetch(ctx, JavaCatalogURL, &feed); err != nil {
		return nil, err
	}
	if feed.JDKs == nil {
		return nil, errors.New("JetBrains JDK catalog is missing its version list")
	}
	found := map[string]Package{}
	var order []string
	for _, product := range feed.JDKs {
		if product.Vendor == "" || !javaFeedPredicate(product.Filter, true) || !javaFeedPredicate(product.Listed, true) || javaFeedPredicate(product.Preview, false) {
			continue
		}
		for _, item := range product.Packages {
			if item.OS != "macOS" || item.Arch != "aarch64" || !javaFeedPredicate(item.Filter, true) {
				continue
			}
			format := ""
			switch item.Type {
			case "targz":
				format = "tar.gz"
			case "zip":
				format = "zip"
			default:
				continue
			}
			if !javaInstallNameRE.MatchString(item.Folder) || (item.Home != "" && item.Home != "Contents/Home") {
				continue
			}
			strip := 0
			if item.Root != "" && item.Root != "." {
				if !safeRelative(item.Root) {
					continue
				}
				strip = len(strings.Split(filepath.ToSlash(filepath.Clean(item.Root)), "/"))
			}
			version := item.Version
			if version == "" {
				version = product.Version
			}
			label := strings.TrimSpace(product.Vendor + " " + product.Product)
			if product.Flavour != "" {
				label += " (" + product.Flavour + ")"
			}
			label += " · " + version
			if product.VendorVersion != "" {
				label += " · " + product.VendorVersion
			}
			// The catalog key includes the distribution/flavour, not just Java's version.
			key := strings.ReplaceAll(item.Folder, "+", "_")
			p := Package{Name: "java", Version: key, Arch: "arm64", JavaInstallName: item.Folder, JavaProduct: label, URL: item.URL, SHA256: strings.ToLower(item.SHA256), Format: format, Strip: strip, License: "See the distribution's license and terms in the upstream archive", Binaries: map[string]string{"java": filepath.Join(item.Home, "bin/java"), "javac": filepath.Join(item.Home, "bin/javac")}}
			if p.Validate() != nil {
				continue
			}
			if previous, ok := found[key]; ok && (previous.SHA256 != p.SHA256 || previous.URL != p.URL) {
				return nil, fmt.Errorf("ambiguous JDK catalog entry %s; refresh before installing", key)
			}
			if _, exists := found[key]; !exists {
				order = append(order, key)
			}
			found[key] = p
		}
	}
	rows := make([]Package, 0, len(found))
	for _, key := range order {
		rows = append(rows, found[key])
	}
	if len(rows) == 0 {
		return nil, errors.New("JetBrains catalog contains no supported macOS arm64 JDK archives")
	}
	return rows, nil
}
func resolveJava(ctx context.Context, key string) (Package, error) {
	rows, err := javaCatalog(ctx)
	if err != nil {
		return Package{}, err
	}
	for _, p := range rows {
		if p.Version == key {
			return p, nil
		}
	}
	return Package{}, errors.New("selected JDK distribution/version is unavailable; refresh the JetBrains catalog")
}

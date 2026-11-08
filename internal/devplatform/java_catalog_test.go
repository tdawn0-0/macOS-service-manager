package devplatform

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"strings"
	"testing"
)

func TestJetBrainsJavaCatalog(t *testing.T) {
	products := []javaFeedProduct{}
	for _, name := range []string{"temurin", "semeru", "graalvm-ce", "liberica"} {
		home := "Contents/Home"
		if name == "liberica" {
			home = ""
		}
		products = append(products, javaFeedProduct{Vendor: name, Product: "JDK", Version: "21.0.13", Packages: []javaFeedPackage{{OS: "macOS", Arch: "aarch64", URL: "https://example.test/" + name + ".tar.gz", SHA256: strings.Repeat("a", 64), Type: "targz", Root: "./jdk-21", Home: home, Folder: name + "-21.0.13"}}})
	}
	skipped := []javaFeedProduct{
		{Vendor: "Intel", Packages: []javaFeedPackage{{OS: "macOS", Arch: "x86_64", Folder: "intel-21"}}},
		{Vendor: "hidden", Listed: json.RawMessage(`false`), Packages: products[0].Packages},
		{Vendor: "preview", Preview: json.RawMessage(`true`), Packages: products[0].Packages},
		{Vendor: "future-filter", Filter: json.RawMessage(`{"type":"unknown"}`), Packages: products[0].Packages},
	}
	old := http.DefaultClient
	defer func() { http.DefaultClient = old }()
	http.DefaultClient = &http.Client{Transport: runtimeTransport(func(r *http.Request) (*http.Response, error) {
		if r.URL.String() != JavaCatalogURL {
			t.Fatal("unexpected catalog source", r.URL)
		}
		b, e := json.Marshal(struct {
			JDKs []javaFeedProduct `json:"jdks"`
		}{append(products, skipped...)})
		if e != nil {
			t.Fatal(e)
		}
		return &http.Response{StatusCode: 200, Header: http.Header{}, Body: io.NopCloser(strings.NewReader(string(b)))}, nil
	})}
	rows, err := javaCatalog(context.Background())
	if err != nil || len(rows) != 4 {
		t.Fatal(rows, err)
	}
	ids := map[string]bool{}
	for _, p := range rows {
		if ids[p.ID()] || p.JavaInstallName != p.Version || p.JavaProduct == "" || p.Strip != 1 {
			t.Fatal("distribution identity/layout lost", p)
		}
		ids[p.ID()] = true
		if strings.HasPrefix(p.Version, "liberica") && p.Binaries["java"] != "bin/java" {
			t.Fatal("flat JDK layout lost", p)
		}
	}
	// Reject traversal in installation names and invalid transport/checksums.
	for _, mutate := range []func(*javaFeedPackage){
		func(p *javaFeedPackage) { p.Folder = "../outside" },
		func(p *javaFeedPackage) { p.Root = "../../escape" },
		func(p *javaFeedPackage) { p.Home = "../../escape" },
		func(p *javaFeedPackage) { p.URL = "http://example.test/jdk" },
		func(p *javaFeedPackage) { p.SHA256 = "bad" },
	} {
		saved := products[0].Packages[0]
		mutate(&products[0].Packages[0])
		rows, err := javaCatalog(context.Background())
		if err != nil || len(rows) != 3 {
			t.Fatal("unsafe feed entry accepted", rows, err)
		}
		products[0].Packages[0] = saved
	}
	duplicate := products[0]
	duplicate.Packages = append([]javaFeedPackage(nil), duplicate.Packages...)
	duplicate.Packages[0].SHA256 = strings.Repeat("b", 64)
	products = append(products, duplicate)
	if _, err := javaCatalog(context.Background()); err == nil {
		t.Fatal("ambiguous download identity accepted")
	}
}

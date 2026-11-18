package main

import (
	"image"
	"image/png"
	"os"
	"path/filepath"
	"testing"
)

// shotsWanted reports whether MSM_SHOTS asks the tests to write screenshots.
// Set it only when deliberately regenerating images, to a directory under the
// ignored build/ directory.
func shotsWanted() bool { return os.Getenv("MSM_SHOTS") != "" }

// saveShot writes img to $MSM_SHOTS/<test name>/<name>.png when MSM_SHOTS is
// set. Each test writes under its own name, so no test overwrites another's
// images.
func saveShot(t *testing.T, name string, img image.Image) {
	t.Helper()
	root := os.Getenv("MSM_SHOTS")
	if root == "" {
		return
	}
	dir := filepath.Join(root, filepath.FromSlash(t.Name()))
	if err := os.MkdirAll(dir, 0755); err != nil {
		t.Fatal(err)
	}
	f, err := os.Create(filepath.Join(dir, name+".png"))
	if err != nil {
		t.Fatal(err)
	}
	if err = png.Encode(f, img); err != nil {
		f.Close()
		t.Fatal(err)
	}
	if err = f.Close(); err != nil {
		t.Fatal(err)
	}
}

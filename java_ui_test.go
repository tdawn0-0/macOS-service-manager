package main

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/egoist/mygo/ui"
	"devbase/internal/devplatform"
)

func TestUIExistingJavaManagement(t *testing.T) {
	h := newDevUIHarnessSize(t, 900, 520, false)
	m := h.a.dev.managerFactory()
	path := filepath.Join(m.JavaInstallationDirectory(), "semeru-25.0.4.15")
	home := filepath.Join(path, "Contents/Home")
	if err := os.MkdirAll(filepath.Join(home, "bin"), 0700); err != nil {
		t.Fatal(err)
	}
	binary, err := os.ReadFile("/bin/echo")
	if err != nil {
		t.Fatal(err)
	}
	if err = os.WriteFile(filepath.Join(home, "bin/java"), binary, 0755); err != nil {
		t.Fatal(err)
	}
	release := []byte("JAVA_VERSION=\"25.0.4.1\"\nJAVA_RUNTIME_VERSION=\"25.0.4.1+1-LTS\"\nJAVA_RUNTIME_NAME=\"IBM Semeru Runtime\"\nIMPLEMENTOR=\"IBM Corporation\"\nOS_ARCH=\"aarch64\"\n")
	if err = os.WriteFile(filepath.Join(home, "release"), release, 0600); err != nil {
		t.Fatal(err)
	}
	h.a.dev.software = "java"
	h.ui.Frame()
	h.waitIdle()
	if len(h.a.dev.javaInstallations) != 1 || h.a.dev.javaInstallations[0].Vendor != "IBM Corporation" {
		t.Fatal("did not auto-discover Semeru", h.a.dev.javaInstallations, h.a.dev.output)
	}
	h.click("Set default")
	s, err := m.Read()
	if err != nil || s.RuntimeDefaults["java"] != h.a.dev.javaInstallations[0].ID {
		t.Fatal(s, err, h.a.dev.output)
	}
	h.click("Copy JAVA_HOME")
	resolved, err := filepath.EvalSymlinks(home)
	if err != nil {
		t.Fatal(err)
	}
	if len(h.copied) == 0 || h.copied[len(h.copied)-1] != resolved {
		t.Fatal(h.copied)
	}
	h.click("Enable in terminal")
	shownText := strings.Join(h.ui.Texts(), "\n")
	if !strings.Contains(shownText, "Enabled for detected shells") {
		t.Fatal("terminal status not shown as enabled", shownText)
	}
	h.click("Remove JDK")
	if _, err = os.Stat(path); err != nil {
		t.Fatal("review deleted before confirmation", err)
	}
	shown := strings.Join(h.ui.Texts(), "\n")
	if !strings.Contains(shown, path) || !strings.Contains(shown, "IBM Semeru Runtime") {
		t.Fatal("review hides target identity", shown)
	}
	h.click("Cancel Java removal")
	if _, err = os.Stat(path); err != nil {
		t.Fatal("cancel removed JDK", err)
	}
	h.click("Remove JDK")
	h.click("Move JDK to Trash")
	if _, err = os.Stat(path); !os.IsNotExist(err) {
		t.Fatal("confirmed removal failed", err, h.a.dev.output)
	}
	if len(h.a.dev.javaInstallations) != 0 {
		t.Fatal("stale Java rows", h.a.dev.javaInstallations)
	}
	s, err = m.Read()
	if err != nil || s.RuntimeDefaults["java"] != "" {
		t.Fatal(s, err)
	}
}

func TestUIJavaDistributions(t *testing.T) {
	for _, size := range [][2]int{{900, 520}, {1120, 720}} {
		for _, dark := range []bool{false, true} {
			t.Run(fmt.Sprintf("%dx%d-dark-%t", size[0], size[1], dark), func(t *testing.T) {
				a := designFixture(t)
				a.dev.software = "java"
				a.dev.version = "21.0.13_9.0.LTS"
				a.dev.javaLoaded = true
				a.dev.javaInstallations = []devplatform.JavaInstallation{
					{ID: "semeru", Name: "IBM Semeru Runtime Open Edition 25", Vendor: "IBM Corporation", Version: "25.0.4.1+1-LTS", Arch: "aarch64", CanUse: true, Path: "/Users/developer/Library/Java/JavaVirtualMachines/semeru-25.0.4.15"},
					{ID: "temurin", Name: "OpenJDK 25", Vendor: "Eclipse Adoptium", Version: "25.0.4.1+1-LTS", Arch: "aarch64", CanUse: true, Path: "/Users/developer/Library/Java/JavaVirtualMachines/temurin-25.0.4.1"},
					{ID: "graalvm", Name: "Oracle GraalVM", Vendor: "Oracle Corporation", Version: "25.0.4.1+1-LTS", Arch: "x86_64", Problem: "Intel JDK; can be removed but cannot be selected on Apple Silicon", Path: "/Users/developer/Library/Java/JavaVirtualMachines/graalvm-25.0.4.1"},
				}
				a.dev.javaRemove = &a.dev.javaInstallations[0]
				tt := ui.NewTester(a.view, size[0], size[1])
				tt.SetDark(dark)
				tt.Frame()
				shown := strings.Join(tt.Texts(), "\n")
				for _, v := range []string{"IBM Corporation", "Eclipse Adoptium", "Oracle Corporation", "Move JDK to Trash", "JDK downloads from the JetBrains catalog"} {
					if !strings.Contains(shown, v) {
						t.Fatal("missing distribution/control", v, shown)
					}
				}
				for _, row := range a.dev.javaInstallations {
					r, ok := tt.Find(row.Label())
					if !ok || r.X+r.W > float32(size[0]) {
						t.Fatal("JDK identity clipped", r, row.Label())
					}
				}
				resource018Screenshot(t, tt, fmt.Sprintf("java-distributions-%dx%d-dark-%t", size[0], size[1], dark))
				for n := 0; n < 18; n++ {
					r, ok := tt.Find("Move JDK to Trash")
					if ok && r.H >= 14 && r.Y >= 50 && r.Y+r.H < float32(size[1]-45) {
						break
					}
					tt.Scroll(float32(size[0]-20), float32(size[1]/2), 0, 140)
					tt.Frame()
				}
				r, ok := tt.Find("Move JDK to Trash")
				if !ok || r.H < 14 || r.Y < 50 || r.Y+r.H > float32(size[1]-45) {
					t.Fatal("confirmation not reachable", r)
				}
				resource018Screenshot(t, tt, fmt.Sprintf("java-removal-%dx%d-dark-%t", size[0], size[1], dark))
			})
		}
	}
}

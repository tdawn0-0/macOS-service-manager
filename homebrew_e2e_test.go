package main

import (
	"context"
	"errors"
	"fmt"
	"image/png"
	"macos-service-manager/internal/devplatform"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestUIHomebrewReviewRepositoryAndFailedInstall(t *testing.T) {
	for _, dark := range []bool{false, true} {
		for _, name := range []string{"php", "postgresql"} {
			t.Run(fmt.Sprintf("%s-dark-%t", name, dark), func(t *testing.T) {
				h := newDevUIHarnessSize(t, 900, 520, dark)
				base := h.a.dev.managerFactory
				formula, version, rack := "shivammathur/php/php", "8.5.99", "php"
				if name == "postgresql" {
					formula, version, rack = "postgresql@17", "17.99", "postgresql@17"
				}
				tapped, attempted, failInstallation := false, false, false
				h.a.dev.managerFactory = func() *devplatform.Manager {
					m := base()
					realRun := m.Run
					m.Run = func(ctx context.Context, program string, args ...string) (string, error) {
						if program == "/usr/bin/env" {
							if len(args) < 5 || args[3] != "/opt/homebrew/bin/brew" {
								return "", errors.New("unexpected Homebrew invocation")
							}
							switch args[4] {
							case "info":
								if failInstallation {
									attempted = true
									return "", errors.New("fixture Homebrew metadata unavailable")
								}
								return fmt.Sprintf(`{"formulae":[{"name":%q,"full_name":%q,"versions":{"stable":%q},"ruby_source_checksum":{"sha256":%q},"license":"test license"}]}`, rack, formula, version, strings.Repeat("a", 64)), nil
							case "tap":
								tapped = true
								return "", nil
							case "install":
								return "", errors.New("unexpected shared Homebrew install in UI fixture")
							}
							return "", errors.New("unexpected Homebrew mutation")
						}
						return realRun(ctx, program, args...)
					}
					return m
				}
				h.click("Software")
				h.click("Software " + name)
				if name == "php" {
					h.click("Add shivammathur/php repository")
					if !tapped {
						t.Fatal("repository action did not add the requested tap")
					}
				}
				h.click("Review package")
				if !h.a.dev.previewMatches() || h.a.dev.version != version || h.a.dev.packagePreview.Package.Formula != formula {
					t.Fatal("review failed to resolve the series to an exact Homebrew keg", h.a.dev.output)
				}
				if !strings.Contains(h.a.dev.packageDetailsText, "Formula SHA-256:") || strings.Contains(h.a.dev.packageDetailsText, "Postgres.app") {
					t.Fatal("review misrepresented the source or checksum", h.a.dev.packageDetailsText)
				}
				if dir := os.Getenv("MSM_HOMEBREW_SHOTS"); dir != "" {
					if err := os.MkdirAll(dir, 0755); err != nil {
						t.Fatal(err)
					}
					f, err := os.Create(filepath.Join(dir, fmt.Sprintf("%s-dark-%t.png", name, dark)))
					if err != nil {
						t.Fatal(err)
					}
					err = png.Encode(f, h.ui.Image())
					closeErr := f.Close()
					if err != nil || closeErr != nil {
						t.Fatal(err, closeErr)
					}
				}
				failInstallation = true
				h.click("Install with Homebrew")
				if !attempted || len(h.a.dev.state.Packages) != 0 || !strings.Contains(h.a.dev.output, "fixture Homebrew metadata unavailable") {
					t.Fatal("failed Homebrew installation was hidden or registered", h.a.dev.output)
				}
			})
		}
	}
}

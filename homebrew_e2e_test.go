package main

import (
	"context"
	"devbase/internal/devplatform"
	"errors"
	"fmt"
	"strings"
	"testing"
)

func TestUIHomebrewReviewAndFailedInstall(t *testing.T) {
	for _, dark := range []bool{false, true} {
		for _, name := range []string{"postgresql"} {
			t.Run(fmt.Sprintf("%s-dark-%t", name, dark), func(t *testing.T) {
				h := newDevUIHarnessSize(t, 900, 520, dark)
				base := h.a.dev.managerFactory
				formula, version, rack := "postgresql@17", "17.99", "postgresql@17"
				attempted, failInstallation := false, false
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
				h.click("Review package")
				if !h.a.dev.previewMatches() || h.a.dev.version != version || h.a.dev.packagePreview.Package.Formula != formula {
					t.Fatal("review failed to resolve the series to an exact Homebrew keg", h.a.dev.output)
				}
				if !strings.Contains(h.a.dev.packageDetailsText, "Formula SHA-256:") || strings.Contains(h.a.dev.packageDetailsText, "Postgres.app") {
					t.Fatal("review misrepresented the source or checksum", h.a.dev.packageDetailsText)
				}
				saveShot(t, fmt.Sprintf("%s-dark-%t", name, dark), h.ui.Image())
				failInstallation = true
				h.click("Install with Homebrew")
				if !attempted || len(h.a.dev.state.Packages) != 0 || !strings.Contains(h.a.dev.output, "fixture Homebrew metadata unavailable") {
					t.Fatal("failed Homebrew installation was hidden or registered", h.a.dev.output)
				}
			})
		}
	}
}

package devplatform

import (
	"bytes"
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func testFishPath(t *testing.T) string {
	t.Helper()
	if path := os.Getenv("MSM_TEST_FISH"); path != "" {
		return path
	}
	path, err := exec.LookPath("fish")
	if err != nil {
		t.Skip("fish is not installed; set MSM_TEST_FISH to a test binary")
	}
	return path
}

func TestBashFishRuntimeStartup(t *testing.T) {
	for _, shell := range []string{"bash", "fish"} {
		t.Run(shell, func(t *testing.T) {
			binary := "/bin/bash"
			if shell == "fish" {
				binary = testFishPath(t)
			}
			m, bins, project := runtimeShellFixture(t)
			m.shellAvailable = func(name string) bool { return name == "zsh" || name == "bash" || name == shell }
			enableRuntimeShellFixture(t, m)
			inspect := `node; command -v java; printf '%s\n' "$JAVA_HOME" "$PATH" "$npm_config_prefix"`
			if shell == "bash" {
				inspect = `node; type -P java; printf '%s\n' "$JAVA_HOME" "$PATH" "$npm_config_prefix"`
			}
			for _, mode := range []string{"-c", "-lc", "-ic", "-lic"} {
				for _, scenario := range []string{"default", "project", "cd", "nested", "nested-project", "from-zsh"} {
					t.Run(mode+"/"+scenario, func(t *testing.T) {
						dir, script, version := m.Home, inspect, "22.14.0"
						quote := shellQuote
						if shell == "fish" {
							quote = fishQuote
						}
						switch scenario {
						case "project":
							dir, version = project, "20.19.0"
						case "cd":
							script, version = "cd "+quote(project)+"; "+inspect, "20.19.0"
						case "nested":
							script = quote(binary) + " -c " + quote(inspect)
						case "nested-project":
							script, version = "/usr/bin/env -C "+quote(project)+" "+quote(binary)+" -c "+quote(inspect), "20.19.0"
						}
						cmd := exec.Command(binary, mode, script)
						if scenario == "from-zsh" {
							cmd = exec.Command("/bin/zsh", "-c", shellQuote(binary)+" "+mode+" "+shellQuote(inspect))
						}
						cmd.Dir = dir
						cmd.Env = []string{"HOME=" + m.Home, "PATH=/usr/bin:/bin:/usr/sbin:/sbin", "TERM=dumb"}
						if shell == "bash" && mode == "-c" && scenario != "from-zsh" {
							cmd.Env = append(cmd.Env, "BASH_ENV="+m.bashEnvPath())
						}
						var stderr bytes.Buffer
						cmd.Stderr = &stderr
						out, err := cmd.Output()
						if err != nil {
							t.Fatalf("%v stderr=%s stdout=%s", err, &stderr, out)
						}
						for _, line := range strings.Split(stderr.String(), "\n") {
							// Interactive Bash on a pipe has no controlling terminal.
							if line != "" && !strings.Contains(line, "cannot set terminal process group") && !strings.Contains(line, "no job control in this shell") {
								t.Fatalf("unexpected stderr: %s", &stderr)
							}
						}
						lines := strings.Split(strings.TrimSuffix(string(out), "\n"), "\n")
						if len(lines) != 5 || lines[0] != version || lines[1] != bins["java"] || lines[2] != filepath.Dir(filepath.Dir(bins["java"])) || lines[4] != m.nodeGlobalPrefix(version) {
							t.Fatalf("environment: %q", out)
						}
						for _, path := range []string{filepath.Dir(bins["java"]), filepath.Join(m.Root, "packages", "node@"+version+"-arm64", "bin"), filepath.Join(m.nodeGlobalPrefix(version), "bin")} {
							count := 0
							for _, entry := range strings.Split(lines[3], ":") {
								if entry == path {
									count++
								}
							}
							if count != 1 {
								t.Fatalf("%s occurs %d times: %s", path, count, lines[3])
							}
						}
						if version == "20.19.0" && strings.Contains(lines[3], "22.14.0") {
							t.Fatalf("stale Node: %s", lines[3])
						}
					})
				}
			}
			// Disable cleans all shells even if none are installed any longer.
			m.shellAvailable = func(string) bool { return false }
			for _, name := range []string{"java"} {
				if err := m.ConfigureRuntimeShell(context.Background(), name, false); err != nil {
					t.Fatal(err)
				}
			}
			if _, err := m.DisableNodeShell(context.Background()); err != nil {
				t.Fatal(err)
			}
			cmd := exec.Command(binary, "-c", `printf '%s\n' "$PATH" "$JAVA_HOME" "$npm_config_prefix"`)
			cmd.Env = []string{"HOME=" + m.Home, "PATH=/usr/bin:/bin", "BASH_ENV=" + m.bashEnvPath()}
			out, err := cmd.CombinedOutput()
			if err != nil || strings.Contains(string(out), m.Root) {
				t.Fatalf("disabled: %s %v", out, err)
			}
		})
	}
}

func TestRuntimeShellDetectsInstalledShellsAndLoginProfile(t *testing.T) {
	for _, profile := range []string{".bash_profile", ".bash_login", ".profile"} {
		t.Run(profile, func(t *testing.T) {
			m, _, _ := shellLinkFixture(t)
			m.shellAvailable = func(name string) bool { return name == "bash" }
			path := filepath.Join(m.Home, profile)
			original := []byte("export USER_SETTING=preserved\n")
			if err := os.WriteFile(path, original, 0640); err != nil {
				t.Fatal(err)
			}
			if _, err := m.EnableNodeShell(context.Background()); err != nil {
				t.Fatal(err)
			}
			for _, name := range []string{".zshenv", ".zprofile", ".zshrc", ".config"} {
				if _, err := os.Lstat(filepath.Join(m.Home, name)); !os.IsNotExist(err) {
					t.Fatalf("unneeded %s created: %v", name, err)
				}
			}
			if profile != ".bash_profile" {
				if _, err := os.Stat(filepath.Join(m.Home, ".bash_profile")); !os.IsNotExist(err) {
					t.Fatal("shadowed user's login profile")
				}
			}
			before, _ := os.Stat(path)
			if _, err := m.EnableNodeShell(context.Background()); err != nil {
				t.Fatal(err)
			}
			after, _ := os.Stat(path)
			if !os.SameFile(before, after) {
				t.Fatal("unchanged startup file rewritten")
			}
			m.shellAvailable = func(string) bool { return false }
			for range 2 {
				if _, err := m.DisableNodeShell(context.Background()); err != nil {
					t.Fatal(err)
				}
			}
			b, _ := os.ReadFile(path)
			if !bytes.Equal(b, original) {
				t.Fatalf("lost user's settings: %q", b)
			}
			if _, err := os.Stat(filepath.Join(m.Home, ".config")); !os.IsNotExist(err) {
				t.Fatal("disable created fish config")
			}
		})
	}
}

func TestRuntimeShellRemembersCustomLocations(t *testing.T) {
	m, _, _ := shellLinkFixture(t)
	t.Setenv("HOME", m.Home)
	m.Home = ""
	m.shellAvailable = func(string) bool { return true }
	zdot, xdg := t.TempDir(), t.TempDir()
	t.Setenv("ZDOTDIR", zdot)
	t.Setenv("XDG_CONFIG_HOME", xdg)
	if _, err := m.EnableNodeShell(context.Background()); err != nil {
		t.Fatal(err)
	}
	t.Setenv("ZDOTDIR", t.TempDir())
	t.Setenv("XDG_CONFIG_HOME", t.TempDir())
	m.shellAvailable = func(string) bool { return false }
	if _, err := m.DisableNodeShell(context.Background()); err != nil {
		t.Fatal(err)
	}
	for _, path := range []string{filepath.Join(zdot, ".zshenv"), filepath.Join(zdot, ".zprofile"), filepath.Join(zdot, ".zshrc"), filepath.Join(xdg, "fish", "conf.d", "devbase.fish")} {
		b, err := os.ReadFile(path)
		if err != nil || bytes.Contains(b, []byte(nodeBlockBegin)) {
			t.Fatalf("left stale config %s: %s %v", path, b, err)
		}
	}
}

func TestBashEnvironmentPreservesUserFile(t *testing.T) {
	m, _, _ := runtimeShellFixture(t)
	enableRuntimeShellFixture(t, m)
	prior := filepath.Join(m.Home, "user ' bash env")
	if err := os.WriteFile(prior, []byte(`export USER_ENV_COUNT=$((${USER_ENV_COUNT:-0}+1))`+"\n"), 0600); err != nil {
		t.Fatal(err)
	}
	for _, parent := range []string{"/bin/zsh", "/bin/bash"} {
		cmd := exec.Command(parent, "-lc", `/bin/bash -c 'printf "%s\n" "$USER_ENV_COUNT"; node'`)
		cmd.Dir = m.Home
		cmd.Env = []string{"HOME=" + m.Home, "PATH=/usr/bin:/bin", "BASH_ENV=" + prior}
		out, err := cmd.CombinedOutput()
		// Bash login itself also consumes the user's BASH_ENV, then its child does.
		count := "1"
		if parent == "/bin/bash" {
			count = "2"
		}
		if err != nil || string(out) != count+"\n22.14.0\n" {
			t.Fatalf("%s: %v %q", parent, err, out)
		}
	}
	cmd := exec.Command("/bin/sh", "-c", strings.Replace(m.BashCommand(), "YOUR_COMMAND", "node", 1))
	cmd.Dir = m.Home
	cmd.Env = []string{"HOME=" + m.Home, "PATH=/usr/bin:/bin"}
	if out, err := cmd.CombinedOutput(); err != nil || string(out) != "22.14.0\n" {
		t.Fatalf("standalone AI command: %v %q", err, out)
	}
}

func TestBashDynamicEnvironmentIsNotOverwritten(t *testing.T) {
	m, _, _ := runtimeShellFixture(t)
	enableRuntimeShellFixture(t, m)
	if err := os.WriteFile(filepath.Join(m.Home, "custom-env"), []byte("export USER_ENV_SEEN=yes\n"), 0600); err != nil {
		t.Fatal(err)
	}
	cmd := exec.Command("/bin/zsh", "-c", `printf '%s\n' "$BASH_ENV"; /bin/bash -c 'printf "%s\n" "$USER_ENV_SEEN"; node'`)
	cmd.Dir = m.Home
	cmd.Env = []string{"HOME=" + m.Home, "PATH=/usr/bin:/bin", "BASH_ENV=$HOME/custom-env"}
	out, err := cmd.CombinedOutput()
	if err != nil || string(out) != "$HOME/custom-env\nyes\n22.14.0\n" {
		t.Fatalf("%v %q", err, out)
	}
}

func TestBashNodeToolsFollowDirectoryChanges(t *testing.T) {
	m, _, project := runtimeShellFixture(t)
	enableRuntimeShellFixture(t, m)
	cmd := exec.Command("/bin/bash", "-c", "cd "+shellQuote(project)+"; npm; npx; corepack")
	cmd.Dir = m.Home
	cmd.Env = []string{"HOME=" + m.Home, "PATH=/usr/bin:/bin", "BASH_ENV=" + m.bashEnvPath()}
	out, err := cmd.CombinedOutput()
	if err != nil || string(out) != strings.Repeat("20.19.0\n", 3) {
		t.Fatalf("%v %q", err, out)
	}
}

func writeShellJavaFixture(t *testing.T, home, label, version, implementor, arch string) string {
	t.Helper()
	if err := os.MkdirAll(filepath.Join(home, "bin"), 0700); err != nil {
		t.Fatal(err)
	}
	for _, tool := range []string{"java", "javac"} {
		script := "#!/bin/sh\nprintf '%s:%s\\n' " + shellQuote(label) + " " + shellQuote(tool) + "\n"
		if err := os.WriteFile(filepath.Join(home, "bin", tool), []byte(script), 0755); err != nil {
			t.Fatal(err)
		}
	}
	release := "JAVA_VERSION=\"" + version + "\"\nIMPLEMENTOR=\"" + implementor + "\"\nOS_ARCH=\"" + arch + "\"\n"
	if err := os.WriteFile(filepath.Join(home, "release"), []byte(release), 0600); err != nil {
		t.Fatal(err)
	}
	return home
}

func assertPathSingleEntry(t *testing.T, pathValue, wanted string, forbidden ...string) {
	t.Helper()
	entries := strings.Split(pathValue, ":")
	count := 0
	for _, entry := range entries {
		if entry == wanted {
			count++
		}
		for _, bad := range forbidden {
			if bad != "" && bad != wanted && entry == bad {
				t.Fatalf("stale PATH entry %s remains in %s", bad, pathValue)
			}
		}
	}
	if count != 1 {
		t.Fatalf("%s occurs %d times in PATH: %s", wanted, count, pathValue)
	}
}

func TestJavaPerDirectoryShellResolution(t *testing.T) {
	t.Setenv("ZDOTDIR", "")
	ctx := context.Background()
	m := New(filepath.Join(t.TempDir(), "managed runtimes ' [x]"))
	m.Home = t.TempDir()
	m.Program = filepath.Join(t.TempDir(), "app")
	if err := os.WriteFile(m.Program, []byte("#!/bin/sh\n"), 0700); err != nil {
		t.Fatal(err)
	}
	m.shellAvailable = func(name string) bool { return name == "zsh" || name == "bash" || name == "fish" }

	jvmDir := m.JavaInstallationDirectory()
	defaultHome := writeShellJavaFixture(t, filepath.Join(jvmDir, "temurin-21.0.1", "Contents", "Home"), "temurin-21", "21.0.1", "Eclipse Adoptium", "aarch64")
	for _, tool := range []string{"java", "javac"} {
		toolPath := filepath.Join(defaultHome, "bin", tool)
		if err := os.Remove(toolPath); err != nil {
			t.Fatal(err)
		}
		if err := os.Symlink("/bin/echo", toolPath); err != nil {
			t.Fatal(err)
		}
	}
	semeruHome := writeShellJavaFixture(t, filepath.Join(jvmDir, "semeru-25.0.4.15", "Contents", "Home"), "semeru-25", "25.0.4.1", "IBM Corporation", "aarch64")
	libericaHome := writeShellJavaFixture(t, filepath.Join(jvmDir, "liberica-17.0.12"), "liberica-17", "17.0.12", "BellSoft", "aarch64")
	correttoHome := writeShellJavaFixture(t, filepath.Join(jvmDir, "corretto-17.0.9", "Contents", "Home"), "corretto-17", "17.0.9", "Amazon.com Inc.", "aarch64")
	writeShellJavaFixture(t, filepath.Join(jvmDir, "zulu-21.jdk", "Contents", "Home"), "intel-zulu-21", "21.0.5", "Azul Systems, Inc.", "x86_64")

	p := Package{Name: "java", Version: "21.0.1", JavaInstallName: "temurin-21.0.1", Arch: "arm64", URL: "https://example.invalid/jdk.tar.gz", SHA256: strings.Repeat("a", 64), Format: "tar.gz", Binaries: map[string]string{"java": "Contents/Home/bin/java", "javac": "Contents/Home/bin/javac"}, License: "GPL-2.0"}
	if err := os.WriteFile(filepath.Join(m.packageDir(p), javaOwnerFile), []byte(m.javaOwner(p)), 0600); err != nil {
		t.Fatal(err)
	}
	if err := m.withState(ctx, func(s *State) error {
		s.Packages[p.ID()] = p
		return nil
	}); err != nil {
		t.Fatal(err)
	}
	if err := m.SetRuntimeDefault(ctx, "java", p.ID()); err != nil {
		t.Fatal(err)
	}
	if err := m.ConfigureRuntimeShell(ctx, "java", true); err != nil {
		t.Fatal(err)
	}

	initSh, err := os.ReadFile(m.runtimeScript("java"))
	if err != nil {
		t.Fatal(err)
	}
	for _, shell := range []string{"/bin/zsh", "/bin/bash"} {
		if out, err := exec.Command(shell, "-n", m.runtimeScript("java")).CombinedOutput(); err != nil {
			t.Fatalf("%s -n failed: %v %s", shell, err, out)
		}
	}
	if !strings.Contains(string(initSh), shellLiteral(jvmDir)) {
		t.Fatalf("init.sh missing jvmDir %q", jvmDir)
	}
	fishScriptPath := strings.TrimSuffix(m.runtimeScript("java"), ".sh") + ".fish"
	fishBytes, err := os.ReadFile(fishScriptPath)
	if err != nil {
		t.Fatal(err)
	}
	for _, want := range []string{"--on-variable PWD", ".java-version", ".sdkmanrc", "OS_ARCH", fishQuote(jvmDir), fishQuote(defaultHome)} {
		if !strings.Contains(string(fishBytes), want) {
			t.Fatalf("init.fish missing %q", want)
		}
	}

	projects := t.TempDir()
	exactDir := filepath.Join(projects, "exact")
	prefixDir := filepath.Join(projects, "prefix")
	flatDir := filepath.Join(projects, "flat")
	sdkDir := filepath.Join(projects, "sdk")
	nestedParent := filepath.Join(projects, "parent")
	nestedChild := filepath.Join(nestedParent, "sub", "deep")
	overrideChild := filepath.Join(nestedParent, "override", "deep")
	intelNameDir := filepath.Join(projects, "intel-name")
	intelVerDir := filepath.Join(projects, "intel-ver")
	unknownDir := filepath.Join(projects, "unknown")
	for _, dir := range []string{exactDir, prefixDir, flatDir, sdkDir, nestedChild, overrideChild, intelNameDir, intelVerDir, unknownDir} {
		if err := os.MkdirAll(dir, 0700); err != nil {
			t.Fatal(err)
		}
	}
	mustWrite := func(dir, name, content string) {
		t.Helper()
		if err := os.WriteFile(filepath.Join(dir, name), []byte(content), 0600); err != nil {
			t.Fatal(err)
		}
	}
	mustWrite(exactDir, ".java-version", "# pinned JDK\n\n  semeru-25.0.4.15  # inline\n")
	mustWrite(prefixDir, ".java-version", "semeru-25\n")
	mustWrite(flatDir, ".java-version", "17.0.12\n")
	mustWrite(sdkDir, ".sdkmanrc", "# sdkman\njava=17.0.9-amzn\nmaven=3.9.6\n")
	mustWrite(nestedParent, ".java-version", "semeru-25\n")
	mustWrite(filepath.Join(nestedParent, "override"), ".sdkmanrc", "java=25-sem\n")
	mustWrite(filepath.Join(nestedParent, "override"), ".java-version", "17.0.12\n")
	mustWrite(intelNameDir, ".java-version", "zulu-21.jdk\n")
	mustWrite(intelVerDir, ".java-version", "21.0.5\n")
	mustWrite(unknownDir, ".java-version", "99.0.0\n")

	allBins := []string{
		filepath.Join(defaultHome, "bin"),
		filepath.Join(semeruHome, "bin"),
		filepath.Join(libericaHome, "bin"),
		filepath.Join(correttoHome, "bin"),
	}

	cases := []struct {
		name     string
		dir      string
		wantHome string
		wantTool string
	}{
		{"default", m.Home, defaultHome, "default"},
		{"exact", exactDir, semeruHome, "semeru-25"},
		{"prefix", prefixDir, semeruHome, "semeru-25"},
		{"flat-java-version", flatDir, libericaHome, "liberica-17"},
		{"sdkmanrc", sdkDir, correttoHome, "corretto-17"},
		{"inherited-parent", nestedChild, semeruHome, "semeru-25"},
		{"child-java-version-beats-parent", overrideChild, libericaHome, "liberica-17"},
		{"skips-intel-dir", intelNameDir, defaultHome, "default"},
		{"skips-intel-version", intelVerDir, defaultHome, "default"},
		{"unmatched-falls-back", unknownDir, defaultHome, "default"},
	}

	t.Run("zsh", func(t *testing.T) {
		for _, mode := range []string{"-c", "-lc", "-ic", "-lic"} {
			for _, tc := range cases {
				t.Run(mode+"/"+tc.name, func(t *testing.T) {
					script := "cd " + shellQuote(tc.dir) + `; printf '%s\n%s\n%s\n%s\n' "$(command -v java)" "$(command -v javac)" "$JAVA_HOME" "$PATH"; java ok; javac ok`
					cmd := exec.Command("/bin/zsh", mode, script)
					cmd.Dir = m.Home
					cmd.Env = []string{"HOME=" + m.Home, "PATH=/usr/bin:/bin:/usr/sbin:/sbin", "TERM=dumb"}
					var stderr bytes.Buffer
					cmd.Stderr = &stderr
					out, err := cmd.Output()
					if err != nil || stderr.Len() != 0 {
						t.Fatalf("%v stderr=%s stdout=%s", err, &stderr, out)
					}
					lines := strings.Split(strings.TrimSuffix(string(out), "\n"), "\n")
					wantBin := filepath.Join(tc.wantHome, "bin")
					wantJava, wantJavac := tc.wantTool+":java", tc.wantTool+":javac"
					if tc.wantTool == "default" {
						wantJava, wantJavac = "ok", "ok"
					}
					if len(lines) != 6 || lines[0] != filepath.Join(wantBin, "java") || lines[1] != filepath.Join(wantBin, "javac") || lines[2] != tc.wantHome || lines[4] != wantJava || lines[5] != wantJavac {
						t.Fatalf("unexpected output: %q", out)
					}
					assertPathSingleEntry(t, lines[3], wantBin, allBins...)
				})
			}
			t.Run(mode+"/switch-and-return", func(t *testing.T) {
				script := "cd " + shellQuote(exactDir) + "; java; cd " + shellQuote(flatDir) + "; javac; cd " + shellQuote(m.Home) + `; printf '%s\n%s\n' "$JAVA_HOME" "$PATH"; java ok`
				cmd := exec.Command("/bin/zsh", mode, script)
				cmd.Dir = m.Home
				cmd.Env = []string{"HOME=" + m.Home, "PATH=/usr/bin:/bin:/usr/sbin:/sbin", "TERM=dumb"}
				out, err := cmd.CombinedOutput()
				if err != nil {
					t.Fatalf("%v %s", err, out)
				}
				lines := strings.Split(strings.TrimSuffix(string(out), "\n"), "\n")
				if len(lines) != 5 || lines[0] != "semeru-25:java" || lines[1] != "liberica-17:javac" || lines[2] != defaultHome || lines[4] != "ok" {
					t.Fatalf("unexpected output: %q", out)
				}
				assertPathSingleEntry(t, lines[3], filepath.Join(defaultHome, "bin"), allBins...)
			})
		}
	})

	t.Run("bash", func(t *testing.T) {
		for _, mode := range []string{"-c", "-lc", "-ic", "-lic"} {
			for _, tc := range cases {
				t.Run(mode+"/"+tc.name, func(t *testing.T) {
					script := "cd " + shellQuote(tc.dir) + `; java ok; javac ok; printf '%s\n%s\n%s\n%s\n' "$(type -P java)" "$(type -P javac)" "$JAVA_HOME" "$PATH"`
					cmd := exec.Command("/bin/bash", mode, script)
					cmd.Dir = m.Home
					cmd.Env = []string{"HOME=" + m.Home, "PATH=/usr/bin:/bin:/usr/sbin:/sbin", "TERM=dumb"}
					if mode == "-c" {
						cmd.Env = append(cmd.Env, "BASH_ENV="+m.bashEnvPath())
					}
					var stderr bytes.Buffer
					cmd.Stderr = &stderr
					out, err := cmd.Output()
					if err != nil {
						t.Fatalf("%v stderr=%s stdout=%s", err, &stderr, out)
					}
					for _, line := range strings.Split(stderr.String(), "\n") {
						if line != "" && !strings.Contains(line, "cannot set terminal process group") && !strings.Contains(line, "no job control in this shell") {
							t.Fatalf("unexpected stderr: %s", &stderr)
						}
					}
					lines := strings.Split(strings.TrimSuffix(string(out), "\n"), "\n")
					wantBin := filepath.Join(tc.wantHome, "bin")
					wantJava, wantJavac := tc.wantTool+":java", tc.wantTool+":javac"
					if tc.wantTool == "default" {
						wantJava, wantJavac = "ok", "ok"
					}
					if len(lines) != 6 || lines[0] != wantJava || lines[1] != wantJavac || lines[2] != filepath.Join(wantBin, "java") || lines[3] != filepath.Join(wantBin, "javac") || lines[4] != tc.wantHome {
						t.Fatalf("unexpected output: %q", out)
					}
					assertPathSingleEntry(t, lines[5], wantBin, allBins...)
				})
			}
			t.Run(mode+"/switch-and-return", func(t *testing.T) {
				script := "cd " + shellQuote(exactDir) + "; java; cd " + shellQuote(flatDir) + "; javac; cd " + shellQuote(m.Home) + `; java ok; printf '%s\n%s\n' "$JAVA_HOME" "$PATH"`
				cmd := exec.Command("/bin/bash", mode, script)
				cmd.Dir = m.Home
				cmd.Env = []string{"HOME=" + m.Home, "PATH=/usr/bin:/bin:/usr/sbin:/sbin", "TERM=dumb"}
				if mode == "-c" {
					cmd.Env = append(cmd.Env, "BASH_ENV="+m.bashEnvPath())
				}
				out, err := cmd.Output()
				if err != nil {
					t.Fatalf("%v %s", err, out)
				}
				lines := strings.Split(strings.TrimSuffix(string(out), "\n"), "\n")
				if len(lines) != 5 || lines[0] != "semeru-25:java" || lines[1] != "liberica-17:javac" || lines[2] != "ok" || lines[3] != defaultHome {
					t.Fatalf("unexpected output: %q", out)
				}
				assertPathSingleEntry(t, lines[4], filepath.Join(defaultHome, "bin"), allBins...)
			})
		}

		t.Run("interactive-prompt-command", func(t *testing.T) {
			stdin := "cd " + shellQuote(exactDir) + "\n" +
				`printf '%s|%s\n' "$JAVA_HOME" "$(type -P java)"` + "\n" +
				"cd " + shellQuote(sdkDir) + "\n" +
				`printf '%s|%s\n' "$JAVA_HOME" "$(type -P java)"` + "\n" +
				"cd " + shellQuote(m.Home) + "\n" +
				`printf '%s|%s\n' "$JAVA_HOME" "$(type -P java)"` + "\n"
			cmd := exec.Command("/bin/bash", "-i")
			cmd.Dir = m.Home
			cmd.Env = []string{"HOME=" + m.Home, "PATH=/usr/bin:/bin", "TERM=dumb", "PS1="}
			cmd.Stdin = strings.NewReader(stdin)
			out, err := cmd.Output()
			if err != nil {
				t.Fatalf("%v %s", err, out)
			}
			got := string(out)
			for _, want := range []string{
				semeruHome + "|" + filepath.Join(semeruHome, "bin", "java"),
				correttoHome + "|" + filepath.Join(correttoHome, "bin", "java"),
				defaultHome + "|" + filepath.Join(defaultHome, "bin", "java"),
			} {
				if !strings.Contains(got, want) {
					t.Fatalf("missing %q in %q", want, got)
				}
			}
		})

		t.Run("preserves-existing-java-function", func(t *testing.T) {
			cmd := exec.Command("/bin/bash", "-c", `java() { echo "custom-java"; }; . `+shellQuote(m.runtimeScript("java"))+`; java`)
			cmd.Dir = m.Home
			cmd.Env = []string{"HOME=" + m.Home, "PATH=/usr/bin:/bin"}
			out, err := cmd.CombinedOutput()
			if err != nil || strings.TrimSpace(string(out)) != "custom-java" {
				t.Fatalf("existing java function was overwritten: %v %q", err, out)
			}
		})
	})

	t.Run("fish", func(t *testing.T) {
		binary := testFishPath(t)
		for _, mode := range []string{"-c", "-lc", "-ic", "-lic"} {
			for _, tc := range cases {
				t.Run(mode+"/"+tc.name, func(t *testing.T) {
					script := "cd " + fishQuote(tc.dir) + `; printf '%s\n%s\n%s\n%s\n' (command -v java) (command -v javac) "$JAVA_HOME" (string join : $PATH); java ok; javac ok`
					cmd := exec.Command(binary, mode, script)
					cmd.Dir = m.Home
					cmd.Env = []string{"HOME=" + m.Home, "PATH=/usr/bin:/bin:/usr/sbin:/sbin", "TERM=dumb"}
					out, err := cmd.CombinedOutput()
					if err != nil {
						t.Fatalf("%v %s", err, out)
					}
					lines := strings.Split(strings.TrimSuffix(string(out), "\n"), "\n")
					wantBin := filepath.Join(tc.wantHome, "bin")
					wantJava, wantJavac := tc.wantTool+":java", tc.wantTool+":javac"
					if tc.wantTool == "default" {
						wantJava, wantJavac = "ok", "ok"
					}
					if len(lines) != 6 || lines[0] != filepath.Join(wantBin, "java") || lines[1] != filepath.Join(wantBin, "javac") || lines[2] != tc.wantHome || lines[4] != wantJava || lines[5] != wantJavac {
						t.Fatalf("unexpected output: %q", out)
					}
					assertPathSingleEntry(t, lines[3], wantBin, allBins...)
				})
			}
		}
	})
}

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
			inspect := `node; command -v java; command -v go; printf '%s\n' "$JAVA_HOME" "$PATH" "$npm_config_prefix"`
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
						if len(lines) != 6 || lines[0] != version || lines[1] != bins["java"] || lines[2] != bins["go"] || lines[3] != filepath.Dir(filepath.Dir(bins["java"])) || lines[5] != m.nodeGlobalPrefix(version) {
							t.Fatalf("environment: %q", out)
						}
						for _, path := range []string{filepath.Dir(bins["java"]), filepath.Dir(bins["go"]), filepath.Join(m.Root, "packages", "node@"+version+"-arm64", "bin"), filepath.Join(m.nodeGlobalPrefix(version), "bin")} {
							count := 0
							for _, entry := range strings.Split(lines[4], ":") {
								if entry == path {
									count++
								}
							}
							if count != 1 {
								t.Fatalf("%s occurs %d times: %s", path, count, lines[4])
							}
						}
						if version == "20.19.0" && strings.Contains(lines[4], "22.14.0") {
							t.Fatalf("stale Node: %s", lines[4])
						}
					})
				}
			}
			// Disable cleans all shells even if none are installed any longer.
			m.shellAvailable = func(string) bool { return false }
			for _, name := range []string{"java", "go"} {
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
	for _, path := range []string{filepath.Join(zdot, ".zshenv"), filepath.Join(zdot, ".zprofile"), filepath.Join(zdot, ".zshrc"), filepath.Join(xdg, "fish", "conf.d", "service-manager.fish")} {
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

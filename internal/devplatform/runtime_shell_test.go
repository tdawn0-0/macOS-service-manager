package devplatform

import (
	"bytes"
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

// Run the real selector in a child process without building another binary.
func TestRuntimeShellResolverProcess(t *testing.T) {
	root := os.Getenv("MSM_SHELL_TEST_ROOT")
	if root == "" {
		return
	}
	dir, err := os.Getwd()
	if err != nil {
		panic(err)
	}
	sel, err := New(root).SelectNode(dir, "")
	if err != nil {
		panic(err)
	}
	if !sel.System {
		fmt.Printf("%s\n%s\n", sel.Path(), sel.Prefix)
	}
	os.Exit(0)
}

func TestRuntimeShellStartupModes(t *testing.T) {
	t.Setenv("ZDOTDIR", "")
	ctx := context.Background()
	m := New(filepath.Join(t.TempDir(), "managed runtimes ' [x]"))
	m.Home = t.TempDir()
	installedNodeFixture(t, m, "22.14.0", "20.19.0")
	if err := m.SetNodeDefault(ctx, "node@22.14.0-arm64"); err != nil {
		t.Fatal(err)
	}
	for _, version := range []string{"22.14.0", "20.19.0"} {
		path := filepath.Join(m.Root, "packages", "node@"+version+"-arm64", "bin", "node")
		if err := os.WriteFile(path, []byte("#!/bin/sh\nprintf '%s\\n' "+shellQuote(version)+"\n"), 0700); err != nil {
			t.Fatal(err)
		}
	}
	testExe, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	m.Program = filepath.Join(t.TempDir(), "resolver")
	helper := "#!/bin/sh\nGORACE=atexit_sleep_ms=0 MSM_SHELL_TEST_ROOT=" + shellQuote(m.Root) + " exec " + shellQuote(testExe) + " -test.run '^TestRuntimeShellResolverProcess$'\n"
	if err := os.WriteFile(m.Program, []byte(helper), 0700); err != nil {
		t.Fatal(err)
	}
	bins := map[string]string{}
	for _, name := range []string{"java", "go"} {
		p := Package{Name: name, Version: "21.0.1", Arch: "arm64", Binaries: map[string]string{name: "bin/" + name}}
		if name == "java" {
			p.Binaries[name] = "Contents/Home/bin/java"
		}
		p = cacheRuntimeArchive(t, m, p)
		if err := m.Install(ctx, p); err != nil {
			t.Fatal(err)
		}
		if err := m.SetRuntimeDefault(ctx, name, p.ID()); err != nil {
			t.Fatal(err)
		}
		bins[name] = filepath.Join(m.packageDir(p), p.Binaries[name])
	}
	// Simulate an old rc-only setup; explicitly enabling upgrades it.
	if err := os.WriteFile(filepath.Join(m.Home, ".zshrc"), []byte(m.nodeBlock()), 0600); err != nil {
		t.Fatal(err)
	}
	if m.NodeShellEnabled() {
		t.Fatal("legacy rc-only integration reported full coverage")
	}
	for range 2 {
		if _, err := m.EnableNodeShell(ctx); err != nil {
			t.Fatal(err)
		}
		for _, name := range []string{"java", "go"} {
			if err := m.ConfigureRuntimeShell(ctx, name, true); err != nil {
				t.Fatal(err)
			}
		}
	}
	project := filepath.Join(m.Home, "project with ' quotes")
	if err := os.Mkdir(project, 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(project, ".nvmrc"), []byte("20\n"), 0600); err != nil {
		t.Fatal(err)
	}
	inspect := `node; command -v java; command -v go; printf '%s\n' "$JAVA_HOME" "$PATH" "$npm_config_prefix"`
	for _, mode := range []string{"-c", "-lc", "-ic", "-lic"} {
		for _, scenario := range []string{"default", "project", "cd", "nested", "nested-project"} {
			t.Run(mode+"/"+scenario, func(t *testing.T) {
				dir, script, version := m.Home, inspect, "22.14.0"
				switch scenario {
				case "project":
					dir, version = project, "20.19.0"
				case "cd":
					script, version = "cd "+shellQuote(project)+"; "+inspect, "20.19.0"
				case "nested":
					script = "/bin/zsh " + mode + " " + shellQuote(inspect)
				case "nested-project":
					script, version = "/usr/bin/env -C "+shellQuote(project)+" /bin/zsh "+mode+" "+shellQuote(inspect), "20.19.0"
				}
				cmd := exec.Command("/bin/zsh", mode, script)
				cmd.Dir = dir
				cmd.Env = []string{"HOME=" + m.Home, "PATH=/usr/bin:/bin:/usr/sbin:/sbin", "TERM=dumb"}
				var stderr bytes.Buffer
				cmd.Stderr = &stderr
				out, err := cmd.Output()
				if err != nil || stderr.Len() != 0 {
					t.Fatalf("%v stderr=%s stdout=%s", err, &stderr, out)
				}
				lines := strings.Split(strings.TrimSuffix(string(out), "\n"), "\n")
				if len(lines) != 6 || lines[0] != version || lines[1] != bins["java"] || lines[2] != bins["go"] || lines[3] != filepath.Dir(filepath.Dir(bins["java"])) || lines[5] != m.nodeGlobalPrefix(version) {
					t.Fatalf("unexpected environment: %q", out)
				}
				entries := strings.Split(lines[4], ":")
				for _, bin := range []string{filepath.Dir(bins["java"]), filepath.Dir(bins["go"]), filepath.Join(m.Root, "packages", "node@"+version+"-arm64", "bin"), filepath.Join(m.nodeGlobalPrefix(version), "bin")} {
					count := 0
					for _, entry := range entries {
						if entry == bin {
							count++
						}
					}
					if count != 1 {
						t.Fatalf("%s occurs %d times in PATH: %s", bin, count, lines[4])
					}
				}
				if version == "20.19.0" && strings.Contains(lines[4], "22.14.0") {
					t.Fatalf("inherited old Node PATH: %s", lines[4])
				}
			})
		}
	}
	for _, name := range []string{"java", "go"} {
		if err := m.ConfigureRuntimeShell(ctx, name, false); err != nil {
			t.Fatal(err)
		}
	}
	if _, err := m.DisableNodeShell(ctx); err != nil {
		t.Fatal(err)
	}
	for _, mode := range []string{"-c", "-lc", "-ic", "-lic"} {
		cmd := exec.Command("/bin/zsh", mode, `printf '%s\n' "$PATH" "$JAVA_HOME" "$npm_config_prefix"`)
		cmd.Env = []string{"HOME=" + m.Home, "PATH=/usr/bin:/bin", "TERM=dumb"}
		out, err := cmd.CombinedOutput()
		if err != nil || strings.Contains(string(out), m.Root) {
			t.Fatalf("disable %s: %s %v", mode, out, err)
		}
	}
}

func TestRuntimeShellPreflightAndZDOTDIR(t *testing.T) {
	m, _, _ := shellLinkFixture(t)
	home := m.Home
	t.Setenv("HOME", home)
	m.Home = ""
	dir := t.TempDir()
	t.Setenv("ZDOTDIR", dir)
	paths, err := m.zshStartupFiles()
	if err != nil {
		t.Fatal(err)
	}
	original := []byte("export EDITOR=vim\n")
	for _, path := range paths {
		if err := os.WriteFile(path, original, 0640); err != nil {
			t.Fatal(err)
		}
	}
	broken := []byte(nodeBlockBegin + "\n")
	if err := os.WriteFile(paths[2], broken, 0640); err != nil {
		t.Fatal(err)
	}
	if _, err := m.EnableNodeShell(context.Background()); err == nil {
		t.Fatal("accepted damaged block")
	}
	for _, path := range paths[:2] {
		b, _ := os.ReadFile(path)
		if !bytes.Equal(b, original) {
			t.Fatal("partially wrote startup files")
		}
	}
	if err := os.WriteFile(paths[2], original, 0640); err != nil {
		t.Fatal(err)
	}
	if _, err := m.EnableNodeShell(context.Background()); err != nil {
		t.Fatal(err)
	}
	if _, err := m.DisableNodeShell(context.Background()); err != nil {
		t.Fatal(err)
	}
	for _, path := range paths {
		b, _ := os.ReadFile(path)
		st, _ := os.Stat(path)
		if !bytes.Equal(b, original) || st.Mode().Perm() != 0640 {
			t.Fatalf("changed user config %s: %q", path, b)
		}
	}
	if _, err := os.Stat(filepath.Join(home, ".zshrc")); !os.IsNotExist(err) {
		t.Fatal("ignored ZDOTDIR")
	}
}

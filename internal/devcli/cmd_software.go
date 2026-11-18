package devcli

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"

	"devbase/internal/devplatform"
)

var softwareCatalogOrder = []string{"node", "java", "python", "mysql", "postgresql", "redis", "mongodb"}

func primaryBinaryName(kind string) string {
	switch kind {
	case "node":
		return "node"
	case "java":
		return "java"
	case "mysql":
		return "mysqld"
	case "postgresql":
		return "postgres"
	case "redis":
		return "redis-server"
	case "mongodb":
		return "mongod"
	default:
		return kind
	}
}

func isPackageUnused(s devplatform.State, id string) bool {
	p, ok := s.Packages[id]
	if !ok {
		return false
	}
	switch p.Name {
	case "mysql", "postgresql", "redis", "mongodb":
	default:
		return false
	}
	for _, inst := range s.Instances {
		if inst.Package == id {
			return false
		}
	}
	return true
}

func resolveInstalledPackageID(s devplatform.State, args []string) (string, error) {
	if len(args) == 1 {
		id := strings.TrimSpace(args[0])
		if _, ok := s.Packages[id]; ok {
			return id, nil
		}
		return "", fmt.Errorf("installed package %q not found", id)
	}
	if len(args) == 2 {
		name := strings.ToLower(strings.TrimSpace(args[0]))
		ver := strings.TrimPrefix(strings.TrimSpace(args[1]), "v")
		var matches []string
		for id, p := range s.Packages {
			if p.Name == name && p.Version == ver {
				matches = append(matches, id)
			}
		}
		if len(matches) == 1 {
			return matches[0], nil
		}
		if len(matches) > 1 {
			return "", fmt.Errorf("multiple installed packages match %s %s: %s", name, ver, strings.Join(matches, ", "))
		}
		return "", fmt.Errorf("installed package %s %s not found", name, ver)
	}
	return "", usageErrorf("expected <package-id> or <software> <version>")
}

func matchJavaInstallation(rows []devplatform.JavaInstallation, target string) (devplatform.JavaInstallation, error) {
	target = strings.TrimSpace(target)
	if target == "" {
		return devplatform.JavaInstallation{}, usageErrorf("Java installation identifier is required")
	}
	for _, r := range rows {
		if r.ID == target || r.PackageID == target || filepath.Base(r.Path) == target || r.Path == target {
			return r, nil
		}
	}
	var versionMatches []devplatform.JavaInstallation
	for _, r := range rows {
		if r.Version == target {
			versionMatches = append(versionMatches, r)
		}
	}
	if len(versionMatches) == 1 {
		return versionMatches[0], nil
	}
	if len(versionMatches) > 1 {
		names := make([]string, 0, len(versionMatches))
		for _, r := range versionMatches {
			names = append(names, filepath.Base(r.Path)+" ("+r.ID+")")
		}
		return devplatform.JavaInstallation{}, fmt.Errorf("multiple JDKs match version %s; choose folder name or ID: %s", target, strings.Join(names, ", "))
	}
	return devplatform.JavaInstallation{}, fmt.Errorf("Java installation %q not found", target)
}

func matchUVPython(rows []devplatform.UVPython, target string) (devplatform.UVPython, error) {
	target = strings.TrimSpace(target)
	for _, r := range rows {
		if r.Key == target {
			return r, nil
		}
	}
	var matches []devplatform.UVPython
	for _, r := range rows {
		if r.Version == target {
			matches = append(matches, r)
		}
	}
	if len(matches) == 1 {
		return matches[0], nil
	}
	if len(matches) > 1 {
		keys := make([]string, 0, len(matches))
		for _, r := range matches {
			keys = append(keys, r.Key)
		}
		return devplatform.UVPython{}, fmt.Errorf("multiple Python versions match %s; choose key: %s", target, strings.Join(keys, ", "))
	}
	return devplatform.UVPython{}, fmt.Errorf("uv Python %q not found", target)
}

func runSoftwareCommand(ctx context.Context, env *Env, args []string) error {
	if hasHelpFlag(args) && len(args) == 1 {
		printTopicHelp(env.Out, "software")
		return nil
	}
	sub := "list"
	if len(args) > 0 {
		sub = args[0]
		args = args[1:]
	}
	if hasHelpFlag(args) {
		printTopicHelp(env.Out, "software")
		return nil
	}
	switch sub {
	case "list":
		return runSoftwareList(ctx, env, args)
	case "versions":
		return runSoftwareVersions(ctx, env, args)
	case "review":
		return runSoftwareReview(ctx, env, args)
	case "install":
		return runSoftwareInstall(ctx, env, args)
	case "uninstall":
		return runSoftwareUninstall(ctx, env, args)
	case "verify":
		return runSoftwareVerify(ctx, env, args)
	case "repair":
		return runSoftwareRepair(ctx, env, args)
	case "default":
		return runSoftwareDefault(ctx, env, args)
	case "shell":
		return runSoftwareShell(ctx, env, args)
	case "which":
		return runSoftwareWhich(ctx, env, args)
	case "uv-path":
		return runSoftwareUVPath(ctx, env, args)
	default:
		return usageErrorf("unknown software subcommand %q", sub)
	}
}

func runSoftwareList(ctx context.Context, env *Env, args []string) error {
	if len(args) > 0 {
		return usageErrorf("unexpected argument %q for software list", args[0])
	}
	m := env.manager()
	s, err := m.Read()
	if err != nil {
		return err
	}
	javaRows, javaErr := m.JavaInstallations(ctx)
	uvPath, uvErr := m.UVPath()
	var uvDesc, uvPin string
	var uvInstalled []devplatform.UVPython
	if uvErr == nil {
		uvDesc, _ = m.UVDescription(ctx)
		uvInstalled, _ = m.UVPythons(ctx, true)
		uvPin, _ = m.UVGlobalPin(ctx)
	}

	type pkgView struct {
		ID      string            `json:"id"`
		Package devplatform.Package `json:"package"`
		Default bool              `json:"default,omitempty"`
		Unused  bool              `json:"unused,omitempty"`
	}
	packages := make([]pkgView, 0, len(s.Packages))
	for _, id := range sortedMapKeys(s.Packages) {
		p := s.Packages[id]
		isDef := (p.Name == "node" && s.NodeDefault == id) || (p.Name == "java" && s.RuntimeDefaults["java"] == id)
		packages = append(packages, pkgView{
			ID:      id,
			Package: p,
			Default: isDef,
			Unused:  isPackageUnused(s, id),
		})
	}
	payload := map[string]any{
		"catalog":          softwareCatalogOrder,
		"packages":         packages,
		"nodeDefault":      s.NodeDefault,
		"runtimeDefaults":  s.RuntimeDefaults,
		"nodeShellEnabled": m.NodeShellEnabled(),
		"javaShellEnabled": m.RuntimeShellEnabled("java"),
		"javaInstallDir":   m.JavaInstallationDirectory(),
		"javaInstallations": javaRows,
		"uv": map[string]any{
			"path":        uvPath,
			"description": uvDesc,
			"globalPin":   uvPin,
			"installed":   uvInstalled,
		},
	}
	if javaErr != nil {
		payload["javaError"] = javaErr.Error()
	}
	if uvErr != nil {
		payload["uvError"] = uvErr.Error()
	}
	return writeResult(env, payload, func(w io.Writer) {
		fmt.Fprintf(w, "Installed packages (%d):\n", len(packages))
		for _, pv := range packages {
			tags := []string{}
			if pv.Default {
				tags = append(tags, "default")
			}
			if pv.Unused {
				tags = append(tags, "unused")
			}
			if pv.Package.Formula != "" {
				tags = append(tags, "homebrew")
			}
			tagText := ""
			if len(tags) > 0 {
				tagText = " [" + strings.Join(tags, ", ") + "]"
			}
			fmt.Fprintf(w, "  - %s (%s %s)%s\n", pv.ID, pv.Package.Name, pv.Package.Version, tagText)
		}
		fmt.Fprintf(w, "Node shell integration: %t (default: %s)\n", m.NodeShellEnabled(), s.NodeDefault)
		fmt.Fprintf(w, "Java shell integration: %t (default: %s, dir: %s)\n", m.RuntimeShellEnabled("java"), s.RuntimeDefaults["java"], m.JavaInstallationDirectory())
		for _, j := range javaRows {
			fmt.Fprintf(w, "  - JDK %s (%s) -> %s\n", j.Label(), j.ID, j.Home)
		}
		if uvErr == nil {
			fmt.Fprintf(w, "Python (uv): %s (pin: %s, installed: %d)\n", uvDesc, uvPin, len(uvInstalled))
			for _, py := range uvInstalled {
				fmt.Fprintf(w, "  - %s -> %s\n", py.Key, py.Path)
			}
		} else {
			fmt.Fprintf(w, "Python (uv): unavailable (%v)\n", uvErr)
		}
	})
}

func runSoftwareVersions(ctx context.Context, env *Env, args []string) error {
	fs := newFlagSet("software versions")
	var refresh bool
	fs.BoolVar(&refresh, "refresh", false, "Force refreshing remote catalog index")
	pos, err := parseFlags(fs, args)
	if err != nil {
		return err
	}
	if len(pos) != 1 {
		return usageErrorf("usage: devbase software versions <node|java|python|mysql|postgresql|redis|mongodb> [--refresh]")
	}
	name := strings.ToLower(strings.TrimSpace(pos[0]))
	m := env.manager()
	switch name {
	case "node":
		releases, stale, err := m.NodeReleases(ctx, refresh)
		if err != nil {
			fallback := devplatform.AvailableVersions("node")
			return writeResult(env, map[string]any{"software": name, "versions": fallback, "stale": true, "error": err.Error()}, func(w io.Writer) {
				fmt.Fprintf(w, "Warning: %v\nFallback Node.js versions: %s\n", err, strings.Join(fallback, ", "))
			})
		}
		return writeResult(env, map[string]any{"software": name, "stale": stale, "releases": releases}, func(w io.Writer) {
			for _, r := range releases {
				if r.LTS != "" {
					fmt.Fprintf(w, "%s\tLTS %s\n", r.Version, r.LTS)
				} else {
					fmt.Fprintln(w, r.Version)
				}
			}
		})
	case "java":
		versions, err := m.RuntimeVersions(ctx, "java")
		if err != nil {
			return err
		}
		return writeResult(env, map[string]any{"software": name, "versions": versions}, func(w io.Writer) {
			for _, v := range versions {
				fmt.Fprintln(w, v)
			}
		})
	case "python":
		rows, err := m.UVPythons(ctx, false)
		if err != nil {
			return err
		}
		var macRows []devplatform.UVPython
		for _, r := range rows {
			if r.OS == "macos" && r.Arch == "aarch64" {
				macRows = append(macRows, r)
			}
		}
		return writeResult(env, map[string]any{"software": name, "versions": macRows}, func(w io.Writer) {
			for _, r := range macRows {
				fmt.Fprintf(w, "%s\t%s\n", r.Key, r.Version)
			}
		})
	case "mysql", "postgresql", "redis", "mongodb":
		versions := devplatform.AvailableVersions(name)
		return writeResult(env, map[string]any{"software": name, "versions": versions}, func(w io.Writer) {
			for _, v := range versions {
				fmt.Fprintln(w, v)
			}
		})
	default:
		return usageErrorf("unsupported software %q", name)
	}
}

func runSoftwareReview(ctx context.Context, env *Env, args []string) error {
	if len(args) != 2 {
		return usageErrorf("usage: devbase software review <software> <version>")
	}
	name := strings.ToLower(strings.TrimSpace(args[0]))
	version := strings.TrimSpace(args[1])
	m := env.manager()
	pkg, err := m.ResolveInstalledOrCatalog(ctx, name, version)
	if err != nil {
		return err
	}
	info, err := m.PackageDetails(ctx, pkg)
	if err != nil {
		return err
	}
	return writeResult(env, info, func(w io.Writer) {
		fmt.Fprintf(w, "Package: %s\n", info.Package.ID())
		fmt.Fprintf(w, "Installed: %t | Cached: %t | Compatible: %t (%s)\n", info.Installed, info.Cached, info.Compatible, info.Compatibility)
		fmt.Fprintf(w, "Source: %s\n", info.Package.URL)
		fmt.Fprintf(w, "SHA-256: %s\n", info.Package.SHA256)
		fmt.Fprintf(w, "License: %s\n", info.Package.License)
		fmt.Fprintf(w, "Location: %s\n", info.Location)
		if info.Integrity != "" {
			fmt.Fprintf(w, "Integrity: %s (%s)\n", info.Integrity, info.IntegrityNote)
		}
	})
}

func runSoftwareInstall(ctx context.Context, env *Env, args []string) error {
	if len(args) != 2 {
		return usageErrorf("usage: devbase software install <software> <version>")
	}
	name := strings.ToLower(strings.TrimSpace(args[0]))
	version := strings.TrimSpace(args[1])
	m := env.manager()
	if name == "python" {
		if err := m.InstallUVPython(ctx, version); err != nil {
			return err
		}
		return writeResult(env, map[string]any{"installed": version, "software": "python"}, func(w io.Writer) {
			fmt.Fprintf(w, "Installed Python via uv: %s\n", version)
		})
	}
	pkg, err := m.ResolveInstalledOrCatalog(ctx, name, version)
	if err != nil {
		return err
	}
	if err := m.Install(ctx, pkg); err != nil {
		return err
	}
	return writeResult(env, map[string]any{"installed": pkg.ID(), "package": pkg, "location": m.PackageDirectory(pkg)}, func(w io.Writer) {
		fmt.Fprintf(w, "Installed %s (%s)\n", pkg.ID(), m.PackageDirectory(pkg))
	})
}

func runSoftwareUninstall(ctx context.Context, env *Env, args []string) error {
	fs := newFlagSet("software uninstall")
	var yes bool
	fs.BoolVar(&yes, "yes", false, "Confirm uninstalling the package")
	pos, err := parseFlags(fs, args)
	if err != nil {
		return err
	}
	if len(pos) < 1 || len(pos) > 2 {
		return usageErrorf("usage: devbase software uninstall (<package-id> | <software> <version-or-id>) --yes")
	}
	if !yes {
		return usageErrorf("uninstalling software requires --yes")
	}
	m := env.manager()
	if len(pos) == 2 && strings.EqualFold(pos[0], "python") {
		installed, err := m.UVPythons(ctx, true)
		if err != nil {
			return err
		}
		target, err := matchUVPython(installed, pos[1])
		if err != nil {
			return err
		}
		if err := m.UVPythonAction(ctx, target.Key, "uninstall"); err != nil {
			return err
		}
		return writeResult(env, map[string]any{"uninstalled": target.Key, "software": "python"}, func(w io.Writer) {
			fmt.Fprintf(w, "Uninstalled Python %s\n", target.Key)
		})
	}
	if len(pos) == 2 && strings.EqualFold(pos[0], "java") {
		rows, err := m.JavaInstallations(ctx)
		if err != nil {
			return err
		}
		row, err := matchJavaInstallation(rows, pos[1])
		if err != nil {
			return err
		}
		trashed, err := m.RemoveJavaInstallation(ctx, row)
		if err != nil {
			return err
		}
		return writeResult(env, map[string]any{"removed": row.ID, "trashedPath": trashed, "software": "java"}, func(w io.Writer) {
			fmt.Fprintf(w, "Moved JDK to Trash: %s\n", trashed)
		})
	}
	s, err := m.Read()
	if err != nil {
		return err
	}
	id, err := resolveInstalledPackageID(s, pos)
	if err != nil {
		return err
	}
	pkg := s.Packages[id]
	if pkg.Name == "java" {
		if rows, err := m.JavaInstallations(ctx); err == nil {
			for _, row := range rows {
				if row.PackageID == id {
					trashed, err := m.RemoveJavaInstallation(ctx, row)
					if err != nil {
						return err
					}
					return writeResult(env, map[string]any{"removed": id, "trashedPath": trashed, "software": "java"}, func(w io.Writer) {
						fmt.Fprintf(w, "Moved JDK to Trash: %s\n", trashed)
					})
				}
			}
		}
	}
	if err := m.Uninstall(ctx, id); err != nil {
		return err
	}
	return writeResult(env, map[string]any{"uninstalled": id, "formula": pkg.Formula}, func(w io.Writer) {
		if pkg.Formula != "" {
			fmt.Fprintf(w, "Removed workspace registration for %s (shared Homebrew keg kept).\n", id)
			return
		}
		fmt.Fprintf(w, "Uninstalled %s.\n", id)
	})
}

func runSoftwareVerify(ctx context.Context, env *Env, args []string) error {
	if len(args) < 1 || len(args) > 2 {
		return usageErrorf("usage: devbase software verify (<package-id> | <software> <version>)")
	}
	m := env.manager()
	s, err := m.Read()
	if err != nil {
		return err
	}
	id, err := resolveInstalledPackageID(s, args)
	if err != nil {
		return err
	}
	res, err := m.VerifyPackage(ctx, id)
	if err != nil {
		return err
	}
	return writeResult(env, map[string]any{"packageId": id, "result": res}, func(w io.Writer) {
		fmt.Fprintf(w, "%s: %s\n", id, res.Text())
	})
}

func runSoftwareRepair(ctx context.Context, env *Env, args []string) error {
	if len(args) < 1 || len(args) > 2 {
		return usageErrorf("usage: devbase software repair (<package-id> | <software> <version>)")
	}
	m := env.manager()
	s, err := m.Read()
	if err != nil {
		return err
	}
	id, err := resolveInstalledPackageID(s, args)
	if err != nil {
		return err
	}
	if err := m.RepairPackage(ctx, id); err != nil {
		return err
	}
	return writeResult(env, map[string]any{"repaired": id}, func(w io.Writer) {
		fmt.Fprintf(w, "Repaired %s.\n", id)
	})
}

func runSoftwareDefault(ctx context.Context, env *Env, args []string) error {
	if len(args) != 2 {
		return usageErrorf("usage: devbase software default <node|java|python> <version-or-id>")
	}
	name := strings.ToLower(strings.TrimSpace(args[0]))
	target := strings.TrimSpace(args[1])
	m := env.manager()
	switch name {
	case "node":
		s, err := m.Read()
		if err != nil {
			return err
		}
		id := target
		if _, ok := s.Packages[id]; !ok {
			id, err = resolveInstalledPackageID(s, []string{"node", target})
			if err != nil {
				return err
			}
		}
		if err := m.SetNodeDefault(ctx, id); err != nil {
			return err
		}
		return writeResult(env, map[string]any{"software": "node", "default": id}, func(w io.Writer) {
			fmt.Fprintf(w, "Default Node.js set to %s.\n", id)
		})
	case "java":
		rows, err := m.JavaInstallations(ctx)
		if err == nil {
			if row, matchErr := matchJavaInstallation(rows, target); matchErr == nil {
				if err := m.SetJavaDefault(ctx, row); err != nil {
					return err
				}
				return writeResult(env, map[string]any{"software": "java", "default": row.ID, "home": row.Home}, func(w io.Writer) {
					fmt.Fprintf(w, "Default Java set to %s (%s).\n", row.Label(), row.Home)
				})
			}
		}
		if err := m.SetRuntimeDefault(ctx, "java", target); err != nil {
			return err
		}
		return writeResult(env, map[string]any{"software": "java", "default": target}, func(w io.Writer) {
			fmt.Fprintf(w, "Default Java set to %s.\n", target)
		})
	case "python":
		installed, err := m.UVPythons(ctx, true)
		if err != nil {
			return err
		}
		row, err := matchUVPython(installed, target)
		if err != nil {
			return err
		}
		if err := m.UVPythonAction(ctx, row.Key, "default"); err != nil {
			return err
		}
		return writeResult(env, map[string]any{"software": "python", "default": row.Key}, func(w io.Writer) {
			fmt.Fprintf(w, "Global uv Python pin set to %s.\n", row.Key)
		})
	default:
		return usageErrorf("default version is supported for node, java, and python (got %q)", name)
	}
}

func runSoftwareShell(ctx context.Context, env *Env, args []string) error {
	if len(args) < 1 || len(args) > 2 {
		return usageErrorf("usage: devbase software shell <node|java> [status|enable|disable|bash-command]")
	}
	name := strings.ToLower(strings.TrimSpace(args[0]))
	if name != "node" && name != "java" {
		return usageErrorf("shell integration is supported for node and java (got %q)", name)
	}
	action := "status"
	if len(args) == 2 {
		action = strings.ToLower(strings.TrimSpace(args[1]))
	}
	m := env.manager()
	enabledNow := func() bool {
		if name == "node" {
			return m.NodeShellEnabled()
		}
		return m.RuntimeShellEnabled("java")
	}
	switch action {
	case "status":
		enabled := enabledNow()
		bashCmd := m.BashCommand()
		return writeResult(env, map[string]any{"software": name, "enabled": enabled, "bashCommand": bashCmd}, func(w io.Writer) {
			fmt.Fprintf(w, "%s shell integration enabled: %t\nBash command: %s\n", name, enabled, bashCmd)
		})
	case "enable":
		var detail string
		var err error
		if name == "node" {
			detail, err = m.EnableNodeShell(ctx)
		} else {
			err = m.ConfigureRuntimeShell(ctx, "java", true)
		}
		if err != nil {
			return err
		}
		return writeResult(env, map[string]any{"software": name, "enabled": true, "files": detail}, func(w io.Writer) {
			fmt.Fprintf(w, "Enabled %s shell integration.\n", name)
		})
	case "disable":
		var detail string
		var err error
		if name == "node" {
			detail, err = m.DisableNodeShell(ctx)
		} else {
			err = m.ConfigureRuntimeShell(ctx, "java", false)
		}
		if err != nil {
			return err
		}
		return writeResult(env, map[string]any{"software": name, "enabled": false, "files": detail}, func(w io.Writer) {
			fmt.Fprintf(w, "Disabled %s shell integration.\n", name)
		})
	case "bash-command":
		cmd := m.BashCommand()
		return writeResult(env, map[string]any{"software": name, "bashCommand": cmd}, func(w io.Writer) {
			fmt.Fprintln(w, cmd)
		})
	default:
		return usageErrorf("unknown software shell action %q", action)
	}
}

func runSoftwareWhich(ctx context.Context, env *Env, args []string) error {
	fs := newFlagSet("software which")
	var dir, pkgID string
	fs.StringVar(&dir, "dir", "", "Directory to evaluate for .node-version / .nvmrc / package.json")
	fs.StringVar(&pkgID, "package", "", "Specific installed package ID")
	pos, err := parseFlags(fs, args)
	if err != nil {
		return err
	}
	if len(pos) != 1 {
		return usageErrorf("usage: devbase software which <software> [--dir <path>] [--package <id>]")
	}
	name := strings.ToLower(strings.TrimSpace(pos[0]))
	m := env.manager()
	s, err := m.Read()
	if err != nil {
		return err
	}
	if pkgID != "" {
		bin, err := m.Binary(&s, pkgID, primaryBinaryName(name))
		if err != nil {
			return err
		}
		return writeResult(env, map[string]any{"software": name, "packageId": pkgID, "binary": bin}, func(w io.Writer) {
			fmt.Fprintln(w, bin)
		})
	}
	switch name {
	case "node":
		if dir == "" {
			dir, _ = os.Getwd()
		}
		sel, err := m.SelectNode(dir, "")
		if err != nil {
			return err
		}
		payload := map[string]any{
			"software": "node",
			"id":       sel.ID,
			"version":  sel.Version,
			"dir":      sel.Dir,
			"binDir":   sel.Bin(),
			"binary":   filepath.Join(sel.Bin(), "node"),
			"prefix":   sel.Prefix,
			"source":   sel.Source,
			"spec":     sel.Spec,
			"system":   sel.System,
		}
		if sel.System {
			payload["binary"] = ""
		}
		return writeResult(env, payload, func(w io.Writer) {
			if sel.System {
				fmt.Fprintln(w, "system")
				return
			}
			fmt.Fprintln(w, filepath.Join(sel.Bin(), "node"))
		})
	case "java":
		defID := s.RuntimeDefaults["java"]
		if defID == "" {
			return errors.New("no default Java installation selected")
		}
		rows, err := m.JavaInstallations(ctx)
		if err == nil {
			for _, r := range rows {
				if r.ID == defID || r.PackageID == defID {
					bin := filepath.Join(r.Home, "bin", "java")
					return writeResult(env, map[string]any{"software": "java", "id": r.ID, "version": r.Version, "javaHome": r.Home, "binary": bin}, func(w io.Writer) {
						fmt.Fprintln(w, bin)
					})
				}
			}
		}
		bin, err := m.Binary(&s, defID, "java")
		if err != nil {
			return err
		}
		return writeResult(env, map[string]any{"software": "java", "id": defID, "javaHome": filepath.Dir(filepath.Dir(bin)), "binary": bin}, func(w io.Writer) {
			fmt.Fprintln(w, bin)
		})
	case "python":
		pin, err := m.UVGlobalPin(ctx)
		if err != nil {
			return err
		}
		installed, err := m.UVPythons(ctx, true)
		if err != nil {
			return err
		}
		if pin != "" {
			if row, err := matchUVPython(installed, pin); err == nil {
				return writeResult(env, map[string]any{"software": "python", "key": row.Key, "version": row.Version, "binary": row.Path}, func(w io.Writer) {
					fmt.Fprintln(w, row.Path)
				})
			}
		}
		return errors.New("no global uv Python pin resolves to an installed interpreter")
	case "mysql", "postgresql", "redis", "mongodb":
		var matches []string
		for _, id := range sortedMapKeys(s.Packages) {
			if s.Packages[id].Name == name {
				matches = append(matches, id)
			}
		}
		if len(matches) == 0 {
			return fmt.Errorf("no %s package installed", name)
		}
		bin, err := m.Binary(&s, matches[0], primaryBinaryName(name))
		if err != nil {
			return err
		}
		return writeResult(env, map[string]any{"software": name, "packageId": matches[0], "binary": bin}, func(w io.Writer) {
			fmt.Fprintln(w, bin)
		})
	default:
		return usageErrorf("unsupported software %q", name)
	}
}

func runSoftwareUVPath(ctx context.Context, env *Env, args []string) error {
	m := env.manager()
	if len(args) == 0 {
		p, err := m.UVPath()
		if err != nil {
			return err
		}
		desc, _ := m.UVDescription(ctx)
		return writeResult(env, map[string]any{"path": p, "description": desc}, func(w io.Writer) {
			if desc != "" {
				fmt.Fprintln(w, desc)
			} else {
				fmt.Fprintln(w, p)
			}
		})
	}
	if len(args) != 1 {
		return usageErrorf("usage: devbase software uv-path [<absolute-path>]")
	}
	if err := m.SetUVPath(ctx, args[0]); err != nil {
		return err
	}
	desc, _ := m.UVDescription(ctx)
	return writeResult(env, map[string]any{"path": args[0], "description": desc}, func(w io.Writer) {
		fmt.Fprintf(w, "Selected uv executable: %s\n", args[0])
	})
}

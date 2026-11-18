package devcli

import (
	"context"
	"fmt"
	"io"
	"os"
	"strconv"
	"strings"

	"devbase/internal/devplatform"
)

func runDatabaseCommand(ctx context.Context, env *Env, args []string) error {
	if hasHelpFlag(args) && len(args) == 1 {
		printTopicHelp(env.Out, "db")
		return nil
	}
	sub := "list"
	if len(args) > 0 {
		sub = args[0]
		args = args[1:]
	}
	if hasHelpFlag(args) {
		printTopicHelp(env.Out, "db")
		return nil
	}
	switch sub {
	case "list":
		return runDBList(ctx, env, args)
	case "create":
		return runDBCreate(ctx, env, args)
	case "start", "stop", "restart":
		return runDBAction(ctx, env, sub, args)
	case "stop-all":
		return runDBStopAll(ctx, env, args)
	case "remove":
		return runDBRemove(ctx, env, args)
	case "delete-data":
		return runDBDeleteData(ctx, env, args)
	case "port":
		return runDBPort(ctx, env, args)
	case "upgrade":
		return runDBUpgrade(ctx, env, args)
	case "inspect":
		return runDBInspect(ctx, env, args)
	case "create-db":
		return runDBCreateDB(ctx, env, args)
	case "drop-db":
		return runDBDropDB(ctx, env, args)
	case "create-user":
		return runDBCreateUser(ctx, env, args)
	case "drop-user":
		return runDBDropUser(ctx, env, args)
	case "connection":
		return runDBConnection(ctx, env, args)
	case "password":
		return runDBPassword(ctx, env, args)
	case "open-cli":
		return runDBOpenCLI(ctx, env, args)
	case "profile":
		return runDBProfile(ctx, env, args)
	case "backup":
		return runDBBackup(ctx, env, args)
	default:
		return usageErrorf("unknown db subcommand %q", sub)
	}
}

func runDBList(ctx context.Context, env *Env, args []string) error {
	if len(args) > 0 {
		return usageErrorf("unexpected argument %q for db list", args[0])
	}
	m := env.manager()
	s, err := m.Read()
	if err != nil {
		return err
	}
	snap := m.Snapshot(ctx, s)
	type dbView struct {
		devplatform.Instance
		Status                string   `json:"status"`
		ResolvedDataDirectory string   `json:"resolvedDataDirectory"`
		UpgradeTargets        []string `json:"upgradeTargets,omitempty"`
	}
	out := make([]dbView, 0, len(s.Instances))
	for _, id := range sortedMapKeys(s.Instances) {
		inst := s.Instances[id]
		out = append(out, dbView{
			Instance:              inst,
			Status:                snap.States["instance:"+id],
			ResolvedDataDirectory: m.DatabaseDataDirectory(inst),
			UpgradeTargets:        devplatform.UpgradeTargets(s, inst),
		})
	}
	return writeResult(env, map[string]any{"instances": out}, func(w io.Writer) {
		if len(out) == 0 {
			fmt.Fprintln(w, "No database instances configured.")
			return
		}
		for _, inst := range out {
			storage := "isolated"
			if inst.DataDirectory != "" {
				storage = "external (" + inst.DataDirectory + ")"
			}
			fmt.Fprintf(w, "%s\t%s\t:%d\t%s\t%s\tpkg=%s\tpersistent=%t\n", inst.ID, inst.Kind, inst.Port, inst.Status, storage, inst.Package, inst.Persistent)
		}
	})
}

func runDBCreate(ctx context.Context, env *Env, args []string) error {
	fs := newFlagSet("db create")
	var kind, pkgID, version, dataDir string
	var port int
	var persistent bool
	fs.StringVar(&kind, "kind", "", "Database engine: mysql, postgresql, redis, mongodb")
	fs.StringVar(&pkgID, "package", "", "Installed package ID")
	fs.StringVar(&version, "version", "", "Engine version to select or resolve")
	fs.IntVar(&port, "port", 0, "Local TCP port (0 picks a free port automatically)")
	fs.BoolVar(&persistent, "persistent", false, "Start instance automatically in background")
	fs.StringVar(&dataDir, "data-dir", "", "Optional external data directory")
	pos, err := parseFlags(fs, args)
	if err != nil {
		return err
	}
	if len(pos) != 1 || strings.TrimSpace(kind) == "" {
		return usageErrorf("usage: devbase db create <id> --kind <mysql|postgresql|redis|mongodb> [--package <pkg-id> | --version <ver>] [--port <port>] [--persistent] [--data-dir <path>]")
	}
	id := strings.TrimSpace(pos[0])
	kind = strings.ToLower(strings.TrimSpace(kind))
	switch kind {
	case "mysql", "postgresql", "redis", "mongodb":
	default:
		return usageErrorf("unsupported database kind %q", kind)
	}
	if port < 0 || port > 65535 {
		return usageErrorf("--port must be 0 or 1–65535")
	}
	m := env.manager()
	s, err := m.Read()
	if err != nil {
		return err
	}
	if pkgID == "" {
		if version != "" {
			if resolved, rerr := resolveInstalledPackageID(s, []string{kind, version}); rerr == nil {
				pkgID = resolved
			} else {
				pkg, cerr := m.ResolveInstalledOrCatalog(ctx, kind, version)
				if cerr != nil {
					return cerr
				}
				pkgID = pkg.ID()
			}
		} else {
			var matches []string
			for _, pid := range sortedMapKeys(s.Packages) {
				if s.Packages[pid].Name == kind {
					matches = append(matches, pid)
				}
			}
			switch len(matches) {
			case 0:
				return usageErrorf("no installed %s package found; specify --package or --version after installing it", kind)
			case 1:
				pkgID = matches[0]
			default:
				return usageErrorf("multiple %s packages installed (%s); pass --package or --version", kind, strings.Join(matches, ", "))
			}
		}
	}
	if port == 0 {
		free, err := m.AvailableLocalPort(ctx)
		if err != nil {
			return err
		}
		port = free
	}
	inst := devplatform.Instance{
		ID:            id,
		Kind:          kind,
		Package:       pkgID,
		Port:          port,
		Persistent:    persistent,
		DataDirectory: strings.TrimSpace(dataDir),
	}
	if err := m.CreateInstance(ctx, inst); err != nil {
		return err
	}
	sAfter, _ := m.Read()
	if saved, ok := sAfter.Instances[id]; ok {
		inst = saved
	}
	return writeResult(env, map[string]any{"instance": inst, "dataDirectory": m.DatabaseDataDirectory(inst)}, func(w io.Writer) {
		fmt.Fprintf(w, "Created %s instance %s on port %d (%s).\n", inst.Kind, inst.ID, inst.Port, inst.Package)
	})
}

func runDBAction(ctx context.Context, env *Env, action string, args []string) error {
	if len(args) != 1 {
		return usageErrorf("usage: devbase db %s <id>", action)
	}
	id := strings.TrimSpace(args[0])
	m := env.manager()
	if err := m.InstanceAction(ctx, id, action); err != nil {
		return err
	}
	return writeResult(env, map[string]any{"id": id, "action": action}, func(w io.Writer) {
		fmt.Fprintf(w, "Database instance %s: %s completed.\n", id, action)
	})
}

func runDBStopAll(ctx context.Context, env *Env, args []string) error {
	fs := newFlagSet("db stop-all")
	var yes bool
	fs.BoolVar(&yes, "yes", false, "Confirm stopping all managed database instances")
	pos, err := parseFlags(fs, args)
	if err != nil {
		return err
	}
	if len(pos) > 0 {
		return usageErrorf("unexpected argument %q for db stop-all", pos[0])
	}
	if !yes {
		return usageErrorf("stopping all managed database instances requires --yes")
	}
	m := env.manager()
	plan, err := m.PreviewShutdown(ctx)
	if err != nil {
		return err
	}
	if err := m.Shutdown(ctx, plan, devplatform.ShutdownOptions{}); err != nil {
		return err
	}
	return writeResult(env, map[string]any{"stopped": len(plan.Instances), "instances": plan.Instances}, func(w io.Writer) {
		fmt.Fprintf(w, "Stopped %d managed database instances.\n", len(plan.Instances))
	})
}

func runDBRemove(ctx context.Context, env *Env, args []string) error {
	fs := newFlagSet("db remove")
	var yes bool
	fs.BoolVar(&yes, "yes", false, "Confirm removing the database instance configuration (data files are retained)")
	pos, err := parseFlags(fs, args)
	if err != nil {
		return err
	}
	if len(pos) != 1 {
		return usageErrorf("usage: devbase db remove <id> --yes")
	}
	if !yes {
		return usageErrorf("removing database instance requires --yes")
	}
	id := strings.TrimSpace(pos[0])
	m := env.manager()
	if err := m.InstanceAction(ctx, id, "remove"); err != nil {
		return err
	}
	return writeResult(env, map[string]any{"removed": id}, func(w io.Writer) {
		fmt.Fprintf(w, "Removed database instance %s (data files retained).\n", id)
	})
}

func runDBDeleteData(ctx context.Context, env *Env, args []string) error {
	fs := newFlagSet("db delete-data")
	var preview bool
	var confirm, fingerprint string
	fs.BoolVar(&preview, "preview", false, "Preview isolated data footprint and fingerprint without deleting")
	fs.StringVar(&confirm, "confirm", "", "Exact instance ID confirmation")
	fs.StringVar(&fingerprint, "fingerprint", "", "Exact deletion preview fingerprint")
	pos, err := parseFlags(fs, args)
	if err != nil {
		return err
	}
	if len(pos) != 1 {
		return usageErrorf("usage: devbase db delete-data <id> (--preview | --confirm <id> --fingerprint <fp>)")
	}
	id := strings.TrimSpace(pos[0])
	m := env.manager()
	if preview {
		p, err := m.PreviewInstanceDeletion(ctx, id)
		if err != nil {
			return err
		}
		return writeResult(env, p, func(w io.Writer) {
			fmt.Fprintf(w, "Instance: %s (%s)\nCan delete permanently: %t\nPath: %s\nEntries: %d | Bytes: %d\nFingerprint: %s\nDetail: %s\n",
				p.InstanceID, p.Kind, p.CanDelete, p.Path, p.Entries, p.Bytes, p.Fingerprint, p.Detail)
		})
	}
	if strings.TrimSpace(confirm) == "" || strings.TrimSpace(fingerprint) == "" {
		return usageErrorf("permanent data deletion requires --confirm <id> and --fingerprint <fp> (run with --preview first)")
	}
	if err := m.DeleteInstanceData(ctx, id, fingerprint, confirm); err != nil {
		return err
	}
	return writeResult(env, map[string]any{"deleted": id, "fingerprint": fingerprint}, func(w io.Writer) {
		fmt.Fprintf(w, "Permanently deleted app-managed data for instance %s.\n", id)
	})
}

func runDBPort(ctx context.Context, env *Env, args []string) error {
	fs := newFlagSet("db port")
	var repair bool
	fs.BoolVar(&repair, "repair", false, "Rewrite instance configuration to match its configured port")
	pos, err := parseFlags(fs, args)
	if err != nil {
		return err
	}
	m := env.manager()
	s, err := m.Read()
	if err != nil {
		return err
	}
	if repair {
		if len(pos) != 1 {
			return usageErrorf("usage: devbase db port <id> --repair")
		}
		id := strings.TrimSpace(pos[0])
		inst, ok := s.Instances[id]
		if !ok {
			return fmt.Errorf("database instance %q not found", id)
		}
		if err := m.RepairInstancePort(ctx, inst); err != nil {
			return err
		}
		return writeResult(env, map[string]any{"id": id, "port": inst.Port, "repaired": true}, func(w io.Writer) {
			fmt.Fprintf(w, "Repaired port configuration for %s (port %d).\n", id, inst.Port)
		})
	}
	if len(pos) != 2 {
		return usageErrorf("usage: devbase db port <id> <new-port> | devbase db port <id> --repair")
	}
	id := strings.TrimSpace(pos[0])
	inst, ok := s.Instances[id]
	if !ok {
		return fmt.Errorf("database instance %q not found", id)
	}
	port, err := strconv.Atoi(pos[1])
	if err != nil || port < 1 || port > 65535 {
		return usageErrorf("port must be 1–65535")
	}
	if err := m.ChangeInstancePort(ctx, inst, port); err != nil {
		return err
	}
	return writeResult(env, map[string]any{"id": id, "oldPort": inst.Port, "port": port}, func(w io.Writer) {
		fmt.Fprintf(w, "Updated %s port from %d to %d.\n", id, inst.Port, port)
	})
}

func runDBUpgrade(ctx context.Context, env *Env, args []string) error {
	fs := newFlagSet("db upgrade")
	var listTargets bool
	fs.BoolVar(&listTargets, "targets", false, "List compatible upgrade target packages")
	pos, err := parseFlags(fs, args)
	if err != nil {
		return err
	}
	if len(pos) < 1 || len(pos) > 2 {
		return usageErrorf("usage: devbase db upgrade <id> [<target-package-or-version>] [--targets]")
	}
	id := strings.TrimSpace(pos[0])
	m := env.manager()
	s, err := m.Read()
	if err != nil {
		return err
	}
	inst, ok := s.Instances[id]
	if !ok {
		return fmt.Errorf("database instance %q not found", id)
	}
	targets := devplatform.UpgradeTargets(s, inst)
	if listTargets || len(pos) == 1 {
		return writeResult(env, map[string]any{"id": id, "currentPackage": inst.Package, "targets": targets}, func(w io.Writer) {
			if len(targets) == 0 {
				fmt.Fprintf(w, "No compatible upgrade targets installed for %s (%s).\n", id, inst.Package)
				return
			}
			for _, t := range targets {
				fmt.Fprintln(w, t)
			}
		})
	}
	want := strings.TrimSpace(pos[1])
	targetID := ""
	for _, t := range targets {
		if t == want || (s.Packages[t].Version != "" && s.Packages[t].Version == want) {
			targetID = t
			break
		}
	}
	if targetID == "" {
		return fmt.Errorf("target %q is not an installed compatible upgrade for %s (available: %v)", want, id, targets)
	}
	if err := m.UpgradeInstancePackage(ctx, inst, targetID); err != nil {
		return err
	}
	return writeResult(env, map[string]any{"id": id, "fromPackage": inst.Package, "toPackage": targetID}, func(w io.Writer) {
		fmt.Fprintf(w, "Upgraded %s from %s to %s.\n", id, inst.Package, targetID)
	})
}

func runDBInspect(ctx context.Context, env *Env, args []string) error {
	if len(args) != 1 {
		return usageErrorf("usage: devbase db inspect <id>")
	}
	id := strings.TrimSpace(args[0])
	cat, err := env.manager().InspectInstanceCatalog(ctx, id)
	if err != nil {
		return err
	}
	return writeResult(env, map[string]any{"id": id, "databases": cat.Databases, "users": cat.Users}, func(w io.Writer) {
		fmt.Fprintf(w, "Databases (%d): %s\n", len(cat.Databases), strings.Join(cat.Databases, ", "))
		fmt.Fprintf(w, "Users (%d): %s\n", len(cat.Users), strings.Join(cat.Users, ", "))
	})
}

func runDBCreateDB(ctx context.Context, env *Env, args []string) error {
	if len(args) != 2 {
		return usageErrorf("usage: devbase db create-db <instance-id> <database-name>")
	}
	if err := env.manager().CreateDatabase(ctx, args[0], args[1]); err != nil {
		return err
	}
	return writeResult(env, map[string]any{"instanceId": args[0], "database": args[1], "created": true}, func(w io.Writer) {
		fmt.Fprintf(w, "Created database %s on %s.\n", args[1], args[0])
	})
}

func runDBDropDB(ctx context.Context, env *Env, args []string) error {
	fs := newFlagSet("db drop-db")
	var yes bool
	fs.BoolVar(&yes, "yes", false, "Confirm dropping the database")
	pos, err := parseFlags(fs, args)
	if err != nil {
		return err
	}
	if len(pos) != 2 {
		return usageErrorf("usage: devbase db drop-db <instance-id> <database-name> --yes")
	}
	if !yes {
		return usageErrorf("dropping a database requires --yes")
	}
	if err := env.manager().DropDatabase(ctx, pos[0], pos[1]); err != nil {
		return err
	}
	return writeResult(env, map[string]any{"instanceId": pos[0], "database": pos[1], "dropped": true}, func(w io.Writer) {
		fmt.Fprintf(w, "Dropped database %s on %s.\n", pos[1], pos[0])
	})
}

func runDBCreateUser(ctx context.Context, env *Env, args []string) error {
	fs := newFlagSet("db create-user")
	var database string
	var passwordStdin bool
	fs.StringVar(&database, "database", "", "Target database to grant access to")
	fs.BoolVar(&passwordStdin, "password-stdin", false, "Read the database user password from stdin")
	pos, err := parseFlags(fs, args)
	if err != nil {
		return err
	}
	if len(pos) != 2 {
		return usageErrorf("usage: devbase db create-user <instance-id> <username> [--database <db>] --password-stdin")
	}
	if !passwordStdin {
		return usageErrorf("create-user requires --password-stdin (never pass secrets in argv)")
	}
	secret, err := readSecretStdin(env.In, "database user password")
	if err != nil {
		return err
	}
	if err := env.manager().CreateUser(ctx, pos[0], pos[1], secret, database); err != nil {
		return err
	}
	return writeResult(env, map[string]any{"instanceId": pos[0], "username": pos[1], "database": database, "created": true}, func(w io.Writer) {
		fmt.Fprintf(w, "Created user %s on %s.\n", pos[1], pos[0])
	})
}

func runDBDropUser(ctx context.Context, env *Env, args []string) error {
	fs := newFlagSet("db drop-user")
	var database string
	var yes bool
	fs.StringVar(&database, "database", "", "MongoDB authentication database when applicable")
	fs.BoolVar(&yes, "yes", false, "Confirm dropping the user")
	pos, err := parseFlags(fs, args)
	if err != nil {
		return err
	}
	if len(pos) != 2 {
		return usageErrorf("usage: devbase db drop-user <instance-id> <username> [--database <db>] --yes")
	}
	if !yes {
		return usageErrorf("dropping a database user requires --yes")
	}
	if err := env.manager().DropUser(ctx, pos[0], pos[1], database); err != nil {
		return err
	}
	return writeResult(env, map[string]any{"instanceId": pos[0], "username": pos[1], "database": database, "dropped": true}, func(w io.Writer) {
		fmt.Fprintf(w, "Dropped user %s on %s.\n", pos[1], pos[0])
	})
}

func runDBConnection(_ context.Context, env *Env, args []string) error {
	fs := newFlagSet("db connection")
	var profileID, database, format string
	fs.StringVar(&profileID, "profile", "", "Saved connection profile ID")
	fs.StringVar(&database, "database", "", "Override database or Redis DB index")
	fs.StringVar(&format, "format", "", "Output format: uri, env, jdbc, cli")
	pos, err := parseFlags(fs, args)
	if err != nil {
		return err
	}
	if len(pos) != 1 {
		return usageErrorf("usage: devbase db connection <id> [--profile <profile-id>] [--database <db>] [--format uri|env|jdbc|cli]")
	}
	id := strings.TrimSpace(pos[0])
	m := env.manager()
	s, err := m.Read()
	if err != nil {
		return err
	}
	inst, ok := s.Instances[id]
	if !ok {
		return fmt.Errorf("database instance %q not found", id)
	}
	var info devplatform.ConnectionInfo
	if profileID != "" {
		info, err = m.ProfileConnectionInfo(id, profileID)
	} else {
		info, err = m.ConnectionInfo(id)
	}
	if err != nil {
		return err
	}
	info = info.WithDatabase(inst.Kind, database)
	envAssign := info.EnvAssignmentForKind(inst.Kind)
	jdbcURL, jdbcErr := info.JDBCURL(inst.Kind)
	cliCmd, cliErr := m.DatabaseCLICommand(id, profileID, database)

	format = strings.ToLower(strings.TrimSpace(format))
	switch format {
	case "", "uri", "env", "jdbc", "cli":
	default:
		return usageErrorf("--format must be one of uri, env, jdbc, cli")
	}
	if format == "jdbc" && jdbcErr != nil {
		return jdbcErr
	}
	if format == "cli" && cliErr != nil {
		return cliErr
	}

	payload := map[string]any{
		"instanceId":     id,
		"kind":           inst.Kind,
		"profileId":      profileID,
		"uri":            info.URI,
		"username":       info.Username,
		"credentialFile": info.CredentialFile,
		"env":            envAssign,
	}
	if jdbcErr == nil {
		payload["jdbc"] = jdbcURL
	}
	if cliErr == nil {
		payload["cli"] = cliCmd
	}
	return writeResult(env, payload, func(w io.Writer) {
		switch format {
		case "uri":
			fmt.Fprintln(w, info.URI)
		case "env":
			fmt.Fprintln(w, envAssign)
		case "jdbc":
			fmt.Fprintln(w, jdbcURL)
		case "cli":
			fmt.Fprintln(w, cliCmd)
		default:
			fmt.Fprintf(w, "URI: %s\n", info.URI)
			fmt.Fprintf(w, "Env: %s\n", envAssign)
			if jdbcErr == nil {
				fmt.Fprintf(w, "JDBC: %s\n", jdbcURL)
			}
			if info.Username != "" {
				fmt.Fprintf(w, "Username: %s\n", info.Username)
			}
			if info.CredentialFile != "" {
				fmt.Fprintf(w, "Credential file: %s\n", info.CredentialFile)
			}
			if cliErr == nil {
				fmt.Fprintf(w, "CLI: %s\n", cliCmd)
			}
		}
	})
}

func runDBPassword(_ context.Context, env *Env, args []string) error {
	if len(args) != 1 {
		return usageErrorf("usage: devbase db password <id>")
	}
	id := strings.TrimSpace(args[0])
	pw, err := env.manager().DatabasePassword(id)
	if err != nil {
		return err
	}
	return writeResult(env, map[string]any{"instanceId": id, "password": pw}, func(w io.Writer) {
		fmt.Fprintln(w, pw)
	})
}

func runDBOpenCLI(ctx context.Context, env *Env, args []string) error {
	fs := newFlagSet("db open-cli")
	var profileID, database string
	var printOnly bool
	fs.StringVar(&profileID, "profile", "", "Saved connection profile ID")
	fs.StringVar(&database, "database", "", "Override database or Redis DB index")
	fs.BoolVar(&printOnly, "print", false, "Print the shell command instead of opening Terminal")
	pos, err := parseFlags(fs, args)
	if err != nil {
		return err
	}
	if len(pos) != 1 {
		return usageErrorf("usage: devbase db open-cli <id> [--profile <profile-id>] [--database <db>] [--print]")
	}
	id := strings.TrimSpace(pos[0])
	m := env.manager()
	cmdText, err := m.DatabaseCLICommand(id, profileID, database)
	if err != nil {
		return err
	}
	if !printOnly {
		if err := m.OpenDatabaseCLI(ctx, id, profileID, database); err != nil {
			return err
		}
	}
	return writeResult(env, map[string]any{"instanceId": id, "profileId": profileID, "database": database, "command": cmdText, "opened": !printOnly}, func(w io.Writer) {
		fmt.Fprintln(w, cmdText)
	})
}

func runDBProfile(ctx context.Context, env *Env, args []string) error {
	sub := "list"
	if len(args) > 0 {
		sub = args[0]
		args = args[1:]
	}
	m := env.manager()
	switch sub {
	case "list":
		if len(args) > 1 {
			return usageErrorf("usage: devbase db profile list [<instance-id>]")
		}
		filterID := ""
		if len(args) == 1 {
			filterID = strings.TrimSpace(args[0])
		}
		s, err := m.Read()
		if err != nil {
			return err
		}
		out := make([]devplatform.ConnectionProfile, 0, len(s.ConnectionProfiles))
		for _, pid := range sortedMapKeys(s.ConnectionProfiles) {
			p := s.ConnectionProfiles[pid]
			if filterID == "" || p.InstanceID == filterID {
				out = append(out, p)
			}
		}
		return writeResult(env, map[string]any{"profiles": out}, func(w io.Writer) {
			if len(out) == 0 {
				fmt.Fprintln(w, "No connection profiles saved.")
				return
			}
			for _, p := range out {
				fmt.Fprintf(w, "%s\tinstance=%s\tname=%q\tuser=%s\tdb=%s\tredisDb=%d\n", p.ID, p.InstanceID, p.Name, p.Username, p.Database, p.RedisDB)
			}
		})
	case "save":
		fs := newFlagSet("db profile save")
		var p devplatform.ConnectionProfile
		fs.StringVar(&p.ID, "id", "", "Existing profile ID to update (auto-generated when empty)")
		fs.StringVar(&p.InstanceID, "instance", "", "Target instance ID (required)")
		fs.StringVar(&p.Name, "name", "", "Display name (required)")
		fs.StringVar(&p.Database, "database", "", "Default database")
		fs.StringVar(&p.Username, "username", "", "Username")
		fs.StringVar(&p.MongoAuthSource, "auth-source", "", "MongoDB authSource database")
		fs.IntVar(&p.RedisDB, "redis-db", 0, "Redis logical DB index (0–15)")
		pos, err := parseFlags(fs, args)
		if err != nil {
			return err
		}
		if len(pos) > 0 {
			return usageErrorf("unexpected argument %q for db profile save", pos[0])
		}
		if strings.TrimSpace(p.InstanceID) == "" || strings.TrimSpace(p.Name) == "" {
			return usageErrorf("usage: devbase db profile save --instance <id> --name <label> [--id <profile-id>] [--database <db>] [--username <user>] [--auth-source <db>] [--redis-db <0-15>]")
		}
		saved, err := m.SaveConnectionProfile(ctx, p)
		if err != nil {
			return err
		}
		return writeResult(env, map[string]any{"profile": saved}, func(w io.Writer) {
			fmt.Fprintf(w, "Saved connection profile %s (%s) on %s.\n", saved.ID, saved.Name, saved.InstanceID)
		})
	case "delete":
		fs := newFlagSet("db profile delete")
		var yes bool
		fs.BoolVar(&yes, "yes", false, "Confirm deleting the saved connection profile")
		pos, err := parseFlags(fs, args)
		if err != nil {
			return err
		}
		if len(pos) != 2 {
			return usageErrorf("usage: devbase db profile delete <instance-id> <profile-id> --yes")
		}
		if !yes {
			return usageErrorf("deleting a connection profile requires --yes")
		}
		if err := m.DeleteConnectionProfile(ctx, pos[0], pos[1]); err != nil {
			return err
		}
		return writeResult(env, map[string]any{"instanceId": pos[0], "profileId": pos[1], "deleted": true}, func(w io.Writer) {
			fmt.Fprintf(w, "Deleted connection profile %s from %s.\n", pos[1], pos[0])
		})
	default:
		return usageErrorf("unknown db profile subcommand %q (expected list, save, or delete)", sub)
	}
}

func runDBBackup(ctx context.Context, env *Env, args []string) error {
	sub := "list"
	if len(args) > 0 {
		sub = args[0]
		args = args[1:]
	}
	m := env.manager()
	switch sub {
	case "list":
		if len(args) != 1 {
			return usageErrorf("usage: devbase db backup list <instance-id>")
		}
		items, err := m.Backups(ctx, args[0])
		if err != nil {
			return err
		}
		return writeResult(env, map[string]any{"instanceId": args[0], "backups": items}, func(w io.Writer) {
			if len(items) == 0 {
				fmt.Fprintf(w, "No backups for %s.\n", args[0])
				return
			}
			for _, b := range items {
				fmt.Fprintf(w, "%s\tdb=%s\tsize=%d\tmodified=%s\n", b.Name, b.Database, b.Size, b.Modified.Format("2006-01-02T15:04:05Z07:00"))
			}
		})
	case "create":
		fs := newFlagSet("db backup create")
		var database string
		fs.StringVar(&database, "database", "", "Specific database name to back up")
		pos, err := parseFlags(fs, args)
		if err != nil {
			return err
		}
		if len(pos) != 1 {
			return usageErrorf("usage: devbase db backup create <instance-id> [--database <db>]")
		}
		path, err := m.Backup(ctx, pos[0], database)
		if err != nil {
			return err
		}
		return writeResult(env, map[string]any{"instanceId": pos[0], "database": database, "path": path}, func(w io.Writer) {
			fmt.Fprintf(w, "Created backup: %s\n", path)
		})
	case "restore":
		fs := newFlagSet("db backup restore")
		var database string
		var yes bool
		fs.StringVar(&database, "database", "", "Target database name to restore into")
		fs.BoolVar(&yes, "yes", false, "Confirm restoring backup (overwrites target database data)")
		pos, err := parseFlags(fs, args)
		if err != nil {
			return err
		}
		if len(pos) != 2 {
			return usageErrorf("usage: devbase db backup restore <instance-id> <backup-name-or-path> [--database <db>] --yes")
		}
		if !yes {
			return usageErrorf("restoring a backup requires --yes")
		}
		id := strings.TrimSpace(pos[0])
		target := strings.TrimSpace(pos[1])
		restorePath := target
		if _, statErr := os.Stat(target); statErr != nil {
			items, err := m.Backups(ctx, id)
			if err != nil {
				return err
			}
			found := ""
			for _, b := range items {
				if b.Name == target || b.Path == target {
					found = b.Path
					break
				}
			}
			if found == "" {
				return fmt.Errorf("backup %q not found for instance %s", target, id)
			}
			restorePath = found
		}
		if err := m.Restore(ctx, id, database, restorePath); err != nil {
			return err
		}
		return writeResult(env, map[string]any{"instanceId": id, "database": database, "path": restorePath, "restored": true}, func(w io.Writer) {
			fmt.Fprintf(w, "Restored %s from %s.\n", id, restorePath)
		})
	case "delete":
		fs := newFlagSet("db backup delete")
		var yes bool
		fs.BoolVar(&yes, "yes", false, "Confirm deleting the backup file")
		pos, err := parseFlags(fs, args)
		if err != nil {
			return err
		}
		if len(pos) != 2 {
			return usageErrorf("usage: devbase db backup delete <instance-id> <backup-name> --yes")
		}
		if !yes {
			return usageErrorf("deleting a backup requires --yes")
		}
		if err := m.DeleteBackup(ctx, pos[0], pos[1]); err != nil {
			return err
		}
		return writeResult(env, map[string]any{"instanceId": pos[0], "name": pos[1], "deleted": true}, func(w io.Writer) {
			fmt.Fprintf(w, "Deleted backup %s from %s.\n", pos[1], pos[0])
		})
	default:
		return usageErrorf("unknown db backup subcommand %q (expected list, create, restore, or delete)", sub)
	}
}

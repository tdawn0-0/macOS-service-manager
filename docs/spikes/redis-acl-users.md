# Spike: Redis 7 ACL User Provisioning, Persistence, and Connection Profile Parity

**Context & Status**: Spike design specification for Redis 7 ACL user management and connection profile parity. Baseline: `deb8547`. Follows security constraints from `docs/development-runtimes-and-data.md:61` and connection profile invariants from `internal/devplatform/connection_profiles.go`.

## 1. Problem Statement & Scope

Devbase manages four database engines (`MySQL`, `PostgreSQL`, `Redis`, `MongoDB`). In Connection Profiles (`internal/devplatform/connection_profiles.go:34-52`), Redis connection profiles allow specifying an optional `Username` and `RedisDB` (`0–15`). However:
1. `internal/devplatform/database_other.go:121` (`otherCreateUser`) returns `errors.New("user management is not available for this engine")` for Redis.
2. `internal/devplatform/connection_profiles.go:47-49` enforces `p.Username != "" && p.Username != "default"` as an error, locking profiles to `default`.
3. `view_databases.go:139-142` short-circuits Redis rendering, concealing the user creation form.
4. `internal/devplatform/database.go:48` provisions Redis with `requirepass <password>`, which lacks multi-user support and cannot persist dynamic users without an external `aclfile`.

This spike specifies the technical design to achieve user management and connection profile parity for Redis 7 / Valkey without modifying production code.

## 2. Redis 7 `aclfile` vs `requirepass` Architecture

### Mutually Exclusive Configuration Modes
In Redis 6/7 and Valkey, user configuration operates in one of two mutually exclusive modes:
- **`requirepass` / inline `user` mode**: Defined directly inside `valkey.conf`. Calling `ACL SAVE` produces `ERR This Redis instance is not configured to use an ACL file. Directives cannot be saved.` Dynamic `ACL SETUSER` commands succeed only in volatile memory and are lost on restart.
- **External `aclfile` mode**: Configured via `aclfile <path>`. Redis forbids combining `aclfile` with `requirepass` or inline `user` directives in `valkey.conf` (failing startup with a fatal syntax error). Calling `ACL SAVE` persists runtime ACL state to disk.

### Target `valkey.conf` and `users.acl` Layout
For newly provisioned instances, `internal/devplatform/database.go:48-56` will omit `requirepass` and configure:
```text
bind 127.0.0.1
port <port>
dir "<data-dir>"
aclfile "<instance-dir>/users.acl"
daemonize no
```
Before starting the instance, Devbase seeds `<instance-dir>/users.acl` with mode `0600`:
```text
user default on ><password> ~* &* +@all
```
When `ACL SAVE` runs, Redis rewrites `users.acl` with hashed credentials (`#<sha256>`). Devbase's administrative caller `withRedisSession` (`internal/devplatform/redis.go:96-128`) continues issuing `AUTH <password>` against `127.0.0.1:<port>`, which matches `user default` transparently.

### Upgrade Path for Existing Instances
Existing instances contain `requirepass` and no `aclfile`. Because Redis does not permit changing `aclfile` at runtime via `CONFIG SET` (boot-only directive):
1. **Running legacy instances**: If `CreateUser` is invoked while running without an `aclfile`, return a clear error: `"Redis instance was configured with legacy requirepass; restart the instance to enable persistent ACL user management"`.
2. **Offline migration**: During `startInstance` (`internal/devplatform/database.go:61-89`), inspect `valkey.conf`. If `requirepass <password>` is detected, extract `<password>`, write initial `users.acl` with `user default`, replace `requirepass ...` with `aclfile "<dir>/users.acl"`, and start cleanly.

## 3. Command & Input Safety

### Binary-Safe RESP Execution
User creation must not invoke external shell scripts or interpolated CLI commands. Instead, leverage `withRedisSession` (`internal/devplatform/redis.go:96-130`), which formats each argument as an isolated RESP bulk string (`$<len>\r\n<bytes>\r\n`):
```go
call([]byte("ACL"), []byte("SETUSER"), []byte(name),
    []byte("reset"), []byte("on"), []byte(">"+password),
    []byte("~*"), []byte("&*"), []byte("+@all"),
    []byte("-@admin"), []byte("-@dangerous"))
call([]byte("ACL"), []byte("SAVE"))
```
Bulk string framing guarantees that spaces, quotes, newlines, semicolons, `#`, and `>` within passwords or usernames cannot inject unexpected directives or split ACL tokens.

### Administrative User Isolation
1. **Reject `name == "default"`**: `otherCreateUser` must reject `strings.EqualFold(name, "default")` with `"default user is reserved for instance administration"`. This prevents destroying or demoting the admin account used by Devbase for backups, health checks, and lifecycle tasks.
2. **Input Validation**: Reject control characters in passwords (`!strings.ContainsAny(password, "\x00\r\n\t")`), matching `TestCreateUserRejectsControlCharactersAndIsolatesPsqlEnvironment`. Usernames must satisfy `validID(name)`.

## 4. Numbered-DB Scope Honesty & Connection Profiles

### Redis ACL Limitations
Unlike relational databases or MongoDB, Redis ACLs control commands (`+@all`), key patterns (`~*`), and channels (`&*`), but **cannot restrict access to specific numbered databases (`0–15`)**. Any user with permission to execute `SELECT` can read and write across all numbered databases.

### UI Honesty & Profile Prefill
1. **Form Disclosure**: In `view_databases.go`, render user creation for Redis with an explicit note:
   *"Redis users authenticate instance-wide across all databases (0–15). Selected database sets the default connection profile."*
2. **Profile Validation**: In `internal/devplatform/connection_profiles.go:47-49`, update `validateConnectionProfile` to permit valid non-empty usernames for Redis (`p.Username == "" || profileText(p.Username, false)`).
3. **Connection URI Generation**: In `ProfileConnectionInfo` (`connection_profiles.go:124`), format:
   ```go
   user := profile.Username
   if user == "" { user = "default" }
   ```
   generating `redis://<username>@127.0.0.1:<port>/<db>`.
4. **Offer Integration**: Ensure `connectionOperationOffer` sets `Username` and `RedisDB` on Redis user creation so `Use created database/user in connection profile` pre-fills the connection draft.

## 5. Storage Location, Crash Rollback & Backup Interaction

### Storage Location Invariant
Per `docs/development-runtimes-and-data.md:61`:
> *"Credentials, configuration, launchd jobs and task records stay in application storage."*

`users.acl` MUST be placed at `filepath.Join(m.instanceDirectory(i.ID), "users.acl")` with `0600` permissions. It must **never** be placed inside `DatabaseDataDirectory(i)`. When an external data directory is configured, external volumes must never receive credential files. Removing an instance deletes application storage while preserving external data, ensuring credentials are wiped cleanly.

### Crash & Rollback Safety
- **Staged Initialization**: If instance creation fails before state commit, `cleanupFailedInstance` wipes `instances/<id>/`, removing `users.acl` atomically without leaving orphaned credential files.
- **Atomic Writes**: `ACL SAVE` writes to a temporary file (`users.acl.tmp`) before renaming, preventing partial file corruption on process termination.

### Logical Backup & Restore Isolation
Devbase backups (`redisBackup` / `redisRestore` in `internal/devplatform/redis.go:164-407`) serialize and restore key-value payloads for a specific numbered database using batch Lua dumps. User ACL definitions are instance-level authentication records, not keyspace data. Backups and restores do not modify `users.acl`.

## 6. Open Questions for Maintainers & Recommended Defaults

1. **Default Command Permissions**:
   - *Options*: Grant full access (`+@all ~* &*`) vs Restrict administrative commands (`+@all -@admin -@dangerous ~* &*`).
   - *Recommendation*: **Restricted (`+@all -@admin -@dangerous ~* &*`)**. Standard application roles require read/write data operations, but should not be permitted to execute `CONFIG`, `SHUTDOWN`, `DEBUG`, or mutate ACLs. The `default` user retains full admin rights.
2. **Legacy Instance Migration**:
   - *Options*: Automatic offline migration on next start vs Manual user opt-in.
   - *Recommendation*: **Automatic offline migration on next start**. Updating `valkey.conf` and seeding `users.acl` while stopped is safe, deterministic, and preserves user credentials without user intervention.
3. **Password Persistence in Connection Profiles**:
   - *Options*: Store password in `ConnectionProfile` vs Exclude credentials.
   - *Recommendation*: **Exclude credentials**. Keep `ConnectionProfile` zero-credential (`docs/resources-acceptance.md`). Profiles provide `redis://<username>@127.0.0.1:<port>/<db>`, and applications inject passwords from local environment variables.

## 7. Follow-on Implementation & Verification Plan

Following design approval, implementation will introduce:
1. `internal/devplatform/redis_acl.go`: `redisCreateUser` implementing binary-safe RESP `ACL SETUSER` and `ACL SAVE`.
2. `internal/devplatform/database.go`: `valkey.conf` generation with `aclfile` and automatic offline migration for legacy instances.
3. `internal/devplatform/connection_profiles.go`: allow named usernames in Redis profiles and URI formatting.
4. `view_databases.go` & `view_database_connections.go`: UI form activation, scope disclaimer, and offer prefill.

**Target Test Suites**:
- `TestRedisACLUserCreationAndPersistence`: Unit test validating `ACL SETUSER`, `ACL SAVE`, and `users.acl` contents.
- `TestRedisACLRejectsDefaultAndControlChars`: Validates rejection of `default` and newline injection in passwords.
- `TestConnectionProfilesRedisCustomUser`: Unit test asserting Redis connection URI formatting with named users.
- `TestUIConnectionProfilesRedisOffer`: UI tester asserting offer prefill from Redis user creation.

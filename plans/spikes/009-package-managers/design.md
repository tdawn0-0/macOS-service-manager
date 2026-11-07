# Project dependency installation — design spike

Status: design validated offline, not a shipped feature. Evidence baseline `0cbf7e9`; reviewed 2026-11-07. No repository scripts, dependency tools, downloads, or installations were executed for this spike.

## Goal

Make the existing dependency action explain and use the project's intended npm, Yarn or pnpm toolchain. Show the exact manager version, source, working root and command before a user runs repository lifecycle scripts. A reviewed, reproducible tool is required even when a lockfile identifies the manager family.

## Non-goals

No Bun, Deno, Composer, lockfile migration, arbitrary command editor, global shell changes, automatic dependency installation on import, or second background task scheduler. No promise that a lockfile makes dependency scripts safe. This spike adds no production Go code.

## Evidence

- `view_development.go`, `devEnvironment`: package.json presence currently enables a button executing `Manager.Exec(..., []string{"npm", "ci"})`, disabled for dirty projects or active tasks.
- `internal/devplatform/operations.go:470`, `Manager.Exec`: existing operation/command-work path can receive explicit argv.
- `docs/local-dev-platform-plan.md`, section 5.2: npm ci is an example of a user-initiated dependency command; import is observational.
- `docs/development-ui.md`: retain the native form style, stable accessibility names, 900×520 minimum, two themes, and persistent save affordance.

Official primary documentation checked 2026-11-07:

| Reference | Confirmed implication |
|---|---|
| [npm ci, CLI v11](https://docs.npmjs.com/cli/v11/commands/npm-ci/) | Requires package-lock.json or npm-shrinkwrap.json; fails rather than updating an inconsistent lock; may remove node_modules. This is an installation with lasting effects, not a preview. |
| [Yarn Classic install](https://classic.yarnpkg.com/lang/en/docs/cli/install/) | Yarn 1 has a frozen-lockfile installation option. Do not apply modern-only flags to Classic. |
| [Modern Yarn install](https://yarnpkg.com/cli/install) | Immutable mode rejects lockfile modifications; install may run build scripts. |
| [pnpm install](https://pnpm.io/cli/install) | Frozen-lockfile retains the lock; offline uses the store only. A workspace install may affect multiple member projects. |
| [Node 22.14 package metadata](https://nodejs.org/download/release/v22.14.0/docs/api/packages.html#packagemanager) | packageManager expresses a manager/version preference. A declaration does not establish a verified executable source. This versioned page matches a runtime in the catalog; avoid assuming current Node bundles the same tools. |

These references validate proposed command semantics. They do not establish app-owned pnpm/Yarn distribution, supported version catalog, or compatibility across every manager version. A later implementation must qualify exact versions against the selected Node.

## Detection

Read bounded package.json and lockfile metadata as data at the selected project root. Never execute config/plugins to detect a manager. Prefer an exact supported packageManager declaration, then a single recognized lockfile family. npm's two recognized lockfiles belong to one family; multiple different families require a decision. Malformed/unsupported declarations block; do not silently fall back to npm. URL/hash/prerelease declarations are deliberately outside this initial exact-version grammar and must be explained as unsupported rather than guessed.

Workspace membership is a separate review boundary. A future Go detector must discover the workspace root without leaving the reviewed checkout, show affected members, and obtain a reviewed-root decision. The prototype receives this already classified boolean input; it does not implement filesystem traversal or validate lockfile contents. No lock means this frozen-install action is blocked; a future explicit initial-lock creation workflow is a separate product choice.

## Execution contract

`resolve_request(fixture)` returns status, manager, version, argv, cwdPolicy, reason and requiresConfirmation. Only `ready` has argv; all cases require user confirmation before any eventual execution. No path, executable, script body or original error is reflected in output. Manager names are finite and command arguments are fixed arrays.

Synthetic `tools` records represent a future resolver's verified exact installations. npm must come from the selected Node bundle. Yarn/pnpm require an app-owned exact version with reviewed integrity, license, runtime compatibility and executable identity. The app does not yet provide that acquisition contract: until it exists, real requests for those tools remain blocked even though synthetic prototype records can model successful resolution. Never invoke system PATH, npx, corepack download, or global npm installation as an implicit fallback.

Commands modeled after tool availability is confirmed:

| Family | Candidate argv | Additional boundary |
|---|---|---|
| npm | npm ci | bundled npm version and selected Node pairing |
| Yarn 1 | yarn install --frozen-lockfile | reviewed exact Classic version |
| Yarn 2–4 | yarn install --immutable | reviewed exact modern version; repository plugins/config still execute in installation |
| pnpm | pnpm install --frozen-lockfile | reviewed exact version and workspace scope |

Offline is initially modeled only for pnpm by appending --offline. It may fail when the store lacks packages; no network retry is implicit. Offline npm/Yarn remain blocked pending an explicit, tested cache/config policy. Normal online installation can use manager registry config; credentials stay local and are excluded from previews/history/export.

## UX

1. One clear tool: save edits, select Install project dependencies, see version/source/root/argv and script warning, then Run or Cancel. Run uses the existing background task and preserves navigation drafts.
2. Conflicting evidence: show declaration and lockfile names; no Run command until the user corrects/selects the intended policy. Cancel leaves files untouched.
3. Missing tool: show exact required version and why unavailable; return to Software or project settings. Do not install it while navigating. Any future acquisition is separately reviewed before dependency execution.

Keep current dirty/task-running disabling. At 900×520 and both themes the command and consequences scroll while Run/Cancel remain reachable. Use native labels `Review dependency install` and `Run dependency install`; include script consequences in the action tooltip/review, not an unrelated page banner.

## Security and cancellation

Execute only after a reviewed snapshot fingerprint of declaration/lock/root/runtime still matches. Changed files require a fresh preview. Args remain an array; no shell concatenation. Reuse runCommandWork ownership, process cancellation and task reservations. Cancellation does not undo node_modules, lifecycle script effects or external network writes. Never overwrite .env, persist registry tokens into shareable declarations, log raw config, or claim all operations are transactional.

## Compatibility

Preserve existing saved environments and command tasks. No shared schema change is required for initial local preview; store any future manager choice separately from secrets. The prototype is stdlib Python for decision testing only; production remains Go/MyGo. Current fixed npm ci behavior stays until a separately reviewed implementation lands.

## Decision matrix

`cases.json` is schemaVersion 1 with 23 independently specified cases. It covers npm lock/no lock, shrinkwrap, exact declarations, Yarn Classic/modern, pnpm/offline, conflicts, no runtime, unavailable versions, untrusted PATH, unsupported tools, workspace review, invalid manifest, dirty drafts, and scripts treated as data. `test_prototype.py` compares the full output and checks input immutability and absence of execution APIs. Cases are not fed into the implementation as answers.

## Rollout

Recommended order (each a future Go implementation milestone):

1. **Bounded detector**: internal/devplatform/project_inspection.go plus tests; classify only approved files. Validate golden detection cases and read limits without executing project code.
2. **Review UI**: devEnvironment button, stable review model and native input tests; require saved project and preview fingerprint; confirm/cancel preserves drafts. Reuse plan 004's project navigation protection.
3. **Tool policy**: define exact catalog/integrity/Node compatibility and acquisition UI for pnpm/Yarn. Until accepted, show blocked rather than substitute npm. Test resolution against synthetic receipts first, then isolated real tools.
4. **Execution**: hand approved argv/root/runtime to Manager.Exec/runCommandWork; verify version and root in an isolated project, cancellation and retry. Keep credentials out of report/history.
5. **Qualification**: run actual npm/Yarn/pnpm lockfile fixtures, workspace member boundaries and registry/offline failures; render minimum/default sizes and themes. Promote only qualified version pairs.

Rejected: invoking whatever PATH provides (not reproducible), automatically downloading through a shim (changes machine before review), running npm install when lock missing (silently creates or changes project state), and implementing a generic shell workflow (larger trust boundary).

## Open questions

Maintainer decisions before production: which exact pnpm/Yarn versions and integrity sources to support; whether initial lock creation is worthwhile; whether monorepos install the whole workspace or a reviewed member; how to present manager-specific config that affects lock interpretation. These do not block the offline design result. Current conservative answers are explicit unavailable/needs-choice states, never guessed execution.

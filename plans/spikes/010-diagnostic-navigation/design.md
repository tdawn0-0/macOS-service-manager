# Diagnostic settings navigation — design spike

Status: offline contract validated; no diagnostic button or product action added. Baseline `0cbf7e9`, inspected 2026-11-07. Plan 004's ongoing navigation guard is an anticipated integration dependency, not a reason to bypass dirty drafts.

## Goal

Let a diagnostic check take the user to the correct local settings page with one explicit navigation. Reduce resource hunting while preserving drafts, stable identity and anonymous report export.

## Non-goals

No automatic repair, CA trust change, hosts write, port reassignment, service stop, package reinstall, data deletion, shell execution, arbitrary URL opening, or public deep-link API. The Python model never reads the machine or invokes a backend. Completing this spike does not mean the App feature shipped.

## Evidence

- `view_diagnostics.go`, devDiagnostics: each check currently renders Status, Layer, Resource, Summary and Remedy text without a target control.
- `internal/devplatform/diagnostic_report.go`, DiagnosticCheck: display resource lacks typed action identity. RedactedJSON currently aliases only Resource, so adding serialized targets naively would expose identifiers.
- `view_development.go`, selectProject: rebuilds draft and service editor; future user navigation must use the plan 004 guard, not call it unconditionally.
- `view_development.go`, selectSite(site, project): existing site selection already accepts owner identity; same site ID can belong to different projects.
- `docs/development-ui.md`: diagnostics return preserves drafts, uses native controls, and exports anonymous resource names. Minimum size 900×520; default 1120×720, light/dark.

## Action schema

A local action has a finite kind and required IDs, never an executable closure, filesystem path, URI, raw error or parsed Remedy sentence. Keep this metadata in a private local report structure (recommended), alongside the display report. A future Go struct can use a typed kind and resource references with no JSON representation. An explicit exported DTO remains the only report serialization path.

| Kind | Stable identity | Existing destination | Missing target |
|---|---|---|---|
| project-service | project ID + service ID | Projects, service settings via user selection guard | refresh diagnostics |
| instance | instance ID | Databases, existing instance editor (`dbID` selection) | refresh diagnostics |
| site | owner project ID (or standalone empty) + site ID | HTTPS through selectSite | refresh diagnostics |
| package | package ID incl exact version/arch | Software, matching installed version details | refresh diagnostics |
| ca | fixed local CA settings action | HTTPS authority settings, no certificate material in target | unavailable if page capability absent |
| back | fixed action, no resource ID | original workspace | preserve exact existing draft |

Port failures target the owning resource's settings and its existing port inspection control; they do not propose a replacement port. Layer text is not a routing key: the producer knows the owning entity and creates the action while constructing the local report. Same-name resources must never be selected by fuzzy or display-name matching.

## Target resolution

`resolve_action` consumes action + currentState + dirtyDraft + optional stale/cancelled; it returns status, target, messageKey, mutations. All cases have mutations=[]; status is navigate, confirm-discard or unavailable. IDs use a finite syntax for the prototype and are still checked for current membership. The real Go resolver must use current State identity and type, not just string validity.

The snapshot is an observation. If explicitly stale, require rerun; if the ID disappeared or changed, return target-missing rather than picking a similarly named object. Site owner participates in lookup. A different live record with the same stable identity may supply refreshed display fields, but the original report must not mutate it. Revalidate again when a modal's positive callback executes because a resource may have disappeared while the user reads it.

## Draft safety

Same-project service navigation preserves its current draft rather than reloading saved values. Cross-project dirty navigation requests the existing Cancel/Discard changes interaction via plan 004's user navigation guard. Cancelling returns navigation-cancelled without a target or mutation. Other workspaces can preserve the stored project draft; no automatic Save is performed. Returning from Diagnostics uses existing workspace state, never reselects the saved project merely to refresh its display.

Future navigation should reject busy/operation-blocked transitions through the same existing guard and revalidate ID after confirmation. The prototype models user cancellation and dirty identity; it does not implement MyGo scheduling or claim race-free modal callbacks.

## Redaction

Recommended production boundary: private action metadata + explicit exported Diagnostic DTO. Do not add a JSON action then only alias Resource. Export only schemaVersion, redacted, checks and each check's approved layer/status/fixed summary/remedy/anonymous resource alias. No target, owner, project ID, service ID, domain, path, command, raw error, arbitrary unknown field, timestamp-as-text or closure crosses this boundary.

The prototype strengthens the boundary by deriving summary/remedy from finite diagnostic codes, not copying untrusted strings. Unknown codes get fixed generic text; unknown layers/status get generic enums. Resource values only define stable per-report aliases; repeated references share an alias, and changing actual identities preserves anonymous structure. Production's existing Go fixed strings can use an equivalent typed message-code mapping without a shared schema migration. A report is not a trusted command transport even when its original fields look safe.

Export contract changes must preserve useful failure-layer information while tests plant synthetic private strings in every candidate field. A field added later is excluded unless explicitly reviewed; unknown fields are never passed through.

## UX

At most one primary navigation button per check: Open project settings, Open database settings, Open HTTPS settings, Open software settings, or Open CA settings. Remedy stays visible. Button tooltip explains the destination; do not label it Repair when it only navigates. System authorization remains a separate explicit action on the destination page.

At 900×520, long resource names wrap within scrollable diagnostic cards; the main control has a stable accessibility name including an internal UI key to distinguish repeated labels without exposing hidden IDs as visible prose. Test light/dark and default size. Opening target settings closes Diagnostics only after a valid navigation or successful discard confirmation; Cancel preserves the diagnostic snapshot and underlying draft. Unavailable actions show a concise refresh reason and offer Run diagnostics again.

## Decision matrix

`cases.json` has 18 independent fixtures covering five settings kinds, standalone back, two same-ID sites with distinct owners, deleted/changed IDs, stale snapshot, unknown action, same/cross project dirty drafts, cancellation, absent remedy target, invalid path-shaped ID and missing service. `prototype.py` is a pure resolver, not a case-ID lookup. Test fixtures are virtual; no existing user state is read.

## Rollout

1. **Typed producer identities**: add local nonserialized action metadata at diagnose's project/instance/site/software/CA construction points. Keep report export separate. Tests verify exact owner and no target for unsupported checks.
2. **Explicit export projection**: internal/devplatform/diagnostic_report.go plus redaction tests; add synthetic private IDs/unknown fields and show anonymous structure invariant across two identities.
3. **Navigation resolver**: view_diagnostics.go and a small local helper; map valid typed action to current State and existing project/instance/site/software/CA selection. Use plan 004 guard for project transitions; no direct backend mutation.
4. **MyGo inputs**: buttons use existing native UI conventions; tests click valid, stale, missing, same-name, Cancel and Discard paths. Verify unsaved service editor fields survive return.
5. **Visual QA**: render minimum/default sizes and themes with long synthetic names and multiple identical labels. Test busy/task constraints without operating actual hosts or keychain.

Each milestone should be separately reviewable with tests before wiring the next. The Python model is disposable contract evidence; port cases into Go and decide whether to retire the prototype when production functionality lands.

## Open questions

Which diagnostic layers lack a single destination, and should they retain text only? Should stale reports always require rerun or may identity-valid checks still navigate with a stale badge? Recommendation: text-only for ambiguous checks, conservative stale rejection first. CA scope should initially be a general settings page, not a specific certificate action. Public report schema should remain independent from local action enum evolution.

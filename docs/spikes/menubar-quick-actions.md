# Spike: Menu Bar Quick-Actions for HTTPS Domains and Database Logs

**Context & Status**: Spike design specification for menu bar quick-actions on macOS. Baseline: `deb8547`. Follows constraints from `docs/product-scope.md:7,13` and `docs/resources-acceptance.md:16` (`AC-UI`).

## 1. Problem & Evidence

The menu bar panel (`menubar.go:38-41`, `340px` wide, `620px` max height) serves as the compact daily control surface for Devbase. Currently, two frequent daily actions force users to open the full 900×520 main window:

1. **Database log diagnostics**: When a database shows `Failed` or fails to start, users must open the main window, select the instance, and click `icoScroll` (`view_development.go:937-941`) to call `m.Logs("db-" + id)` (`internal/devplatform/logs.go:15-37`). While `menubar.go:281-296` provides a hover button to copy connection URIs, it lacks any log action. macOS Console helper `system.OpenInConsole` (`internal/system/system.go:106-122`) is never leveraged from the menu bar.
2. **Opening HTTPS `.test` domains**: In the main window, `view_https_proxy.go:80-89` provides an `icoLink` action on each rule to launch `/usr/bin/open https://<domain>[:<port>]`. In `menubar.go:364`, `panelProxy` only offers a click target to open settings in the main window; users cannot jump directly to their active web app.

## 2. Product Scope Guardrails & AC-UI Invariants

Devbase maintains a strict product boundary between managed background instances and reverse-proxy routing rules:

- **Proxy rules are not service processes**: Per `docs/product-scope.md:7`, *"The menu bar shows independently managed database instances and one HTTPS proxy; installed software and the number of proxy rules are not running-process counts."*
- **Rules lack lifecycle controls**: Per `docs/product-scope.md:13`, *"Rules have no separate resource ID, detail page, enable switch, or process lifecycle... The app does not start or manage the upstream process; users start it in their terminal or IDE."*
- **Single proxy entry invariant**: `docs/resources-acceptance.md:16` (`AC-UI`) mandates that *"menu bar shows one proxy entry rather than individual sites"*.
- **Regression enforcement**: `resource_lifecycle_ui_test.go:31-33` strictly fails if rule domains appear as top-level menu resources (`strings.Contains(shown, "one.test") || strings.Contains(shown, "two.test")`). `menubar.go:329-331` reiterates: *"Its rules are not resources of their own, and are not listed."*

Any design must respect this distinction: domains must never display toggle switches, status dots, or standalone resource rows in the menu bar.

## 3. Concrete UX & API Design

### A. Database Hover Log Action (Zero Scope Tension)

In `menubar.go:281-296`, `panelDatabase` currently passes a single `iconButton` (`icoCopy`) into `panelRow`'s `hover` closure.
- **Design**: Add a second `iconButton(c, icoScroll, "Open logs for "+id, tk.TextSecondary)` beside `icoCopy`.
- **Action flow & fallback**:
  1. Target log files: `filepath.Join(m.Root, "logs", "db-"+id+".log")` and `.err.log`.
  2. If `db-<id>.err.log` (non-empty) or `db-<id>.log` exists, execute `system.OpenInConsole(path)` (`internal/system/system.go:106-122`).
  3. If no log files exist on disk (unstarted or clean instance) or `OpenInConsole` fails, fall back to opening the main window at that instance log view (`a.openInstance(id)`, `d.showResult = true`, `openWindow()`), where `internal/devplatform/logs.go:15-37` renders *"No logs yet. Start the service to capture its output."*
  4. Post an informative toast: `"Opened logs in Console"` or `"Opened database logs"`.

### B. HTTPS Domain Launch (Two Options for Maintainer Choice)

- **Option A: Strict AC-UI Single-Row Quick-Action (Recommended)**
  - Maintain `panelProxy` (`menubar.go:329-378`) as a single row.
  - When `len(d.state.ProxyRules) == 1` and `status == "Running"`:
    - Populate `panelRow`'s currently `nil` hover slot (`menubar.go:364`) with `iconButton(c, icoLink, "Open HTTPS URL", tk.TextSecondary)`.
    - Clicking it runs `m.Run(m.Context, "/usr/bin/open", targetURL)` using the domain and port logic from `view_https_proxy.go:80-89`.
  - When `len(d.state.ProxyRules) > 1` (or proxy is stopped):
    - Omit the hover button. The row title/subtitle (`"Proxy · Running"`, `"N rules · :443"`) and click target remain the entry point to `devHTTPS` in the main window.
  - *Compliance*: 100% compliant with `AC-UI` (`docs/resources-acceptance.md:16`), keeps `menubar.go:330` intact, and cleanly passes `resource_lifecycle_ui_test.go:31-33` without test changes.

- **Option B: Explicit Folded Bookmark Links (Requires Spec Amendment)**
  - Under `panelProxy`, render a folded-by-default disclosure section via `panelSection` (`menubar.go:149-186`): `"Saved URLs (N) ▸"`.
  - When explicitly expanded by user click, render read-only hyperlink rows (icon `icoLink` + domain text), with NO switch and NO status dot. Clicking opens `/usr/bin/open`.
  - *Compliance*: Requires updating `menubar.go:330` and amending `AC-UI` wording in `docs/resources-acceptance.md:16`. Test `resource_lifecycle_ui_test.go:31-33` would need to assert that URLs are hidden when collapsed.

## 4. Layout & 340px Geometry Analysis

- **Row Budget** (`menubar.go:38-41`, `panelWidth = 340px`):
  - Row padding: `Padding(0, 8)` (`menubar.go:193`) leaves `324px` content width.
  - Leading icon tile + gap: `26px + 10px = 36px`.
  - Trailing switch: `choice.SwitchProps{Size: theme.Small}` (`menubar.go:304`) consumes `~36px + 10px gap = 46px`.
  - Database hover buttons (`icoCopy` + `icoScroll`): `2 × 26px` + `4px` gap + `10px` row gap = `66px`.
  - Text body (`title` + `sub`): Wraps in `ui.Column` with `Grow(1).MinWidth(0)` and `SingleLine().MinWidth(0)` (`menubar.go:207-211`).
  - Available body width when hovered: `324px - 36px - 46px - 66px = 176px`. This accommodates labels like `local-postgres` and subtitles like `PostgreSQL · :5432` with automatic ellipsis truncation and `Tooltip(title)` fallback (`menubar.go:208`).
- **Vertical Height Budget**:
  - Each `panelRow` is fixed at `Height(40)` (`menubar.go:193`). Hover actions are inline; height consumption is zero.
  - Overall panel ceiling is `panelMaxHeight = 620px` (`menubar.go:40`). Option A adds zero vertical pixels; Option B would add `N × 30px` when expanded.
  - Verification: At both 340px menu panel and 900×520 main tester frame (`resource_ui_test.go:26-28`), row truncation and bounds behave predictably.

## 5. Open Questions & Recommendations

1. **Console.app vs Main Window Viewer for Logs**:
   - *Question*: Should the database log hover button launch macOS `Console.app` directly or open the in-app viewer?
   - *Recommendation*: Attempt `system.OpenInConsole` (`internal/system/system.go:106-122`) when a `.log` or `.err.log` file exists on disk. If absent or empty, fall back to opening the instance page in the main window (`view_development.go:937-941`).
2. **HTTPS Multi-Rule Choice (Option A vs Option B)**:
   - *Question*: Should the menu bar remain single-row (Option A) or support a folded disclosure for multiple rules (Option B)?
   - *Recommendation*: **Option A**. It satisfies the 80% use-case (single active project domain), preserves strict `AC-UI` (`docs/resources-acceptance.md:16`), prevents menu bar clutter, and avoids scope creep into process/site management.
3. **Menu Bar Dismissal on Action Launch**:
   - *Question*: Should the menu bar panel close when launching a browser URL or Console.app?
   - *Recommendation*: Keep panel open (matching `Copy connection URI` in `menubar.go:281-295`), and display a feedback toast notification.

## 6. Implementation & Verification Plan

Following maintainer decision on Option A vs Option B:
1. **Database log action**:
   - Update `menubar.go:281-296` to insert `icoScroll` icon button into `panelRow`'s `hover` callback.
   - Implement log target resolution checking `db-<id>.log` and `db-<id>.err.log`.
   - Add unit test in `resource_ui_test.go` asserting hover action presence and fallback behavior.
2. **HTTPS domain launch**:
   - If **Option A**: Update `menubar.go:364` to wire `icoLink` into `panelRow`'s `hover` closure conditioned on `len(d.state.ProxyRules) == 1 && status == "Running"`. Verify with `go test . -run TestUIIndependentResourceLifecycle`.
   - If **Option B**: Update `menubar.go:329-378` with `panelSection`, update `AC-UI` in `docs/resources-acceptance.md:16`, and update `resource_lifecycle_ui_test.go:31-33` to test folded vs unfolded states.
3. **Verification gate**: Run `go test . -run '^(TestUIResourceOnlyNavigation|TestUIIndependentResourceLifecycle)$' -count=1` and `python3 scripts/collect-third-party-notices.py --check`.

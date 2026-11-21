//! Design tokens from the Go app's `theme.go`, as typed styles read them.
//!
//! Colors live in `theme.json` (`nsh.json` `"theme"`): the Liquid Glass
//! sidebar and workspace surfaces, plus the semantic roles of Lobe UI. This
//! module names them for Rust code, so screens never write a color, radius or
//! font size of their own ([`crate::components::placeholder`],
//! [`crate::components::sidebar`]).
//!
//! | Go (`theme.go`) | Here |
//! |---|---|
//! | `glassTone.sidebarSolid` | `theme().sidebar` |
//! | `glassTone.pane` | `theme().background` |
//! | `glassTone.sheet` | [`sheet`] (token `sheet`) |
//! | `glassTone.panel` | [`panel`] (token `panel`) |
//! | `glassTone.edge` | [`hairline`] (`theme().border`) |
//! | `glassTone.hover` | [`row_hover`] (token `row-hover`) |
//! | `glassTone.selected` | `theme().sidebar_accent` |
//! | `glassPanel` radius 12 | [`panel_radius`] (token `radius-panel`) |
//! | `textSmall` 12, `textCaption` 11 | [`TEXT_SMALL`], [`TEXT_CAPTION`] |
//! | `t.Spacing = 3.5` | [`SPACING`], [`space`] |

use nsh_base::theme;
use nsh_core::prelude::*;

/// Body text: the theme's base size (Go's theme drew text at 13).
pub(crate) const TEXT_BODY: f32 = 13.0;

/// Secondary text below the base size (Go's `textSmall`).
pub(crate) const TEXT_SMALL: f32 = 12.0;

/// Captions and metadata (Go's `textCaption`).
pub(crate) const TEXT_CAPTION: f32 = 11.0;

/// Hidden title bar height in points, matching Go's `barHeight` (traffic lights
/// sit centered in this strip).
pub(crate) const TITLE_BAR_HEIGHT: f32 = 52.0;

/// The Go app's spacing step, used when a theme does not set `spacing`.
const DEFAULT_SPACING: f32 = 3.5;

/// `steps` spacing steps in points: [`space(4.0)`](space) is 14pt.
pub(crate) fn space(steps: f32) -> Points {
    px(theme().number("spacing").unwrap_or(DEFAULT_SPACING) * steps)
}

/// The workspace's scrolling detail surface (`glassTone.sheet`).
pub(crate) fn sheet() -> AdaptiveColor {
    color_or("sheet", theme().card)
}

/// The card surface on the sheet (`glassTone.panel`).
pub(crate) fn panel() -> AdaptiveColor {
    color_or("panel", theme().card)
}

/// A list row or button under the pointer (`glassTone.hover`).
pub(crate) fn row_hover() -> AdaptiveColor {
    color_or("row-hover", theme().accent)
}

/// The row chosen in a list (`glassTone.selected`).
pub(crate) fn row_selected() -> AdaptiveColor {
    color_or("row-selected", theme().sidebar_accent)
}

/// A well inside a panel (`glassTone.inset`).
pub(crate) fn inset() -> AdaptiveColor {
    color_or("inset", theme().muted)
}

/// The fill of an outlined control (`glassTone.control`).
pub(crate) fn control() -> AdaptiveColor {
    color_or("control", theme().card)
}

/// A state that runs or succeeded (Lobe's `success`).
pub(crate) fn success() -> AdaptiveColor {
    color_or("success", theme().primary)
}

/// A state that waits or needs attention (Lobe's `warning`).
pub(crate) fn warning() -> AdaptiveColor {
    color_or("warning", theme().primary)
}

/// The thin line between parts of the window (`glassTone.edge`).
pub(crate) fn hairline() -> AdaptiveColor {
    theme().border
}

/// The corner radius of a card on the sheet (Go's `glassPanel` radius of 12).
pub(crate) fn panel_radius() -> Points {
    px(theme().number("radius-panel").unwrap_or(12.0))
}

/// Subtle adaptive tint over macOS 26 Liquid Glass in the sidebar so navigation
/// labels stay legible across bright or busy wallpapers.
pub(crate) fn sidebar_glass_tint() -> AdaptiveColor {
    theme().sidebar.opacity(0.18)
}

/// `name` from the app's theme, or `fallback` when a theme does not set it.
fn color_or(name: &str, fallback: AdaptiveColor) -> AdaptiveColor {
    theme().color(name).unwrap_or(fallback)
}

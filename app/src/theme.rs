//! Design tokens for Devbase's macOS 26 / Linear surface and color system.
//!
//! Colors live in `theme.json` (`nsh.json` `"theme"`): the Liquid Glass
//! sidebar and workspace surfaces, plus semantic status and control tokens.
//! This module names them for Rust code so screens never hard-code colors,
//! radii, or spacing steps.

use nsh_base::theme;
use nsh_core::prelude::*;

/// Body text: the theme's base size (13pt).
pub(crate) const TEXT_BODY: f32 = 13.0;

/// Secondary text below the base size (12pt).
pub(crate) const TEXT_SMALL: f32 = 12.0;

/// Captions and metadata (11pt).
pub(crate) const TEXT_CAPTION: f32 = 11.0;

/// Hidden title bar height in points (traffic lights sit centered in this strip).
pub(crate) const TITLE_BAR_HEIGHT: f32 = 52.0;

/// Base integer spacing grid step in points, used when a theme does not set `spacing`.
const DEFAULT_SPACING: f32 = 4.0;

/// `steps` spacing steps in points: [`space(4.0)`](space) is 16pt on the 4px grid.
pub(crate) fn space(steps: f32) -> Points {
    px(theme().number("spacing").unwrap_or(DEFAULT_SPACING) * steps)
}

/// The workspace's scrolling detail surface (`sheet`).
pub(crate) fn sheet() -> AdaptiveColor {
    color_or("sheet", theme().card)
}

/// The card surface on the sheet (`panel`).
pub(crate) fn panel() -> AdaptiveColor {
    color_or("panel", theme().card)
}

/// A list row or button under the pointer (`row-hover`).
pub(crate) fn row_hover() -> AdaptiveColor {
    color_or("row-hover", theme().accent)
}

/// The row chosen in a list (`row-selected`).
pub(crate) fn row_selected() -> AdaptiveColor {
    color_or("row-selected", theme().sidebar_accent)
}

/// A well inside a panel (`inset`).
pub(crate) fn inset() -> AdaptiveColor {
    color_or("inset", theme().muted)
}

/// The fill of an outlined control (`control`).
pub(crate) fn control() -> AdaptiveColor {
    color_or("control", theme().card)
}

/// The off-state track fill for toggle switches, guaranteed visible over both
/// `panel` and `inset` surfaces in Light and Dark modes (`switch-track-off`).
pub(crate) fn switch_track_off() -> AdaptiveColor {
    color_or("switch-track-off", theme().foreground.opacity(0.20))
}

/// A state that runs or succeeded (`success`).
pub(crate) fn success() -> AdaptiveColor {
    color_or("success", theme().primary)
}

/// A state that waits or needs attention (`warning`).
pub(crate) fn warning() -> AdaptiveColor {
    color_or("warning", theme().primary)
}

/// The thin hairline divider between surfaces (`border`).
pub(crate) fn hairline() -> AdaptiveColor {
    theme().border
}

/// The corner radius of a card on the sheet (`radius-panel`, 10pt).
pub(crate) fn panel_radius() -> Points {
    px(theme().number("radius-panel").unwrap_or(10.0))
}

/// Subtle adaptive tint over macOS 26 Liquid Glass in the sidebar so navigation
/// labels stay legible across bright or busy wallpapers.
pub(crate) fn sidebar_glass_tint() -> AdaptiveColor {
    theme().sidebar.opacity(0.20)
}

/// Adaptive glass tint applied to `NSGlassEffectView` inside the menu bar popover.
pub(crate) fn popover_glass_tint() -> AdaptiveColor {
    theme().popover.opacity(0.45)
}

/// Subtle translucent surface scrim layered above `NSGlassEffectView` inside the
/// menu bar popover so light and dark text stay crisp over any desktop wallpaper.
pub(crate) fn popover_surface_scrim() -> AdaptiveColor {
    theme().popover.opacity(0.82)
}

/// `name` from the app's theme, or `fallback` when a theme does not set it.
fn color_or(name: &str, fallback: AdaptiveColor) -> AdaptiveColor {
    theme().color(name).unwrap_or(fallback)
}

//! The navigation rail: the app's identity over the six sections. On macOS 26
//! outside full screen the rail renders over native Liquid Glass
//! (`NSGlassEffectView`), falling back to `theme().sidebar` in full screen or
//! off macOS; the workspace beside it stays opaque.

use devcore::service::{Source, State as ServiceState};
use nsh_base::{button, theme};
use nsh_core::{
    GlassEffect, PointerEvents,
    element::{ImageSrc, Svg, desktop::title_bar, svg},
    prelude::*,
};

use crate::{
    command_palette,
    components::{
        controls,
        icons::{self, Icon},
    },
    meta, shell,
    store::{self, Section, SectionGroup, databases, ports, proxy, services},
    theme as tokens,
};

/// Monochrome `currentColor` variant of Devbase's signature dual-toggle mark
/// (`assets/icons/menubar.svg`), rendered subtly beside the sidebar wordmark.
const DEVBASE_GLYPH_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 36 36">
  <path fill="currentColor" fill-rule="evenodd" d="M9.5 3h17a6.5 6.5 0 0 1 0 13h-17a6.5 6.5 0 0 1 0-13Zm17 2.9a3.6 3.6 0 1 0 0 7.2 3.6 3.6 0 0 0 0-7.2Z"/>
  <rect x="4.2" y="21.2" width="27.6" height="10.6" rx="5.3" fill="none" stroke="currentColor" stroke-width="2.6"/>
  <circle cx="9.5" cy="26.5" r="2.7" fill="currentColor"/>
</svg>"##;

fn section_icon(section: Section) -> Svg {
    icons::render(match section {
        Section::Services => Icon::Layers,
        Section::Ports => Icon::Plug,
        Section::Development => Icon::Package,
        Section::Databases => Icon::Database,
        Section::HttpsProxy => Icon::ShieldCheck,
        Section::Diagnostics => Icon::Activity,
        Section::Settings => Icon::Settings,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BadgeTone {
    Running,
    Failed,
    Busy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NavBadge {
    tone: BadgeTone,
    label: Option<String>,
}

fn is_internal_devbase_service(name: &str, id: &str) -> bool {
    name.starts_with("xyz.jyeu.devbase.dev.") || id.contains("xyz.jyeu.devbase.dev.")
}

fn services_badge() -> Option<NavBadge> {
    let mut running = 0usize;
    let mut failed = 0usize;
    for source in Source::platform_all().iter().copied() {
        for item in services::loaded_items(source) {
            if is_internal_devbase_service(&item.name, &item.id) {
                continue;
            }
            if item.state == ServiceState::Failed {
                failed += 1;
            } else if item.up() {
                running += 1;
            }
        }
    }
    if failed > 0 {
        Some(NavBadge {
            tone: BadgeTone::Failed,
            label: Some(running.to_string()),
        })
    } else if running > 0 {
        Some(NavBadge {
            tone: BadgeTone::Running,
            label: Some(running.to_string()),
        })
    } else {
        None
    }
}

fn databases_badge() -> Option<NavBadge> {
    let instances = databases::instances();
    if instances.is_empty() {
        return None;
    }
    let mut running = 0usize;
    let mut failed = 0usize;
    for instance in &instances {
        match databases::status(&instance.id).as_str() {
            "Running" | "Healthy" => running += 1,
            "Failed" => failed += 1,
            _ => {}
        }
    }
    if failed > 0 {
        Some(NavBadge {
            tone: BadgeTone::Failed,
            label: Some(format!("{running}/{}", instances.len())),
        })
    } else if running > 0 {
        Some(NavBadge {
            tone: BadgeTone::Running,
            label: Some(format!("{running}/{}", instances.len())),
        })
    } else {
        None
    }
}

fn proxy_badge() -> Option<NavBadge> {
    let proxy_status = proxy::proxy_status();
    let tunnel_status = proxy::tunnel_status();
    let rules = proxy::saved_rules().len();
    if proxy_status == "Failed" || tunnel_status == "Failed" {
        Some(NavBadge {
            tone: BadgeTone::Failed,
            label: (rules > 0).then(|| rules.to_string()),
        })
    } else if proxy_status == "Running" || tunnel_status == "Running" {
        Some(NavBadge {
            tone: BadgeTone::Running,
            label: (rules > 0).then(|| rules.to_string()),
        })
    } else if matches!(
        proxy_status.as_str(),
        "Starting" | "Stopping" | "Restarting"
    ) || matches!(
        tunnel_status.as_str(),
        "Starting" | "Stopping" | "Restarting"
    ) {
        Some(NavBadge {
            tone: BadgeTone::Busy,
            label: None,
        })
    } else {
        None
    }
}

fn ports_badge() -> Option<NavBadge> {
    let active = ports::all_ports().len();
    if active > 0 {
        Some(NavBadge {
            tone: BadgeTone::Running,
            label: Some(active.to_string()),
        })
    } else {
        None
    }
}

fn section_badge(section: Section) -> Option<NavBadge> {
    match section {
        Section::Services => services_badge(),
        Section::Ports => ports_badge(),
        Section::Databases => databases_badge(),
        Section::HttpsProxy => proxy_badge(),
        Section::Development | Section::Diagnostics | Section::Settings => None,
    }
}

fn section_badge_test_id(section: Section) -> &'static str {
    match section {
        Section::Services => "sidebar-badge-services",
        Section::Ports => "sidebar-badge-ports",
        Section::Development => "sidebar-badge-development",
        Section::Databases => "sidebar-badge-databases",
        Section::HttpsProxy => "sidebar-badge-https-proxy",
        Section::Diagnostics => "sidebar-badge-diagnostics",
        Section::Settings => "sidebar-badge-settings",
    }
}

fn badge_dot_color(tone: BadgeTone) -> AdaptiveColor {
    match tone {
        BadgeTone::Running => tokens::success(),
        BadgeTone::Failed => theme().destructive,
        BadgeTone::Busy => tokens::warning(),
    }
}

fn nav_badge(section: Section) -> impl IntoElement {
    let test_id = section_badge_test_id(section);
    when(
        move || section_badge(section).is_some(),
        move || {
            view()
                .test_id(test_id)
                .flex_none()
                .flex_row()
                .items_center()
                .gap(px(5.0))
                .child(view().size(px(6.0)).flex_none().rounded_full().bg(move || {
                    section_badge(section)
                        .map(|b| badge_dot_color(b.tone))
                        .unwrap_or(AdaptiveColor::TRANSPARENT)
                }))
                .child(when(
                    move || section_badge(section).and_then(|b| b.label).is_some(),
                    move || {
                        text(move || {
                            section_badge(section)
                                .and_then(|b| b.label)
                                .unwrap_or_default()
                        })
                        .text_size(px(tokens::TEXT_CAPTION))
                        .tabular_nums()
                        .text_color(move || theme().muted_foreground)
                    },
                ))
        },
    )
}

pub(crate) fn sidebar() -> impl IntoElement {
    view()
        .test_id("sidebar")
        .relative()
        .w(px(204.0))
        .flex_none()
        .flex_col()
        .bg(move || {
            if shell::sidebar_glass_active() {
                AdaptiveColor::TRANSPARENT
            } else {
                theme().sidebar
            }
        })
        .child(when(shell::sidebar_glass_active, || {
            view()
                .test_id("sidebar-glass")
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .size_full()
                .pointer_events(PointerEvents::None)
                .glass_effect(GlassEffect::Regular)
                .glass_tint_color(move || Some(tokens::sidebar_glass_tint()))
        }))
        .child(title_bar_strip())
        .child(
            view()
                .flex_1()
                .flex_col()
                .px(tokens::space(2.0))
                .pt(move || {
                    if title_bar().top.0 > 0.0 {
                        px(2.0)
                    } else {
                        tokens::space(3.0)
                    }
                })
                .pb(tokens::space(2.5))
                .child(identity())
                .child(nav_sections())
                .child(view().flex_1().window_drag(true))
                .child(
                    view()
                        .w_full()
                        .flex_none()
                        .pt(tokens::space(2.0))
                        .border_t_1()
                        .border_color(tokens::hairline)
                        .child(logs_toggle()),
                ),
        )
}

/// Reserves the hidden title bar's height for macOS traffic lights and lets
/// the user drag the window from the top of the rail.
fn title_bar_strip() -> impl IntoElement {
    view()
        .test_id("sidebar-title-bar")
        .w_full()
        .flex_none()
        .h(move || title_bar().top)
        .window_drag(true)
}

/// App brand mark + wordmark on the left paired with a quiet `⌘K` Command
/// Palette trigger button on the right.
fn identity() -> impl IntoElement {
    view()
        .w_full()
        .flex_none()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(tokens::space(2.0))
        .px(tokens::space(2.0))
        .pb(tokens::space(3.0))
        .window_drag(true)
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .flex_row()
                .items_center()
                .gap(tokens::space(2.0))
                .child(
                    view()
                        .size(px(20.0))
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.0))
                        .bg(move || theme().sidebar_primary.opacity(0.14))
                        .border_1()
                        .border_color(move || theme().sidebar_primary.opacity(0.24))
                        .child(
                            svg(ImageSrc::svg_markup(DEVBASE_GLYPH_SVG))
                                .size(px(12.0))
                                .text_color(move || theme().sidebar_primary),
                        ),
                )
                .child(
                    text(meta::NAME)
                        .flex_1()
                        .min_w(px(0.0))
                        .text_size(px(14.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || theme().foreground)
                        .truncate(),
                ),
        )
        .child(
            view()
                .flex_none()
                .tooltip(controls::tooltip(format!(
                    "Command palette ({})",
                    crate::platform::command_palette_shortcut_label()
                )))
                .child(
                    button()
                        .test_id("sidebar-search")
                        .accessibility_label("Command palette")
                        .h(px(24.0))
                        .px(px(6.0))
                        .flex_none()
                        .flex_row()
                        .items_center()
                        .justify_center()
                        .gap(px(4.0))
                        .rounded(px(6.0))
                        .bg(tokens::inset)
                        .border_1()
                        .border_color(tokens::hairline)
                        .hover(|s| s.bg(tokens::row_hover()))
                        .focus_visible(|s| s.ring(px(2.0)).ring_color(move || theme().sidebar_ring))
                        .on_press(command_palette::open)
                        .child(
                            icons::search()
                                .size(px(11.0))
                                .flex_none()
                                .text_color(move || theme().muted_foreground),
                        )
                        .child(
                            text(crate::platform::command_palette_shortcut_label())
                                .text_size(px(10.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(move || theme().muted_foreground),
                        ),
                ),
        )
}

/// Primary Workspace sections and secondary System sections, separated by a
/// quiet 12pt spacer and subtle 1px hairline divider.
fn nav_sections() -> impl IntoElement {
    let mut stack = view().flex_col();
    for (index, group) in SectionGroup::ALL.iter().copied().enumerate() {
        if index > 0 {
            stack = stack.child(
                view()
                    .test_id("sidebar-group-divider")
                    .w_full()
                    .py(tokens::space(2.0))
                    .px(tokens::space(2.0))
                    .child(view().h(px(1.0)).w_full().bg(tokens::hairline)),
            );
        }
        stack = stack.child(nav_group(group));
    }
    stack
}

/// One semantic group of navigation items.
fn nav_group(group: SectionGroup) -> impl IntoElement {
    view()
        .test_id(group.test_id())
        .flex_col()
        .gap(px(2.0))
        .child(group.sections().iter().map(|section| nav_item(*section)))
}

/// One section: a full-width row with an inline icon, selected while it is
/// the workspace's, plus right-aligned live status indicators where applicable.
fn nav_item(section: Section) -> impl IntoElement {
    button()
        .accessibility_label(section.label())
        .w_full()
        .h(px(30.0))
        .flex_row()
        .items_center()
        .justify_start()
        .gap(tokens::space(2.0))
        .px(tokens::space(2.5))
        .rounded(move || theme().radius)
        .selected(move || store::active_section() == section)
        .bg(move || {
            if store::active_section() == section {
                theme().sidebar_accent
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .hover(|s| s.bg(tokens::row_hover()))
        .active(|s| s.bg(move || theme().sidebar_accent))
        .focus_visible(|s| s.ring(px(2.0)).ring_color(move || theme().sidebar_ring))
        .on_press(move || store::select_section(section))
        .child(
            section_icon(section)
                .size(px(15.0))
                .flex_none()
                .text_color(move || {
                    if store::active_section() == section {
                        theme().sidebar_primary
                    } else {
                        theme().muted_foreground
                    }
                }),
        )
        .child(
            text(section.label())
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(tokens::TEXT_BODY))
                .style_when(
                    move || store::active_section() == section,
                    |s| s.font_weight(FontWeight::MEDIUM),
                )
                .text_color(move || {
                    if store::active_section() == section {
                        theme().sidebar_accent_foreground
                    } else {
                        theme().sidebar_foreground
                    }
                })
                .truncate(),
        )
        .child(nav_badge(section))
}

/// Toggles the bottom workspace live log drawer.
fn logs_toggle() -> impl IntoElement {
    let terminal_icon = icons::terminal();
    button()
        .test_id("sidebar-logs-toggle")
        .accessibility_label("Logs")
        .w_full()
        .h(px(30.0))
        .flex_row()
        .items_center()
        .justify_start()
        .gap(tokens::space(2.0))
        .px(tokens::space(2.5))
        .rounded(move || theme().radius)
        .selected(crate::dev_logs::is_open)
        .bg(move || {
            if crate::dev_logs::is_open() {
                theme().sidebar_accent
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .hover(|s| s.bg(tokens::row_hover()))
        .active(|s| s.bg(move || theme().sidebar_accent))
        .focus_visible(|s| s.ring(px(2.0)).ring_color(move || theme().sidebar_ring))
        .on_press(crate::dev_logs::toggle)
        .child(
            terminal_icon
                .size(px(15.0))
                .flex_none()
                .text_color(move || {
                    if crate::dev_logs::is_open() {
                        theme().sidebar_primary
                    } else {
                        theme().muted_foreground
                    }
                }),
        )
        .child(
            text("Logs")
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(tokens::TEXT_BODY))
                .style_when(crate::dev_logs::is_open, |s| {
                    s.font_weight(FontWeight::MEDIUM)
                })
                .text_color(move || {
                    if crate::dev_logs::is_open() {
                        theme().sidebar_accent_foreground
                    } else {
                        theme().sidebar_foreground
                    }
                })
                .truncate(),
        )
        .child(when(crate::dev_logs::is_open, || {
            view()
                .test_id("sidebar-logs-active-dot")
                .size(px(6.0))
                .flex_none()
                .rounded_full()
                .bg(move || theme().sidebar_primary)
        }))
}

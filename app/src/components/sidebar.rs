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
    meta, shell,
    store::{self, Section, databases, proxy, services},
    theme as tokens,
};

const ICON_OPEN: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round">"#;
const ICON_CLOSE: &str = "</svg>";

fn section_icon(section: Section) -> Svg {
    let body = match section {
        // Lucide layers (Go's iconAll)
        Section::Services => {
            r#"<path d="M12.83 2.18a2 2 0 0 0-1.66 0L2.6 6.08a1 1 0 0 0 0 1.83l8.58 3.91a2 2 0 0 0 1.66 0l8.58-3.9a1 1 0 0 0 0-1.83z"/><path d="M2 12a1 1 0 0 0 .58.91l8.6 3.91a2 2 0 0 0 1.65 0l8.58-3.9A1 1 0 0 0 22 12"/><path d="M2 17a1 1 0 0 0 .58.91l8.6 3.91a2 2 0 0 0 1.65 0l8.58-3.9A1 1 0 0 0 22 17"/>"#
        }
        // Lucide package (Go's iconPackage)
        Section::Development => {
            r#"<path d="M11 21.73a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73z"/><path d="M12 22V12"/><polyline points="3.29 7 12 12 20.71 7"/><path d="m7.5 4.27 9 5.15"/>"#
        }
        // Lucide database (Go's iconDatabase)
        Section::Databases => {
            r#"<ellipse cx="12" cy="5" rx="9" ry="3"/><path d="M3 5V19A9 3 0 0 0 21 19V5"/><path d="M3 12A9 3 0 0 0 21 12"/>"#
        }
        // Lucide shield-check
        Section::HttpsProxy => {
            r#"<path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"/><path d="m9 12 2 2 4-4"/>"#
        }
        // Lucide activity
        Section::Diagnostics => {
            r#"<path d="M22 12h-2.48a2 2 0 0 0-1.93 1.46l-2.35 8.36a.25.25 0 0 1-.48 0L9.24 2.18a.25.25 0 0 0-.48 0l-2.35 8.36A2 2 0 0 1 4.49 12H2"/>"#
        }
        // Lucide settings
        Section::Settings => {
            r#"<path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/><circle cx="12" cy="12" r="3"/>"#
        }
    };
    svg(ImageSrc::svg_markup(&format!(
        "{ICON_OPEN}{body}{ICON_CLOSE}"
    )))
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
    for source in [
        Source::Brew,
        Source::UserAgent,
        Source::LoginItem,
        Source::SystemAgent,
        Source::Daemon,
    ] {
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

fn section_badge(section: Section) -> Option<NavBadge> {
    match section {
        Section::Services => services_badge(),
        Section::Databases => databases_badge(),
        Section::HttpsProxy => proxy_badge(),
        Section::Development | Section::Diagnostics | Section::Settings => None,
    }
}

fn section_badge_test_id(section: Section) -> &'static str {
    match section {
        Section::Services => "sidebar-badge-services",
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
                .child(
                    view()
                        .w(px(6.0))
                        .h(px(6.0))
                        .flex_none()
                        .rounded_full()
                        .bg(move || {
                            section_badge(section)
                                .map(|b| badge_dot_color(b.tone))
                                .unwrap_or(AdaptiveColor::TRANSPARENT)
                        }),
                )
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
        .w_52()
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
                        px(4.0)
                    } else {
                        tokens::space(3.0)
                    }
                })
                .pb(tokens::space(3.0))
                .child(identity())
                .child(
                    view()
                        .flex_col()
                        .gap(tokens::space(0.5))
                        .child(Section::ALL.iter().map(|section| nav_item(*section))),
                )
                .child(view().flex_1().window_drag(true))
                .child(logs_toggle())
                .child(footer()),
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

/// The app's name and what it is, above the sections.
fn identity() -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(0.5))
        .px(tokens::space(3.0))
        .pb(tokens::space(4.0))
        .window_drag(true)
        .child(
            text(meta::NAME)
                .text_base()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().foreground),
        )
        .child(
            text("macOS developer services")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground),
        )
}

/// One section: a full-width row with an inline icon, selected while it is
/// the workspace's, plus right-aligned live status indicators where applicable.
fn nav_item(section: Section) -> impl IntoElement {
    button()
        .w_full()
        .flex_row()
        .items_center()
        .justify_start()
        .gap(tokens::space(2.5))
        .px(tokens::space(2.5))
        .py(tokens::space(1.5))
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
                .text_sm()
                .style_when(
                    move || store::active_section() == section,
                    |s| s.font_weight(FontWeight::MEDIUM),
                )
                .text_color(move || theme().sidebar_foreground)
                .truncate(),
        )
        .child(nav_badge(section))
}

/// Toggles the bottom workspace live log drawer.
fn logs_toggle() -> impl IntoElement {
    let terminal_icon = svg(ImageSrc::svg_markup(&format!(
        r#"{ICON_OPEN}<polyline points="4 17 10 11 4 5"/><line x1="12" x2="20" y1="19" y2="19"/>{ICON_CLOSE}"#
    )));
    button()
        .test_id("sidebar-logs-toggle")
        .w_full()
        .flex_row()
        .items_center()
        .justify_start()
        .gap(tokens::space(2.5))
        .px(tokens::space(2.5))
        .py(tokens::space(1.5))
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
                .text_sm()
                .style_when(crate::dev_logs::is_open, |s| {
                    s.font_weight(FontWeight::MEDIUM)
                })
                .text_color(move || theme().sidebar_foreground)
                .truncate(),
        )
        .child(
            view()
                .flex_none()
                .flex_row()
                .items_center()
                .gap(px(5.0))
                .child(when(crate::dev_logs::is_open, || {
                    view()
                        .test_id("sidebar-logs-active-dot")
                        .w(px(6.0))
                        .h(px(6.0))
                        .flex_none()
                        .rounded_full()
                        .bg(move || theme().sidebar_primary)
                }))
                .child(
                    text("⌘L")
                        .text_size(px(10.0))
                        .text_color(move || theme().muted_foreground),
                ),
        )
}

/// The build's identity, at the bottom of the rail.
fn footer() -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(0.5))
        .px(tokens::space(3.0))
        .pt(tokens::space(4.0))
        .child(
            text(format!("Version {}", meta::VERSION))
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground),
        )
        .child(
            text(meta::IDENTIFIER)
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground),
        )
        .child(
            text(format!("macOS {}+", meta::MINIMUM_MACOS))
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground),
        )
}

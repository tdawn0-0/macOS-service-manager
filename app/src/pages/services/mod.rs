//! System-service inspector: the launchd sources, their jobs, and the
//! inspector of the job chosen.
//!
//! The page reads [`crate::store::services`] and never calls the provider
//! itself: a list, a detail or an action is a task that writes the store when
//! it answers.

mod inspector;
mod list;

use std::time::Duration;

use devcore::service::{Item, Source};
use nsh_base::{ResizeHandleContext, ResizeHandleState, h_resizable, resizable_panel, theme};
use nsh_core::prelude::*;

use crate::{
    components::icons,
    store::services::{self as svc, SOURCES, ServiceScope},
    theme as tokens,
};

const INSPECTOR_MIN_WIDTH: f32 = 272.0;
const INSPECTOR_MAX_WIDTH: f32 = 480.0;
const INSPECTOR_DEFAULT_WIDTH: f32 = 304.0;
const STORE_DEFAULT_INSPECTOR_WIDTH: f32 = 336.0;
const LIST_MIN_WIDTH: f32 = 320.0;

pub(crate) fn services() -> impl IntoElement {
    // Balance default split widths so long reverse-DNS labels remain legible
    // while the Inspector is open, without overriding a user's manual resize.
    if (untracked(svc::inspector_width) - STORE_DEFAULT_INSPECTOR_WIDTH).abs() < 0.5 {
        svc::set_inspector_width(INSPECTOR_DEFAULT_WIDTH);
    }
    // The page is made once; reading the source happens in a task, not in a
    // binding or in render.
    svc::load_active();
    // The log tab follows its file while it is open.
    set_interval(Duration::from_secs(1), svc::tick_log);

    view()
        .flex_1()
        .flex_row()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(source_rail())
        .child(
            h_resizable()
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .handle_accessibility_label("Resize inspector")
                .render_handle(|ctx: &ResizeHandleContext| {
                    let active = ctx.is_active();
                    let hovered = ctx.state == ResizeHandleState::Hovered;
                    view()
                        .w(if active || hovered { px(2.0) } else { px(1.0) })
                        .h_full()
                        .bg(move || {
                            if active {
                                theme().primary
                            } else if hovered {
                                theme().ring
                            } else {
                                AdaptiveColor::TRANSPARENT
                            }
                        })
                })
                .child(
                    resizable_panel()
                        .min_size(px(LIST_MIN_WIDTH))
                        .flex_col()
                        .min_h(px(0.0))
                        .child(list::list_pane()),
                )
                .child(
                    resizable_panel()
                        .size(move || Some(px(svc::inspector_width())))
                        .size_range(px(INSPECTOR_MIN_WIDTH)..px(INSPECTOR_MAX_WIDTH))
                        .fixed(true)
                        .visible(move || svc::selected_id().is_some())
                        .on_resize(move |size: &Points| svc::set_inspector_width(size.0))
                        .flex_col()
                        .min_h(px(0.0))
                        .child(when(
                            move || svc::selected_id().is_some(),
                            inspector::inspector,
                        )),
                ),
        )
}

/// Whether `item` points to an editable launchd property list.
pub(super) fn is_editable_plist(item: &Item) -> bool {
    matches!(
        item.source,
        Source::UserAgent | Source::SystemAgent | Source::Daemon
    ) && !item.path.as_os_str().is_empty()
        && item
            .path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("plist"))
}

/// The launchd sources, with how many of their jobs there are and how
/// many run.
fn source_rail() -> impl IntoElement {
    view()
        .w(px(164.0))
        .flex_none()
        .flex_col()
        .bg(move || theme().background)
        .border_r_1()
        .border_color(tokens::hairline)
        .px(tokens::space(2.0))
        .pb(tokens::space(3.0))
        .child(
            view()
                .h(px(tokens::TITLE_BAR_HEIGHT))
                .flex_none()
                .flex_row()
                .items_center()
                .px(tokens::space(2.5))
                .window_drag(true)
                .child(
                    text("Services")
                        .text_size(px(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || theme().foreground),
                ),
        )
        .child(
            view()
                .flex_col()
                .gap(tokens::space(0.5))
                .child(all_items_row())
                .child(
                    view()
                        .h(px(1.0))
                        .my(tokens::space(1.0))
                        .mx(tokens::space(1.5))
                        .bg(tokens::hairline),
                )
                .child(SOURCES.iter().map(|source| source_row(*source))),
        )
}

/// A subtle tooltip bubble for icon-only actions in the Services screen.
pub(super) fn hint_tooltip(label: impl Into<String>) -> nsh_base::TooltipElement {
    nsh_base::tooltip()
        .px(tokens::space(2.0))
        .py(tokens::space(1.0))
        .rounded(move || theme().radius)
        .bg(tokens::panel)
        .border_1()
        .border_color(tokens::hairline)
        .child(
            text(label.into())
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().foreground),
        )
}

fn format_counts((total, running): (usize, usize)) -> String {
    if total == 0 {
        String::new()
    } else if running > 0 {
        format!("{running}/{total}")
    } else {
        total.to_string()
    }
}

fn all_items_row() -> impl IntoElement {
    nsh_base::button()
        .test_id("source-all")
        .w_full()
        .flex_row()
        .items_center()
        .justify_start()
        .gap(tokens::space(2.0))
        .px(tokens::space(2.5))
        .py(tokens::space(1.5))
        .rounded(move || theme().radius)
        .bg(move || {
            if svc::active_scope() == ServiceScope::All {
                tokens::row_selected()
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .hover(move |style| style.bg(tokens::row_hover()))
        .focus_visible(|s| s.ring(px(2.0)).ring_color(move || theme().ring))
        .on_press(move || svc::select_scope(ServiceScope::All))
        .child(
            icons::layers()
                .size(px(14.0))
                .flex_none()
                .text_color(move || {
                    if svc::active_scope() == ServiceScope::All {
                        theme().foreground
                    } else {
                        theme().muted_foreground
                    }
                }),
        )
        .child(
            text(ServiceScope::All.title())
                .flex_1()
                .min_w(px(0.0))
                .text_sm()
                .text_color(move || theme().sidebar_foreground)
                .truncate(),
        )
        .child(
            text(move || format_counts(svc::total_counts()))
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground)
                .tabular_nums()
                .flex_none(),
        )
}

fn source_row(source: Source) -> impl IntoElement {
    nsh_base::button()
        .test_id(match source {
            Source::Brew => "source-brew",
            Source::LoginItem => "source-login-item",
            Source::UserAgent => "source-user-agent",
            Source::SystemAgent => "source-system-agent",
            Source::Daemon => "source-daemon",
            Source::SystemdUser => "source-systemd-user",
            Source::SystemdSystem => "source-systemd-system",
            Source::Autostart => "source-autostart",
        })
        .w_full()
        .flex_row()
        .items_center()
        .justify_start()
        .gap(tokens::space(2.0))
        .px(tokens::space(2.5))
        .py(tokens::space(1.5))
        .rounded(move || theme().radius)
        .bg(move || {
            if svc::active_scope() == ServiceScope::Source(source) {
                tokens::row_selected()
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .hover(move |style| style.bg(tokens::row_hover()))
        .focus_visible(|s| s.ring(px(2.0)).ring_color(move || theme().ring))
        .on_press(move || svc::select_source(source))
        .child(
            icons::source(source)
                .size(px(14.0))
                .flex_none()
                .text_color(move || {
                    if svc::active_scope() == ServiceScope::Source(source) {
                        theme().foreground
                    } else {
                        theme().muted_foreground
                    }
                }),
        )
        .child(
            text(source.title())
                .flex_1()
                .min_w(px(0.0))
                .text_sm()
                .text_color(move || theme().sidebar_foreground)
                .truncate(),
        )
        .child(
            text(move || format_counts(svc::counts(source)))
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground)
                .tabular_nums()
                .flex_none(),
        )
}

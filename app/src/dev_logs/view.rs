//! Native `nsh` view for the collapsible bottom workspace live log drawer,
//! ported from `view_dev_logs.go`.
//!
//! Reactivity invariants:
//! - The log viewport mounts a single stable `scroll()` container with
//!   `each(...)` for log lines (never recreating `scroll()` inside a coarse
//!   `dynamic(...)` on every 1-second poll), so scroll position and text
//!   selection survive live updates.
//! - The 1-second `set_interval` poller is mounted only while the drawer is
//!   open and not paused; closing or pausing cancels the timer automatically.

use std::time::Duration;

use nsh_base::{input, switch, tab, tabs, theme, tooltip};
use nsh_core::{element::Svg, keymap::When, prelude::*};

use crate::{
    components::icons,
    dev_logs::store::{self, EmptyState, Stream},
    theme as tokens,
};

/// Minimum height of the expanded workspace log drawer in points so the
/// toolbar, metadata row, and viewport remain usable when mounted standalone
/// or compressed inside a vertical resizable panel.
const DRAWER_MIN_HEIGHT: f32 = 100.0;

/// Collapsible bottom workspace live log drawer. Renders nothing when closed.
pub fn drawer() -> impl IntoElement {
    when(store::is_open, drawer_panel)
}

fn drawer_panel() -> impl IntoElement {
    let scroll_ref = element_ref();
    let filter_ref = element_ref();
    effect(move || {
        let rev = store::lines_revision();
        if rev > 0 && !store::is_paused() {
            scroll_ref.scroll_to_end(false);
        }
    });

    on_key("mod+f", move |_| {
        if !crate::editor::is_open() {
            filter_ref.focus();
        }
    });

    view()
        .test_id("dev-log-drawer")
        .w_full()
        .flex_1()
        .min_h(px(DRAWER_MIN_HEIGHT))
        .flex_col()
        .bg(tokens::sheet)
        .border_t_1()
        .border_color(tokens::hairline)
        // Run the 1-second poller only while the drawer is open AND not paused.
        .child(when(move || !store::is_paused(), live_poller))
        // Escape clears the active log filter first, then closes the drawer,
        // without intercepting Escape when the modal plist editor is open.
        .child(when(move || !crate::editor::is_open(), escape_shortcut))
        .child(toolbar(filter_ref))
        .child(metadata_row())
        .child(log_viewport(scroll_ref))
}

fn escape_shortcut() -> impl IntoElement {
    on_key_with(
        "escape",
        KeyOptions {
            when: When::Always,
            ..KeyOptions::default()
        },
        |_| {
            if !store::query().is_empty() {
                store::set_query(String::new());
            } else {
                store::close();
            }
        },
    );
    view().w(px(0.0)).h(px(0.0)).flex_none()
}

fn live_poller() -> impl IntoElement {
    set_interval(Duration::from_secs(1), store::tick);
    view().w(px(0.0)).h(px(0.0)).flex_none()
}

fn toolbar(filter_ref: ElementRef) -> impl IntoElement {
    view()
        .flex_none()
        .flex_row()
        .items_center()
        .gap(tokens::space(2.0))
        .px(tokens::space(4.0))
        .pt(tokens::space(2.5))
        .pb(tokens::space(1.5))
        .child(target_tabs())
        .child(stream_tabs())
        .child(filter_input(filter_ref))
        .child(live_switch())
        .child(action_buttons())
}

fn target_tabs() -> impl IntoElement {
    scroll().horizontal().min_w(px(0.0)).scrollbar(false).child(
        tabs()
            .test_id("dev-log-targets")
            .accessibility_label("Log target")
            .flex_row()
            .items_center()
            .gap(px(2.0))
            .p(px(2.0))
            .rounded(move || theme().radius)
            .bg(tokens::panel)
            .border_1()
            .border_color(tokens::hairline)
            .child(each(
                store::targets,
                |target| target.job_id.clone(),
                |target, _| {
                    let initial = target.get_untracked();
                    let test_id = format!("dev-log-target-{}", initial.job_id);
                    let job_id_selected = initial.job_id.clone();
                    let job_id_press = initial.job_id.clone();
                    let job_id_bg = initial.job_id.clone();
                    let job_id_fg = initial.job_id.clone();
                    let job_id_tip = initial.job_id.clone();
                    view().flex_none().tooltip(tip_owned(job_id_tip)).child(
                        tab()
                            .test_id(&test_id)
                            .selected(move || store::active_job_id() == job_id_selected)
                            .h(px(22.0))
                            .px(tokens::space(2.0))
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .rounded(move || theme().radius)
                            .bg(move || {
                                if store::active_job_id() == job_id_bg {
                                    tokens::row_selected()
                                } else {
                                    AdaptiveColor::TRANSPARENT
                                }
                            })
                            .hover(move |style| style.bg(tokens::row_hover()))
                            .on_press(move || store::select_target(&job_id_press))
                            .child(
                                text(move || target.with(|item| item.title.clone()))
                                    .text_size(px(tokens::TEXT_CAPTION))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(move || {
                                        if store::active_job_id() == job_id_fg {
                                            theme().foreground
                                        } else {
                                            theme().muted_foreground
                                        }
                                    }),
                            ),
                    )
                },
            )),
    )
}

fn stream_tabs() -> impl IntoElement {
    tabs()
        .test_id("dev-log-streams")
        .accessibility_label("Log stream")
        .flex_none()
        .flex_row()
        .items_center()
        .gap(px(2.0))
        .p(px(2.0))
        .rounded(move || theme().radius)
        .bg(tokens::panel)
        .border_1()
        .border_color(tokens::hairline)
        .child(Stream::ALL.into_iter().map(|stream| {
            let id = match stream {
                Stream::Stderr => "dev-log-stream-stderr",
                Stream::Stdout => "dev-log-stream-stdout",
            };
            tab()
                .test_id(id)
                .selected(move || store::active_stream() == stream)
                .h(px(22.0))
                .px(tokens::space(2.0))
                .flex_none()
                .items_center()
                .justify_center()
                .rounded(move || theme().radius)
                .bg(move || {
                    if store::active_stream() == stream {
                        tokens::row_selected()
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .hover(move |style| style.bg(tokens::row_hover()))
                .on_press(move || store::set_stream(stream))
                .child(
                    text(stream.label())
                        .text_size(px(tokens::TEXT_CAPTION))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(move || {
                            if store::active_stream() == stream {
                                theme().foreground
                            } else {
                                theme().muted_foreground
                            }
                        }),
                )
        }))
}

fn filter_input(filter_ref: ElementRef) -> impl IntoElement {
    view()
        .flex_1()
        .min_w(px(120.0))
        .h(px(26.0))
        .flex_row()
        .items_center()
        .gap(tokens::space(1.5))
        .px(tokens::space(2.0))
        .rounded(move || theme().radius)
        .bg(tokens::control)
        .border_1()
        .border_color(tokens::hairline)
        .focus_within(|s| s.border_color(move || theme().ring))
        .child(
            icons::search()
                .size(px(12.0))
                .flex_none()
                .text_color(move || theme().muted_foreground),
        )
        .child(
            input()
                .test_id("dev-log-filter")
                .element_ref(&filter_ref)
                .flex_1()
                .min_w(px(0.0))
                .h(px(24.0))
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().foreground)
                .placeholder("Filter logs")
                .label("Filter logs")
                .value(store::query)
                .on_change(move |value: &str| store::set_query(value.to_owned())),
        )
        .child(when(
            move || !store::query().is_empty(),
            || {
                view().flex_none().tooltip(tip("Clear filter")).child(
                    nsh_base::button()
                        .test_id("dev-log-filter-clear")
                        .size(px(18.0))
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .rounded(move || theme().radius)
                        .hover(move |style| style.bg(tokens::row_hover()))
                        .focus_visible(|s| s.ring(px(2.0)).ring_color(move || theme().ring))
                        .accessibility_label("Clear filter")
                        .on_press(|| store::set_query(String::new()))
                        .child(
                            icons::close()
                                .size(px(11.0))
                                .text_color(move || theme().muted_foreground),
                        ),
                )
            },
        ))
}

fn live_switch() -> impl IntoElement {
    view()
        .flex_none()
        .tooltip(tip("Toggle live log tailing"))
        .child(
            switch()
                .test_id("dev-log-follow-switch")
                .checked(move || !store::is_paused())
                .accessibility_label("Live follow")
                .flex_none()
                .flex_row()
                .items_center()
                .gap(tokens::space(1.5))
                .rounded(move || theme().radius)
                .focus_visible(|s| s.ring(px(2.0)).ring_color(move || theme().ring))
                .track_style(|style| {
                    style
                        .w(px(28.0))
                        .h(px(16.0))
                        .rounded_full()
                        .p(px(2.0))
                        .flex_row()
                        .items_center()
                        .bg(move || {
                            if !store::is_paused() {
                                tokens::success()
                            } else {
                                tokens::inset()
                            }
                        })
                        .border_1()
                        .border_color(move || {
                            if !store::is_paused() {
                                tokens::success()
                            } else {
                                tokens::hairline()
                            }
                        })
                })
                .thumb_style(|style| {
                    style
                        .size(px(10.0))
                        .rounded_full()
                        .bg(move || theme().background)
                        .ml(move || {
                            if !store::is_paused() {
                                px(12.0)
                            } else {
                                px(0.0)
                            }
                        })
                })
                .on_change(move |live: &bool| store::set_paused(!*live))
                .child(
                    text(move || if store::is_paused() { "Paused" } else { "Live" })
                        .text_size(px(tokens::TEXT_CAPTION))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(move || {
                            if store::is_paused() {
                                theme().muted_foreground
                            } else {
                                theme().foreground
                            }
                        }),
                ),
        )
}

fn action_buttons() -> impl IntoElement {
    view()
        .flex_none()
        .flex_row()
        .items_center()
        .gap(px(2.0))
        .child(drawer_icon_button(
            "dev-log-refresh",
            icons::refresh(),
            "Refresh logs",
            || false,
            store::refresh,
        ))
        .child(drawer_icon_button(
            "dev-log-copy",
            icons::copy(),
            "Copy logs",
            || !store::can_copy(),
            store::copy_logs,
        ))
        .child(drawer_icon_button(
            "dev-log-open-console",
            icons::external(),
            "Open in Console",
            || store::active_path().is_none(),
            store::open_in_console,
        ))
        .child(drawer_icon_button(
            "dev-log-close",
            icons::close(),
            "Close logs",
            || false,
            store::close,
        ))
}

fn drawer_icon_button(
    test_id: &'static str,
    icon: Svg,
    label: &'static str,
    disabled: impl Fn() -> bool + Copy + 'static,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    view().flex_none().tooltip(tip(label)).child(
        nsh_base::button()
            .test_id(test_id)
            .size(px(26.0))
            .flex_none()
            .items_center()
            .justify_center()
            .rounded(move || theme().radius)
            .hover(move |style| style.bg(tokens::row_hover()))
            .focus_visible(|s| s.ring(px(2.0)).ring_color(move || theme().ring))
            .disabled(disabled)
            .opacity(move || if disabled() { 0.4_f32 } else { 1.0_f32 })
            .accessibility_label(label)
            .on_press(on_press)
            .child(
                icon.size(px(14.0))
                    .text_color(move || theme().muted_foreground),
            ),
    )
}

fn metadata_row() -> impl IntoElement {
    view()
        .flex_none()
        .flex_row()
        .items_center()
        .gap(tokens::space(2.0))
        .px(tokens::space(4.0))
        .pb(tokens::space(1.5))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .tooltip(
                    tooltip()
                        .disabled(move || store::active_path().is_none())
                        .px(tokens::space(2.0))
                        .py(tokens::space(1.0))
                        .rounded(move || theme().radius)
                        .bg(tokens::panel)
                        .border_1()
                        .border_color(tokens::hairline)
                        .child(
                            text(move || {
                                store::active_path()
                                    .map(|path| path.display().to_string())
                                    .unwrap_or_default()
                            })
                            .text_size(px(tokens::TEXT_CAPTION))
                            .text_color(move || theme().foreground),
                        ),
                )
                .child(
                    text(move || store::metadata_summary().unwrap_or_default())
                        .test_id("dev-log-meta")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground)
                        .truncate(),
                ),
        )
        .child(when(
            move || store::notice().is_some(),
            || {
                text(move || store::notice().unwrap_or_default())
                    .test_id("dev-log-notice")
                    .flex_none()
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || theme().foreground)
            },
        ))
}

/// Stable `scroll()` viewport: never reconstructed on 1-second polls.
/// Individual lines are diffed in place via `each(...)`.
fn log_viewport(scroll_ref: ElementRef) -> impl IntoElement {
    view()
        .flex_1()
        .min_h(px(0.0))
        .px(tokens::space(4.0))
        .pb(tokens::space(3.0))
        .child(
            scroll()
                .test_id("dev-log-scroll")
                .element_ref(&scroll_ref)
                .flex_1()
                .min_h(px(0.0))
                .selectable(true)
                .rounded(move || theme().radius)
                .bg(tokens::panel)
                .border_1()
                .border_color(tokens::hairline)
                .child(
                    view()
                        .flex_col()
                        .gap(px(2.0))
                        .px(tokens::space(2.5))
                        .py(tokens::space(2.0))
                        .child(when(
                            move || store::empty_state().is_some(),
                            empty_state_banner,
                        ))
                        .child(each(
                            store::filtered_lines,
                            |line| line.index,
                            |line, _| {
                                text(move || line.with(|entry| entry.text.clone()))
                                    .test_id("dev-log-line")
                                    .font_family("monospace")
                                    .text_size(px(tokens::TEXT_CAPTION))
                                    .text_color(move || theme().foreground)
                                    .selectable(true)
                            },
                        )),
                ),
        )
}

fn empty_state_banner() -> impl IntoElement {
    text(move || match store::empty_state() {
        Some(EmptyState::Error(message)) => message,
        Some(EmptyState::Reading) => "Reading…".to_string(),
        Some(EmptyState::Missing) => "No logs yet.".to_string(),
        Some(EmptyState::Empty) => "The log is empty.".to_string(),
        Some(EmptyState::NoMatches) => "No matching log lines.".to_string(),
        None => String::new(),
    })
    .test_id("dev-log-status")
    .text_size(px(tokens::TEXT_SMALL))
    .text_color(move || match store::empty_state() {
        Some(EmptyState::Error(_)) => theme().destructive,
        _ => theme().muted_foreground,
    })
}

fn tip(label: &'static str) -> nsh_base::TooltipElement {
    tip_owned(label.to_string())
}

fn tip_owned(label: String) -> nsh_base::TooltipElement {
    tooltip()
        .px(tokens::space(2.0))
        .py(tokens::space(1.0))
        .rounded(move || theme().radius)
        .bg(tokens::panel)
        .border_1()
        .border_color(tokens::hairline)
        .child(
            text(label)
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().foreground),
        )
}

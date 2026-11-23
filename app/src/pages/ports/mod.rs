//! Ports: active listening TCP port inventory, starring/watching with change
//! notifications, bind-first port checking, one-click HTTPS rule handoff, and
//! safe single-PID / Devbase-aware stop actions.

use devcore::databases::{ListeningPort, PortCategory, PortOwner};
use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{
    components::{
        controls::{self as ui, TagTone, Tone},
        icons,
    },
    shell,
    store::{
        ports::{self, PortFilter},
        system,
    },
    theme as tokens,
};

fn parse_valid_port(input: &str) -> Option<i64> {
    input
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|port| (1..=65535).contains(port))
}

pub(crate) fn ports() -> impl IntoElement {
    ports::load();

    view()
        .test_id("ports-page")
        .flex_1()
        .flex_col()
        .min_w(px(0.0))
        .child(header())
        .child(
            scroll().flex_1().min_h(px(0.0)).child(
                view()
                    .w_full()
                    .flex_col()
                    .items_center()
                    .px(tokens::space(6.0))
                    .py(tokens::space(5.0))
                    .child(
                        view()
                            .w_full()
                            .max_w(px(920.0))
                            .flex_col()
                            .gap(tokens::space(4.0))
                            .child(filter_and_check_bar())
                            .child(dynamic(feedback_banner))
                            .child(dynamic(inspection_banner))
                            .child(active_ports_card())
                            .child(dynamic(offline_saved_card)),
                    ),
            ),
        )
}

fn header() -> impl IntoElement {
    let search_ref = element_ref();
    let seen_find = std::cell::Cell::new(untracked(shell::find_revision));
    effect(move || {
        let rev = shell::find_revision();
        if rev != seen_find.get() {
            seen_find.set(rev);
            if crate::store::active_section() == crate::store::Section::Ports {
                search_ref.focus();
            }
        }
    });

    ui::page_header(
        "Ports",
        None::<&str>,
        view()
            .flex_none()
            .flex_row()
            .items_center()
            .gap(tokens::space(1.5))
            .child(
                view()
                    .w(px(196.0))
                    .flex_none()
                    .h(px(28.0))
                    .flex_row()
                    .items_center()
                    .gap(tokens::space(1.5))
                    .px(tokens::space(2.0))
                    .rounded(move || theme().radius)
                    .bg(tokens::inset)
                    .border_1()
                    .border_color(tokens::hairline)
                    .child(
                        icons::search()
                            .size(px(12.0))
                            .flex_none()
                            .text_color(move || theme().muted_foreground),
                    )
                    .child(
                        nsh_base::input()
                            .test_id("ports-search")
                            .element_ref(&search_ref)
                            .flex_1()
                            .min_w(px(0.0))
                            .h(px(26.0))
                            .text_size(px(tokens::TEXT_SMALL))
                            .placeholder("Port, process, PID…")
                            .value(ports::search)
                            .on_change(|next: &str| ports::set_search(next.to_string())),
                    ),
            )
            .child(auto_refresh_button())
            .child(ui::action_button(
                "Find free",
                Tone::Default,
                "ports-free-port",
                system::suggesting_port,
                system::find_free_port,
            ))
            .child(
                view().flex_none().tooltip(ui::tooltip("Refresh")).child(
                    nsh_base::button()
                        .test_id("ports-refresh")
                        .size(px(28.0))
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .rounded(move || theme().radius)
                        .hover(move |style| style.bg(tokens::row_hover()))
                        .focus_visible(|style| style.ring(px(2.0)).ring_color(move || theme().ring))
                        .disabled(system::ports_loading)
                        .opacity(move || {
                            if system::ports_loading() {
                                0.45_f32
                            } else {
                                1.0_f32
                            }
                        })
                        .accessibility_label("Refresh ports")
                        .on_press(ports::refresh)
                        .child(
                            icons::refresh()
                                .size(px(14.0))
                                .text_color(move || theme().muted_foreground),
                        ),
                ),
            ),
    )
}

fn auto_refresh_button() -> impl IntoElement {
    view()
        .flex_none()
        .tooltip(ui::tooltip(
            "Auto-refresh every 5 seconds while viewing Ports",
        ))
        .child(
            nsh_base::button()
                .test_id("ports-auto-refresh")
                .h(px(28.0))
                .flex_none()
                .flex_row()
                .items_center()
                .gap(tokens::space(1.5))
                .px(tokens::space(2.5))
                .rounded(move || theme().radius)
                .bg(move || {
                    if ports::auto_refresh_enabled() {
                        tokens::success().opacity(0.14)
                    } else {
                        tokens::control()
                    }
                })
                .border_1()
                .border_color(move || {
                    if ports::auto_refresh_enabled() {
                        tokens::success().opacity(0.42)
                    } else {
                        tokens::hairline()
                    }
                })
                .hover(move |style| style.bg(tokens::row_hover()))
                .focus_visible(|style| style.ring(px(2.0)).ring_color(move || theme().ring))
                .accessibility_label("Auto-refresh every 5 seconds")
                .on_press(ports::toggle_auto_refresh)
                .child(view().size(px(6.0)).flex_none().rounded_full().bg(move || {
                    if ports::auto_refresh_enabled() {
                        tokens::success()
                    } else {
                        theme().muted_foreground.opacity(0.4)
                    }
                }))
                .child(
                    text("5s")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .font_weight(FontWeight::MEDIUM)
                        .tabular_nums()
                        .text_color(move || {
                            if ports::auto_refresh_enabled() {
                                tokens::success()
                            } else {
                                theme().muted_foreground
                            }
                        }),
                ),
        )
}

fn filter_and_check_bar() -> impl IntoElement {
    view()
        .w_full()
        .flex_row()
        .items_center()
        .justify_between()
        .flex_wrap()
        .gap(tokens::space(2.5))
        .child(category_segmented_bar())
        .child(
            view()
                .flex_none()
                .flex_row()
                .items_center()
                .gap(tokens::space(1.5))
                .child(view().w(px(88.0)).flex_none().child(ui::text_field(
                    "ports-check-input",
                    "3000",
                    system::port_input,
                    system::set_port_input,
                )))
                .child(ui::action_button(
                    "Check port",
                    Tone::Default,
                    "ports-check-port",
                    || system::checking_port() || parse_valid_port(&system::port_input()).is_none(),
                    system::check_port,
                )),
        )
}

fn category_segmented_bar() -> impl IntoElement {
    let mut bar = view()
        .test_id("ports-filter-bar")
        .flex_none()
        .flex_row()
        .items_center()
        .gap(px(2.0))
        .p(px(2.0))
        .rounded(move || theme().radius)
        .bg(tokens::inset)
        .border_1()
        .border_color(tokens::hairline);

    for filter in PortFilter::ALL {
        bar = bar.child(filter_tab(filter));
    }
    bar
}

fn filter_tab(filter: PortFilter) -> impl IntoElement {
    let is_selected = move || ports::filter() == filter;
    nsh_base::button()
        .test_id(filter.test_id())
        .h(px(24.0))
        .flex_none()
        .flex_row()
        .items_center()
        .gap(tokens::space(1.0))
        .px(tokens::space(2.0))
        .rounded(px(5.0))
        .bg(move || {
            if is_selected() {
                tokens::panel()
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .hover(move |style| {
            if is_selected() {
                style.bg(tokens::panel())
            } else {
                style.bg(tokens::row_hover())
            }
        })
        .focus_visible(|style| style.ring(px(2.0)).ring_color(move || theme().ring))
        .on_press(move || ports::set_filter(filter))
        .child(
            text(filter.label())
                .text_size(px(tokens::TEXT_CAPTION))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || {
                    if is_selected() {
                        theme().foreground
                    } else {
                        theme().muted_foreground
                    }
                }),
        )
        .child(when(
            move || ports::count_for_filter(filter) > 0,
            move || {
                text(move || ports::count_for_filter(filter).to_string())
                    .text_size(px(10.0))
                    .tabular_nums()
                    .text_color(move || theme().muted_foreground)
            },
        ))
}

fn feedback_banner() -> Option<AnyElement> {
    let error = ports::action_error().or_else(system::port_error)?;
    Some(ui::plain_note(error, true).into_element())
}

fn inspection_banner() -> Option<AnyElement> {
    let port = parse_valid_port(&system::port_input())?;
    let inspection = system::port_inspection(port)?;
    let (self_pid, self_uid) = ports::current_identity();
    let tone = match inspection.status.as_str() {
        "Available" => TagTone::Success,
        "In use" => TagTone::Warning,
        "Devbase proxy" => TagTone::Neutral,
        _ => TagTone::Warning,
    };
    let show_detail = !inspection.detail.is_empty()
        && inspection.status != "Available"
        && inspection.status != "Devbase proxy"
        && inspection.listeners.is_empty();
    let detail = inspection.detail.clone();
    let stoppable = inspection
        .listeners
        .iter()
        .find(|listener| {
            !listener.own
                && listener.pid > 1
                && listener.pid != self_pid
                && listener.uid == self_uid
        })
        .cloned();

    Some(
        view()
            .test_id("ports-inspection-card")
            .w_full()
            .flex_col()
            .gap(tokens::space(1.5))
            .px(tokens::space(4.0))
            .py(tokens::space(3.0))
            .rounded(tokens::panel_radius)
            .bg(tokens::inset)
            .border_1()
            .border_color(tokens::hairline)
            .child(
                view()
                    .w_full()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(tokens::space(2.0))
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap(tokens::space(2.0))
                            .child(
                                text(format!("Port {}", inspection.port))
                                    .font_family("monospace")
                                    .tabular_nums()
                                    .text_size(px(tokens::TEXT_SMALL))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(move || theme().foreground),
                            )
                            .child(ui::tag(inspection.status.clone(), tone)),
                    )
                    .children(stoppable.map(|listener| {
                        let name = listener.name.clone();
                        let pid = listener.pid;
                        let check_port = inspection.port;
                        ui::action_button(
                            format!(
                                "Stop {} (PID {pid})",
                                if name.is_empty() { "process" } else { &name }
                            ),
                            Tone::Danger,
                            "ports-inspection-stop",
                            false,
                            move || {
                                ports::stop_conflict_pid(check_port, pid, name.clone(), || {
                                    system::check_port();
                                });
                            },
                        )
                    })),
            )
            .children(inspection.listeners.into_iter().map(|listener| {
                let owner = if listener.uid >= 0 {
                    format!("UID {}", listener.uid)
                } else {
                    "UID unknown".to_string()
                };
                text(format!(
                    "PID {} · {} · {} · {}",
                    listener.pid, owner, listener.name, listener.address
                ))
                .font_family("monospace")
                .tabular_nums()
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground)
                .selectable(true)
            }))
            .child(when(inspection.truncated, || {
                text("First 64 shown")
                    .tabular_nums()
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || theme().muted_foreground)
            }))
            .child(when(show_detail, move || {
                text(detail.clone())
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || theme().muted_foreground)
            }))
            .into_element(),
    )
}

fn active_ports_card() -> impl IntoElement {
    view()
        .w_full()
        .flex_col()
        .gap(tokens::space(1.5))
        .child(
            view()
                .w_full()
                .flex_row()
                .items_center()
                .justify_between()
                .px(tokens::space(1.5))
                .child(
                    text("Active listeners")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || theme().muted_foreground),
                )
                .child(
                    text(|| {
                        let total = ports::all_ports().len();
                        let shown = ports::filtered_ports().len();
                        if total == 0 {
                            "0 listening".to_string()
                        } else if shown == total {
                            format!("{total} listening")
                        } else {
                            format!("{shown} of {total} shown")
                        }
                    })
                    .text_size(px(tokens::TEXT_CAPTION))
                    .tabular_nums()
                    .text_color(move || theme().muted_foreground),
                ),
        )
        .child(
            view()
                .test_id("ports-list-card")
                .w_full()
                .flex_col()
                .rounded(tokens::panel_radius)
                .bg(tokens::panel)
                .border_1()
                .border_color(tokens::hairline)
                .overflow_hidden()
                .child(dynamic(|| Some(ports_list_body().into_element()))),
        )
}

fn ports_list_body() -> impl IntoElement {
    let entries = ports::filtered_ports();
    let loading = system::ports_loading();
    let (self_pid, self_uid) = ports::current_identity();

    if loading && entries.is_empty() {
        return view()
            .w_full()
            .px(tokens::space(4.0))
            .py(tokens::space(5.0))
            .child(
                text("Scanning listening ports…")
                    .text_size(px(tokens::TEXT_SMALL))
                    .text_color(move || theme().muted_foreground),
            );
    }

    if entries.is_empty() {
        let has_query = !ports::search().trim().is_empty() || ports::filter() != PortFilter::All;
        let msg = if has_query {
            "No listening ports match the current filter."
        } else {
            "No active listening ports found."
        };
        return view()
            .test_id("ports-empty")
            .w_full()
            .px(tokens::space(4.0))
            .py(tokens::space(5.0))
            .child(
                text(msg)
                    .text_size(px(tokens::TEXT_SMALL))
                    .text_color(move || theme().muted_foreground),
            );
    }

    let mut list = view().w_full().flex_col();
    for (index, entry) in entries.into_iter().enumerate() {
        if index > 0 {
            list = list.child(view().w_full().px(tokens::space(4.0)).child(ui::hairline()));
        }
        list = list.child(port_row(entry, self_pid, self_uid));
    }
    list
}

fn port_row(entry: ListeningPort, self_pid: i64, self_uid: i64) -> impl IntoElement {
    let port = entry.port;
    let starred = entry.starred;
    let watched = entry.watched;
    let stopping = ports::is_stopping(port);
    let force_kill_ready = entry.pid.is_some() && ports::force_kill_pid_for(port) == entry.pid;

    let process_name = if entry.process.is_empty() {
        "unknown".to_string()
    } else {
        entry.process.clone()
    };
    let pid_text = entry.pid.map(|p| format!("PID {p}"));
    let bind_addr = if entry.address.is_empty() {
        "—".to_string()
    } else {
        entry.address.clone()
    };
    let show_command = !entry.command.is_empty() && entry.command != process_name;
    let command_text = entry.command.clone();
    let command_tip = entry.command.clone();

    let owner_badge = match &entry.owner {
        PortOwner::DevbaseDatabase { id, engine } => {
            Some((format!("Devbase DB · {id} ({engine})"), TagTone::Success))
        }
        PortOwner::DevbaseProxy => Some(("Devbase Proxy".to_string(), TagTone::Success)),
        PortOwner::DevbaseRule { domain } => Some((format!("https://{domain}"), TagTone::Success)),
        PortOwner::External => None,
    };

    let category_tone = match entry.category {
        PortCategory::Dev => TagTone::Neutral,
        PortCategory::Database => TagTone::Success,
        PortCategory::Web => TagTone::Neutral,
        PortCategory::System => TagTone::Warning,
    };

    let can_add_https = matches!(entry.owner, PortOwner::External)
        && port >= 1024
        && entry.category != PortCategory::Database;
    let has_https_rule = matches!(entry.owner, PortOwner::DevbaseRule { .. });

    let (stop_label, stop_tone, stop_disabled, stop_tip) = match &entry.owner {
        PortOwner::DevbaseDatabase { id, .. } => (
            "Stop DB".to_string(),
            Tone::Default,
            stopping,
            format!("Stop Devbase database instance {id}"),
        ),
        PortOwner::DevbaseProxy => (
            "Stop proxy".to_string(),
            Tone::Default,
            stopping,
            "Stop Devbase HTTPS proxy".to_string(),
        ),
        PortOwner::DevbaseRule { .. } | PortOwner::External => {
            if force_kill_ready {
                (
                    "Force kill".to_string(),
                    Tone::Danger,
                    stopping,
                    format!("Send SIGKILL to PID {}", entry.pid.unwrap_or_default()),
                )
            } else if entry.can_terminate_pid(self_pid, self_uid) {
                (
                    "Stop".to_string(),
                    Tone::Default,
                    stopping,
                    format!("Send SIGTERM to PID {}", entry.pid.unwrap_or_default()),
                )
            } else {
                (
                    "Protected".to_string(),
                    Tone::Default,
                    true,
                    "Protected or owned by another user — manage system daemons in Services"
                        .to_string(),
                )
            }
        }
    };

    let entry_for_open = entry.clone();
    let entry_for_copy = entry.clone();
    let entry_for_stop = entry.clone();
    let process_for_https = process_name.clone();

    view()
        .test_id(&format!("port-row-{port}"))
        .w_full()
        .min_h(px(48.0))
        .flex_row()
        .items_center()
        .gap(tokens::space(2.5))
        .px(tokens::space(3.5))
        .py(tokens::space(2.0))
        .hover(move |style| style.bg(tokens::row_hover()))
        .child(
            view()
                .flex_none()
                .flex_row()
                .items_center()
                .gap(px(2.0))
                .child(
                    view()
                        .flex_none()
                        .tooltip(ui::tooltip(if starred {
                            "Unstar port"
                        } else {
                            "Pin port to top"
                        }))
                        .child(
                            nsh_base::button()
                                .test_id(&format!("port-star-{port}"))
                                .size(px(24.0))
                                .flex_none()
                                .items_center()
                                .justify_center()
                                .rounded(px(5.0))
                                .hover(move |style| style.bg(tokens::row_hover()))
                                .focus_visible(|style| {
                                    style.ring(px(2.0)).ring_color(move || theme().ring)
                                })
                                .accessibility_label(if starred {
                                    format!("Unstar port {port}")
                                } else {
                                    format!("Star port {port}")
                                })
                                .on_press(move || ports::toggle_starred(port))
                                .child(
                                    if starred {
                                        icons::star_filled()
                                    } else {
                                        icons::star()
                                    }
                                    .size(px(13.0))
                                    .text_color(move || {
                                        if starred {
                                            tokens::warning()
                                        } else {
                                            theme().muted_foreground.opacity(0.65)
                                        }
                                    }),
                                ),
                        ),
                )
                .child(
                    view()
                        .flex_none()
                        .tooltip(ui::tooltip(if watched {
                            "Stop watching port"
                        } else {
                            "Watch port for open, close, or process changes"
                        }))
                        .child(
                            nsh_base::button()
                                .test_id(&format!("port-watch-{port}"))
                                .size(px(24.0))
                                .flex_none()
                                .items_center()
                                .justify_center()
                                .rounded(px(5.0))
                                .hover(move |style| style.bg(tokens::row_hover()))
                                .focus_visible(|style| {
                                    style.ring(px(2.0)).ring_color(move || theme().ring)
                                })
                                .accessibility_label(if watched {
                                    format!("Unwatch port {port}")
                                } else {
                                    format!("Watch port {port}")
                                })
                                .on_press(move || ports::toggle_watched(port))
                                .child(icons::bell().size(px(13.0)).text_color(move || {
                                    if watched {
                                        theme().primary
                                    } else {
                                        theme().muted_foreground.opacity(0.65)
                                    }
                                })),
                        ),
                ),
        )
        .child(
            text(format!(":{port}"))
                .w(px(64.0))
                .flex_none()
                .font_family("monospace")
                .tabular_nums()
                .text_size(px(tokens::TEXT_BODY))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().foreground),
        )
        .child(
            view()
                .w(px(80.0))
                .flex_none()
                .flex_row()
                .items_center()
                .child(
                    text(bind_addr)
                        .font_family("monospace")
                        .tabular_nums()
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground)
                        .truncate(),
                ),
        )
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .flex_col()
                .gap(px(2.0))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .flex_wrap()
                        .gap(tokens::space(1.5))
                        .child(
                            text(process_name)
                                .text_size(px(tokens::TEXT_SMALL))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(move || theme().foreground)
                                .truncate(),
                        )
                        .children(pid_text.map(|pid| {
                            text(pid)
                                .font_family("monospace")
                                .tabular_nums()
                                .text_size(px(tokens::TEXT_CAPTION))
                                .text_color(move || theme().muted_foreground)
                        }))
                        .child(ui::tag(entry.category.label(), category_tone))
                        .children(owner_badge.map(|(label, tone)| ui::tag(label, tone))),
                )
                .children(show_command.then(|| {
                    view().w_full().tooltip(ui::tooltip(command_tip)).child(
                        text(command_text)
                            .font_family("monospace")
                            .text_size(px(11.0))
                            .text_color(move || theme().muted_foreground)
                            .truncate(),
                    )
                })),
        )
        .child(
            view()
                .flex_none()
                .flex_row()
                .items_center()
                .gap(tokens::space(1.0))
                .children(has_https_rule.then(|| {
                    let open_entry = entry_for_open.clone();
                    ui::action_button(
                        "Open HTTPS",
                        Tone::Default,
                        format!("port-open-https-{port}"),
                        false,
                        move || ports::open_port_url(&open_entry),
                    )
                }))
                .children(can_add_https.then(|| {
                    let proc_hint = process_for_https.clone();
                    ui::action_button(
                        "Add HTTPS",
                        Tone::Default,
                        format!("port-add-https-{port}"),
                        false,
                        move || ports::expose_via_https(port, &proc_hint),
                    )
                }))
                .child(
                    view()
                        .flex_none()
                        .tooltip(ui::tooltip("Copy local URL"))
                        .child(
                            nsh_base::button()
                                .test_id(&format!("port-copy-{port}"))
                                .size(px(26.0))
                                .flex_none()
                                .items_center()
                                .justify_center()
                                .rounded(move || theme().radius)
                                .hover(move |style| style.bg(tokens::row_hover()))
                                .focus_visible(|style| {
                                    style.ring(px(2.0)).ring_color(move || theme().ring)
                                })
                                .accessibility_label(format!("Copy URL for port {port}"))
                                .on_press(move || ports::copy_port_url(&entry_for_copy))
                                .child(
                                    icons::copy()
                                        .size(px(13.0))
                                        .text_color(move || theme().muted_foreground),
                                ),
                        ),
                )
                .child(
                    view()
                        .flex_none()
                        .tooltip(ui::tooltip(stop_tip))
                        .child(ui::action_button(
                            stop_label,
                            stop_tone,
                            format!("port-stop-{port}"),
                            stop_disabled,
                            move || ports::stop_port(entry_for_stop.clone(), force_kill_ready),
                        )),
                ),
        )
}

fn offline_saved_card() -> Option<AnyElement> {
    let saved = ports::offline_saved_ports();
    if saved.is_empty() {
        return None;
    }
    let mut rows = view().w_full().flex_col();
    for (index, (port, starred, watched)) in saved.into_iter().enumerate() {
        if index > 0 {
            rows = rows.child(view().w_full().px(tokens::space(4.0)).child(ui::hairline()));
        }
        rows = rows.child(
            view()
                .test_id(&format!("port-offline-{port}"))
                .w_full()
                .min_h(px(40.0))
                .flex_row()
                .items_center()
                .justify_between()
                .gap(tokens::space(2.0))
                .px(tokens::space(4.0))
                .py(tokens::space(1.5))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(tokens::space(2.0))
                        .child(
                            text(format!(":{port}"))
                                .font_family("monospace")
                                .tabular_nums()
                                .text_size(px(tokens::TEXT_SMALL))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(move || theme().muted_foreground),
                        )
                        .child(ui::tag("Offline", TagTone::Neutral))
                        .children(starred.then(|| ui::tag("Starred", TagTone::Warning)))
                        .children(watched.then(|| ui::tag("Watching", TagTone::Success))),
                )
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(tokens::space(1.0))
                        .children(starred.then(|| {
                            ui::action_button(
                                "Unstar",
                                Tone::Default,
                                format!("port-offline-unstar-{port}"),
                                false,
                                move || ports::toggle_starred(port),
                            )
                        }))
                        .children(watched.then(|| {
                            ui::action_button(
                                "Unwatch",
                                Tone::Default,
                                format!("port-offline-unwatch-{port}"),
                                false,
                                move || ports::toggle_watched(port),
                            )
                        })),
                ),
        );
    }

    Some(
        view()
            .test_id("ports-offline-saved-card")
            .w_full()
            .flex_col()
            .gap(tokens::space(1.5))
            .child(
                text("Saved offline ports")
                    .px(tokens::space(1.5))
                    .text_size(px(tokens::TEXT_CAPTION))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(move || theme().muted_foreground),
            )
            .child(
                view()
                    .w_full()
                    .flex_col()
                    .rounded(tokens::panel_radius)
                    .bg(tokens::panel)
                    .border_1()
                    .border_color(tokens::hairline)
                    .overflow_hidden()
                    .child(rows),
            )
            .into_element(),
    )
}

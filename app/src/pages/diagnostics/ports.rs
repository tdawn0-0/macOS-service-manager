//! The port card, ported from `view_ports.go`: check one port through
//! devcore's bind-first inspection, suggest a free port, and list the
//! current user's listening ports.

use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{
    components::controls::{self as ui, Tone},
    store::system,
    theme as tokens,
};

fn parse_valid_port(input: &str) -> Option<i64> {
    input
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|port| (1..=65535).contains(port))
}

pub(crate) fn ports_card() -> impl IntoElement {
    ui::card(
        "Ports",
        view()
            .flex_col()
            .gap(tokens::space(2.5))
            .child(dynamic(|| {
                system::port_error().map(|error| ui::plain_note(error, true))
            }))
            .child(dynamic(|| {
                system::port_notice().map(|notice| ui::plain_note(notice, false))
            }))
            .child(
                view()
                    .flex_row()
                    .items_end()
                    .gap(tokens::space(2.0))
                    .child(view().w(px(110.0)).flex_none().child(ui::field(
                        "Port",
                        ui::text_field(
                            "diagnostics-port",
                            "3000",
                            system::port_input,
                            system::set_port_input,
                        ),
                    )))
                    .child(ui::action_button(
                        "Check port",
                        Tone::Default,
                        "diagnostics-check-port",
                        || {
                            system::checking_port()
                                || parse_valid_port(&system::port_input()).is_none()
                        },
                        system::check_port,
                    ))
                    .child(ui::action_button(
                        "Find a free port",
                        Tone::Default,
                        "diagnostics-free-port",
                        system::suggesting_port,
                        system::find_free_port,
                    )),
            )
            .child(dynamic(|| {
                parse_valid_port(&system::port_input())
                    .and_then(system::port_inspection)
                    .map(port_result)
            }))
            .child(
                view()
                    .flex_col()
                    .gap(tokens::space(1.5))
                    .pt(tokens::space(1.0))
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap(tokens::space(2.0))
                            .child(
                                text("Listening ports")
                                    .text_size(px(tokens::TEXT_CAPTION))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(move || theme().muted_foreground),
                            )
                            .child(view().flex_1())
                            .child(ui::action_button(
                                "Refresh",
                                Tone::Default,
                                "diagnostics-refresh-ports",
                                system::ports_loading,
                                system::refresh_ports,
                            )),
                    )
                    .child(dynamic(|| Some(listening_ports_body().into_element()))),
            ),
    )
}

fn listening_ports_body() -> impl IntoElement {
    let listening = system::listening_ports();
    let loading = system::ports_loading();
    view()
        .flex_col()
        .gap(tokens::space(1.5))
        .child(when(loading, || {
            text("Reading listening ports…")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
        }))
        .child(when(listening.is_empty() && !loading, || {
            text("No user-owned listening ports.")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
        }))
        .children(listening.into_iter().map(|port| {
            view()
                .test_id(&format!("listening-{}", port.port))
                .flex_row()
                .items_center()
                .gap(tokens::space(2.0))
                .child(
                    text(format!("Port {}", port.port))
                        .w(px(90.0))
                        .flex_none()
                        .font_family("monospace")
                        .tabular_nums()
                        .text_size(px(tokens::TEXT_SMALL))
                        .text_color(move || theme().foreground),
                )
                .child(
                    text(port.process)
                        .flex_1()
                        .min_w(px(0.0))
                        .font_family("monospace")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground)
                        .truncate(),
                )
        }))
}

fn port_result(inspection: devcore::databases::PortInspection) -> impl IntoElement {
    let port = inspection.port;
    let own_proxy = inspection.status == "Devbase proxy";
    let show_detail = !inspection.detail.is_empty()
        && inspection.status != "Available"
        && !own_proxy
        && inspection.listeners.is_empty();
    let detail = inspection.detail.clone();
    ui::inset(
        view()
            .flex_col()
            .gap(tokens::space(1.0))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap(tokens::space(2.0))
                    .child(
                        text(format!("Port {port}"))
                            .font_family("monospace")
                            .tabular_nums()
                            .text_size(px(tokens::TEXT_SMALL))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(move || theme().foreground),
                    )
                    .child(ui::status_tag(inspection.status.clone())),
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
            })),
    )
}

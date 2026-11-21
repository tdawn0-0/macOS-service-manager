//! The port card: check one port through devcore's bind-first inspection,
//! suggest a free port, and list the current user's listening ports.

use nsh_base::theme;
use nsh_core::prelude::*;

use super::{inset_hairline, section_card};
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
    section_card(
        "Ports",
        None,
        view()
            .w_full()
            .flex_col()
            .child(
                view()
                    .w_full()
                    .flex_col()
                    .gap(tokens::space(2.5))
                    .px(tokens::space(4.0))
                    .py(tokens::space(3.0))
                    .child(dynamic(|| {
                        system::port_error().map(|error| ui::plain_note(error, true))
                    }))
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap(tokens::space(2.0))
                            .child(view().w(px(112.0)).flex_none().child(ui::text_field(
                                "diagnostics-port",
                                "3000",
                                system::port_input,
                                system::set_port_input,
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
                                "Find free port",
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
                    })),
            )
            .child(inset_hairline())
            .child(dynamic(|| Some(listening_ports_body().into_element()))),
    )
}

fn listening_ports_body() -> impl IntoElement {
    let listening = system::listening_ports();
    let loading = system::ports_loading();
    let mut rows = view()
        .w_full()
        .flex_col()
        .child(when(loading, || {
            text("Loading…")
                .px(tokens::space(4.0))
                .py(tokens::space(3.0))
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
        }))
        .child(when(listening.is_empty() && !loading, || {
            text("No listening ports")
                .px(tokens::space(4.0))
                .py(tokens::space(3.0))
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
        }));

    for (index, port) in listening.into_iter().enumerate() {
        if index > 0 {
            rows = rows.child(inset_hairline());
        }
        rows = rows.child(
            view()
                .test_id(&format!("listening-{}", port.port))
                .w_full()
                .min_h(px(36.0))
                .flex_row()
                .items_center()
                .gap(tokens::space(2.0))
                .px(tokens::space(4.0))
                .py(tokens::space(2.0))
                .child(
                    text(format!("Port {}", port.port))
                        .w(px(92.0))
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
                ),
        );
    }

    rows
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

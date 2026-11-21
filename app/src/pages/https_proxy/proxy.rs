//! The HTTPS proxy page: the editable rule table and its listener, the proxy
//! and Cloudflare tunnel services, and the local certificate trust summary
//! (`view_https_proxy.go` `devHTTPS`). The Local certificates page lives in
//! `authority.rs`.

use devproxy::ca::material::TrustState;
use nsh_base::{dialog, menu, menu_item, menu_separator, theme};
use nsh_core::{
    ContextMenuEvent,
    element::{
        ImageSrc, Svg,
        desktop::{MenuItem as DesktopMenuItem, popup_menu_at},
        svg,
    },
    prelude::*,
};

use crate::{
    components::{
        controls::{self as ui, TagTone, Tone},
        icons::{self, Icon},
        note,
    },
    store::proxy::{self, RuleDraft, TUNNEL_MODE_CUSTOM_DOMAIN, TUNNEL_MODE_WORKERS_DEV},
    theme as tokens,
};

pub(crate) fn rules_page() -> impl IntoElement {
    view()
        .flex_1()
        .min_w(px(0.0))
        .flex_col()
        .child(header())
        .child(notes())
        .child(
            scroll().flex_1().min_h(px(0.0)).child(
                view()
                    .w_full()
                    .items_center()
                    .px(tokens::space(5.0))
                    .py(tokens::space(5.0))
                    .child(
                        view()
                            .w_full()
                            .max_w(px(880.0))
                            .flex_col()
                            .gap(tokens::space(4.0))
                            .child(status_strip())
                            .child(rules_card())
                            .child(tunnel_card())
                            .child(dynamic(logs_card)),
                    ),
            ),
        )
        .child(when(|| proxy::qr_dialog().is_some(), tunnel_qr_dialog))
}

fn header() -> impl IntoElement {
    ui::page_header_with_leading(
        Some(
            icons::shield()
                .size(px(16.0))
                .flex_none()
                .text_color(move || theme().muted_foreground),
        ),
        "HTTPS Proxy",
        None::<&str>,
        Some(
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(2.0))
                .child(
                    view()
                        .test_id("proxy-publication")
                        .child(dynamic(|| Some(ui::status_tag(proxy::publication())))),
                )
                .child(ui::action_button(
                    "Add rule",
                    Tone::Primary,
                    "proxy-add-rule",
                    false,
                    proxy::add_rule,
                )),
        ),
    )
    .test_id("proxy-rules")
}

fn notes() -> impl IntoElement {
    view()
        .flex_col()
        .flex_none()
        .child(dynamic(move || {
            proxy::action_error().map(|message| note(icons::warn(), message, true))
        }))
        .child(dynamic(move || {
            proxy::load_error().map(|message| note(icons::warn(), message, true))
        }))
}

// --- Top status strip --------------------------------------------------------

fn status_strip() -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(2.5))
        .p(tokens::space(3.5))
        .rounded(tokens::panel_radius)
        .bg(tokens::panel)
        .border_1()
        .border_color(tokens::hairline)
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(3.0))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(tokens::space(1.5))
                        .child(ui::switch(
                            "proxy-switch",
                            || proxy::proxy_status() == "Running",
                            || {
                                let on = proxy::proxy_status() == "Running";
                                proxy::busy()
                                    || proxy::dirty()
                                    || (proxy::saved_rules().is_empty() && !on)
                            },
                            "HTTPS proxy service",
                            proxy::set_proxy_enabled,
                        ))
                        .child(
                            text("Proxy")
                                .text_size(px(tokens::TEXT_SMALL))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(move || theme().foreground),
                        )
                        .child(dynamic(|| {
                            let status = proxy::proxy_status();
                            (status != "Running" && status != "Stopped")
                                .then(|| ui::status_tag(status))
                        }))
                        .child(
                            text(move || {
                                format!(
                                    "{} · listener :{}",
                                    proxy::plural(proxy::saved_rules().len(), "rule"),
                                    proxy::listener_port()
                                )
                            })
                            .text_size(px(tokens::TEXT_CAPTION))
                            .tabular_nums()
                            .text_color(move || theme().muted_foreground),
                        ),
                )
                .child(strip_divider())
                .child(dynamic(|| Some(ca_strip_pill())))
                .child(strip_divider())
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(tokens::space(1.5))
                        .child(ui::switch(
                            "tunnel-switch",
                            || proxy::tunnel_status() == "Running",
                            || {
                                let on = proxy::tunnel_status() == "Running";
                                proxy::busy()
                                    || proxy::tunnel_dirty()
                                    || (!proxy::has_tunnel_rules() && !on)
                            },
                            "Cloudflare tunnel service",
                            proxy::set_tunnel_enabled,
                        ))
                        .child(
                            text("Tunnel")
                                .text_size(px(tokens::TEXT_SMALL))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(move || theme().foreground),
                        )
                        .child(dynamic(|| {
                            let status = proxy::tunnel_status();
                            (status != "Running" && status != "Stopped")
                                .then(|| ui::status_tag(status))
                        })),
                ),
        )
        .child(dynamic(ca_untrusted_warning))
        .child(ui::hairline())
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(1.5))
                .child(ui::action_button(
                    "Trust local CA",
                    Tone::Default,
                    "ca-trust",
                    || proxy::busy() || proxy::dirty(),
                    proxy::trust_active,
                ))
                .child(ui::action_button(
                    "Remove CA trust",
                    Tone::Default,
                    "ca-remove-trust",
                    || proxy::busy() || proxy::dirty(),
                    proxy::remove_active_trust,
                ))
                .child(ui::action_button(
                    "Local certificates",
                    Tone::Default,
                    "ca-open",
                    proxy::busy,
                    proxy::open_authority,
                ))
                .child(view().flex_1())
                .child(ui::action_button(
                    "Restart",
                    Tone::Default,
                    "proxy-restart",
                    || proxy::busy() || proxy::dirty(),
                    proxy::restart_proxy,
                ))
                .child(ui::action_button(
                    "Read logs",
                    Tone::Default,
                    "proxy-logs",
                    proxy::busy,
                    || proxy::read_logs("proxy"),
                ))
                .child(ui::action_button(
                    "Logs",
                    Tone::Default,
                    "proxy-open-logs",
                    false,
                    || crate::dev_logs::open("proxy"),
                )),
        )
}

fn strip_divider() -> impl IntoElement {
    view()
        .w(px(1.0))
        .h(px(16.0))
        .flex_none()
        .bg(tokens::hairline)
}

fn ca_strip_pill() -> impl IntoElement {
    let label = proxy::authority()
        .as_ref()
        .map(|status| {
            if status.active.status == "Not created" {
                "Not created".to_string()
            } else if status.active.trust == TrustState::Trusted {
                "Trusted".to_string()
            } else {
                "Not trusted".to_string()
            }
        })
        .unwrap_or_default();
    view()
        .flex_row()
        .items_center()
        .gap(tokens::space(1.5))
        .child(
            text("Local CA")
                .text_size(px(tokens::TEXT_SMALL))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || theme().foreground),
        )
        .child(when(!label.is_empty(), {
            let label = label.clone();
            move || {
                view()
                    .test_id("ca-trust-label")
                    .child(ui::status_tag(label.clone()))
            }
        }))
}

fn ca_untrusted_warning() -> Option<AnyElement> {
    let warning = proxy::proxy_on()
        && proxy::authority()
            .as_ref()
            .is_some_and(|status| status.active.trust != TrustState::Trusted);
    warning.then(|| {
        note(
            icons::warn(),
            "Proxy is running, but the local CA is not trusted yet.",
            false,
        )
        .into_element()
    })
}

// --- Rules -------------------------------------------------------------------

fn rules_card() -> impl IntoElement {
    ui::card(
        "Rules",
        view()
            .flex_col()
            .gap(tokens::space(2.0))
            .child(when(
                move || !proxy::rules().is_empty(),
                || {
                    view()
                        .flex_row()
                        .items_center()
                        .gap(tokens::space(2.0))
                        .px(tokens::space(3.0))
                        .child(
                            text("Domain")
                                .flex_1()
                                .min_w(px(200.0))
                                .text_size(px(tokens::TEXT_CAPTION))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(move || theme().muted_foreground),
                        )
                        .child(
                            text("Target")
                                .w(px(276.0))
                                .flex_none()
                                .text_size(px(tokens::TEXT_CAPTION))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(move || theme().muted_foreground),
                        )
                        .child(view().flex_1())
                },
            ))
            .child(when(
                move || proxy::rules().is_empty(),
                || {
                    view().py(tokens::space(4.0)).items_center().child(
                        text("No rules")
                            .text_size(px(tokens::TEXT_SMALL))
                            .text_color(move || theme().muted_foreground),
                    )
                },
            ))
            .child(each(proxy::rules, |row| row.id, rule_row))
            .child(footer()),
    )
}

fn rule_row(row: ReadSignal<RuleDraft>, index: ReadSignal<usize>) -> impl IntoElement {
    let id = row.get().id;
    view()
        .test_id(&format!("proxy-rule-{id}"))
        .flex_col()
        .gap(tokens::space(2.0))
        .p(tokens::space(3.0))
        .rounded(move || theme().radius)
        .bg(tokens::inset)
        .border_1()
        .border_color(tokens::hairline)
        .on_context_menu(move |event: &ContextMenuEvent| {
            let items = rule_context_menu_items(row, index);
            drop(popup_menu_at(items, event.page_position));
        })
        .child(top_row(row, index))
        .child(when(move || row.get().tunnel, {
            move || tunnel_row(index, row)
        }))
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(2.0))
                .child(
                    text(move || devcore::model::local_domain(&row.get().domain))
                        .font_family("monospace")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground)
                        .selectable(true),
                )
                .child(view().flex_1().min_w(px(0.0)).child(dynamic(move || {
                    (!proxy::dirty()).then(|| {
                        activation(devcore::model::local_domain(&row.get().domain)).into_element()
                    })
                }))),
        )
}

fn rule_context_menu_items(
    row: ReadSignal<RuleDraft>,
    index: ReadSignal<usize>,
) -> Vec<DesktopMenuItem> {
    let idx = index.get_untracked();
    let draft = row.get_untracked();
    let dirty = untracked(proxy::dirty);
    let busy = untracked(proxy::busy);
    let domain = devcore::model::local_domain(&draft.domain);
    let has_domain = !domain.is_empty();
    let url = untracked(|| proxy::rule_url(idx));
    let copy_url = url.clone();
    let open_target = url;
    let tunnel_on = draft.tunnel;
    let checking = untracked(|| proxy::checking_proxy(&domain));
    let check_domain = domain;

    vec![
        DesktopMenuItem::new("Copy HTTPS URL")
            .icon(icons::image_src(Icon::Copy))
            .disabled(dirty || !has_domain)
            .on_select(move || copy(&copy_url)),
        DesktopMenuItem::new("Open HTTPS URL")
            .icon(icons::image_src(Icon::Link))
            .disabled(dirty || !has_domain)
            .on_select(move || open_url_async(open_target.clone())),
        DesktopMenuItem::separator(),
        DesktopMenuItem::new("Toggle Tunnel")
            .checked(tunnel_on)
            .disabled(busy)
            .on_select(move || proxy::set_tunnel(index.get_untracked(), !tunnel_on)),
        DesktopMenuItem::new("Check HTTPS")
            .icon(icons::image_src(Icon::Shield))
            .disabled(busy || dirty || checking || !has_domain)
            .on_select(move || proxy::check_proxy(check_domain.clone())),
        DesktopMenuItem::new("View Proxy Logs")
            .icon(icons::image_src(Icon::FileText))
            .on_select(|| crate::dev_logs::open("proxy")),
        DesktopMenuItem::separator(),
        DesktopMenuItem::new("Remove")
            .icon(icons::image_src(Icon::Trash))
            .destructive(true)
            .on_select(move || proxy::remove_rule(index.get_untracked())),
    ]
}

fn top_row(row: ReadSignal<RuleDraft>, index: ReadSignal<usize>) -> impl IntoElement {
    let id = row.get().id;
    view()
        .flex_row()
        .items_center()
        .flex_wrap()
        .gap(tokens::space(2.0))
        .child(
            view()
                .flex_1()
                .min_w(px(200.0))
                .flex_row()
                .items_center()
                .gap(px(2.0))
                .child(ui::text_field(
                    format!("proxy-domain-{id}"),
                    "app",
                    move || row.get().domain,
                    move |value| proxy::set_domain(index.get(), value),
                ))
                .child(
                    text(proxy::LOCAL_DOMAIN_SUFFIX)
                        .font_family("monospace")
                        .text_size(px(tokens::TEXT_SMALL))
                        .text_color(move || theme().muted_foreground),
                ),
        )
        .child(
            view()
                .w(px(276.0))
                .flex_none()
                .flex_row()
                .items_center()
                .gap(tokens::space(1.5))
                .child(
                    icons::arrow_right()
                        .size(px(14.0))
                        .flex_none()
                        .text_color(move || theme().muted_foreground),
                )
                .child(ui::text_field(
                    format!("proxy-upstream-{id}"),
                    "127.0.0.1",
                    move || row.get().upstream_host,
                    move |value| proxy::set_upstream(index.get(), value),
                ))
                .child(
                    text(":")
                        .text_size(px(tokens::TEXT_SMALL))
                        .text_color(move || theme().muted_foreground),
                )
                .child(ui::narrow_field(
                    format!("proxy-port-{id}"),
                    72.0,
                    "3000",
                    move || row.get().local_port,
                    move |value| proxy::set_local_port(index.get(), value),
                ))
                .child(ports_menu(index, id)),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(1.5))
                .child(ui::switch(
                    format!("proxy-tunnel-{id}"),
                    move || row.get().tunnel,
                    proxy::busy,
                    "Tunnel",
                    move |next| proxy::set_tunnel(index.get(), next),
                ))
                .child(
                    text("Tunnel")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground),
                ),
        )
        .child(view().flex_1())
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(1.0))
                .child(dynamic({
                    move || {
                        if proxy::dirty() {
                            return None;
                        }
                        let url = proxy::rule_url(index.get());
                        let copy_url = url.clone();
                        Some(
                            view()
                                .flex_row()
                                .items_center()
                                .gap(tokens::space(1.0))
                                .child(link_icon_button(
                                    icons::copy(),
                                    "Copy HTTPS URL",
                                    format!("proxy-copy-url-{id}"),
                                    move || copy(&copy_url),
                                ))
                                .child(link_icon_button(
                                    icons::link(),
                                    "Open HTTPS URL",
                                    format!("proxy-open-url-{id}"),
                                    move || open_url_async(url.clone()),
                                ))
                                .into_element(),
                        )
                    }
                }))
                .child(ui::row_icon_button(
                    icons::trash(),
                    "Remove rule",
                    format!("proxy-remove-{id}"),
                    false,
                    move || proxy::remove_rule(index.get()),
                )),
        )
}

fn ports_menu(index: ReadSignal<usize>, id: u64) -> impl IntoElement {
    menu()
        .popup_style(|style| {
            style
                .bg(tokens::panel())
                .rounded(tokens::panel_radius())
                .border_1()
                .border_color(tokens::hairline())
                .p(tokens::space(1.0))
        })
        .trigger(
            nsh_base::button()
                .test_id(&format!("proxy-ports-{id}"))
                .size(px(24.0))
                .flex_none()
                .items_center()
                .justify_center()
                .rounded(move || theme().radius)
                .cursor_pointer()
                .hover(move |style| style.bg(tokens::row_hover()))
                .focus_visible(|style| style.ring(px(2.0)).ring_color(move || theme().ring))
                .accessibility_label("Choose a listening local port")
                .child(
                    icons::plug()
                        .size(px(13.0))
                        .text_color(move || theme().muted_foreground),
                ),
        )
        .child(dynamic(move || Some(ports_menu_items(index))))
        .child(menu_separator())
        .child(
            menu_item()
                .px(tokens::space(3.0))
                .py(tokens::space(1.5))
                .rounded(move || theme().radius)
                .hover(move |style| style.bg(tokens::row_hover()))
                .on_select(proxy::refresh_ports)
                .child(
                    text("Refresh listening ports")
                        .text_size(px(tokens::TEXT_SMALL))
                        .text_color(move || theme().foreground),
                ),
        )
}

fn ports_menu_items(index: ReadSignal<usize>) -> impl IntoElement {
    let ports = proxy::ports();
    let loading = proxy::ports_loading();
    let error = proxy::ports_error();
    let mut entries: Vec<AnyElement> = Vec::new();
    if loading && ports.is_empty() {
        entries.push(disabled_item("Scanning listening ports…"));
    } else if error.is_some() && ports.is_empty() {
        entries.push(disabled_item("Could not scan listening ports"));
    } else if ports.is_empty() {
        entries.push(disabled_item("No listening local ports"));
    } else {
        for port in ports {
            entries.push(
                menu_item()
                    .px(tokens::space(3.0))
                    .py(tokens::space(1.5))
                    .rounded(move || theme().radius)
                    .hover(move |style| style.bg(tokens::row_hover()))
                    .on_select(move || proxy::set_local_port(index.get(), port.port.to_string()))
                    .child(
                        text(if port.process.is_empty() {
                            port.port.to_string()
                        } else {
                            format!("{} · {}", port.port, port.process)
                        })
                        .text_size(px(tokens::TEXT_SMALL))
                        .tabular_nums()
                        .text_color(move || theme().foreground),
                    )
                    .into_element(),
            );
        }
    }
    view().flex_col().children(entries)
}

fn disabled_item(label: &'static str) -> AnyElement {
    menu_item()
        .disabled(true)
        .px(tokens::space(3.0))
        .py(tokens::space(1.5))
        .child(
            text(label)
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground),
        )
        .into_element()
}

fn tunnel_row(index: ReadSignal<usize>, row: ReadSignal<RuleDraft>) -> impl IntoElement {
    let id = row.get().id;
    view()
        .flex_col()
        .gap(tokens::space(2.0))
        .pl(tokens::space(3.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(2.0))
                .child(
                    icons::globe()
                        .size(px(13.0))
                        .flex_none()
                        .text_color(move || theme().muted_foreground),
                )
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(tokens::space(1.5))
                        .child(ui::chip(
                            TUNNEL_MODE_WORKERS_DEV,
                            move || row.get().tunnel_mode != TUNNEL_MODE_CUSTOM_DOMAIN,
                            format!("proxy-mode-workers-{id}"),
                            proxy::busy,
                            move || proxy::set_tunnel_mode(index.get(), TUNNEL_MODE_WORKERS_DEV),
                        ))
                        .child(ui::chip(
                            TUNNEL_MODE_CUSTOM_DOMAIN,
                            move || row.get().tunnel_mode == TUNNEL_MODE_CUSTOM_DOMAIN,
                            format!("proxy-mode-custom-{id}"),
                            proxy::busy,
                            move || proxy::set_tunnel_mode(index.get(), TUNNEL_MODE_CUSTOM_DOMAIN),
                        )),
                )
                .child(when(
                    move || row.get().tunnel_mode == TUNNEL_MODE_CUSTOM_DOMAIN,
                    {
                        move || {
                            view()
                                .flex_row()
                                .items_center()
                                .gap(tokens::space(1.5))
                                .child(ui::narrow_field(
                                    format!("proxy-tunnel-domain-{id}"),
                                    210.0,
                                    "app.example.com",
                                    move || row.get().tunnel_domain,
                                    move |value| proxy::set_tunnel_domain(index.get(), value),
                                ))
                                .child(host_menu(index, id))
                        }
                    },
                ))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(tokens::space(1.5))
                        .child(ui::switch(
                            format!("proxy-auth-{id}"),
                            move || row.get().tunnel_auth,
                            proxy::busy,
                            "Protect",
                            move |next| proxy::set_tunnel_auth(index.get(), next),
                        ))
                        .child(
                            text("Protect")
                                .text_size(px(tokens::TEXT_CAPTION))
                                .text_color(move || theme().muted_foreground),
                        ),
                )
                .child(view().flex_1())
                .child(
                    text({
                        move || {
                            let _ = row.get();
                            let preview = proxy::tunnel_preview(index.get());
                            if !preview.is_empty() {
                                return preview;
                            }
                            let worker = proxy::tunnel_worker(index.get());
                            if worker.is_empty() {
                                "Deploy to assign URL".to_string()
                            } else {
                                format!("{worker} (.workers.dev)")
                            }
                        }
                    })
                    .font_family("monospace")
                    .text_size(px(tokens::TEXT_SMALL))
                    .text_color(move || theme().muted_foreground)
                    .number_of_lines(1),
                ),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(1.0))
                .child(dynamic({
                    move || {
                        if proxy::dirty() {
                            return None;
                        }
                        let idx = index.get();
                        let draft = row.get();
                        let preview = proxy::tunnel_preview(idx);
                        if preview.is_empty() {
                            return None;
                        }
                        let copy_url = preview.clone();
                        let open_url_target = preview.clone();
                        let qr_url = preview;
                        let qr_domain = devcore::model::local_domain(&draft.domain);
                        let qr_protected = draft.tunnel_auth;
                        Some(
                            view()
                                .flex_row()
                                .items_center()
                                .gap(tokens::space(1.0))
                                .child(link_icon_button(
                                    icons::copy(),
                                    "Copy tunnel URL",
                                    format!("proxy-copy-tunnel-{id}"),
                                    move || copy(&copy_url),
                                ))
                                .child(link_icon_button(
                                    icons::link(),
                                    "Open tunnel URL",
                                    format!("proxy-open-tunnel-{id}"),
                                    move || open_url_async(open_url_target.clone()),
                                ))
                                .child(qr_trigger_button(format!("rule-qr-{idx}"), move || {
                                    proxy::open_qr_dialog(
                                        qr_domain.clone(),
                                        qr_url.clone(),
                                        qr_protected,
                                    )
                                }))
                                .into_element(),
                        )
                    }
                }))
                .child(dynamic({
                    move || {
                        if proxy::dirty() || !row.get().tunnel_auth {
                            return None;
                        }
                        let domain = devcore::model::local_domain(&row.get().domain);
                        Some(
                            view()
                                .flex_row()
                                .items_center()
                                .gap(tokens::space(1.0))
                                .child({
                                    let domain = domain.clone();
                                    ui::row_icon_button(
                                        icons::key(),
                                        "Copy tunnel token",
                                        format!("proxy-copy-token-{id}"),
                                        proxy::busy,
                                        move || proxy::copy_tunnel_token(domain.clone()),
                                    )
                                })
                                .child(ui::row_icon_button(
                                    icons::rotate_ccw(),
                                    "Rotate tunnel token",
                                    format!("proxy-rotate-token-{id}"),
                                    proxy::busy,
                                    {
                                        let domain = domain.clone();
                                        move || proxy::rotate_tunnel_token(domain.clone())
                                    },
                                ))
                                .into_element(),
                        )
                    }
                })),
        )
}

fn host_menu(index: ReadSignal<usize>, id: u64) -> impl IntoElement {
    let has_suggestions = computed(move || !proxy::host_suggestions(index.get()).is_empty());
    menu()
        .popup_style(|style| {
            style
                .bg(tokens::panel())
                .rounded(tokens::panel_radius())
                .border_1()
                .border_color(tokens::hairline())
                .p(tokens::space(1.0))
        })
        .trigger(
            nsh_base::button()
                .test_id(&format!("proxy-dns-suggestion-{id}"))
                .h(px(28.0))
                .flex_none()
                .items_center()
                .justify_center()
                .px(tokens::space(2.0))
                .rounded(move || theme().radius)
                .border_1()
                .border_color(tokens::hairline)
                .hover(move |style| style.bg(tokens::row_hover()))
                .focus_visible(|style| style.ring(px(2.0)).ring_color(move || theme().ring))
                .disabled(move || !has_suggestions.get())
                .opacity(move || {
                    if has_suggestions.get() {
                        1.0_f32
                    } else {
                        0.4_f32
                    }
                })
                .child(
                    text("Existing DNS…")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().foreground),
                ),
        )
        .child(dynamic(move || Some(host_menu_items(index))))
}

fn host_menu_items(index: ReadSignal<usize>) -> impl IntoElement {
    let suggestions = proxy::host_suggestions(index.get());
    let mut entries: Vec<AnyElement> = Vec::new();
    for host in suggestions {
        let chosen = host.clone();
        entries.push(
            menu_item()
                .px(tokens::space(3.0))
                .py(tokens::space(1.5))
                .rounded(move || theme().radius)
                .hover(move |style| style.bg(tokens::row_hover()))
                .on_select(move || proxy::set_tunnel_domain(index.get(), chosen.clone()))
                .child(
                    text(host)
                        .text_size(px(tokens::TEXT_SMALL))
                        .text_color(move || theme().foreground),
                )
                .into_element(),
        );
    }
    view().flex_col().children(entries)
}

/// `devProxyActivation`: tags from the explicit observations only. The Go
/// diagnostic report is not part of this screen yet; the HTTP and tunnel
/// checks are.
fn activation(domain: String) -> impl IntoElement {
    let busy = proxy::busy();
    let dirty = proxy::dirty();
    let local_port = proxy::saved_rule_port(&domain);
    let live_tunnel = proxy::live_tunnel(&domain);
    let mut tags: Vec<AnyElement> = Vec::new();
    let mut problems: Vec<(String, String, Option<String>)> = Vec::new();
    let key = domain.replace('.', "-");

    let proxy_checking = proxy::checking_proxy(&domain);
    if proxy_checking {
        tags.push(ui::tag("Checking HTTPS…", TagTone::Neutral).into_element());
    } else if let Some(result) = proxy::proxy_check(&domain) {
        if !proxy::check_is_current(&result) {
            tags.push(ui::tag("HTTP result expired", TagTone::Warning).into_element());
        } else if let Some(failure) = &result.failure {
            tags.push(ui::tag("HTTP failed", TagTone::Error).into_element());
            let hint = result.diagnostic_hint(local_port, None);
            problems.push((
                format!("proxy-probe-hint-{key}"),
                format!("HTTP · {failure}"),
                hint,
            ));
        } else {
            tags.push(
                ui::tag(
                    format!(
                        "HTTP {} · {}s ago",
                        result.status_code,
                        proxy::check_age(&result)
                    ),
                    TagTone::Success,
                )
                .into_element(),
            );
        }
    }

    if let Some(status) = live_tunnel.as_deref() {
        let (label, tone) = match status {
            "Connected" => ("Tunnel connected", TagTone::Success),
            "Conflict" => ("Tunnel conflict", TagTone::Error),
            _ => ("Tunnel connecting", TagTone::Warning),
        };
        tags.push(ui::tag(label, tone).into_element());
    }

    let tunnel_checking = proxy::checking_tunnel(&domain);
    if tunnel_checking {
        tags.push(ui::tag("Checking tunnel…", TagTone::Neutral).into_element());
    } else if let Some(result) = proxy::tunnel_check(&domain) {
        if !proxy::check_is_current(&result) {
            tags.push(ui::tag("Tunnel result expired", TagTone::Warning).into_element());
        } else if let Some(failure) = &result.failure {
            tags.push(ui::tag("Tunnel failed", TagTone::Error).into_element());
            let hint = result.diagnostic_hint(local_port, live_tunnel.as_deref());
            problems.push((
                format!("tunnel-probe-hint-{key}"),
                format!("Tunnel · {failure}"),
                hint,
            ));
        } else {
            tags.push(
                ui::tag(
                    format!(
                        "Tunnel HTTP {} · {}s ago",
                        result.status_code,
                        proxy::check_age(&result)
                    ),
                    TagTone::Success,
                )
                .into_element(),
            );
        }
    }

    let has_tunnel_url = proxy::tunnel_url_available(&domain);
    let activation_id = format!("proxy-activation-{key}");
    let tunnel_check_id = format!("proxy-check-tunnel-{key}");
    let https_check_id = format!("proxy-check-https-{key}");
    let check_domain = domain.clone();
    let tunnel_domain = domain.clone();
    view()
        .test_id(&activation_id)
        .flex_col()
        .gap(tokens::space(1.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(1.5))
                .children(tags)
                .child(view().flex_1())
                .child(when(has_tunnel_url, move || {
                    view().child(ui::action_button(
                        "Check tunnel response",
                        Tone::Default,
                        tunnel_check_id.clone(),
                        busy || dirty || tunnel_checking,
                        {
                            let domain = tunnel_domain.clone();
                            move || proxy::check_tunnel(domain.clone())
                        },
                    ))
                }))
                .child(ui::action_button(
                    "Check HTTPS response",
                    Tone::Default,
                    https_check_id.clone(),
                    busy || dirty || proxy_checking,
                    {
                        let domain = check_domain.clone();
                        move || proxy::check_proxy(domain.clone())
                    },
                )),
        )
        .child(problems.into_iter().map(|(hint_id, problem, hint)| {
            view()
                .flex_col()
                .gap(px(2.0))
                .child(
                    text(problem)
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground),
                )
                .child(hint.map(move |diagnosis_hint| {
                    view()
                        .test_id(&hint_id)
                        .flex_row()
                        .items_center()
                        .gap(tokens::space(1.0))
                        .child(
                            icons::warn()
                                .size(px(12.0))
                                .flex_none()
                                .text_color(move || theme().destructive),
                        )
                        .child(
                            text(diagnosis_hint)
                                .text_size(px(tokens::TEXT_CAPTION))
                                .text_color(move || theme().foreground)
                                .selectable(true),
                        )
                }))
        }))
}

// --- Footer ------------------------------------------------------------------

fn footer() -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .flex_wrap()
        .gap(tokens::space(2.0))
        .pt(tokens::space(1.5))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(1.5))
                .child(
                    text("Port")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(move || theme().muted_foreground),
                )
                .child(ui::narrow_field(
                    "proxy-listener-port",
                    76.0,
                    "443",
                    proxy::listener_port_field,
                    proxy::set_listener_port,
                )),
        )
        .child(
            view()
                .flex_1()
                .min_w(px(200.0))
                .child(dynamic(|| Some(footer_state()))),
        )
}

fn footer_state() -> impl IntoElement {
    let dirty = proxy::dirty();
    let publication = proxy::publication();
    let busy = proxy::busy();
    let note = if dirty {
        "Unsaved changes"
    } else if publication == "Pending" {
        "Saved, not applied yet"
    } else if publication == "Published" {
        "Applied"
    } else {
        "Not applied"
    };
    let tone = if dirty || publication == "Pending" {
        TagTone::Warning
    } else if publication == "Published" {
        TagTone::Success
    } else {
        TagTone::Neutral
    };
    let empty = proxy::saved_rules().is_empty();
    view()
        .flex_row()
        .items_center()
        .gap(tokens::space(2.0))
        .child(view().test_id("proxy-note").child(ui::tag(note, tone)))
        .child(view().flex_1())
        .child(when(dirty, move || {
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(1.5))
                .child(ui::action_button(
                    "Discard changes",
                    Tone::Default,
                    "proxy-discard",
                    busy,
                    proxy::discard,
                ))
                .child(ui::action_button(
                    "Save HTTPS rules",
                    Tone::Default,
                    "proxy-save",
                    busy,
                    proxy::save,
                ))
                .child(ui::action_button(
                    "Save & apply",
                    Tone::Primary,
                    "proxy-save-apply",
                    busy,
                    proxy::save_and_apply,
                ))
        }))
        .child(when(!dirty, move || {
            view().child(ui::action_button(
                "Apply proxy",
                if publication == "Pending" {
                    Tone::Primary
                } else {
                    Tone::Default
                },
                "proxy-apply",
                busy || empty,
                proxy::apply,
            ))
        }))
}

// --- Cloudflare tunnel -------------------------------------------------------

fn tunnel_card() -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(3.0))
        .p(tokens::space(4.0))
        .rounded(tokens::panel_radius)
        .bg(tokens::panel)
        .border_1()
        .border_color(tokens::hairline)
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(2.0))
                .child(
                    icons::globe()
                        .size(px(14.0))
                        .flex_none()
                        .text_color(move || theme().muted_foreground),
                )
                .child(
                    text("Cloudflare Tunnel")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || theme().muted_foreground),
                )
                .child(view().flex_1())
                .child(dynamic(|| Some(tunnel_meta()))),
        )
        .child(
            view()
                .flex_row()
                .items_end()
                .flex_wrap()
                .gap(tokens::space(3.0))
                .child(dynamic(|| Some(token_block())))
                .child(ui::field(
                    "Account ID",
                    ui::narrow_field(
                        "tunnel-account",
                        200.0,
                        "Auto-detected",
                        proxy::account_id_field,
                        proxy::set_account_id,
                    ),
                ))
                .child(ui::field(
                    "Client ID",
                    ui::narrow_field(
                        "tunnel-client",
                        180.0,
                        "mac-auto",
                        proxy::client_id_field,
                        proxy::set_client_id,
                    ),
                )),
        )
        .child(ui::hairline())
        .child(dynamic(|| Some(tunnel_actions_row())))
}

fn token_block() -> impl IntoElement {
    let token = proxy::token_configured();
    let busy = proxy::busy();
    view()
        .flex_col()
        .gap(tokens::space(1.0))
        .child(
            text("API token")
                .text_size(px(tokens::TEXT_CAPTION))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || theme().muted_foreground),
        )
        .child(
            view()
                .h(px(28.0))
                .flex_row()
                .items_center()
                .gap(tokens::space(1.5))
                .child(view().test_id("tunnel-token-status").child(ui::tag(
                    if token {
                        "Configured"
                    } else {
                        "Not configured"
                    },
                    if token {
                        TagTone::Success
                    } else {
                        TagTone::Warning
                    },
                )))
                .child(ui::action_button(
                    "Choose token file…",
                    Tone::Default,
                    "tunnel-token-file",
                    busy,
                    proxy::choose_token_file_dialog,
                )),
        )
}

fn tunnel_meta() -> impl IntoElement {
    let subdomain = proxy::subdomain();
    let zones = proxy::zones();
    let workers = proxy::worker_count();
    let mut meta: Vec<String> = Vec::new();
    if !subdomain.is_empty() {
        meta.push(format!("*.{subdomain}.workers.dev"));
    }
    if workers > 0 {
        meta.push(proxy::plural(workers, "worker"));
    }
    if !zones.is_empty() {
        meta.push(format!("Zones: {}", zones.join(", ")));
    }
    let meta = meta.join(" · ");
    view().test_id("tunnel-meta").child(
        text(meta)
            .font_family("monospace")
            .text_size(px(tokens::TEXT_CAPTION))
            .tabular_nums()
            .text_color(move || theme().muted_foreground),
    )
}

fn tunnel_actions_row() -> impl IntoElement {
    let busy = proxy::busy();
    let dirty = proxy::dirty();
    let tunnel_dirty = proxy::tunnel_dirty();
    let has_tunnel = proxy::has_tunnel_rules();
    let token = proxy::token_configured();
    let on = proxy::tunnel_on();
    view().flex_col().gap(tokens::space(2.0)).child(
        view()
            .flex_row()
            .items_center()
            .flex_wrap()
            .gap(tokens::space(1.5))
            .child(ui::action_button(
                "Deploy Workers",
                Tone::Default,
                "tunnel-deploy",
                busy || dirty || tunnel_dirty || !has_tunnel || !token,
                proxy::deploy_workers,
            ))
            .child(ui::action_button(
                "Refresh zones",
                Tone::Default,
                "tunnel-refresh",
                busy || tunnel_dirty || !token,
                proxy::refresh_cloudflare,
            ))
            .child(ui::action_button(
                "Restart",
                Tone::Default,
                "tunnel-restart",
                busy || !on,
                proxy::restart_tunnel,
            ))
            .child(ui::action_button(
                "Read logs",
                Tone::Default,
                "tunnel-logs",
                busy,
                || proxy::read_logs("tunnel"),
            ))
            .child(ui::action_button(
                "Logs",
                Tone::Default,
                "tunnel-open-logs",
                false,
                || crate::dev_logs::open("tunnel"),
            ))
            .child(ui::action_button(
                "Remove token",
                Tone::Danger,
                "tunnel-remove-token",
                busy || !token,
                proxy::request_remove_cloudflare,
            ))
            .child(view().flex_1())
            .child(when(tunnel_dirty, || ui::tag("Unsaved", TagTone::Warning)))
            .child(ui::action_button(
                "Discard",
                Tone::Default,
                "tunnel-discard",
                busy || !tunnel_dirty,
                proxy::discard_tunnel,
            ))
            .child(ui::action_button(
                "Save",
                Tone::Primary,
                "tunnel-save",
                busy || !tunnel_dirty,
                proxy::save_cloudflare,
            )),
    )
}

fn logs_card() -> Option<AnyElement> {
    let (title, body) = proxy::logs()?;
    Some(
        ui::card(
            title,
            view()
                .flex_col()
                .gap(tokens::space(2.0))
                .child(
                    text(body)
                        .font_family("monospace")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().foreground)
                        .selectable(true),
                )
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .child(view().flex_1())
                        .child(ui::action_button(
                            "Dismiss",
                            Tone::Default,
                            "proxy-logs-dismiss",
                            false,
                            proxy::dismiss_logs,
                        )),
                ),
        )
        .into_element(),
    )
}

// --- Tunnel QR dialog --------------------------------------------------------

fn qr_icon() -> Svg {
    const QR_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect width="5" height="5" x="3" y="3" rx="1"/><rect width="5" height="5" x="16" y="3" rx="1"/><rect width="5" height="5" x="3" y="16" rx="1"/><path d="M21 16h-3a2 2 0 0 0-2 2v3"/><path d="M21 21v.01"/><path d="M12 7v3a2 2 0 0 1-2 2H7"/><path d="M3 12h.01"/><path d="M12 3h.01"/><path d="M12 16v.01"/><path d="M16 12h1"/><path d="M21 12v.01"/><path d="M12 21v-1"/></svg>"#;
    svg(ImageSrc::svg_markup(QR_SVG))
}

fn qr_trigger_button(test_id: String, on_press: impl Fn() + 'static) -> impl IntoElement {
    view()
        .flex_none()
        .tooltip(ui::tooltip("Show tunnel QR code"))
        .child(
            nsh_base::button()
                .test_id(&test_id)
                .h(px(24.0))
                .flex_none()
                .flex_row()
                .items_center()
                .justify_center()
                .gap(tokens::space(1.0))
                .px(tokens::space(1.5))
                .rounded(move || theme().radius)
                .cursor_pointer()
                .hover(move |style| style.bg(tokens::row_hover()))
                .focus_visible(|style| style.ring(px(2.0)).ring_color(move || theme().ring))
                .accessibility_label("Show tunnel QR code")
                .on_press(on_press)
                .child(
                    qr_icon()
                        .size(px(13.0))
                        .text_color(move || theme().muted_foreground),
                )
                .child(
                    text("QR")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground),
                ),
        )
}

fn tunnel_qr_dialog() -> impl IntoElement {
    dialog()
        .label("Tunnel QR code")
        .on_cancel(proxy::close_qr_dialog)
        .child(
            view()
                .size_full()
                .relative()
                .items_center()
                .justify_center()
                .bg(AdaptiveColor::rgb(0x000000, 0x000000).opacity(0.35))
                .child(
                    view()
                        .test_id("tunnel-qr-backdrop")
                        .absolute()
                        .top(px(0.0))
                        .right(px(0.0))
                        .bottom(px(0.0))
                        .left(px(0.0))
                        .on_press(|_: &()| proxy::close_qr_dialog()),
                )
                .child(dynamic(|| proxy::qr_dialog().map(tunnel_qr_card))),
        )
}

fn tunnel_qr_card(target: proxy::TunnelQrTarget) -> AnyElement {
    let qr_markup = devproxy::qr::svg_for_text(&target.url);
    let copy_url = target.url.clone();
    let curl_domain = target.domain.clone();
    let curl_url = target.url.clone();
    let protected = target.protected;

    view()
        .test_id("tunnel-qr-dialog")
        .w(px(420.0))
        .p(tokens::space(5.0))
        .flex_col()
        .gap(tokens::space(3.0))
        .rounded(tokens::panel_radius)
        .bg(tokens::panel)
        .border_1()
        .border_color(tokens::hairline)
        .shadow_xl()
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(2.0))
                .child(
                    qr_icon()
                        .size(px(16.0))
                        .flex_none()
                        .text_color(move || theme().muted_foreground),
                )
                .child(
                    text("Tunnel QR code")
                        .text_size(px(16.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || theme().foreground),
                )
                .child(view().flex_1())
                .child(when(protected, || {
                    ui::tag("Token protected", TagTone::Warning)
                })),
        )
        .child(
            view()
                .items_center()
                .justify_center()
                .py(tokens::space(2.0))
                .child(match qr_markup {
                    Ok(markup) => view()
                        .p(tokens::space(3.0))
                        .rounded(tokens::panel_radius)
                        .bg(AdaptiveColor::rgb(0xffffff, 0xffffff))
                        .border_1()
                        .border_color(tokens::hairline)
                        .child(
                            svg(ImageSrc::svg_markup(&markup))
                                .test_id("tunnel-qr-svg")
                                .size(px(216.0)),
                        )
                        .into_element(),
                    Err(error) => note(icons::warn(), error, true).into_element(),
                }),
        )
        .child(
            view()
                .flex_col()
                .gap(tokens::space(0.5))
                .child(
                    text("Tunnel URL")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(move || theme().muted_foreground),
                )
                .child(ui::mono_value("tunnel-qr-url", target.url)),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(1.5))
                .child(ui::action_button(
                    "Copy URL",
                    Tone::Primary,
                    "tunnel-qr-copy-url",
                    false,
                    move || {
                        set_clipboard(copy_url.clone());
                        crate::toast::success("Copied tunnel URL");
                    },
                ))
                .child(when(protected, move || {
                    let domain = curl_domain.clone();
                    let url = curl_url.clone();
                    ui::action_button(
                        "Copy curl with token",
                        Tone::Default,
                        "tunnel-qr-copy-curl",
                        proxy::busy,
                        move || proxy::copy_tunnel_curl_with_token(domain.clone(), url.clone()),
                    )
                }))
                .child(view().flex_1())
                .child(ui::action_button(
                    "Close",
                    Tone::Default,
                    "tunnel-qr-close",
                    false,
                    proxy::close_qr_dialog,
                )),
        )
        .into_element()
}

// --- Small actions -----------------------------------------------------------

/// A compact 24px icon-only URL link button with a pointer cursor and native tooltip.
fn link_icon_button(
    icon: Svg,
    label: impl Into<String>,
    test_id: impl Into<String>,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let label = label.into();
    let tip = label.clone();
    let test_id = test_id.into();
    view().flex_none().tooltip(ui::tooltip(tip)).child(
        nsh_base::button()
            .test_id(&test_id)
            .h(px(24.0))
            .flex_none()
            .flex_row()
            .items_center()
            .justify_center()
            .gap(tokens::space(1.0))
            .px(tokens::space(1.5))
            .rounded(move || theme().radius)
            .cursor_pointer()
            .hover(move |style| style.bg(tokens::row_hover()))
            .focus_visible(|style| style.ring(px(2.0)).ring_color(move || theme().ring))
            .accessibility_label(label)
            .on_press(on_press)
            .child(
                icon.size(px(13.0))
                    .text_color(move || theme().muted_foreground),
            ),
    )
}

fn copy(value: &str) {
    set_clipboard(value.to_string());
    crate::toast::info("Copied URL");
}

fn open_url_async(url: String) {
    spawn(async move {
        let _ = open_url(url).await;
    });
}

/// The card's Back / rules switch uses this from `authority.rs` too.
pub(crate) fn back_to_rules() {
    proxy::close_authority();
}

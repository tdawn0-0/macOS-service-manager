//! The HTTPS proxy page: the editable rule table and its listener, the proxy
//! and Cloudflare tunnel services, and the local certificate trust summary
//! (`view_https_proxy.go` `devHTTPS`). The Local certificates page lives in
//! `authority.rs`.

use devproxy::ca::material::TrustState;
use nsh_base::{menu, menu_item, menu_separator, theme};
use nsh_core::{
    ContextMenuEvent,
    element::{
        Svg,
        desktop::{MenuItem as DesktopMenuItem, popup_menu_at},
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
                    .flex_col()
                    .gap(tokens::space(4.0))
                    .px(tokens::space(5.0))
                    .pt(tokens::space(4.0))
                    .pb(tokens::space(5.0))
                    .child(rules_card())
                    .child(proxy_card())
                    .child(tunnel_card())
                    .child(dynamic(|| Some(trust_card())))
                    .child(dynamic(logs_card)),
            ),
        )
}

fn header() -> impl IntoElement {
    ui::page_header_with_leading(
        Some(
            icons::shield()
                .size(px(16.0))
                .flex_none()
                .text_color(move || theme().muted_foreground),
        ),
        "HTTPS proxy rules",
        Some(move || {
            format!(
                "{} · listener :{}",
                proxy::plural(proxy::saved_rules().len(), "rule"),
                proxy::listener_port()
            )
        }),
        Some(
            view()
                .test_id("proxy-publication")
                .child(dynamic(|| Some(ui::status_tag(proxy::publication())))),
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

// --- Rules -------------------------------------------------------------------

fn rules_card() -> impl IntoElement {
    ui::card(
        "Rules",
        view()
            .flex_col()
            .gap(tokens::space(2.0))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .child(view().flex_1())
                    .child(ui::action_button(
                        "Add rule",
                        Tone::Default,
                        "proxy-add-rule",
                        false,
                        proxy::add_rule,
                    )),
            )
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
        .child(dynamic({
            move || {
                (!proxy::dirty()).then(|| {
                    activation(devcore::model::local_domain(&row.get().domain)).into_element()
                })
            }
        }))
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
                .min_w(px(220.0))
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
                .w(px(348.0))
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
        .child(
            text(move || devcore::model::local_domain(&row.get().domain))
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground)
                .selectable(true),
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
                        let preview = proxy::tunnel_preview(index.get());
                        if preview.is_empty() {
                            return None;
                        }
                        let copy_url = preview.clone();
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
                                    move || open_url_async(preview.clone()),
                                ))
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
    let mut tags: Vec<AnyElement> = Vec::new();
    let mut problems: Vec<String> = Vec::new();

    let proxy_checking = proxy::checking_proxy(&domain);
    if proxy_checking {
        tags.push(ui::tag("Checking HTTPS…", TagTone::Neutral).into_element());
    } else if let Some(result) = proxy::proxy_check(&domain) {
        if !proxy::check_is_current(&result) {
            tags.push(ui::tag("HTTP result expired", TagTone::Warning).into_element());
        } else if let Some(failure) = &result.failure {
            tags.push(ui::tag("HTTP failed", TagTone::Error).into_element());
            problems.push(format!("HTTP · {failure}"));
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

    if let Some(status) = proxy::live_tunnel(&domain) {
        let (label, tone) = match status.as_str() {
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
            problems.push(format!("Tunnel · {failure}"));
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
    let key = domain.replace('.', "-");
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
        .child(problems.into_iter().map(|problem| {
            text(problem)
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground)
        }))
}

// --- Footer ------------------------------------------------------------------

fn footer() -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(2.0))
        .pt(tokens::space(1.0))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(2.0))
                .child(view().w(px(200.0)).flex_none().child(ui::text_field(
                    "proxy-listener-port",
                    "443",
                    proxy::listener_port_field,
                    proxy::set_listener_port,
                )))
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

// --- Service cards -----------------------------------------------------------

fn proxy_card() -> impl IntoElement {
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
                .gap(tokens::space(2.0))
                .child(
                    icons::shield()
                        .size(px(14.0))
                        .flex_none()
                        .text_color(move || theme().muted_foreground),
                )
                .child(
                    text("Proxy")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || theme().muted_foreground),
                )
                .child(dynamic(|| {
                    let status = proxy::proxy_status();
                    (status != "Running" && status != "Stopped").then(|| ui::status_tag(status))
                }))
                .child(view().flex_1())
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(tokens::space(1.5))
                        .child(
                            text("HTTPS proxy service")
                                .text_size(px(tokens::TEXT_CAPTION))
                                .text_color(move || theme().muted_foreground),
                        )
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
                        )),
                ),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(1.5))
                .child(ui::action_button(
                    "Restart HTTPS proxy",
                    Tone::Default,
                    "proxy-restart",
                    || proxy::busy() || proxy::dirty(),
                    proxy::restart_proxy,
                ))
                .child(ui::action_button(
                    "Read proxy logs",
                    Tone::Default,
                    "proxy-logs",
                    proxy::busy,
                    || proxy::read_logs("proxy"),
                )),
        )
}

fn tunnel_card() -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(3.0))
        .p(tokens::space(4.0))
        .rounded(tokens::panel_radius)
        .bg(tokens::panel)
        .border_1()
        .border_color(tokens::hairline)
        .child(tunnel_header())
        .child(
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(3.0))
                .child(dynamic(|| Some(token_block())))
                .child(ui::field(
                    "Account ID",
                    ui::narrow_field(
                        "tunnel-account",
                        220.0,
                        "Auto-detected",
                        proxy::account_id_field,
                        proxy::set_account_id,
                    ),
                ))
                .child(ui::field(
                    "Client ID",
                    ui::narrow_field(
                        "tunnel-client",
                        200.0,
                        "mac-auto",
                        proxy::client_id_field,
                        proxy::set_client_id,
                    ),
                )),
        )
        .child(dynamic(|| Some(tunnel_meta())))
        .child(dynamic(|| Some(tunnel_footer())))
        .child(dynamic(|| Some(tunnel_actions())))
}

fn tunnel_header() -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .gap(tokens::space(2.0))
        .child(
            icons::globe()
                .size(px(14.0))
                .flex_none()
                .text_color(move || theme().muted_foreground),
        )
        .child(
            text("Cloudflare tunnel")
                .text_size(px(tokens::TEXT_CAPTION))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().muted_foreground),
        )
        .child(dynamic(|| {
            let status = proxy::tunnel_status();
            (status != "Running" && status != "Stopped").then(|| ui::status_tag(status))
        }))
        .child(view().flex_1())
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(1.5))
                .child(
                    text("Cloudflare tunnel service")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground),
                )
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
                )),
        )
}

fn token_block() -> impl IntoElement {
    let token = proxy::token_configured();
    let busy = proxy::busy();
    view()
        .w(px(280.0))
        .flex_col()
        .gap(tokens::space(1.0))
        .child(
            text("Cloudflare API token")
                .text_size(px(tokens::TEXT_CAPTION))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || theme().muted_foreground),
        )
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(2.0))
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

fn tunnel_footer() -> impl IntoElement {
    let busy = proxy::busy();
    let tunnel_dirty = proxy::tunnel_dirty();
    view()
        .flex_row()
        .items_center()
        .gap(tokens::space(1.5))
        .child(view().flex_1())
        .child(when(tunnel_dirty, || {
            ui::tag("Unsaved tunnel changes", TagTone::Warning)
        }))
        .child(ui::action_button(
            "Discard tunnel changes",
            Tone::Default,
            "tunnel-discard",
            busy || !tunnel_dirty,
            proxy::discard_tunnel,
        ))
        .child(ui::action_button(
            "Save Cloudflare settings",
            Tone::Primary,
            "tunnel-save",
            busy || !tunnel_dirty,
            proxy::save_cloudflare,
        ))
}

fn tunnel_actions() -> impl IntoElement {
    let busy = proxy::busy();
    let dirty = proxy::dirty();
    let tunnel_dirty = proxy::tunnel_dirty();
    let has_tunnel = proxy::has_tunnel_rules();
    let token = proxy::token_configured();
    let on = proxy::tunnel_on();
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
            "Restart tunnel",
            Tone::Default,
            "tunnel-restart",
            busy || !on,
            proxy::restart_tunnel,
        ))
        .child(ui::action_button(
            "Remove Cloudflare token",
            Tone::Danger,
            "tunnel-remove-token",
            busy || !token,
            proxy::request_remove_cloudflare,
        ))
        .child(ui::action_button(
            "Read tunnel logs",
            Tone::Default,
            "tunnel-logs",
            busy,
            || proxy::read_logs("tunnel"),
        ))
}

fn trust_card() -> impl IntoElement {
    let busy = proxy::busy();
    let dirty = proxy::dirty();
    let authority = proxy::authority();
    let label = authority
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
    let warning = proxy::proxy_on()
        && authority
            .as_ref()
            .is_some_and(|status| status.active.trust != TrustState::Trusted);
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
                .gap(tokens::space(2.0))
                .child(
                    icons::key()
                        .size(px(14.0))
                        .flex_none()
                        .text_color(move || theme().muted_foreground),
                )
                .child(
                    text("Local certificate trust")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || theme().muted_foreground),
                )
                .child(when(!label.is_empty(), {
                    let label = label.clone();
                    move || {
                        view()
                            .test_id("ca-trust-label")
                            .child(ui::status_tag(label.clone()))
                    }
                }))
                .child(view().flex_1())
                .child(ui::action_button(
                    "Local certificates",
                    Tone::Default,
                    "ca-open",
                    busy,
                    proxy::open_authority,
                )),
        )
        .child(when(warning, || {
            note(
                icons::warn(),
                "Proxy is running, but the local CA is not trusted yet.",
                false,
            )
        }))
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
                    busy || dirty,
                    proxy::trust_active,
                ))
                .child(ui::action_button(
                    "Remove CA trust",
                    Tone::Default,
                    "ca-remove-trust",
                    busy || dirty,
                    proxy::remove_active_trust,
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

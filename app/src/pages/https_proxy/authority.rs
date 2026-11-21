//! The Local certificates page: the active authority, the replacement flow
//! and every saved generation (`view_authority.go` `devAuthority`).
//!
//! The Go view asks macOS for permission inside keychain trust changes; the
//! screen itself only enables a trust action when its certificate is usable.

use devproxy::ca::material::AuthorityInfo;
use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{
    components::{
        controls::{self as ui, TagTone, Tone},
        icons, note,
    },
    store::proxy,
    theme as tokens,
};

use super::proxy::back_to_rules;

pub(crate) fn authority_page() -> impl IntoElement {
    view()
        .test_id("ca-page")
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
                            .max_w(px(840.0))
                            .flex_col()
                            .gap(tokens::space(4.0))
                            .child(dynamic(|| Some(cards()))),
                    ),
            ),
        )
}

fn header() -> impl IntoElement {
    ui::page_header_with_leading(
        Some(
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(2.0))
                .child(ui::action_button(
                    "← HTTPS Proxy",
                    Tone::Default,
                    "ca-back",
                    false,
                    back_to_rules,
                ))
                .child(
                    icons::shield()
                        .size(px(16.0))
                        .flex_none()
                        .text_color(move || theme().muted_foreground),
                ),
        ),
        "Local certificates",
        None::<&str>,
        Some(ui::action_button(
            "Refresh",
            Tone::Default,
            "ca-refresh",
            proxy::busy,
            proxy::refresh_authority,
        )),
    )
    .test_id("ca-header")
}

fn notes() -> impl IntoElement {
    view().flex_col().flex_none().child(dynamic(move || {
        proxy::action_error().map(|message| note(icons::warn(), message, true))
    }))
}

fn cards() -> impl IntoElement {
    let Some(status) = proxy::authority() else {
        return view().into_element();
    };
    let prepared = status.prepared.clone();
    let has_prepared = prepared.is_some();
    let previous_id = status
        .previous
        .as_ref()
        .map(|info| info.id.clone())
        .unwrap_or_default();

    let active_unusable = !status.active.usable && status.active.status != "Not created";
    let active_no_fingerprint = status.active.fingerprint.is_empty();
    let active = info_card(
        "ca-active",
        "Active authority",
        status.active.clone(),
        view()
            .flex_row()
            .items_center()
            .flex_wrap()
            .gap(tokens::space(1.5))
            .child(ui::action_button(
                "Trust local CA",
                Tone::Primary,
                "ca-active-trust",
                move || proxy::busy() || active_unusable,
                proxy::trust_active,
            ))
            .child(copy_button(
                "ca-active-copy",
                status.active.fingerprint.clone(),
            ))
            .child(ui::action_button(
                "Remove CA trust",
                Tone::Default,
                "ca-active-remove",
                move || proxy::busy() || active_no_fingerprint,
                proxy::remove_active_trust,
            )),
    );

    let repair = ui::card(
        "Replace or repair",
        view()
            .flex_row()
            .items_center()
            .flex_wrap()
            .gap(tokens::space(1.5))
            .child(ui::action_button(
                "Prepare replacement CA",
                Tone::Default,
                "ca-prepare",
                move || proxy::busy() || has_prepared,
                proxy::prepare_authority,
            ))
            .child(ui::action_button(
                "Apply proxy",
                Tone::Default,
                "ca-apply-proxy",
                proxy::busy,
                proxy::authority_apply_proxy,
            ))
            .child(ui::action_button(
                "Stop HTTPS proxy",
                Tone::Default,
                "ca-stop-proxy",
                proxy::busy,
                proxy::authority_stop_proxy,
            )),
    );

    let prepared_card = prepared.clone().map(|info| {
        let prepared_is_active = status.prepared_is_active;
        let label = if prepared_is_active {
            "Finish CA activation"
        } else {
            "Activate prepared CA"
        };
        let trusted = info.trust == devproxy::ca::material::TrustState::Trusted;
        let usable = info.usable;
        info_card(
            "ca-prepared",
            "Prepared replacement",
            info.clone(),
            view()
                .flex_row()
                .items_center()
                .flex_wrap()
                .gap(tokens::space(1.5))
                .child(ui::action_button(
                    "Trust prepared CA",
                    Tone::Primary,
                    "ca-prepared-trust",
                    move || proxy::busy() || !usable,
                    proxy::trust_prepared,
                ))
                .child(ui::action_button(
                    label,
                    Tone::Primary,
                    "ca-prepared-activate",
                    move || proxy::busy() || !usable || (!prepared_is_active && !trusted),
                    proxy::activate_authority,
                ))
                .child(copy_button("ca-prepared-copy", info.fingerprint.clone()))
                .child(ui::action_button(
                    "Discard prepared CA",
                    Tone::Danger,
                    "ca-prepared-discard",
                    proxy::busy,
                    proxy::discard_prepared,
                )),
        )
    });

    let archives: Vec<AnyElement> = status
        .archives
        .iter()
        .filter(|info| !info.fingerprint.is_empty())
        .map(|info| archive_card(info, &previous_id, has_prepared))
        .collect();

    let runtime_trust = runtime_trust_card(&status.active);

    view()
        .flex_col()
        .gap(tokens::space(4.0))
        .child(active)
        .child(runtime_trust)
        .child(repair)
        .child(prepared_card)
        .child(archives)
        .into_element()
}

fn runtime_trust_card(active: &AuthorityInfo) -> impl IntoElement {
    let cert_path = proxy::runtime_ca_cert_path(active);
    let dotenv_lines = proxy::ca_dotenv_lines(&cert_path);
    let shell_cert_path = cert_path.clone();
    let dotenv_cert_path = cert_path.clone();
    let not_created = active.status == "Not created";

    ui::card(
        "CLI & Runtime Trust",
        view()
            .test_id("ca-runtime-trust")
            .flex_col()
            .gap(tokens::space(2.5))
            .child(when(not_created, || {
                view()
                    .flex_row()
                    .items_center()
                    .child(ui::tag("Not created", TagTone::Warning))
            }))
            .child(
                view()
                    .flex_col()
                    .gap(tokens::space(1.0))
                    .child(ui::field_label("Certificate"))
                    .child(ui::inset(ui::mono_value("ca-runtime-cert-path", cert_path))),
            )
            .child(
                view()
                    .flex_col()
                    .gap(tokens::space(1.0))
                    .child(ui::field_label("Environment exports"))
                    .child(ui::inset(ui::mono_value(
                        "ca-runtime-exports",
                        dotenv_lines,
                    ))),
            )
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .flex_wrap()
                    .gap(tokens::space(1.5))
                    .child(ui::action_button(
                        "Copy shell exports",
                        Tone::Default,
                        "ca-copy-shell-exports",
                        false,
                        move || proxy::copy_ca_shell_exports(&shell_cert_path),
                    ))
                    .child(ui::action_button(
                        "Copy .env lines",
                        Tone::Default,
                        "ca-copy-dotenv",
                        false,
                        move || proxy::copy_ca_dotenv(&dotenv_cert_path),
                    )),
            ),
    )
}

fn info_card(
    test_id: &'static str,
    title: &'static str,
    info: AuthorityInfo,
    actions: impl IntoElement,
) -> impl IntoElement {
    let trust = info.trust;
    let detail = info.detail.clone();
    let fingerprint = info.fingerprint.clone();
    let validity = proxy::validity(&info);
    ui::card(
        title,
        view()
            .test_id(test_id)
            .flex_col()
            .gap(tokens::space(2.5))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .flex_wrap()
                    .gap(tokens::space(1.5))
                    .child(
                        text(info.name.clone())
                            .flex_1()
                            .min_w(px(160.0))
                            .text_size(px(tokens::TEXT_SMALL))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(move || theme().foreground),
                    )
                    .child(ui::status_tag(info.status.clone()))
                    .child(when(
                        trust == devproxy::ca::material::TrustState::Trusted,
                        || ui::tag("Trusted", TagTone::Success),
                    ))
                    .child(when(
                        trust != devproxy::ca::material::TrustState::Trusted
                            && trust != devproxy::ca::material::TrustState::NotChecked,
                        move || ui::tag(format!("Trust: {}", trust.as_str()), TagTone::Warning),
                    )),
            )
            .child(when(!detail.is_empty(), move || {
                text(detail.clone())
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || theme().muted_foreground)
                    .selectable(true)
            }))
            .child(when(!fingerprint.is_empty(), move || {
                view()
                    .flex_col()
                    .gap(tokens::space(1.0))
                    .child(ui::field_label("SHA-256"))
                    .child(ui::inset(ui::mono_value(
                        format!("{test_id}-fingerprint"),
                        proxy::split_fingerprint(&fingerprint),
                    )))
                    .child(
                        text(validity.clone())
                            .text_size(px(tokens::TEXT_CAPTION))
                            .tabular_nums()
                            .text_color(move || theme().muted_foreground),
                    )
            }))
            .child(actions),
    )
}

fn copy_button(test_id: &'static str, fingerprint: String) -> impl IntoElement {
    let disabled = fingerprint.is_empty();
    ui::action_button(
        "Copy fingerprint",
        Tone::Default,
        test_id,
        disabled,
        move || {
            set_clipboard(fingerprint.clone());
        },
    )
}

fn archive_card(info: &AuthorityInfo, previous_id: &str, prepared: bool) -> AnyElement {
    let short = proxy::short_id(&info.id);
    let title = if info.id == previous_id {
        format!("Previous authority · {short}")
    } else {
        format!("Saved authority · {short}")
    };
    let id_use = info.id.clone();
    let fingerprint_use = info.fingerprint.clone();
    let id_trust = info.id.clone();
    let fingerprint_trust = info.fingerprint.clone();
    let id_remove = info.id.clone();
    let fingerprint_remove = info.fingerprint.clone();
    let usable = info.usable;
    ui::card(
        title,
        view()
            .test_id(&format!("ca-archive-{short}"))
            .flex_col()
            .gap(tokens::space(2.0))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .flex_wrap()
                    .gap(tokens::space(1.5))
                    .child(ui::status_tag(info.status.clone()))
                    .child(when(
                        info.trust == devproxy::ca::material::TrustState::Trusted,
                        || ui::tag("Trusted", TagTone::Success),
                    ))
                    .child(when(
                        info.trust != devproxy::ca::material::TrustState::Trusted
                            && info.trust != devproxy::ca::material::TrustState::NotChecked,
                        {
                            let trust = info.trust;
                            move || ui::tag(format!("Trust: {}", trust.as_str()), TagTone::Warning)
                        },
                    )),
            )
            .child(ui::mono_value(
                format!("ca-archive-{short}-fingerprint"),
                proxy::split_fingerprint(&info.fingerprint),
            ))
            .child(
                text(proxy::validity(info))
                    .text_size(px(tokens::TEXT_CAPTION))
                    .tabular_nums()
                    .text_color(move || theme().muted_foreground),
            )
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .flex_wrap()
                    .gap(tokens::space(1.5))
                    .child(ui::action_button(
                        "Use this CA",
                        Tone::Primary,
                        format!("ca-archive-use-{short}"),
                        move || proxy::busy() || !usable || prepared,
                        move || proxy::restore_authority(id_use.clone(), fingerprint_use.clone()),
                    ))
                    .child(ui::action_button(
                        "Trust",
                        Tone::Default,
                        format!("ca-archive-trust-{short}"),
                        move || proxy::busy() || !usable,
                        move || proxy::trust_saved(id_trust.clone(), fingerprint_trust.clone()),
                    ))
                    .child(ui::action_button(
                        "Remove trust",
                        Tone::Default,
                        format!("ca-archive-remove-trust-{short}"),
                        proxy::busy,
                        move || {
                            proxy::remove_saved_trust(id_remove.clone(), fingerprint_remove.clone())
                        },
                    )),
            ),
    )
    .into_element()
}

//! The Local certificates page: the active authority, the replacement flow
//! and every saved generation (`view_authority.go` `devAuthority`).
//!
//! The Go view asks macOS for permission inside keychain trust changes; the
//! screen itself only enables a trust action when its certificate is usable.

use devproxy::ca::material::AuthorityInfo;
use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{
    components::{icons as app_icons, note},
    store::proxy,
    theme as tokens,
};

use super::{
    icons,
    proxy::back_to_rules,
    ui::{self, TagTone, Tone},
};

pub(crate) fn authority_page() -> impl IntoElement {
    view()
        .test_id("ca-page")
        .flex_1()
        .min_w(px(0.0))
        .child(dynamic(|| Some(page_body())))
}

fn page_body() -> impl IntoElement {
    let busy = proxy::busy();
    view()
        .flex_1()
        .min_w(px(0.0))
        .flex_col()
        .child(
            view()
                .px(tokens::space(5.0))
                .pt(tokens::space(2.0))
                .child(ui::action_button(
                    "Back to HTTPS rules",
                    Tone::Default,
                    "ca-back",
                    false,
                    back_to_rules,
                )),
        )
        .child(header())
        .child(notes())
        .child(
            scroll().flex_1().min_h(px(0.0)).child(
                view()
                    .flex_col()
                    .gap(tokens::space(4.0))
                    .px(tokens::space(5.0))
                    .pb(tokens::space(5.0))
                    .child(dynamic(move || Some(cards(busy)))),
            ),
        )
}

fn header() -> impl IntoElement {
    let busy = proxy::busy();
    view()
        .h(px(60.0))
        .flex_none()
        .flex_row()
        .items_center()
        .gap(tokens::space(2.0))
        .px(tokens::space(5.0))
        .pt(tokens::space(2.0))
        .child(
            icons::shield()
                .size(px(16.0))
                .flex_none()
                .text_color(move || theme().muted_foreground),
        )
        .child(
            view()
                .flex_col()
                .gap(tokens::space(0.5))
                .flex_1()
                .min_w(px(0.0))
                .child(
                    text("Local certificates")
                        .text_size(px(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || theme().foreground),
                )
                .child(
                    text("The authority the HTTPS proxy signs with")
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground),
                ),
        )
        .child(ui::action_button(
            "Refresh certificate status",
            Tone::Default,
            "ca-refresh",
            busy,
            proxy::refresh_authority,
        ))
}

fn notes() -> impl IntoElement {
    view()
        .flex_col()
        .flex_none()
        .child(dynamic(move || {
            proxy::action_error().map(|message| note(app_icons::warn(), message, true))
        }))
        .child(dynamic(move || {
            proxy::notice().map(|message| note(icons::check(), message, false))
        }))
}

fn cards(busy: bool) -> impl IntoElement {
    let Some(status) = proxy::authority() else {
        return view().into_element();
    };
    let prepared = status.prepared.clone();
    let previous_id = status
        .previous
        .as_ref()
        .map(|info| info.id.clone())
        .unwrap_or_default();

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
                busy || (!status.active.usable && status.active.status != "Not created"),
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
                busy || status.active.fingerprint.is_empty(),
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
                busy || prepared.is_some(),
                proxy::prepare_authority,
            ))
            .child(ui::action_button(
                "Apply proxy",
                Tone::Default,
                "ca-apply-proxy",
                busy,
                proxy::authority_apply_proxy,
            ))
            .child(ui::action_button(
                "Stop HTTPS proxy",
                Tone::Default,
                "ca-stop-proxy",
                busy,
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
                    busy || !info.usable,
                    proxy::trust_prepared,
                ))
                .child(ui::action_button(
                    label,
                    Tone::Primary,
                    "ca-prepared-activate",
                    busy || !info.usable || (!prepared_is_active && !trusted),
                    proxy::activate_authority,
                ))
                .child(copy_button("ca-prepared-copy", info.fingerprint.clone()))
                .child(ui::action_button(
                    "Discard prepared CA",
                    Tone::Danger,
                    "ca-prepared-discard",
                    busy,
                    proxy::discard_prepared,
                )),
        )
    });

    let archives: Vec<AnyElement> = status
        .archives
        .iter()
        .filter(|info| !info.fingerprint.is_empty())
        .map(|info| archive_card(info, &previous_id, prepared.is_some(), busy))
        .collect();

    view()
        .flex_col()
        .gap(tokens::space(4.0))
        .child(active)
        .child(repair)
        .child(prepared_card)
        .child(archives)
        .into_element()
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
            .gap(tokens::space(2.0))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap(tokens::space(1.5))
                    .child(
                        text(info.name.clone())
                            .text_size(px(tokens::TEXT_SMALL))
                            .text_color(move || theme().muted_foreground),
                    ),
            )
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .flex_wrap()
                    .gap(tokens::space(1.5))
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
                    .gap(tokens::space(0.5))
                    .child(
                        text("SHA-256")
                            .text_size(px(tokens::TEXT_CAPTION))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(move || theme().muted_foreground),
                    )
                    .child(ui::mono_value(
                        format!("{test_id}-fingerprint"),
                        proxy::split_fingerprint(&fingerprint),
                    ))
                    .child(
                        text(validity.clone())
                            .text_size(px(tokens::TEXT_CAPTION))
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

fn archive_card(info: &AuthorityInfo, previous_id: &str, prepared: bool, busy: bool) -> AnyElement {
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
                        busy || !usable || prepared,
                        move || proxy::restore_authority(id_use.clone(), fingerprint_use.clone()),
                    ))
                    .child(ui::action_button(
                        "Trust",
                        Tone::Default,
                        format!("ca-archive-trust-{short}"),
                        busy || !usable,
                        move || proxy::trust_saved(id_trust.clone(), fingerprint_trust.clone()),
                    ))
                    .child(ui::action_button(
                        "Remove trust",
                        Tone::Default,
                        format!("ca-archive-remove-trust-{short}"),
                        busy,
                        move || {
                            proxy::remove_saved_trust(id_remove.clone(), fingerprint_remove.clone())
                        },
                    )),
            ),
    )
    .into_element()
}

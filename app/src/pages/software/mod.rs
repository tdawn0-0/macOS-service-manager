//! Software: the runtimes and database engines Devbase manages, with the
//! versions installed, the catalog's versions, the review of one before it is
//! installed, its file checks and the download cache (Phase 2).
//!
//! The page reads [`crate::store::software`] and never calls devcore itself:
//! a read, a review and every install, uninstall, check or repair is a task
//! that writes the store when it answers.

mod detail;
mod icons;

use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{
    components::icons as app_icons,
    store::{
        Section,
        software::{self as sw, SoftwareKind},
    },
    theme as tokens,
};

pub(crate) fn software() -> impl IntoElement {
    // The page is made once; reading the development home is a task, not a
    // binding or render work.
    sw::load();

    view()
        .flex_1()
        .flex_row()
        .min_w(px(0.0))
        .child(rail())
        .child(detail::detail_pane())
}

/// The runtimes and engines, grouped as the Go master list grouped them.
fn rail() -> impl IntoElement {
    view()
        .w(px(188.0))
        .flex_none()
        .flex_col()
        .bg(move || theme().background)
        .border_r_1()
        .border_color(tokens::hairline)
        .px(tokens::space(2.0))
        .py(tokens::space(3.0))
        .child(
            view()
                .flex_col()
                .gap(tokens::space(0.5))
                .px(tokens::space(2.0))
                .pb(tokens::space(3.0))
                .child(
                    text("Software")
                        .text_base()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(move || theme().foreground),
                )
                .child(
                    text(Section::Development.summary())
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground)
                        .number_of_lines(2),
                ),
        )
        .child(
            view()
                .flex_col()
                .gap(tokens::space(0.5))
                .child(group("Runtimes", SoftwareKind::RUNTIMES))
                .child(
                    view()
                        .flex_col()
                        .gap(tokens::space(0.5))
                        .pt(tokens::space(3.0))
                        .child(group("Database engines", SoftwareKind::DATABASES)),
                ),
        )
        .child(view().flex_1())
        .child(
            view().px(tokens::space(2.0)).pt(tokens::space(3.0)).child(
                text("Downloads are verified before they are installed")
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || theme().muted_foreground)
                    .number_of_lines(2),
            ),
        )
}

fn group<const N: usize>(title: &'static str, kinds: [SoftwareKind; N]) -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(0.5))
        .child(
            text(title)
                .px(tokens::space(2.0))
                .pb(tokens::space(1.0))
                .text_size(px(tokens::TEXT_CAPTION))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().muted_foreground),
        )
        .child(kinds.into_iter().map(software_row))
}

fn software_row(kind: SoftwareKind) -> impl IntoElement {
    nsh_base::button()
        .test_id(&format!("software-{}", kind.key()))
        .w_full()
        .flex_row()
        .items_center()
        .justify_start()
        .gap(tokens::space(2.0))
        .px(tokens::space(2.0))
        .py(tokens::space(1.5))
        .rounded(move || theme().radius)
        .bg(move || {
            if sw::active_software() == kind {
                tokens::row_selected()
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .hover(move |style| style.bg(tokens::row_hover()))
        .on_press(move || sw::select_software(kind))
        .child(
            icons::software(kind)
                .size(px(14.0))
                .flex_none()
                .text_color(move || {
                    if sw::active_software() == kind {
                        theme().foreground
                    } else {
                        theme().muted_foreground
                    }
                }),
        )
        .child(
            text(kind.title())
                .flex_1()
                .min_w(px(0.0))
                .text_sm()
                .text_color(move || theme().sidebar_foreground)
                .truncate(),
        )
        .child(
            text(move || trailing(kind))
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground)
                .flex_none(),
        )
}

/// The default version, else the only installed version, else the count.
fn trailing(kind: SoftwareKind) -> String {
    if let Some(default) = sw::default_version(kind) {
        return default;
    }
    let count = sw::installed_count(kind);
    if count == 0 {
        String::new()
    } else if count == 1 {
        sw::installed(kind)
            .first()
            .map(|package| package.version.clone())
            .unwrap_or_default()
    } else {
        count.to_string()
    }
}

/// Re-exported so the detail pane names the icon set once.
pub(super) use app_icons::{check, refresh, warn};
pub(super) use icons::{folder, package};

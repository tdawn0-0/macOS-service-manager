//! The report card and the runtime snapshot card.

use nsh_base::theme;
use nsh_core::prelude::*;

use super::section_card;
use crate::{components::controls as ui, store::system, theme as tokens};

/// The summary line: `%d failed · %d warnings · %d checks`.
pub(crate) fn summary() -> String {
    let Some(report) = system::report() else {
        return String::new();
    };
    let failures = report
        .checks
        .iter()
        .filter(|check| check.status == "Failed")
        .count();
    let warnings = report
        .checks
        .iter()
        .filter(|check| check.status == "Warning")
        .count();
    let unknown = report
        .checks
        .iter()
        .filter(|check| check.status == "Unknown")
        .count();
    let mut line = format!(
        "{failures} failed · {warnings} warnings · {} checks",
        report.checks.len()
    );
    if unknown > 0 {
        // Naming unobserved checks keeps the summary from claiming a healthy environment.
        line.push_str(&format!(" · {unknown} unknown"));
    }
    line
}

/// The report's status pill. `Unknown` keeps its own honest label.
pub(crate) fn status() -> String {
    let Some(report) = system::report() else {
        return String::new();
    };
    let failures = report.checks.iter().any(|check| check.status == "Failed");
    let warnings = report.checks.iter().any(|check| check.status == "Warning");
    let unknown = report.checks.iter().any(|check| check.status == "Unknown");
    if failures {
        "Failed".to_string()
    } else if warnings {
        "Not ready".to_string()
    } else if unknown {
        "Unknown".to_string()
    } else {
        "Healthy".to_string()
    }
}

pub(crate) fn runtime_card() -> impl IntoElement {
    section_card(
        "Runtime",
        None,
        dynamic(|| Some(runtime_body().into_element())),
    )
}

fn runtime_body() -> impl IntoElement {
    let snapshot = system::runtime_snapshot();
    let loading = system::runtime_loading();
    let error = system::runtime_error();
    let empty = snapshot.is_empty();
    view()
        .flex_col()
        .children(error.map(|error| {
            view()
                .px(tokens::space(4.0))
                .py(tokens::space(2.5))
                .child(ui::plain_note(error, true))
        }))
        .child(when(empty && !loading, || {
            text("No runtime data")
                .px(tokens::space(4.0))
                .py(tokens::space(3.0))
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
        }))
        .child(when(loading, || {
            text("Loading…")
                .px(tokens::space(4.0))
                .py(tokens::space(3.0))
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
        }))
        .children(
            snapshot
                .into_iter()
                .enumerate()
                .map(|(index, (key, state))| {
                    let row = view()
                        .test_id(&format!("runtime-{key}"))
                        .min_h(px(36.0))
                        .flex_row()
                        .items_center()
                        .gap(tokens::space(2.0))
                        .px(tokens::space(4.0))
                        .py(tokens::space(2.0))
                        .child(
                            text(key)
                                .flex_1()
                                .min_w(px(0.0))
                                .font_family("monospace")
                                .text_size(px(tokens::TEXT_SMALL))
                                .text_color(move || theme().foreground),
                        )
                        .child(ui::status_tag(state));
                    if index == 0 {
                        row
                    } else {
                        row.border_t_1().border_color(tokens::hairline)
                    }
                }),
        )
}

/// The checks, one row per `DiagnosticCheck`, as devcore returns them.
pub(crate) fn checks_card() -> impl IntoElement {
    section_card(
        "Checks",
        None,
        dynamic(|| Some(checks_body().into_element())),
    )
}

fn checks_body() -> impl IntoElement {
    let report = system::report();
    let loading = system::report_loading();
    let error = system::report_error();
    let has_report = report.is_some();
    view()
        .flex_col()
        .children(error.map(|error| {
            view()
                .px(tokens::space(4.0))
                .py(tokens::space(2.5))
                .child(ui::plain_note(error, true))
        }))
        .child(when(!has_report && !loading, || {
            text("No checks yet")
                .px(tokens::space(4.0))
                .py(tokens::space(3.0))
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
        }))
        .child(when(loading, || {
            text("Running…")
                .px(tokens::space(4.0))
                .py(tokens::space(3.0))
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
        }))
        .children(report.into_iter().flat_map(|report| {
            report.checks.into_iter().enumerate().map(|(index, check)| {
                let remedy = check.remedy.clone();
                let row = view()
                    .flex_col()
                    .gap(tokens::space(1.0))
                    .px(tokens::space(4.0))
                    .py(tokens::space(2.5))
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap(tokens::space(2.0))
                            .child(ui::status_tag(format!(
                                "{} · {}",
                                check.status, check.layer
                            )))
                            .child(
                                text(check.resource.clone())
                                    .test_id(&format!("check-{index}"))
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .text_size(px(tokens::TEXT_SMALL))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(move || theme().foreground)
                                    .truncate(),
                            ),
                    )
                    .child(
                        text(check.summary.clone())
                            .text_size(px(tokens::TEXT_SMALL))
                            .text_color(move || theme().muted_foreground),
                    )
                    .child(when(!remedy.is_empty(), move || {
                        text(remedy.clone())
                            .text_size(px(tokens::TEXT_CAPTION))
                            .text_color(move || theme().muted_foreground)
                    }));
                if index == 0 {
                    row
                } else {
                    row.border_t_1().border_color(tokens::hairline)
                }
            })
        }))
}

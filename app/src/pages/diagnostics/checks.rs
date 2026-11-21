//! The report card and the runtime snapshot card, ported from the checks
//! section of `view_diagnostics.go`.

use nsh_base::theme;
use nsh_core::prelude::*;

use crate::{components::controls as ui, store::system, theme as tokens};

/// The Go summary line: `%d failed · %d warnings · %d checks`.
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
        // The Phase 3/4 probes report Unknown until they are wired; naming
        // them keeps the summary from claiming a healthy environment.
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
    ui::card("Runtime", dynamic(|| Some(runtime_body().into_element())))
}

fn runtime_body() -> impl IntoElement {
    let snapshot = system::runtime_snapshot();
    let loading = system::runtime_loading();
    let error = system::runtime_error();
    let empty = snapshot.is_empty();
    view()
        .flex_col()
        .gap(tokens::space(1.5))
        .children(error.map(|error| ui::plain_note(error, true)))
        .child(when(empty && !loading, || {
            text("No runtime observations yet.")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
        }))
        .child(when(loading, || {
            text("Reading runtime status…")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
        }))
        .children(snapshot.into_iter().map(|(key, state)| {
            view()
                .test_id(&format!("runtime-{key}"))
                .flex_row()
                .items_center()
                .gap(tokens::space(2.0))
                .child(
                    text(key)
                        .flex_1()
                        .min_w(px(0.0))
                        .font_family("monospace")
                        .text_size(px(tokens::TEXT_SMALL))
                        .text_color(move || theme().foreground),
                )
                .child(ui::status_tag(state))
        }))
}

/// The checks, one inset per `DiagnosticCheck`, exactly as devcore returns
/// them (including `Unknown` Phase 3/4 probes).
pub(crate) fn checks_card() -> impl IntoElement {
    ui::card("Checks", dynamic(|| Some(checks_body().into_element())))
}

fn checks_body() -> impl IntoElement {
    let report = system::report();
    let loading = system::report_loading();
    let error = system::report_error();
    let has_report = report.is_some();
    view()
        .flex_col()
        .gap(tokens::space(2.0))
        .children(error.map(|error| ui::plain_note(error, true)))
        .child(when(!has_report && !loading, || {
            text("No checks run yet. Run diagnostics to inspect packages and databases.")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
        }))
        .child(when(loading, || {
            text("Running diagnostics…")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
        }))
        .children(report.into_iter().flat_map(|report| {
            report.checks.into_iter().enumerate().map(|(index, check)| {
                let remedy = check.remedy.clone();
                ui::inset(
                    view()
                        .flex_col()
                        .gap(tokens::space(1.0))
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
                        })),
                )
            })
        }))
}

//! The report card and the runtime snapshot card.

use nsh_base::theme;
use nsh_core::prelude::*;

use super::{inset_hairline, section_card};
use crate::{
    components::controls::{self as ui, TagTone},
    store::system,
    theme as tokens,
};

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

/// Maps raw runtime snapshot keys (`instance:pg-main`, `proxy`, `tunnel`)
/// into human-readable labels while callers preserve the raw key on `test_id`.
fn runtime_display(key: &str) -> (String, Option<&'static str>, bool) {
    if let Some(name) = key.strip_prefix("instance:") {
        (name.to_string(), Some("Database"), false)
    } else {
        match key {
            "proxy" => ("HTTPS Proxy".to_string(), None, false),
            "tunnel" => ("Cloudflare Tunnel".to_string(), None, false),
            other => (other.to_string(), None, true),
        }
    }
}

fn check_tone(status: &str) -> TagTone {
    match status {
        "OK" | "Healthy" | "Running" | "Available" | "Succeeded" | "Installed" | "Published"
        | "Trusted" | "Ready" => TagTone::Success,
        "Warning" | "Not ready" | "Unknown" | "Stopped" | "Disabled" | "Pending"
        | "Not checked" => TagTone::Warning,
        "Failed" | "Invalid" | "Expired" => TagTone::Error,
        _ => TagTone::Neutral,
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
    let mut rows = view()
        .w_full()
        .flex_col()
        .children(error.map(|error| {
            view()
                .px(tokens::space(4.0))
                .py(tokens::space(3.0))
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
        }));

    for (index, (key, state)) in snapshot.into_iter().enumerate() {
        if index > 0 {
            rows = rows.child(inset_hairline());
        }
        let (label, category, monospace) = runtime_display(&key);
        let name_el = if monospace {
            text(label)
                .font_family("monospace")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().foreground)
                .truncate()
                .into_element()
        } else {
            text(label)
                .text_size(px(tokens::TEXT_BODY))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || theme().foreground)
                .truncate()
                .into_element()
        };
        rows = rows.child(
            view()
                .test_id(&format!("runtime-{key}"))
                .w_full()
                .min_h(px(40.0))
                .flex_row()
                .items_center()
                .gap(tokens::space(2.0))
                .px(tokens::space(4.0))
                .py(tokens::space(2.0))
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex_row()
                        .items_center()
                        .gap(tokens::space(2.0))
                        .child(name_el)
                        .children(category.map(|cat| {
                            text(cat)
                                .flex_none()
                                .text_size(px(tokens::TEXT_CAPTION))
                                .text_color(move || theme().muted_foreground)
                        })),
                )
                .child(ui::status_tag(state)),
        );
    }

    rows
}

/// The checks, one row per `DiagnosticCheck`, as devcore returns them.
pub(crate) fn checks_card() -> impl IntoElement {
    let summary_trailing = view()
        .flex_none()
        .flex_row()
        .items_center()
        .gap(tokens::space(2.0))
        .child(
            text(summary)
                .text_size(px(tokens::TEXT_CAPTION))
                .tabular_nums()
                .text_color(move || theme().muted_foreground)
                .truncate(),
        )
        .child(dynamic(|| {
            let status = status();
            (!status.is_empty()).then(|| ui::status_tag(status))
        }))
        .into_element();

    section_card(
        "Checks",
        Some(summary_trailing),
        dynamic(|| Some(checks_body().into_element())),
    )
}

fn checks_body() -> impl IntoElement {
    let report = system::report();
    let loading = system::report_loading();
    let error = system::report_error();
    let has_report = report.is_some();
    let mut rows = view()
        .w_full()
        .flex_col()
        .children(error.map(|error| {
            view()
                .px(tokens::space(4.0))
                .py(tokens::space(3.0))
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
        }));

    if let Some(report) = report {
        for (index, check) in report.checks.into_iter().enumerate() {
            if index > 0 {
                rows = rows.child(inset_hairline());
            }
            let tone = check_tone(&check.status);
            let remedy = check.remedy.clone();
            rows = rows.child(
                view()
                    .w_full()
                    .flex_col()
                    .gap(tokens::space(1.0))
                    .px(tokens::space(4.0))
                    .py(tokens::space(3.0))
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap(tokens::space(2.0))
                            .child(ui::tag(format!("{} · {}", check.status, check.layer), tone))
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
            );
        }
    }

    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_display_maps_known_keys_cleanly() {
        assert_eq!(
            runtime_display("instance:pg-main"),
            ("pg-main".to_string(), Some("Database"), false)
        );
        assert_eq!(
            runtime_display("proxy"),
            ("HTTPS Proxy".to_string(), None, false)
        );
        assert_eq!(
            runtime_display("tunnel"),
            ("Cloudflare Tunnel".to_string(), None, false)
        );
        assert_eq!(
            runtime_display("custom:svc"),
            ("custom:svc".to_string(), None, true)
        );
    }
}

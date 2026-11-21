//! The database rail: every registered instance with its engine, port and
//! state, then the new-instance form and the unfinished-work lists.

use nsh_base::theme;
use nsh_core::{
    ContextMenuEvent,
    element::desktop::{MenuItem, popup_menu_at},
    prelude::*,
};

use crate::{
    components::{controls as ui, icons},
    store::{self, Section, databases as db},
    theme as tokens,
};

pub(crate) fn rail() -> impl IntoElement {
    view()
        .w(px(208.0))
        .flex_none()
        .flex_col()
        .bg(move || theme().background)
        .border_r_1()
        .border_color(tokens::hairline)
        .child(ui::page_header_with_leading(
            None::<AnyElement>,
            "Databases",
            None::<&str>,
            Some(new_instance_button()),
        ))
        .child(
            view()
                .flex_1()
                .min_h(px(0.0))
                .flex_col()
                .px(tokens::space(2.0))
                .py(tokens::space(2.5))
                .child(when(
                    || db::running_count() > 0,
                    || {
                        view()
                            .flex_row()
                            .items_center()
                            .justify_end()
                            .pb(tokens::space(1.5))
                            .child(
                                nsh_base::button()
                                    .test_id("db-stop-all")
                                    .px(tokens::space(1.5))
                                    .py(tokens::space(0.5))
                                    .rounded(move || theme().radius)
                                    .hover(move |style| style.bg(tokens::row_hover()))
                                    .focus_visible(|style| {
                                        style.ring(px(2.0)).ring_color(move || theme().ring)
                                    })
                                    .on_press(db::stop_all_running)
                                    .child(
                                        text("Stop all")
                                            .text_size(px(tokens::TEXT_CAPTION))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(move || theme().muted_foreground),
                                    ),
                            )
                    },
                ))
                .child(dynamic(|| Some(rows())))
                .child(view().flex_1())
                .child(
                    view()
                        .flex_col()
                        .gap(tokens::space(0.5))
                        .pt(tokens::space(2.0))
                        .child(view().pb(tokens::space(1.5)).child(ui::hairline()))
                        .child(nav_row(
                            "Database tasks",
                            "db-work",
                            || db::database_work().len().to_string(),
                            || db::mode() == db::Mode::Work,
                            db::open_work,
                        ))
                        .child(nav_row(
                            "Initializations",
                            "db-initializations",
                            || db::pending_initializations().len().to_string(),
                            || db::mode() == db::Mode::Initializations,
                            db::open_initializations,
                        ))
                        .child(nav_row(
                            "Pending deletions",
                            "db-deletions",
                            || db::pending_deletions().len().to_string(),
                            || db::mode() == db::Mode::Deletions,
                            db::open_deletions,
                        )),
                ),
        )
}

fn new_instance_button() -> impl IntoElement {
    view()
        .flex_none()
        .tooltip(ui::tooltip("New instance"))
        .child(
            nsh_base::button()
                .test_id("db-new")
                .size(px(26.0))
                .flex_none()
                .items_center()
                .justify_center()
                .rounded(move || theme().radius)
                .selected(|| db::mode() == db::Mode::New)
                .bg(move || {
                    if db::mode() == db::Mode::New {
                        tokens::row_selected()
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .hover(move |style| style.bg(tokens::row_hover()))
                .focus_visible(|style| style.ring(px(2.0)).ring_color(move || theme().ring))
                .accessibility_label("New instance")
                .on_press(db::open_new)
                .child(icons::plus().size(px(14.0)).text_color(move || {
                    if db::mode() == db::Mode::New {
                        theme().foreground
                    } else {
                        theme().muted_foreground
                    }
                })),
        )
}

/// The registered instances, read from the store each time they change.
fn rows() -> impl IntoElement {
    let items = db::instances();
    let empty = items.is_empty();
    view()
        .flex_col()
        .gap(tokens::space(0.5))
        .child(when(empty, || {
            text("None yet")
                .px(tokens::space(2.0))
                .py(tokens::space(1.0))
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground)
        }))
        .child(items.into_iter().map(instance_row))
}

fn instance_row(instance: devcore::model::Instance) -> impl IntoElement {
    let id = instance.id.clone();
    let kind = instance.kind.clone();
    let package = instance.package.clone();
    let selected = db::mode() == db::Mode::Instance && db::selected_id().as_deref() == Some(&id);
    let press_id = id.clone();
    let ctx_id = id.clone();
    let ctx_kind = kind.clone();
    let ctx_package = package.clone();
    let title = instance_title(&kind);
    let meta = format!("{}:{}", title, instance.port);
    let working = db::status(&id);
    let running = matches!(working.as_str(), "Running" | "Healthy");
    let active = matches!(working.as_str(), "Starting" | "Restarting" | "Stopping");
    view()
        .w_full()
        .on_context_menu(move |event: &ContextMenuEvent| {
            let items = instance_context_menu(&ctx_id, &ctx_kind, &ctx_package);
            drop(popup_menu_at(items, event.page_position));
        })
        .child(
            nsh_base::button()
                .test_id(&format!("db-instance-{id}"))
                .w_full()
                .flex_row()
                .items_center()
                .justify_start()
                .gap(tokens::space(2.0))
                .px(tokens::space(2.0))
                .py(tokens::space(1.5))
                .rounded(move || theme().radius)
                .selected(selected)
                .bg(move || {
                    if selected {
                        tokens::row_selected()
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .hover(move |style| style.bg(tokens::row_hover()))
                .focus_visible(|style| style.ring(px(2.0)).ring_color(move || theme().ring))
                .on_press(move || db::open_instance(&press_id))
                .child(
                    icons::engine(&kind)
                        .size(px(14.0))
                        .flex_none()
                        .text_color(move || {
                            if selected {
                                theme().foreground
                            } else {
                                theme().muted_foreground
                            }
                        }),
                )
                .child(
                    view()
                        .flex_col()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(
                            text(id)
                                .text_sm()
                                .text_color(move || theme().sidebar_foreground)
                                .truncate(),
                        )
                        .child(
                            text(meta)
                                .text_size(px(tokens::TEXT_CAPTION))
                                .tabular_nums()
                                .text_color(move || theme().muted_foreground)
                                .truncate(),
                        ),
                )
                .child(
                    view()
                        .size(px(8.0))
                        .flex_none()
                        .rounded(px(4.0))
                        .bg(move || {
                            if running {
                                tokens::success()
                            } else if active {
                                tokens::warning()
                            } else {
                                tokens::hairline()
                            }
                        }),
                ),
        )
}

fn instance_context_menu(id: &str, kind: &str, package: &str) -> Vec<MenuItem> {
    let busy = db::operations_busy();
    let running = db::is_running(id);
    let start_id = id.to_string();
    let stop_id = id.to_string();
    let restart_id = id.to_string();
    let copy_id = id.to_string();
    let cli_id = id.to_string();
    let log_job = format!("db-{id}");
    let remove_id = id.to_string();
    let new_kind = kind.to_string();
    let new_package = package.to_string();

    let mut items = vec![
        MenuItem::new("Start")
            .disabled(busy || running)
            .on_select(move || db::start_instance(&start_id)),
        MenuItem::new("Stop")
            .disabled(busy || !running)
            .on_select(move || db::stop_instance(&stop_id)),
        MenuItem::new("Restart")
            .disabled(busy)
            .on_select(move || db::restart_instance(&restart_id)),
        MenuItem::separator(),
        MenuItem::new("Copy URI")
            .disabled(busy)
            .on_select(move || db::copy_uri(&copy_id)),
    ];

    if kind != "mongodb" {
        items.push(
            MenuItem::new("Open CLI in Terminal")
                .disabled(busy)
                .on_select(move || db::open_cli(&cli_id)),
        );
    }

    items.push(MenuItem::new("View Logs").on_select(move || crate::dev_logs::open(&log_job)));
    items.push(
        MenuItem::new("New Instance with Same Version")
            .on_select(move || db::open_new_with_package(&new_kind, &new_package)),
    );
    items.push(MenuItem::separator());
    items.push(
        MenuItem::new("Remove…")
            .destructive(true)
            .disabled(busy)
            .on_select(move || db::request_remove_instance(&remove_id)),
    );

    items
}

pub(crate) fn instance_title(kind: &str) -> &'static str {
    match kind {
        "mysql" => "MySQL",
        "postgresql" => "PostgreSQL",
        "redis" => "Redis",
        "mongodb" => "MongoDB",
        _ => "Database",
    }
}

fn nav_row(
    label: &'static str,
    test_id: &'static str,
    count: impl Fn() -> String + 'static,
    selected: impl IntoReactive<bool> + 'static,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    let selected = selected.into_reactive();
    let selected_bg = selected.clone();
    let selected_text = selected.clone();
    nsh_base::button()
        .test_id(test_id)
        .w_full()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(tokens::space(2.0))
        .px(tokens::space(2.0))
        .py(tokens::space(1.25))
        .rounded(move || theme().radius)
        .selected(move || selected.get())
        .bg(move || {
            if selected_bg.get() {
                tokens::row_selected()
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .hover(move |style| style.bg(tokens::row_hover()))
        .focus_visible(|style| style.ring(px(2.0)).ring_color(move || theme().ring))
        .on_press(on_press)
        .child(
            text(label)
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || {
                    if selected_text.get() {
                        theme().foreground
                    } else {
                        theme().muted_foreground
                    }
                })
                .truncate(),
        )
        .child(
            text(count)
                .text_size(px(tokens::TEXT_CAPTION))
                .tabular_nums()
                .text_color(move || theme().muted_foreground),
        )
}

/// Moves the workspace to the software screen of an engine, as the Go
/// "Open Software" button does.
pub(crate) fn open_software(kind: &str) {
    if let Some(software) = crate::store::software::SoftwareKind::from_key(kind) {
        crate::store::software::select_software(software);
    }
    store::select_section(Section::Development);
}

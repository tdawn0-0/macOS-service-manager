//! Native `nsh` UI for the LaunchAgent / LaunchDaemon property-list editor,
//! ported from Go's `view_editor.go`.
//!
//! Uses idiomatic `nsh_base` controls (`input`, `switch`, `checkbox`, `button`,
//! `tabs`, `toggle_group`, `tooltip`, `dialog`) with fine-grained signal
//! bindings — no coarse `dynamic(...)` wrappers around text inputs, so focus
//! and caret position stay intact across every keystroke.

use devcore::service::Source;
use nsh_base::{
    TooltipElement, button, checkbox, dialog, input, switch, tab, tabs, theme, toggle,
    toggle_group, tooltip,
};
use nsh_core::{
    desktop::WindowKind,
    element::{AutoCapitalize, desktop},
    prelude::*,
};

use crate::theme as tokens;

use super::store::{
    self, CommandMode, JOB_KINDS, RestartPolicy, ScheduleMode, kind_label, signals,
};

/// The editor's full form panel. Directly mountable in `nsh_testing::Headless`
/// or embedded inside [`editor_window`].
pub fn editor_panel() -> impl IntoElement {
    let s = signals();

    view()
        .test_id("plist-editor")
        .flex_1()
        .flex_col()
        .min_h(px(0.0))
        .w_full()
        .p(tokens::space(4.5))
        .gap(tokens::space(3.5))
        .bg(move || theme().background)
        .text_color(move || theme().foreground)
        .child(header())
        .child(when(move || !store::is_existing(), kind_selector))
        .child(when(store::is_loading, loading_banner))
        .child(
            scroll().flex_1().min_h(px(0.0)).child(
                view()
                    .flex_col()
                    .gap(tokens::space(3.5))
                    .pr(tokens::space(1.0))
                    .child(program_section(s))
                    .child(when_section(s))
                    .child(when(
                        move || s.source.get() == Source::Daemon,
                        move || runs_as_section(s),
                    ))
                    .child(logs_section(s)),
            ),
        )
        .child(when(store::has_error, error_banner))
        .child(footer(s))
}

/// Presents the plist editor as a native modal dialog window on desktop
/// (`WindowKind::Dialog`) or an `nsh_base::dialog()` overlay off-desktop.
pub fn editor_window() -> impl IntoElement {
    if os().is_desktop() {
        desktop::window()
            .id("plist-editor")
            .kind(WindowKind::Dialog)
            .parent("main")
            .modal(true)
            .title(move || {
                if store::is_existing() {
                    "Edit Job".to_string()
                } else {
                    "New Job".to_string()
                }
            })
            .default_size(size(px(620.0), px(720.0)))
            .open(store::is_open)
            .on_open_change(|event| {
                if !event.open {
                    store::close();
                }
            })
            .content(editor_panel)
            .into_element()
    } else {
        when(store::is_open, || {
            dialog()
                .label(move || {
                    if store::is_existing() {
                        "Edit Job".to_string()
                    } else {
                        "New Job".to_string()
                    }
                })
                .on_cancel(|_: &()| store::close())
                .child(
                    view()
                        .size_full()
                        .items_center()
                        .justify_center()
                        .bg(move || theme().background.opacity(0.6))
                        .child(
                            view()
                                .w(px(620.0))
                                .max_h(px(720.0))
                                .rounded(tokens::panel_radius)
                                .bg(tokens::panel)
                                .border_1()
                                .border_color(tokens::hairline)
                                .overflow_hidden()
                                .child(editor_panel()),
                        ),
                )
        })
        .into_element()
    }
}

// --- Header & kind selector --------------------------------------------------

fn header() -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(0.75))
        .child(
            text(move || {
                if store::is_existing() {
                    "Edit Job"
                } else {
                    "New Job"
                }
            })
            .test_id("editor-title")
            .text_size(px(16.0))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(move || theme().foreground),
        )
        .child(when(store::has_subtitle, || {
            text(store::subtitle)
                .test_id("editor-subtitle")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
                .truncate()
        }))
}

fn kind_selector() -> impl IntoElement {
    let s = signals();
    tabs()
        .test_id("editor-kind-tabs")
        .accessibility_label("Job kind")
        .flex_row()
        .self_start()
        .p(px(2.0))
        .gap(px(2.0))
        .rounded(move || theme().radius)
        .bg(tokens::inset)
        .border_1()
        .border_color(tokens::hairline)
        .children(JOB_KINDS.into_iter().map(move |kind| {
            let id = format!("editor-kind-{}", kind.as_str());
            tab()
                .test_id(&id)
                .selected(move || s.source.get() == kind)
                .on_press(move |_: &()| store::set_source(kind))
                .h(px(24.0))
                .px(tokens::space(2.5))
                .items_center()
                .justify_center()
                .rounded(move || px((theme().radius.0 - 2.0).max(2.0)))
                .bg(move || {
                    if s.source.get() == kind {
                        tokens::panel()
                    } else {
                        tokens::panel().opacity(0.0)
                    }
                })
                .child(
                    text(kind_label(kind))
                        .text_size(px(tokens::TEXT_SMALL))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(move || {
                            if s.source.get() == kind {
                                theme().foreground
                            } else {
                                theme().muted_foreground
                            }
                        }),
                )
        }))
}

fn loading_banner() -> impl IntoElement {
    view()
        .px(tokens::space(3.0))
        .py(tokens::space(2.0))
        .rounded(move || theme().radius)
        .bg(tokens::inset)
        .child(
            text("Loading property list…")
                .test_id("editor-loading")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground),
        )
}

// --- Sections ----------------------------------------------------------------

fn program_section(s: store::EditorSignals) -> impl IntoElement {
    fieldset(
        "Program",
        view()
            .flex_col()
            .gap(tokens::space(2.5))
            .child(field_with_tooltip(
                "Label",
                move || {
                    if store::is_existing() {
                        "The label of an existing job does not change.".to_string()
                    } else {
                        "A reverse-domain name that is unique among your jobs.".to_string()
                    }
                },
                text_input_box(
                    "editor-label",
                    "Label",
                    move || "com.example.my-job".to_string(),
                    move || s.label.get(),
                    move |next| s.label.set(next),
                    store::is_existing,
                ),
            ))
            .child(field_static(
                "Runs",
                segmented_row(
                    "editor-command-mode",
                    "How the command runs",
                    false,
                    vec![
                        segment_item(
                            "editor-cmd-shell",
                            "Shell command",
                            move || s.command_mode.get() == CommandMode::Shell,
                            move || s.command_mode.set(CommandMode::Shell),
                        ),
                        segment_item(
                            "editor-cmd-program",
                            "Program",
                            move || s.command_mode.get() == CommandMode::Program,
                            move || s.command_mode.set(CommandMode::Program),
                        ),
                    ],
                ),
            ))
            .child(field_with_tooltip(
                "Command",
                move || match s.command_mode.get() {
                    CommandMode::Shell => {
                        "Runs with /bin/sh -c. launchd's PATH lacks Homebrew: give full paths, or set PATH below.".to_string()
                    }
                    CommandMode::Program => {
                        "One item per line: the program with its absolute path, then its arguments."
                            .to_string()
                    }
                },
                multiline_input_box(
                    "editor-args",
                    "Command",
                    px(68.0),
                    move || match s.command_mode.get() {
                        CommandMode::Shell => "/opt/homebrew/bin/redis-cli ping".to_string(),
                        CommandMode::Program => {
                            "/opt/homebrew/bin/node\n/Users/me/server.js".to_string()
                        }
                    },
                    move || s.args.get(),
                    move |next| s.args.set(next),
                ),
            ))
            .child(field_static(
                "Working directory",
                text_input_box(
                    "editor-workdir",
                    "Working directory",
                    || "Optional, as /Users/me/project".to_string(),
                    move || s.working_dir.get(),
                    move |next| s.working_dir.set(next),
                    || false,
                ),
            ))
            .child(field_static(
                "Environment",
                multiline_input_box(
                    "editor-env",
                    "Environment",
                    px(56.0),
                    || "KEY=value".to_string(),
                    move || s.env.get(),
                    move |next| s.env.set(next),
                ),
            )),
    )
}

fn when_section(s: store::EditorSignals) -> impl IntoElement {
    let at_load_label = move || {
        if s.source.get() == Source::Daemon {
            "Starts at boot".to_string()
        } else {
            "Starts at login".to_string()
        }
    };
    let also_at_load_label = move || {
        if s.source.get() == Source::Daemon {
            "Also starts at boot".to_string()
        } else {
            "Also starts at login".to_string()
        }
    };

    fieldset(
        "When it runs",
        view()
            .flex_col()
            .gap(tokens::space(2.5))
            .child(field_static(
                "Starts",
                segmented_row_reactive(
                    "editor-schedule-mode",
                    "What starts the job",
                    move || s.calendar_custom.get(),
                    vec![
                        segment_item(
                            "editor-when-service",
                            "As a service",
                            move || s.schedule_mode.get() == ScheduleMode::Service,
                            || store::set_schedule_mode(ScheduleMode::Service),
                        ),
                        segment_item(
                            "editor-when-interval",
                            "At an interval",
                            move || s.schedule_mode.get() == ScheduleMode::Interval,
                            || store::set_schedule_mode(ScheduleMode::Interval),
                        ),
                        segment_item(
                            "editor-when-calendar",
                            "On a calendar",
                            move || s.schedule_mode.get() == ScheduleMode::Calendar,
                            || store::set_schedule_mode(ScheduleMode::Calendar),
                        ),
                    ],
                ),
            ))
            .child(when(
                move || s.schedule_mode.get() == ScheduleMode::Service,
                move || {
                    view()
                        .flex_col()
                        .gap(tokens::space(2.5))
                        .child(switch_row(
                            "editor-run-at-load",
                            at_load_label,
                            move || {
                                s.run_at_load.get()
                                    || s.restart.get() == RestartPolicy::OnCrash
                            },
                            move || s.restart.get() == RestartPolicy::OnCrash,
                            move |next| s.run_at_load.set(next),
                        ))
                        .child(field_with_tooltip(
                            "Restarts",
                            move || match s.restart.get() {
                                RestartPolicy::Never => {
                                    "launchd leaves the job stopped once it exits.".to_string()
                                }
                                RestartPolicy::OnCrash => {
                                    "Starts it again if it exits with a failure, and so also starts it at load."
                                        .to_string()
                                }
                                RestartPolicy::Always => {
                                    "Starts it again whenever it exits.".to_string()
                                }
                            },
                            when(
                                move || s.keep_alive_custom.get(),
                                || {
                                    text("By conditions of its own")
                                        .test_id("editor-keep-alive-custom")
                                        .text_size(px(tokens::TEXT_SMALL))
                                        .text_color(move || theme().muted_foreground)
                                },
                            )
                            .otherwise(move || {
                                segmented_row(
                                    "editor-restart-policy",
                                    "Restarts",
                                    false,
                                    vec![
                                        segment_item(
                                            "editor-restart-never",
                                            "Never",
                                            move || s.restart.get() == RestartPolicy::Never,
                                            move || s.restart.set(RestartPolicy::Never),
                                        ),
                                        segment_item(
                                            "editor-restart-crash",
                                            "After a crash",
                                            move || s.restart.get() == RestartPolicy::OnCrash,
                                            move || s.restart.set(RestartPolicy::OnCrash),
                                        ),
                                        segment_item(
                                            "editor-restart-always",
                                            "Always",
                                            move || s.restart.get() == RestartPolicy::Always,
                                            move || s.restart.set(RestartPolicy::Always),
                                        ),
                                    ],
                                )
                            }),
                        ))
                        .child(switch_row(
                            "editor-start-on-mount",
                            || "Starts when a disk mounts".to_string(),
                            move || s.start_on_mount.get(),
                            || false,
                            move |next| s.start_on_mount.set(next),
                        ))
                        .child(field_with_tooltip(
                            "Queue directories",
                            || {
                                "Runs the job while any of these is not empty. Separate them with commas."
                                    .to_string()
                            },
                            text_input_box(
                                "editor-queue-dirs",
                                "Queue directories",
                                || "Optional, as /Users/me/inbox, /Users/me/outbox".to_string(),
                                move || s.queue_dirs.get(),
                                move |next| s.queue_dirs.set(next),
                                || false,
                            ),
                        ))
                },
            ))
            .child(when(
                move || s.schedule_mode.get() == ScheduleMode::Interval,
                move || {
                    view()
                        .flex_col()
                        .gap(tokens::space(2.5))
                        .child(field_static(
                            "Run every (seconds)",
                            text_input_box(
                                "editor-interval",
                                "Run every (seconds)",
                                || "3600".to_string(),
                                move || s.interval.get(),
                                move |next| s.interval.set(next),
                                || false,
                            ),
                        ))
                        .child(switch_row(
                            "editor-interval-run-at-load",
                            also_at_load_label,
                            move || s.run_at_load.get(),
                            || false,
                            move |next| s.run_at_load.set(next),
                        ))
                },
            ))
            .child(when(
                move || s.schedule_mode.get() == ScheduleMode::Calendar,
                move || {
                    view()
                        .flex_col()
                        .gap(tokens::space(2.5))
                        .child(when(
                            move || s.calendar_custom.get(),
                            || {
                                field_static(
                                    "Calendar",
                                    text("A calendar of its own, which stays as it is")
                                        .test_id("editor-calendar-custom")
                                        .text_size(px(tokens::TEXT_SMALL))
                                        .text_color(move || theme().muted_foreground),
                                )
                            },
                        )
                        .otherwise(move || {
                            field_with_tooltip(
                                "Calendar",
                                || {
                                    "A number, a list (1,15), a range (1-5) or an interval (*/10). An empty field is any value."
                                        .to_string()
                                },
                                calendar_fields(s),
                            )
                        }))
                        .child(switch_row(
                            "editor-calendar-run-at-load",
                            also_at_load_label,
                            move || s.run_at_load.get(),
                            || false,
                            move |next| s.run_at_load.set(next),
                        ))
                },
            )),
    )
}

fn calendar_fields(s: store::EditorSignals) -> impl IntoElement {
    view()
        .flex_row()
        .gap(tokens::space(2.0))
        .child(calendar_cell(
            "editor-cal-minute",
            "Minute",
            "0-59",
            s.cal_minute,
        ))
        .child(calendar_cell("editor-cal-hour", "Hour", "0-23", s.cal_hour))
        .child(calendar_cell("editor-cal-day", "Day", "1-31", s.cal_day))
        .child(calendar_cell(
            "editor-cal-month",
            "Month",
            "1-12",
            s.cal_month,
        ))
        .child(calendar_cell(
            "editor-cal-weekday",
            "Weekday",
            "0-6, Sun 0",
            s.cal_weekday,
        ))
}

fn calendar_cell(
    test_id: &'static str,
    label: &'static str,
    hint: &'static str,
    sig: Signal<String>,
) -> impl IntoElement {
    view()
        .flex_1()
        .min_w(px(0.0))
        .flex_col()
        .gap(px(3.0))
        .tooltip(tooltip_bubble(move || hint.to_string()))
        .child(
            text(label)
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground)
                .truncate(),
        )
        .child(text_input_box(
            test_id,
            label,
            || "*".to_string(),
            move || sig.get(),
            move |next| sig.set(next),
            || false,
        ))
}

fn runs_as_section(s: store::EditorSignals) -> impl IntoElement {
    fieldset(
        "Runs as",
        view()
            .flex_col()
            .gap(tokens::space(2.5))
            .child(field_with_tooltip(
                "User",
                || "Without one, the job runs as root.".to_string(),
                text_input_box(
                    "editor-user",
                    "User",
                    || "root".to_string(),
                    move || s.user_name.get(),
                    move |next| s.user_name.set(next),
                    || false,
                ),
            ))
            .child(field_static(
                "Group",
                text_input_box(
                    "editor-group",
                    "Group",
                    || "Optional, with a user".to_string(),
                    move || s.group_name.get(),
                    move |next| s.group_name.set(next),
                    || false,
                ),
            ))
            .child(field_with_tooltip(
                "Root directory",
                || {
                    "Runs the job chrooted into it, where the command must be found too."
                        .to_string()
                },
                text_input_box(
                    "editor-root-dir",
                    "Root directory",
                    || "Optional absolute path".to_string(),
                    move || s.root_dir.get(),
                    move |next| s.root_dir.set(next),
                    || false,
                ),
            )),
    )
}

fn logs_section(s: store::EditorSignals) -> impl IntoElement {
    fieldset(
        "Logs",
        view()
            .flex_col()
            .gap(tokens::space(2.5))
            .child(field_static(
                "Output log",
                text_input_box(
                    "editor-stdout",
                    "Output log",
                    || "Optional absolute path".to_string(),
                    move || s.stdout.get(),
                    move |next| s.stdout.set(next),
                    || false,
                ),
            ))
            .child(field_static(
                "Error log",
                text_input_box(
                    "editor-stderr",
                    "Error log",
                    || "Optional absolute path".to_string(),
                    move || s.stderr.get(),
                    move |next| s.stderr.set(next),
                    || false,
                ),
            ))
            .child(when(store::has_log_default_option, move || {
                checkbox_row(
                    "editor-log-default",
                    move || {
                        let path = store::log_default_preview_path().unwrap_or_default();
                        format!("Write to {path} if no log file is set")
                    },
                    move || s.log_default.get(),
                    move |next| s.log_default.set(next),
                )
            })),
    )
}

// --- Error & Footer ----------------------------------------------------------

fn error_banner() -> impl IntoElement {
    view()
        .px(tokens::space(3.0))
        .py(tokens::space(2.0))
        .rounded(move || theme().radius)
        .bg(move || theme().destructive.opacity(0.1))
        .child(
            text(move || store::error().unwrap_or_default())
                .test_id("editor-error")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().destructive),
        )
}

fn footer(s: store::EditorSignals) -> impl IntoElement {
    let disabled = move || store::is_saving() || store::is_loading();
    view()
        .flex_row()
        .items_center()
        .gap(tokens::space(2.0))
        .pt(tokens::space(1.0))
        .child(checkbox_row(
            "editor-apply",
            move || {
                if store::is_existing() {
                    "Reload it after saving".to_string()
                } else {
                    "Load it now".to_string()
                }
            },
            move || s.apply.get(),
            move |next| s.apply.set(next),
        ))
        .child(view().flex_1())
        .child(action_button(
            "editor-cancel",
            "Cancel",
            false,
            || false,
            store::close,
        ))
        .child(action_button(
            "editor-save",
            "Save",
            true,
            disabled,
            store::save,
        ))
}

// --- Shared field & control helpers ------------------------------------------

fn fieldset(title: &'static str, body: impl IntoElement) -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(2.0))
        .p(tokens::space(3.5))
        .rounded(tokens::panel_radius)
        .bg(tokens::panel)
        .border_1()
        .border_color(tokens::hairline)
        .child(
            text(title)
                .text_size(px(tokens::TEXT_SMALL))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().foreground),
        )
        .child(body)
}

fn field_static(label: &'static str, control: impl IntoElement) -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(1.0))
        .min_w(px(0.0))
        .child(
            text(label)
                .text_size(px(tokens::TEXT_CAPTION))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || theme().muted_foreground),
        )
        .child(control)
}

fn field_with_tooltip(
    label: &'static str,
    tip: impl Fn() -> String + 'static,
    control: impl IntoElement,
) -> impl IntoElement {
    view()
        .flex_col()
        .gap(tokens::space(1.0))
        .min_w(px(0.0))
        .tooltip(tooltip_bubble(tip))
        .child(
            text(label)
                .text_size(px(tokens::TEXT_CAPTION))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || theme().muted_foreground),
        )
        .child(control)
}

fn tooltip_bubble(tip: impl Fn() -> String + 'static) -> TooltipElement {
    tooltip()
        .px(tokens::space(2.0))
        .py(tokens::space(1.0))
        .rounded(move || theme().radius)
        .bg(move || theme().popover)
        .border_1()
        .border_color(tokens::hairline)
        .child(
            text(tip)
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().popover_foreground),
        )
}

fn text_input_box(
    test_id: &'static str,
    label: &'static str,
    placeholder: impl Fn() -> String + 'static,
    value: impl Fn() -> String + 'static,
    on_change: impl Fn(String) + 'static,
    readonly: impl Fn() -> bool + 'static,
) -> impl IntoElement {
    view()
        .h(px(28.0))
        .flex_row()
        .items_center()
        .rounded(move || theme().radius)
        .bg(tokens::control)
        .border_1()
        .border_color(tokens::hairline)
        .child(
            input()
                .test_id(test_id)
                .label(label)
                .flex_1()
                .min_w(px(0.0))
                .h(px(26.0))
                .px(tokens::space(2.0))
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().foreground)
                .placeholder(placeholder)
                .value(value)
                .readonly(readonly)
                .auto_correct(false)
                .auto_capitalize(AutoCapitalize::None)
                .on_change(move |next: &str| on_change(next.to_owned())),
        )
}

fn multiline_input_box(
    test_id: &'static str,
    label: &'static str,
    height: Points,
    placeholder: impl Fn() -> String + 'static,
    value: impl Fn() -> String + 'static,
    on_change: impl Fn(String) + 'static,
) -> impl IntoElement {
    view()
        .min_h(height)
        .flex_row()
        .items_stretch()
        .rounded(move || theme().radius)
        .bg(tokens::control)
        .border_1()
        .border_color(tokens::hairline)
        .child(
            input()
                .test_id(test_id)
                .label(label)
                .flex_1()
                .min_w(px(0.0))
                .min_h(height)
                .px(tokens::space(2.0))
                .py(tokens::space(1.5))
                .multiline(true)
                .font_family("monospace")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().foreground)
                .placeholder(placeholder)
                .value(value)
                .auto_correct(false)
                .auto_capitalize(AutoCapitalize::None)
                .on_change(move |next: &str| on_change(next.to_owned())),
        )
}

fn segment_item(
    test_id: &'static str,
    label: &'static str,
    active: impl Fn() -> bool + Copy + 'static,
    on_select: impl Fn() + 'static,
) -> AnyElement {
    toggle()
        .test_id(test_id)
        .pressed(active)
        .on_change(move |_: &bool| on_select())
        .h(px(24.0))
        .px(tokens::space(2.5))
        .items_center()
        .justify_center()
        .rounded(move || px((theme().radius.0 - 2.0).max(2.0)))
        .bg(move || {
            if active() {
                tokens::panel()
            } else {
                tokens::panel().opacity(0.0)
            }
        })
        .child(
            text(label)
                .text_size(px(tokens::TEXT_SMALL))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || {
                    if active() {
                        theme().foreground
                    } else {
                        theme().muted_foreground
                    }
                }),
        )
        .into_element()
}

fn segmented_row(
    test_id: &'static str,
    label: &'static str,
    disabled: bool,
    items: Vec<AnyElement>,
) -> impl IntoElement {
    segmented_row_reactive(test_id, label, move || disabled, items)
}

fn segmented_row_reactive(
    test_id: &'static str,
    _label: &'static str,
    disabled: impl Fn() -> bool + Copy + 'static,
    items: Vec<AnyElement>,
) -> impl IntoElement {
    toggle_group()
        .test_id(test_id)
        .disabled(disabled)
        .flex_row()
        .self_start()
        .p(px(2.0))
        .gap(px(2.0))
        .rounded(move || theme().radius)
        .bg(tokens::inset)
        .border_1()
        .border_color(tokens::hairline)
        .opacity(move || if disabled() { 0.5_f32 } else { 1.0_f32 })
        .children(items)
}

fn switch_row(
    test_id: &'static str,
    label: impl Fn() -> String + 'static,
    checked: impl Fn() -> bool + Copy + 'static,
    disabled: impl Fn() -> bool + Copy + 'static,
    on_change: impl Fn(bool) + 'static,
) -> impl IntoElement {
    view()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(tokens::space(2.0))
        .child(
            text(label)
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().foreground),
        )
        .child(
            switch()
                .test_id(test_id)
                .checked(checked)
                .disabled(disabled)
                .on_change(move |next: &bool| on_change(*next))
                .opacity(move || if disabled() { 0.5_f32 } else { 1.0_f32 })
                .track_style(move |s| {
                    s.w(px(30.0))
                        .h(px(18.0))
                        .flex_row()
                        .items_center()
                        .px(px(2.0))
                        .rounded(px(9.0))
                        .bg(move || {
                            if checked() {
                                theme().primary
                            } else {
                                theme().secondary
                            }
                        })
                })
                .thumb_style(move |s| {
                    s.size(px(14.0))
                        .rounded(px(7.0))
                        .bg(move || theme().primary_foreground)
                        .ml(move || if checked() { px(12.0) } else { px(0.0) })
                }),
        )
}

fn checkbox_row(
    test_id: &'static str,
    label: impl Fn() -> String + 'static,
    checked: impl Fn() -> bool + Copy + 'static,
    on_change: impl Fn(bool) + 'static,
) -> impl IntoElement {
    checkbox()
        .test_id(test_id)
        .checked(checked)
        .on_change(move |next: &bool| on_change(*next))
        .flex_row()
        .items_center()
        .gap(tokens::space(2.0))
        .indicator_style(move |s| {
            s.size(px(16.0))
                .items_center()
                .justify_center()
                .rounded(px(4.0))
                .border_1()
                .border_color(move || {
                    if checked() {
                        theme().primary
                    } else {
                        tokens::hairline()
                    }
                })
                .bg(move || {
                    if checked() {
                        theme().primary
                    } else {
                        tokens::control()
                    }
                })
        })
        .indicator(view().child(when(checked, || {
            text("✓")
                .text_size(px(10.0))
                .font_weight(FontWeight::BOLD)
                .text_color(move || theme().primary_foreground)
        })))
        .child(
            text(label)
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().foreground),
        )
}

fn action_button(
    test_id: &'static str,
    label: &'static str,
    primary: bool,
    disabled: impl Fn() -> bool + Copy + 'static,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    button()
        .test_id(test_id)
        .h(px(28.0))
        .px(tokens::space(3.0))
        .items_center()
        .justify_center()
        .rounded(move || theme().radius)
        .bg(move || {
            if primary {
                theme().primary
            } else {
                tokens::control()
            }
        })
        .border_1()
        .border_color(move || {
            if primary {
                theme().primary
            } else {
                tokens::hairline()
            }
        })
        .disabled(disabled)
        .opacity(move || if disabled() { 0.5_f32 } else { 1.0_f32 })
        .on_press(on_press)
        .child(
            text(label)
                .text_size(px(tokens::TEXT_SMALL))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || {
                    if primary {
                        theme().primary_foreground
                    } else {
                        theme().foreground
                    }
                }),
        )
}

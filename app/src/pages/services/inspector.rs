//! The inspector: what the chosen item is doing, what can be done to it,
//! whether it starts by itself, and its values and logs.

use std::path::PathBuf;

use devcore::service::{Action, Autostart, Item, LogFile, Row, Source, State};
use nsh_base::{menu, menu_item, menu_separator, theme};
use nsh_core::prelude::*;

use super::hint_tooltip;
use crate::{
    components::{icon_button, icons, labeled_button, state_color, status_dot},
    store::services::{self as svc, DetailTab, LogBodyMode},
    theme as tokens,
};

/// The right-hand pane; the caller shows it only while an item is chosen.
pub(crate) fn inspector() -> impl IntoElement {
    let selected_id = computed(svc::selected_id);
    view()
        .test_id("inspector")
        .w(px(336.0))
        .flex_none()
        .flex_col()
        .bg(tokens::sheet)
        .border_l_1()
        .border_color(tokens::hairline)
        .child(bar())
        .child(dynamic(move || {
            let id = selected_id.get()?;
            Some(untracked(|| item_pane(id)))
        }))
}

fn bar() -> impl IntoElement {
    view()
        .h(px(52.0))
        .flex_none()
        .flex_row()
        .items_center()
        .justify_end()
        .px(tokens::space(3.0))
        .pt(tokens::space(2.0))
        .child(icon_button(icons::close(), "Close details", || {
            svc::clear_selection()
        }))
}

/// Mounted once per selected item ID; every changing piece inside uses a
/// scoped reactive binding so 1-second log polls, notices, busy toggles, and
/// pointer hover never tear down the pane or reset scroll position.
fn item_pane(id: String) -> impl IntoElement {
    let busy_id = id.clone();
    let busy = computed(move || svc::is_busy(&busy_id));
    let notice = computed(svc::notice);
    let active_tab = computed(svc::detail_tab);
    let info_id = id.clone();
    let log_id = id.clone();

    view()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .child(
            view()
                .flex_none()
                .flex_col()
                .gap(tokens::space(3.0))
                .px(tokens::space(4.0))
                .pb(tokens::space(3.0))
                .child(identity(id.clone()))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(tokens::space(2.0))
                        .child(state_chip(id.clone()))
                        .child(when(
                            move || busy.get(),
                            || {
                                text("Working")
                                    .text_size(px(tokens::TEXT_CAPTION))
                                    .text_color(move || theme().muted_foreground)
                            },
                        )),
                )
                .child(action_row(id.clone()))
                .child(autostart_card(id.clone()))
                .child(dynamic(move || {
                    notice.get().map(|message| {
                        text(message)
                            .text_size(px(tokens::TEXT_CAPTION))
                            .text_color(tokens::success)
                    })
                })),
        )
        .child(tabs(id))
        .child(
            when(
                move || active_tab.get() == DetailTab::Info,
                move || info_tab(info_id.clone()),
            )
            .otherwise(move || log_tab(log_id.clone())),
        )
}

/// The item's name and what kind of item it is.
fn identity(id: String) -> impl IntoElement {
    let name_id = id.clone();
    let name = computed(move || {
        svc::item_by_id(&name_id)
            .map(|item| item.name)
            .unwrap_or_default()
    });
    let subtitle = computed(move || {
        svc::item_by_id(&id)
            .and_then(|item| item.kind)
            .unwrap_or_default()
    });
    view()
        .flex_col()
        .gap(tokens::space(0.5))
        .child(
            text(move || name.get())
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().foreground)
                .number_of_lines(2)
                .selectable(true),
        )
        .child(when(
            move || !subtitle.with(String::is_empty),
            move || {
                text(move || subtitle.get())
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || theme().muted_foreground)
                    .truncate()
            },
        ))
}

/// What the item is doing, in the color of its state.
fn state_chip(id: String) -> impl IntoElement {
    let status = computed(move || {
        svc::item_by_id(&id).map_or((State::None, 0), |item| (item.state, item.pid))
    });
    view()
        .flex_row()
        .items_center()
        .gap(px(7.0))
        .pl(px(8.0))
        .pr(px(9.0))
        .py(px(3.0))
        .rounded(px(99.0))
        .bg(move || state_color(status.get().0).opacity(0.12))
        .child(when(
            move || status.get().0 != State::None,
            move || status_dot(move || status.get().0),
        ))
        .child(
            text(move || {
                let (state, pid) = status.get();
                let mut label = if state == State::None {
                    "Not running".to_string()
                } else {
                    state.label().to_string()
                };
                if pid > 0 {
                    label.push_str(&format!(" · PID {pid}"));
                }
                label
            })
            .text_size(px(tokens::TEXT_CAPTION))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(move || state_color(status.get().0))
            .truncate(),
        )
}

fn is_editable_plist(item: &Item) -> bool {
    matches!(
        item.source,
        Source::UserAgent | Source::SystemAgent | Source::Daemon
    ) && !item.path.as_os_str().is_empty()
        && item
            .path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("plist"))
}

/// The actions an item is mostly used for, each with a place of its own, and
/// a menu of the rest.
fn action_row(id: String) -> impl IntoElement {
    let action_state = computed(move || {
        let item = svc::item_by_id(&id)?;
        let actions = svc::provider(item.source)
            .map(|provider| provider.actions(&item))
            .unwrap_or_default();
        Some((item, actions, svc::is_busy(&id)))
    });
    dynamic(move || {
        let (item, actions, busy) = action_state.get()?;
        let primary = devcore::service::primary(&actions);
        let restart = actions
            .contains(&Action::Restart)
            .then_some(Action::Restart);
        let edit_target = is_editable_plist(&item).then(|| (item.source, item.path.clone()));
        Some(
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(2.0))
                .child(primary.map(|action| action_button(&item, action, true, busy)))
                .child(restart.map(|action| action_button(&item, action, false, busy)))
                .child(edit_target.map(|(source, path)| {
                    nsh_base::button()
                        .test_id("edit-job")
                        .h(px(26.0))
                        .flex_none()
                        .flex_row()
                        .items_center()
                        .gap(tokens::space(1.0))
                        .px(tokens::space(2.0))
                        .rounded(move || theme().radius)
                        .bg(tokens::control)
                        .border_1()
                        .border_color(tokens::hairline)
                        .hover(move |style| style.bg(tokens::row_hover()))
                        .on_press(move || crate::editor::open_edit(source, path.clone()))
                        .child(
                            icons::edit()
                                .size(px(12.0))
                                .flex_none()
                                .text_color(move || theme().foreground),
                        )
                        .child(
                            text("Edit")
                                .text_size(px(tokens::TEXT_CAPTION))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(move || theme().foreground),
                        )
                }))
                .child(more_menu(&item, actions, busy)),
        )
    })
}

fn action_button(item: &Item, action: Action, primary: bool, busy: bool) -> AnyElement {
    let pressed = item.clone();
    let tip = action_tip(item, action);
    let button = labeled_button(
        action.label(),
        primary && action != Action::Stop,
        busy,
        move || svc::request_action(pressed.clone(), action),
    );
    if tip.is_empty() {
        button.into_element()
    } else {
        view()
            .flex_1()
            .flex_row()
            .tooltip(hint_tooltip(tip))
            .child(button)
            .into_element()
    }
}

fn more_menu(item: &Item, actions: Vec<Action>, busy: bool) -> impl IntoElement {
    let primary = devcore::service::primary(&actions);
    let mut entries: Vec<AnyElement> = Vec::new();
    for action in actions.iter().copied() {
        if matches!(action, Action::Enable | Action::Disable | Action::Remove)
            || Some(action) == primary
            || action == Action::Restart
        {
            continue;
        }
        let pressed = item.clone();
        entries.push(
            menu_item()
                .disabled(busy)
                .px(tokens::space(3.0))
                .py(tokens::space(1.5))
                .rounded(move || theme().radius)
                .hover(move |style| style.bg(tokens::row_hover()))
                .on_select(move || svc::request_action(pressed.clone(), action))
                .child(
                    text(action.label())
                        .text_size(px(tokens::TEXT_SMALL))
                        .text_color(move || theme().foreground),
                )
                .into_element(),
        );
    }
    if actions.contains(&Action::Remove) || !entries.is_empty() {
        entries.push(menu_separator().into_element());
    }
    let copy_path = item.path.display().to_string();
    entries.push(
        menu_item()
            .px(tokens::space(3.0))
            .py(tokens::space(1.5))
            .rounded(move || theme().radius)
            .hover(move |style| style.bg(tokens::row_hover()))
            .on_select(move || svc::copy(&copy_path))
            .child(
                text("Copy Path")
                    .text_size(px(tokens::TEXT_SMALL))
                    .text_color(move || theme().foreground),
            )
            .into_element(),
    );
    let reveal = item.path.clone();
    entries.push(
        menu_item()
            .px(tokens::space(3.0))
            .py(tokens::space(1.5))
            .rounded(move || theme().radius)
            .hover(move |style| style.bg(tokens::row_hover()))
            .on_select(move || {
                let path = reveal.clone();
                Owner::detached().run(move || {
                    spawn(async move {
                        let _ = reveal_path(path).await;
                    });
                });
            })
            .child(
                text("Reveal in Finder")
                    .text_size(px(tokens::TEXT_SMALL))
                    .text_color(move || theme().foreground),
            )
            .into_element(),
    );
    if actions.contains(&Action::Remove) {
        if !entries.is_empty() {
            entries.push(menu_separator().into_element());
        }
        let pressed = item.clone();
        entries.push(
            menu_item()
                .disabled(busy)
                .px(tokens::space(3.0))
                .py(tokens::space(1.5))
                .rounded(move || theme().radius)
                .hover(move |style| style.bg(tokens::row_hover()))
                .on_select(move || svc::request_action(pressed.clone(), Action::Remove))
                .child(
                    text("Remove…")
                        .text_size(px(tokens::TEXT_SMALL))
                        .text_color(move || theme().destructive),
                )
                .into_element(),
        );
    }
    view().flex_none().tooltip(hint_tooltip("More")).child(
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
                    .test_id("more-actions")
                    .size(px(26.0))
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .rounded(move || theme().radius)
                    .border_1()
                    .border_color(tokens::hairline)
                    .hover(move |style| style.bg(tokens::row_hover()))
                    .accessibility_label("More")
                    .child(
                        icons::more()
                            .size(px(14.0))
                            .text_color(move || theme().foreground),
                    ),
            )
            .children(entries),
    )
}

fn toggle_action(id: &str) -> Option<(Item, Action)> {
    let item = svc::item_by_id(id)?;
    let actions = svc::provider(item.source)?.actions(&item);
    let toggle = devcore::service::toggle(&actions)?;
    Some((item, toggle))
}

/// Whether an item starts by itself: the switch that flips it.
fn autostart_card(id: String) -> impl IntoElement {
    let has_toggle_id = id.clone();
    let has_toggle = computed(move || toggle_action(&has_toggle_id).is_some());
    when(
        move || has_toggle.get(),
        move || {
            let id = id.clone();
            let on_id = id.clone();
            let on = computed(move || {
                svc::item_by_id(&on_id).is_some_and(|item| item.autostart == Autostart::On)
            });
            let busy_id = id.clone();
            let busy = computed(move || svc::is_busy(&busy_id));
            let label_id = id.clone();
            let admin_id = id.clone();
            let words_id = id.clone();
            let switch_id = id.clone();

            view()
                .flex_col()
                .flex_none()
                .px(tokens::space(4.0))
                .pb(tokens::space(3.0))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(tokens::space(2.5))
                        .px(tokens::space(3.0))
                        .py(tokens::space(2.5))
                        .rounded(tokens::panel_radius)
                        .bg(tokens::panel)
                        .border_1()
                        .border_color(tokens::hairline)
                        .child(
                            nsh_base::button()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex_row()
                                .items_center()
                                .gap(tokens::space(1.5))
                                .disabled(move || busy.get())
                                .on_press(move || {
                                    if let Some((item, toggle)) = toggle_action(&words_id) {
                                        svc::request_action(item, toggle);
                                    }
                                })
                                .child(
                                    text(move || {
                                        svc::item_by_id(&label_id)
                                            .map(|item| toggle_label(&item))
                                            .unwrap_or("Load at login")
                                            .to_string()
                                    })
                                    .text_size(px(tokens::TEXT_BODY))
                                    .text_color(move || theme().foreground),
                                )
                                .child(when(
                                    move || {
                                        svc::item_by_id(&admin_id).is_some_and(|item| item.admin)
                                    },
                                    || {
                                        icons::lock()
                                            .size(px(11.0))
                                            .flex_none()
                                            .text_color(move || theme().muted_foreground)
                                    },
                                )),
                        )
                        .child(
                            nsh_base::switch()
                                .test_id("autostart-toggle")
                                .checked(move || on.get())
                                .disabled(move || busy.get())
                                .accessibility_label(move || {
                                    Some(SharedString::from(if on.get() { "On" } else { "Off" }))
                                })
                                .opacity(move || if busy.get() { 0.5_f32 } else { 1.0_f32 })
                                .track_style(move |s| {
                                    s.w(px(30.0))
                                        .h(px(18.0))
                                        .flex_none()
                                        .flex_row()
                                        .items_center()
                                        .px(px(2.0))
                                        .rounded(px(9.0))
                                        .bg(move || {
                                            if on.get() {
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
                                        .translate_x(move || {
                                            DefiniteLength::from(px(if on.get() {
                                                12.0
                                            } else {
                                                0.0
                                            }))
                                        })
                                })
                                .on_change(move |_: &bool| {
                                    if let Some((item, toggle)) = toggle_action(&switch_id) {
                                        svc::request_action(item, toggle);
                                    }
                                }),
                        ),
                )
        },
    )
}

/// What the switch of an item's autostart says.
fn toggle_label(item: &Item) -> &'static str {
    if item.source == Source::Daemon {
        "Load at boot"
    } else {
        "Load at login"
    }
}

/// The Info / Logs tabs, shown when the job writes logs.
fn tabs(id: String) -> impl IntoElement {
    let has_logs = computed(move || svc::has_logs(&id));
    when(
        move || has_logs.get(),
        || {
            let current = computed(svc::detail_tab);
            nsh_base::tabs()
                .accessibility_label("Inspector sections")
                .flex_none()
                .flex_row()
                .px(tokens::space(3.0))
                .py(tokens::space(1.5))
                .child(tab_button("Info", DetailTab::Info, current))
                .child(tab_button("Logs", DetailTab::Logs, current))
                .child(view().flex_1())
        },
    )
}

fn tab_button(label: &'static str, tab: DetailTab, current: Memo<DetailTab>) -> impl IntoElement {
    nsh_base::tab()
        .test_id(match tab {
            DetailTab::Info => "tab-info",
            DetailTab::Logs => "tab-logs",
        })
        .selected(move || current.get() == tab)
        .h(px(24.0))
        .px(tokens::space(2.0))
        .items_center()
        .justify_center()
        .rounded(move || theme().radius)
        .bg(move || {
            if current.get() == tab {
                tokens::row_selected()
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .hover(move |style| style.bg(tokens::row_hover()))
        .on_press(move |_: &()| svc::set_detail_tab(tab))
        .child(
            text(label)
                .text_size(px(tokens::TEXT_CAPTION))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || theme().foreground),
        )
}

fn info_tab(id: String) -> impl IntoElement {
    let err_id = id.clone();
    let error = computed(move || svc::detail_error(&err_id));
    let load_id = id.clone();
    let loading = computed(move || svc::detail_loading(&load_id));
    let rows = computed(move || svc::detail_rows(&id));

    view()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .child(dynamic(move || {
            error.get().map(|message| {
                text(message)
                    .px(tokens::space(4.0))
                    .py(tokens::space(2.0))
                    .text_size(px(tokens::TEXT_SMALL))
                    .text_color(move || theme().destructive)
            })
        }))
        .child(when(
            move || loading.get(),
            || {
                text("Loading")
                    .px(tokens::space(4.0))
                    .py(tokens::space(3.0))
                    .text_size(px(tokens::TEXT_SMALL))
                    .text_color(move || theme().muted_foreground)
            },
        ))
        .child(dynamic(move || {
            let groups = group_rows(rows.get());
            (!groups.is_empty()).then(move || {
                scroll().flex_1().min_h(px(0.0)).selectable(true).child(
                    view()
                        .flex_col()
                        .gap(tokens::space(4.0))
                        .px(tokens::space(4.0))
                        .py(tokens::space(2.0))
                        .child(
                            groups
                                .into_iter()
                                .map(|(title, rows)| info_group(title, rows)),
                        ),
                )
            })
        }))
}

fn group_rows(rows: Vec<Row>) -> Vec<(String, Vec<Row>)> {
    let mut groups: Vec<(String, Vec<Row>)> = Vec::new();
    for row in rows {
        match groups.last_mut() {
            Some((title, rows)) if title == &row.group => rows.push(row),
            _ => groups.push((row.group.clone(), vec![row])),
        }
    }
    groups
}

fn info_group(title: String, rows: Vec<Row>) -> impl IntoElement {
    let has_title = !title.is_empty();
    view()
        .flex_col()
        .gap(tokens::space(1.5))
        .flex_none()
        .child(when(has_title, move || {
            text(title.clone())
                .px(tokens::space(1.0))
                .text_size(px(tokens::TEXT_CAPTION))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().muted_foreground)
        }))
        .child(
            view()
                .flex_col()
                .rounded(tokens::panel_radius)
                .bg(tokens::panel)
                .border_1()
                .border_color(tokens::hairline)
                .overflow_hidden()
                .child(rows.into_iter().map(info_row)),
        )
}

/// Shows a value: beside its name if it fits, below if not, with a button to
/// copy it while the pointer is over the row.
fn info_row(row: Row) -> impl IntoElement {
    let key = format!("{}:{}", row.group, row.key);
    let hovered_key = key.clone();
    let opacity_key = key.clone();
    let long = row.value.len() > 30 || looks_like_code(&row);
    let value = info_value(&row);
    let copy_label = format!("Copy {}", row.key);
    view()
        .relative()
        .w_full()
        .px(tokens::space(3.0))
        .py(tokens::space(2.0))
        .border_b_1()
        .border_color(tokens::hairline)
        .hover(move |style| style.bg(tokens::row_hover()))
        .on_mouse_enter(move |_: &MouseEvent| svc::set_hovered_row(Some(hovered_key.clone())))
        .on_mouse_leave(move |_: &MouseEvent| {
            if svc::hovered_row().as_deref() == Some(key.as_str()) {
                svc::set_hovered_row(None);
            }
        })
        .child(if long {
            view()
                .flex_col()
                .gap(px(3.0))
                .child(info_key(&row, false))
                .child(value)
                .into_element()
        } else {
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(2.5))
                .child(info_key(&row, true))
                .child(value)
                .into_element()
        })
        .child(
            view()
                .absolute()
                .top(px(2.0))
                .right(px(4.0))
                .tooltip(hint_tooltip(copy_label.clone()))
                .child(
                    nsh_base::button()
                        .size(px(22.0))
                        .items_center()
                        .justify_center()
                        .rounded(move || theme().radius)
                        .bg(tokens::panel)
                        .opacity(move || {
                            if svc::hovered_row().as_deref() == Some(opacity_key.as_str()) {
                                1.0_f32
                            } else {
                                0.0_f32
                            }
                        })
                        .accessibility_label(copy_label)
                        .on_press({
                            let value = row.value.clone();
                            move || svc::copy(&value)
                        })
                        .child(
                            icons::copy()
                                .size(px(12.0))
                                .text_color(move || theme().muted_foreground),
                        ),
                ),
        )
}

fn info_key(row: &Row, grow: bool) -> impl IntoElement {
    let key = row.key.clone();
    let text = text(key)
        .text_size(if grow {
            px(tokens::TEXT_SMALL)
        } else {
            px(tokens::TEXT_CAPTION)
        })
        .text_color(move || theme().muted_foreground);
    if grow {
        text.flex_1().min_w(px(0.0)).truncate().into_element()
    } else {
        text.into_element()
    }
}

fn info_value(row: &Row) -> AnyElement {
    let code = looks_like_code(row);
    let value = text(row.value.clone())
        .text_size(px(if code {
            tokens::TEXT_CAPTION
        } else {
            tokens::TEXT_SMALL
        }))
        .text_color(move || theme().foreground)
        .text_right()
        .selectable(true);
    if code {
        value.font_family("monospace").into_element()
    } else {
        value.into_element()
    }
}

/// A value reads better in a fixed font when it is a path or a command.
fn looks_like_code(row: &Row) -> bool {
    row.value.starts_with('/') || row.key == "Command"
}

fn log_tab(id: String) -> impl IntoElement {
    let log_files = computed(move || svc::detail_logs(&id));
    dynamic(move || {
        let logs = log_files.get();
        if logs.is_empty() {
            return view()
                .flex_col()
                .px(tokens::space(4.0))
                .child(
                    text("No log files.")
                        .text_size(px(tokens::TEXT_SMALL))
                        .text_color(move || theme().muted_foreground),
                )
                .into_element();
        }
        untracked(|| log_tab_content(logs)).into_element()
    })
}

fn log_tab_content(logs: Vec<LogFile>) -> impl IntoElement {
    let mode = computed(svc::log_body_mode);
    let has_multiple = logs.len() > 1;
    view()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .gap(tokens::space(2.0))
        .px(tokens::space(4.0))
        .py(tokens::space(2.0))
        .child(when(has_multiple, move || {
            let selected = computed(svc::detail_log_file);
            view().flex_row().gap(tokens::space(1.0)).child(
                nsh_base::tabs()
                    .accessibility_label("Log files")
                    .flex_row()
                    .rounded(move || theme().radius)
                    .border_1()
                    .border_color(tokens::hairline)
                    .child(logs.iter().enumerate().map(|(index, file)| {
                        let label = file.label.clone();
                        nsh_base::tab()
                            .selected(move || selected.get() == index)
                            .h(px(24.0))
                            .px(tokens::space(2.0))
                            .items_center()
                            .justify_center()
                            .rounded(move || theme().radius)
                            .bg(move || {
                                if selected.get() == index {
                                    tokens::row_selected()
                                } else {
                                    AdaptiveColor::TRANSPARENT
                                }
                            })
                            .hover(move |style| style.bg(tokens::row_hover()))
                            .on_press(move |_: &()| svc::set_log_file(index))
                            .child(
                                text(label)
                                    .text_size(px(tokens::TEXT_CAPTION))
                                    .text_color(move || theme().foreground),
                            )
                    })),
            )
        }))
        .child(
            view()
                .flex_row()
                .items_center()
                .gap(tokens::space(1.5))
                .child(
                    text(move || {
                        svc::active_log_file()
                            .map(|file| file.path.display().to_string())
                            .unwrap_or_default()
                    })
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || theme().muted_foreground)
                    .truncate(),
                )
                .child(icon_button(icons::external(), "Open in Console", || {
                    if let Some(file) = svc::active_log_file_untracked() {
                        open_log(file.path);
                    }
                })),
        )
        .child(dynamic(move || match mode.get() {
            LogBodyMode::Error => text(move || svc::log_error().unwrap_or_default())
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().destructive)
                .into_element(),
            LogBodyMode::Reading => text("Reading…")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
                .into_element(),
            LogBodyMode::Empty => text("The log is empty.")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
                .into_element(),
            LogBodyMode::Lines => untracked(log_lines_view).into_element(),
        }))
}

fn log_lines_view() -> impl IntoElement {
    view()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .gap(tokens::space(1.0))
        .child(when(svc::log_truncated, || {
            text(move || {
                format!(
                    "The last {} lines, updated while this tab is open.",
                    svc::log_line_count()
                )
            })
            .text_size(px(tokens::TEXT_CAPTION))
            .text_color(move || theme().muted_foreground)
        }))
        .child(
            scroll()
                .test_id("log-scroll")
                .flex_1()
                .min_h(px(0.0))
                .rounded(move || theme().radius)
                .bg(tokens::panel)
                .border_1()
                .border_color(tokens::hairline)
                .child(
                    view()
                        .flex_col()
                        .gap(px(2.0))
                        .p(tokens::space(2.5))
                        .child(each(
                            svc::indexed_log_lines,
                            |(index, _)| *index,
                            |line, _| {
                                text(move || line.with(|(_, content)| content.clone()))
                                    .font_family("monospace")
                                    .text_size(px(tokens::TEXT_CAPTION))
                                    .text_color(move || theme().foreground)
                                    .selectable(true)
                            },
                        )),
                ),
        )
}

fn open_log(path: PathBuf) {
    Owner::detached().run(move || {
        spawn(async move {
            let _ = blocking(move || {
                std::process::Command::new("open")
                    .arg("-a")
                    .arg("Console")
                    .arg(&path)
                    .status()
                    .map(|_| ())
            })
            .await;
        });
    });
}

/// Explains an action, where its name does not.
fn action_tip(item: &Item, action: Action) -> String {
    if item.admin {
        return format!(
            "{} (runs as root through the privileged helper)",
            action.label()
        );
    }
    match action {
        Action::Load => "Register it with launchd".to_string(),
        Action::Unload => "Unregister it from launchd, which stops it".to_string(),
        Action::Start => "Start it now".to_string(),
        Action::Restart => "Stop it and start it again".to_string(),
        Action::Stop => {
            "Ask it to stop (SIGTERM); a job that is kept alive starts again".to_string()
        }
        _ => String::new(),
    }
}

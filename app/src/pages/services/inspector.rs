//! The inspector: what the chosen item is doing, what can be done to it,
//! whether it starts by itself, and its values and logs.

use std::path::PathBuf;

use devcore::service::{Action, Autostart, Item, Row, Source};
use nsh_base::{menu, menu_item, menu_separator, theme};
use nsh_core::prelude::*;

use crate::{
    components::{icons, icon_button, labeled_button, state_color, status_dot},
    store::services::{self as svc, DetailTab},
    theme as tokens,
};

/// The right-hand pane; the caller shows it only while an item is chosen.
pub(crate) fn inspector() -> impl IntoElement {
    view()
        .test_id("inspector")
        .w(px(336.0))
        .flex_none()
        .flex_col()
        .bg(move || tokens::sheet())
        .border_l_1()
        .border_color(move || tokens::hairline())
        .child(bar())
        .child(dynamic(move || svc::selected_item().map(item_pane)))
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

fn item_pane(item: Item) -> impl IntoElement {
    let actions = svc::provider(item.source)
        .map(|provider| provider.actions(&item))
        .unwrap_or_default();
    let busy = svc::is_busy(&item.id);
    // Subscribe this pane to everything it shows.
    let detail = svc::detail();
    let tab = svc::detail_tab();
    let notice = svc::notice();
    let has_logs = detail
        .as_ref()
        .is_some_and(|(id, _, logs, _, _)| id == &item.id && !logs.is_empty());

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
                .child(identity(&item))
                .child(
                    view()
                        .flex_row()
                        .items_center()
                        .gap(tokens::space(2.0))
                        .child(state_chip(&item))
                        .child(when(busy, || {
                            text("Working")
                                .text_size(px(tokens::TEXT_CAPTION))
                                .text_color(move || theme().muted_foreground)
                        })),
                )
                .child(action_row(&item, &actions, busy))
                .child(autostart_card(&item, &actions, busy))
                .child(notice.map(|message| {
                    text(message)
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || tokens::success())
                })),
        )
        .child(tabs(has_logs))
        .child(if tab == DetailTab::Info {
            info_tab(&item, &detail).into_element()
        } else {
            log_tab(&item)
        })
}

/// The item's name and what kind of item it is.
fn identity(item: &Item) -> impl IntoElement {
    let subtitle = item.kind.clone().unwrap_or_default();
    let empty = subtitle.is_empty();
    view()
        .flex_col()
        .gap(tokens::space(0.5))
        .child(
            text(item.name.clone())
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().foreground)
                .number_of_lines(2)
                .selectable(true),
        )
        .child(when(!empty, move || {
            text(subtitle.clone())
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground)
                .truncate()
        }))
}

/// What the item is doing, in the color of its state.
fn state_chip(item: &Item) -> impl IntoElement {
    let color = state_color(item.state);
    let mut label = if item.state == devcore::service::State::None {
        "Not running".to_string()
    } else {
        item.state.label().to_string()
    };
    if item.pid > 0 {
        label.push_str(&format!(" · PID {}", item.pid));
    }
    let state = item.state;
    view()
        .flex_row()
        .items_center()
        .gap(px(7.0))
        .pl(px(8.0))
        .pr(px(9.0))
        .py(px(3.0))
        .rounded(px(99.0))
        .bg(move || color.opacity(0.12))
        .child(when(state != devcore::service::State::None, move || {
            status_dot(move || state)
        }))
        .child(
            text(label)
                .text_size(px(tokens::TEXT_CAPTION))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || color)
                .truncate(),
        )
}

/// The actions an item is mostly used for, each with a place of its own, and
/// a menu of the rest.
fn action_row(item: &Item, actions: &[Action], busy: bool) -> impl IntoElement {
    let primary = devcore::service::primary(actions);
    let restart = actions.contains(&Action::Restart).then_some(Action::Restart);
    view()
        .flex_row()
        .items_center()
        .gap(tokens::space(2.0))
        .child(primary.map(|action| action_button(item, action, true, busy)))
        .child(restart.map(|action| action_button(item, action, false, busy)))
        .child(more_menu(item, actions.to_vec(), busy))
}

fn action_button(item: &Item, action: Action, primary: bool, busy: bool) -> impl IntoElement {
    let pressed = item.clone();
    let _tip = action_tip(item, action);
    labeled_button(
        action.label(),
        primary && action != Action::Stop,
        busy,
        move || svc::request_action(pressed.clone(), action),
    )
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
                spawn(async move {
                    let _ = reveal_path(path).await;
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
                .border_color(move || tokens::hairline())
                .hover(move |style| style.bg(tokens::row_hover()))
                .accessibility_label("More")
                .child(
                    icons::more()
                        .size(px(14.0))
                        .text_color(move || theme().foreground),
                ),
        )
        .children(entries)
}

/// Whether an item starts by itself: the switch that flips it.
fn autostart_card(item: &Item, actions: &[Action], busy: bool) -> Option<impl IntoElement> {
    let toggle = devcore::service::toggle(actions)?;
    let on = item.autostart == Autostart::On;
    let label = toggle_label(item);
    let words_item = item.clone();
    let switch_item = item.clone();
    let words_busy = busy;
    let switch_busy = busy;
    Some(
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
                    .rounded(move || tokens::panel_radius())
                    .bg(move || tokens::panel())
                    .border_1()
                    .border_color(move || tokens::hairline())
                    .child(
                        nsh_base::button()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex_row()
                            .items_center()
                            .gap(tokens::space(1.5))
                            .disabled(words_busy)
                            .on_press(move || {
                                svc::request_action(words_item.clone(), toggle)
                            })
                            .child(
                                text(label)
                                    .text_size(px(tokens::TEXT_BODY))
                                    .text_color(move || theme().foreground),
                            )
                            .child(when(item.admin, || {
                                icons::lock()
                                    .size(px(11.0))
                                    .flex_none()
                                    .text_color(move || theme().muted_foreground)
                            })),
                    )
                    .child(switch(on, switch_busy, move || {
                        svc::request_action(switch_item.clone(), toggle)
                    })),
            ),
    )
}

/// The custom switch: a track with a thumb at the end that means on.
fn switch(on: bool, busy: bool, on_press: impl Fn() + 'static) -> impl IntoElement {
    let track = view()
        .w(px(30.0))
        .h(px(18.0))
        .flex_none()
        .flex_row()
        .items_center()
        .px(px(2.0))
        .rounded(px(9.0))
        .bg(move || {
            if on {
                theme().primary
            } else {
                theme().secondary
            }
        });
    let track = if on {
        track.justify_end()
    } else {
        track.justify_start()
    };
    nsh_base::button()
        .test_id("autostart-toggle")
        .accessibility_label(if on { "On" } else { "Off" })
        .disabled(busy)
        .opacity(if busy { 0.5 } else { 1.0 })
        .on_press(move || on_press())
        .child(track.child(
            view()
                .size(px(14.0))
                .rounded(px(7.0))
                .bg(move || theme().primary_foreground),
        ))
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
fn tabs(has_logs: bool) -> impl IntoElement {
    when(has_logs, || {
        let tab = svc::detail_tab();
        view()
            .flex_none()
            .flex_row()
            .px(tokens::space(3.0))
            .py(tokens::space(1.5))
            .child(tab_button("Info", DetailTab::Info, tab))
            .child(tab_button("Logs", DetailTab::Logs, tab))
            .child(view().flex_1())
    })
}

fn tab_button(label: &'static str, tab: DetailTab, current: DetailTab) -> impl IntoElement {
    nsh_base::button()
        .test_id(match tab {
            DetailTab::Info => "tab-info",
            DetailTab::Logs => "tab-logs",
        })
        .h(px(24.0))
        .px(tokens::space(2.0))
        .items_center()
        .rounded(move || theme().radius)
        .bg(move || {
            if current == tab {
                tokens::row_selected()
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .on_press(move || svc::set_detail_tab(tab))
        .child(
            text(label)
                .text_size(px(tokens::TEXT_CAPTION))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || theme().foreground),
        )
}

fn info_tab(
    item: &Item,
    detail: &Option<(String, Vec<Row>, Vec<svc::LogFile>, bool, Option<String>)>,
) -> impl IntoElement {
    let rows = match detail {
        Some((id, rows, _, _, _)) if id == &item.id => rows.clone(),
        _ => Vec::new(),
    };
    let loading = detail
        .as_ref()
        .is_some_and(|(id, rows, _, loading, _)| id == &item.id && *loading && rows.is_empty());
    let error = detail
        .as_ref()
        .filter(|(id, _, _, _, _)| id == &item.id)
        .and_then(|(_, _, _, _, error)| error.clone());
    let groups = group_rows(rows);
    let has_groups = !groups.is_empty();
    view()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .child(error.map(|message| {
            text(message)
                .px(tokens::space(4.0))
                .py(tokens::space(2.0))
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().destructive)
        }))
        .child(when(loading, || {
            text("Loading")
                .px(tokens::space(4.0))
                .py(tokens::space(3.0))
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
        }))
        .child(when(has_groups, move || {
            let groups = groups.clone();
            scroll()
                .flex_1()
                .min_h(px(0.0))
                .selectable(true)
                .child(
                    view()
                        .flex_col()
                        .gap(tokens::space(4.0))
                        .px(tokens::space(4.0))
                        .py(tokens::space(2.0))
                        .child(groups.into_iter().map(|(title, rows)| info_group(title, rows))),
                )
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
                .rounded(move || tokens::panel_radius())
                .bg(move || tokens::panel())
                .border_1()
                .border_color(move || tokens::hairline())
                .overflow_hidden()
                .child(rows.into_iter().map(info_row)),
        )
}

/// Shows a value: beside its name if it fits, below if not, with a button to
/// copy it while the pointer is over the row.
fn info_row(row: Row) -> impl IntoElement {
    let key = format!("{}:{}", row.group, row.key);
    let hovered = svc::hovered_row().as_deref() == Some(key.as_str());
    let hovered_key = key.clone();
    let long = row.value.len() > 30 || looks_like_code(&row);
    let value = info_value(&row);
    view()
        .relative()
        .w_full()
        .px(tokens::space(3.0))
        .py(tokens::space(2.0))
        .border_b_1()
        .border_color(move || tokens::hairline())
        .hover(move |style| style.bg(tokens::row_hover()))
        .on_mouse_enter(move |_: &MouseEvent| {
            svc::set_hovered_row(Some(hovered_key.clone()))
        })
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
            nsh_base::button()
                .absolute()
                .top(px(2.0))
                .right(px(4.0))
                .size(px(22.0))
                .items_center()
                .justify_center()
                .rounded(move || theme().radius)
                .bg(move || tokens::panel())
                .opacity(if hovered { 1.0 } else { 0.0 })
                .accessibility_label(format!("Copy {}", row.key))
                .on_press({
                    let value = row.value.clone();
                    move || svc::copy(&value)
                })
                .child(
                    icons::copy()
                        .size(px(12.0))
                        .text_color(move || theme().muted_foreground),
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
    if grow { text.flex_1().min_w(px(0.0)).truncate().into_element() } else { text.into_element() }
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

fn log_tab(item: &Item) -> AnyElement {
    let (file, lines, error, truncated) = match svc::log_view() {
        Some(view) => view,
        None => {
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
    };
    let logs = match svc::detail() {
        Some((id, _, logs, _, _)) if id == item.id => logs,
        _ => Vec::new(),
    };
    let selected = svc::detail_log_file();
    let path = file.path.clone();
    let open = file.path.clone();
    view()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .gap(tokens::space(2.0))
        .px(tokens::space(4.0))
        .py(tokens::space(2.0))
        .child(when(logs.len() > 1, move || {
            view()
                .flex_row()
                .gap(tokens::space(1.0))
                .child(
                    view()
                        .flex_row()
                        .rounded(move || theme().radius)
                        .border_1()
                        .border_color(move || tokens::hairline())
                        .child(logs.iter().enumerate().map(|(index, file)| {
                            let label = file.label.clone();
                            nsh_base::button()
                                .h(px(24.0))
                                .px(tokens::space(2.0))
                                .items_center()
                                .rounded(move || theme().radius)
                                .bg(move || {
                                    if selected == index {
                                        tokens::row_selected()
                                    } else {
                                        AdaptiveColor::TRANSPARENT
                                    }
                                })
                                .on_press(move || svc::set_log_file(index))
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
                    text(path.display().to_string())
                        .flex_1()
                        .min_w(px(0.0))
                        .text_size(px(tokens::TEXT_CAPTION))
                        .text_color(move || theme().muted_foreground)
                        .truncate(),
                )
                .child(icon_button(icons::external(), "Open in Console", move || {
                    open_log(open.clone())
                })),
        )
        .child(match (error, lines) {
            (Some(message), _) => text(message)
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().destructive)
                .into_element(),
            (None, None) => text("Reading…")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
                .into_element(),
            (None, Some(lines)) if lines.is_empty() => text("The log is empty.")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
                .into_element(),
            (None, Some(lines)) => {
                let count = lines.len();
                view()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .gap(tokens::space(1.0))
                    .child(when(truncated, move || {
                        text(format!("The last {count} lines, updated while this tab is open."))
                            .text_size(px(tokens::TEXT_CAPTION))
                            .text_color(move || theme().muted_foreground)
                    }))
                    .child(
                        scroll()
                            .flex_1()
                            .min_h(px(0.0))
                            .rounded(move || theme().radius)
                            .bg(move || tokens::panel())
                            .border_1()
                            .border_color(move || tokens::hairline())
                            .child(
                                view()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .p(tokens::space(2.5))
                                    .child(lines.into_iter().map(|line| {
                                        text(line)
                                            .font_family("monospace")
                                            .text_size(px(tokens::TEXT_CAPTION))
                                            .text_color(move || theme().foreground)
                                            .selectable(true)
                                    })),
                            ),
                    )
                    .into_element()
            }
        })
        .into_element()
}

fn open_log(path: PathBuf) {
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

//! The list pane: the source's items, filtered by the search field and the
//! All / Running / Stopped filter, each row with its state and the one
//! action it is mostly used for.

use devcore::service::{Action, Item, Source};
use nsh_base::{input, theme};
use nsh_core::prelude::*;

use crate::{
    components::{icon_button, icons, note, status_dot},
    store::services::{self as svc, Filter},
    theme as tokens,
};

pub(crate) fn list_pane() -> impl IntoElement {
    view()
        .flex_1()
        .min_w(px(0.0))
        .flex_col()
        .child(header())
        .child(search_row())
        .child(view().h(px(1.0)).flex_none().bg(move || tokens::hairline()))
        .child(when_some_note())
        .child(list())
}

/// The title of the list and what reads it.
fn header() -> impl IntoElement {
    view()
        .h(px(52.0))
        .flex_none()
        .flex_row()
        .items_center()
        .gap(tokens::space(2.0))
        .px(tokens::space(5.0))
        .pt(tokens::space(2.0))
        .child(
            text(move || svc::active_source().title().to_string())
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .flex_1()
                .truncate(),
        )
        .child(when(
            move || svc::active_loading(),
            || {
                text("Loading")
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || theme().muted_foreground)
            },
        ))
        .child(icon_button(icons::refresh(), "Refresh", || {
            svc::refresh_active()
        }))
}

/// The search field and the three filters.
fn search_row() -> impl IntoElement {
    view()
        .flex_none()
        .flex_row()
        .items_center()
        .gap(tokens::space(2.0))
        .px(tokens::space(4.0))
        .pb(tokens::space(3.0))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .h(px(28.0))
                .flex_row()
                .items_center()
                .gap(tokens::space(1.5))
                .px(tokens::space(2.0))
                .rounded(move || theme().radius)
                .bg(move || tokens::control())
                .border_1()
                .border_color(move || tokens::hairline())
                .child(
                    icons::search()
                        .size(px(13.0))
                        .flex_none()
                        .text_color(move || theme().muted_foreground),
                )
                .child(
                    input()
                        .test_id("services-search")
                        .flex_1()
                        .min_w(px(0.0))
                        .h(px(26.0))
                        .text_size(px(tokens::TEXT_SMALL))
                        .text_color(move || theme().foreground)
                        .placeholder("Search")
                        .value(move || svc::query())
                        .on_change(move |text: &str| svc::set_query(text.to_owned())),
                ),
        )
        .child(
            view()
                .flex_none()
                .flex_row()
                .items_center()
                .rounded(move || theme().radius)
                .border_1()
                .border_color(move || tokens::hairline())
                .child(Filter::ALL.iter().map(|filter| filter_button(*filter))),
        )
}

fn filter_button(filter: Filter) -> impl IntoElement {
    nsh_base::button()
        .test_id(match filter {
            Filter::All => "filter-all",
            Filter::Running => "filter-running",
            Filter::Stopped => "filter-stopped",
        })
        .h(px(26.0))
        .items_center()
        .justify_center()
        .px(tokens::space(2.0))
        .bg(move || {
            if svc::filter() == filter {
                tokens::row_selected()
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .hover(move |style| style.bg(tokens::row_hover()))
        .on_press(move || svc::set_filter(filter))
        .child(
            text(filter.label())
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || {
                    if svc::filter() == filter {
                        theme().foreground
                    } else {
                        theme().muted_foreground
                    }
                }),
        )
}

/// The load error, an action's error, and the note about system changes.
fn when_some_note() -> impl IntoElement {
    view()
        .flex_col()
        .flex_none()
        .child(dynamic(move || {
            svc::active_error().map(|message| note(icons::warn(), message, true))
        }))
        .child(dynamic(move || {
            svc::action_error().map(|message| note(icons::warn(), message, true))
        }))
        .child(when(
            move || matches!(svc::active_source(), Source::SystemAgent | Source::Daemon),
            || {
                note(
                    icons::lock(),
                    "The privileged helper is not installed. Changing system agents and daemons takes root, so those actions will fail until it is.",
                    false,
                )
            },
        ))
}

fn list() -> impl IntoElement {
    when(move || svc::visible_items().is_empty(), empty_list).otherwise(|| {
        scroll().flex_1().min_h(px(0.0)).child(
            view()
                .flex_col()
                .w_full()
                .py(tokens::space(2.0))
                .px(tokens::space(2.0))
                .child(each(
                    move || svc::visible_items(),
                    |item| item.id.clone(),
                    |item, _| row(item),
                )),
        )
    })
}

fn empty_list() -> impl IntoElement {
    view()
        .flex_1()
        .items_center()
        .justify_center()
        .gap(tokens::space(2.5))
        .child(
            icons::search()
                .size(px(26.0))
                .text_color(move || theme().muted_foreground)
                .opacity(0.5),
        )
        .child(when(
            move || !svc::query().is_empty() || svc::filter() != Filter::All,
            || {
                view()
                    .flex_col()
                    .items_center()
                    .gap(tokens::space(2.0))
                    .child(
                        text("Nothing matches")
                            .text_size(px(tokens::TEXT_BODY))
                            .text_color(move || theme().muted_foreground),
                    )
                    .child(
                        nsh_base::button()
                            .test_id("clear-search")
                            .px(tokens::space(2.5))
                            .py(tokens::space(1.0))
                            .rounded(move || theme().radius)
                            .hover(move |style| style.bg(tokens::row_hover()))
                            .on_press(svc::clear_search)
                            .child(
                                text("Clear search and filter")
                                    .text_size(px(tokens::TEXT_SMALL))
                                    .text_color(move || theme().primary),
                            ),
                    )
            },
        ))
        .child(when(
            move || svc::query().is_empty() && svc::filter() == Filter::All,
            || {
                text("No items")
                    .text_size(px(tokens::TEXT_BODY))
                    .text_color(move || theme().muted_foreground)
            },
        ))
}

/// One item of the list: its state, its name, the lock for an item that
/// takes root, and at most one action, which shows while the pointer is over
/// the row or the row is chosen.
fn row(item: ReadSignal<Item>) -> impl IntoElement {
    let name = item.with_untracked(|item| item.name.clone());
    let test_id = format!("item-{}", name);
    let action_id = format!("action-{}", name);
    view()
        .test_id(&test_id)
        .w_full()
        .h(px(34.0))
        .flex_row()
        .items_center()
        .gap(tokens::space(2.5))
        .pl(tokens::space(3.0))
        .pr(tokens::space(1.0))
        .rounded(move || theme().radius)
        .role(AccessibilityRole::Button)
        .bg(move || {
            if svc::is_selected(&item.with_untracked(|item| item.id.clone())) {
                tokens::row_selected()
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .hover(move |style| style.bg(tokens::row_hover()))
        .on_mouse_enter(move |_: &MouseEvent| {
            svc::set_hovered_item(Some(item.with_untracked(|item| item.id.clone())))
        })
        .on_mouse_leave(move |_: &MouseEvent| {
            let id = item.with_untracked(|item| item.id.clone());
            if svc::hovered_item().as_deref() == Some(id.as_str()) {
                svc::set_hovered_item(None);
            }
        })
        .on_press(move || svc::select_item(&item.with_untracked(|item| item.id.clone())))
        .child(status_dot(move || item.get().state))
        .child(
            text(name.clone())
                .flex_1()
                .min_w(px(0.0))
                .text_size(px(tokens::TEXT_BODY))
                .text_color(move || theme().foreground)
                .truncate(),
        )
        .child(when(
            move || item.get().admin,
            move || {
                icons::lock()
                    .size(px(11.0))
                    .flex_none()
                    .text_color(move || theme().muted_foreground)
            },
        ))
        .child(when(
            move || !autostart_note(&item.get()).is_empty(),
            move || {
                text(move || autostart_note(&item.get()).to_string())
                    .text_size(px(tokens::TEXT_CAPTION))
                    .text_color(move || theme().muted_foreground)
                    .flex_none()
            },
        ))
        .child(
            text(move || item.get().state.label().to_string())
                .w(px(60.0))
                .flex_none()
                .text_right()
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
                .truncate(),
        )
        .child(
            view()
                .size(px(84.0))
                .flex_none()
                .flex_row()
                .items_center()
                .justify_end()
                .child(when(
                    move || svc::is_busy(&item.with_untracked(|item| item.id.clone())),
                    || {
                        text("Working")
                            .text_size(px(tokens::TEXT_CAPTION))
                            .text_color(move || theme().muted_foreground)
                    },
                ))
                .child(when(
                    move || {
                        !svc::is_busy(&item.with_untracked(|item| item.id.clone()))
                            && primary_action(&item.get()).is_some()
                    },
                    move || {
                        let action_id = action_id.clone();
                        row_action(item, action_id)
                    },
                )),
        )
}

/// The main action of a row, revealed on hover or while the row is chosen.
fn row_action(item: ReadSignal<Item>, action_id: String) -> impl IntoElement {
    nsh_base::button()
        .test_id(&action_id)
        .h(px(24.0))
        .px(tokens::space(2.0))
        .items_center()
        .justify_center()
        .rounded(move || theme().radius)
        .bg(move || tokens::control())
        .border_1()
        .border_color(move || tokens::hairline())
        .hover(move |style| style.bg(tokens::row_hover()))
        .opacity(move || {
            let id = item.with_untracked(|item| item.id.clone());
            let shown =
                svc::hovered_item().as_deref() == Some(id.as_str()) || svc::is_selected(&id);
            if shown { 1.0_f32 } else { 0.0_f32 }
        })
        .accessibility_label(move || {
            Some(SharedString::from(
                primary_action(&item.get())
                    .map(|action| action.label().to_string())
                    .unwrap_or_default(),
            ))
        })
        .on_press(move || {
            let item = item.get_untracked();
            if let Some(action) = primary_action(&item) {
                svc::request_action(item, action);
            }
        })
        .child(
            text(move || {
                primary_action(&item.get())
                    .map(|action| action.label().to_string())
                    .unwrap_or_default()
            })
            .text_size(px(tokens::TEXT_CAPTION))
            .text_color(move || theme().foreground),
        )
}

/// The action a row's button shows, from the provider's current actions.
fn primary_action(item: &Item) -> Option<Action> {
    let provider = svc::provider(item.source)?;
    devcore::service::primary(&provider.actions(item))
}

/// A word about an item's autostart, when it is worth one: an item that
/// starts by itself when the user would not expect it to.
fn autostart_note(item: &Item) -> &'static str {
    if item.autostart == devcore::service::Autostart::Off {
        "disabled"
    } else {
        ""
    }
}

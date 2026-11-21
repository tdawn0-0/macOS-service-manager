//! The list pane: the source's items, filtered by the search field and the
//! All / Running / Stopped filter, each row with its state and the one
//! action it is mostly used for.

use std::path::PathBuf;

use devcore::service::{Action, Item, Source};
use nsh_base::{input, theme};
use nsh_core::{
    element::desktop::{MenuItem, popup_menu_at},
    keymap::When,
    prelude::*,
};

use super::is_editable_plist;
use crate::{
    components::{icon_button, icons, note, status_dot},
    store::services::{self as svc, Filter, ServiceScope},
    theme as tokens,
};

pub(crate) fn list_pane() -> impl IntoElement {
    let search_ref = element_ref();
    let drag_over = signal(false);
    let login_drag_over =
        move || svc::active_source() == Some(Source::LoginItem) && drag_over.get();

    view()
        .test_id("services-list-pane")
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .flex_col()
        .bg(move || {
            if login_drag_over() {
                theme().primary.opacity(0.06)
            } else {
                AdaptiveColor::TRANSPARENT
            }
        })
        .on_file_drag_enter(move |e: &FileDragEvent| {
            if svc::active_source() == Some(Source::LoginItem)
                && (e.uris.is_empty()
                    || e.uris
                        .iter()
                        .any(|uri| app_bundle_path_from_uri(uri).is_some()))
            {
                drag_over.set(true);
            }
        })
        .on_file_drag_leave(move |_: &()| drag_over.set(false))
        .on_file_drop(move |e: &FileDropEvent| {
            drag_over.set(false);
            if svc::active_source() == Some(Source::LoginItem) {
                for uri in &e.uris {
                    if let Some(path) = app_bundle_path_from_uri(uri) {
                        svc::add_login_item_path(path);
                    }
                }
            }
        })
        .child(when(
            move || !crate::editor::is_open() && !crate::dev_logs::is_open(),
            move || search_shortcut(search_ref),
        ))
        .child(when(
            move || {
                !crate::editor::is_open()
                    && !crate::dev_logs::is_open()
                    && (!svc::query().is_empty()
                        || svc::filter() != Filter::All
                        || svc::selected_id().is_some())
            },
            escape_shortcut,
        ))
        .child(header())
        .child(search_row(search_ref))
        .child(view().h(px(1.0)).flex_none().bg(tokens::hairline))
        .child(when(login_drag_over, drop_hint_banner))
        .child(when_some_note())
        .child(list())
}

fn search_shortcut(search_ref: ElementRef) -> impl IntoElement {
    on_key("mod+f", move |_| search_ref.focus());
    let seen_find = std::cell::Cell::new(untracked(crate::shell::find_revision));
    effect(move || {
        let rev = crate::shell::find_revision();
        if rev != seen_find.get() {
            seen_find.set(rev);
            search_ref.focus();
        }
    });
    view().w(px(0.0)).h(px(0.0)).flex_none()
}

fn escape_shortcut() -> impl IntoElement {
    on_key_with(
        "escape",
        KeyOptions {
            when: When::Always,
            ..KeyOptions::default()
        },
        |_| {
            if !svc::query().is_empty() || svc::filter() != Filter::All {
                svc::clear_search();
            } else if svc::selected_id().is_some() {
                svc::clear_selection();
            }
        },
    );
    view().w(px(0.0)).h(px(0.0)).flex_none()
}

fn drop_hint_banner() -> impl IntoElement {
    view()
        .test_id("login-item-drop-hint")
        .flex_none()
        .flex_row()
        .items_center()
        .gap(tokens::space(2.0))
        .px(tokens::space(4.0))
        .py(tokens::space(2.0))
        .border_b_1()
        .border_color(move || theme().ring)
        .bg(move || theme().primary.opacity(0.08))
        .child(
            icons::plus()
                .size(px(13.0))
                .flex_none()
                .text_color(move || theme().primary),
        )
        .child(
            text("Drop an .app bundle to add it to Login Items")
                .text_size(px(tokens::TEXT_SMALL))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || theme().foreground),
        )
}

/// The title of the list and what reads it.
fn header() -> impl IntoElement {
    view()
        .h(px(tokens::TITLE_BAR_HEIGHT))
        .flex_none()
        .flex_row()
        .items_center()
        .gap(tokens::space(2.0))
        .px(tokens::space(5.0))
        .window_drag(true)
        .child(
            text(move || svc::active_scope().title().to_string())
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(move || theme().foreground)
                .flex_1()
                .truncate(),
        )
        .child(when(svc::active_loading, || {
            text("Loading")
                .text_size(px(tokens::TEXT_CAPTION))
                .text_color(move || theme().muted_foreground)
        }))
        .child(when(
            move || {
                matches!(
                    svc::active_source(),
                    Some(Source::UserAgent | Source::SystemAgent | Source::Daemon)
                )
            },
            || {
                header_action_button("new-job", "New Job", || {
                    if let Some(source) = svc::active_source() {
                        crate::editor::open_new(source);
                    }
                })
            },
        ))
        .child(when(
            move || svc::active_source() == Some(Source::LoginItem),
            || header_action_button("add-login-item", "Add Login Item", svc::add_login_item),
        ))
        .child(icon_button(icons::refresh(), "Refresh", || {
            svc::refresh_active()
        }))
}

fn header_action_button(
    test_id: &'static str,
    label: &'static str,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    nsh_base::button()
        .test_id(test_id)
        .h(px(26.0))
        .flex_none()
        .flex_row()
        .items_center()
        .gap(tokens::space(1.5))
        .px(tokens::space(2.5))
        .rounded(move || theme().radius)
        .bg(tokens::control)
        .border_1()
        .border_color(tokens::hairline)
        .hover(move |style| style.bg(tokens::row_hover()))
        .focus_visible(|s| s.ring(px(2.0)).ring_color(move || theme().ring))
        .on_press(on_press)
        .child(
            icons::plus()
                .size(px(12.0))
                .flex_none()
                .text_color(move || theme().foreground),
        )
        .child(
            text(label)
                .text_size(px(tokens::TEXT_CAPTION))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || theme().foreground),
        )
}

/// The search field and the three filters.
fn search_row(search_ref: ElementRef) -> impl IntoElement {
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
                .bg(tokens::control)
                .border_1()
                .border_color(tokens::hairline)
                .focus_within(|s| s.border_color(move || theme().ring))
                .child(
                    icons::search()
                        .size(px(13.0))
                        .flex_none()
                        .text_color(move || theme().muted_foreground),
                )
                .child(
                    input()
                        .test_id("services-search")
                        .element_ref(&search_ref)
                        .flex_1()
                        .min_w(px(0.0))
                        .h(px(26.0))
                        .text_size(px(tokens::TEXT_SMALL))
                        .text_color(move || theme().foreground)
                        .placeholder("Search")
                        .value(svc::query)
                        .on_change(move |text: &str| svc::set_query(text.to_owned())),
                ),
        )
        .child(
            nsh_base::toggle_group()
                .flex_none()
                .flex_row()
                .items_center()
                .rounded(move || theme().radius)
                .border_1()
                .border_color(tokens::hairline)
                .child(Filter::ALL.iter().map(|filter| filter_button(*filter))),
        )
}

fn filter_button(filter: Filter) -> impl IntoElement {
    nsh_base::toggle()
        .test_id(match filter {
            Filter::All => "filter-all",
            Filter::Running => "filter-running",
            Filter::Stopped => "filter-stopped",
        })
        .pressed(move || svc::filter() == filter)
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
        .focus_visible(|s| s.ring(px(2.0)).ring_color(move || theme().ring))
        .on_change(move |_: &bool| svc::set_filter(filter))
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

/// The load error, an action's error, login-item restrictions, and the note
/// about system changes.
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
        .child(dynamic(move || {
            svc::active_restriction().map(restriction_note)
        }))
        .child(when(
            move || {
                matches!(
                    svc::active_source(),
                    Some(Source::SystemAgent | Source::Daemon)
                ) && !svc::helper_installed()
            },
            || {
                note(
                    icons::lock(),
                    "The privileged helper is not installed. Changing system agents and daemons takes root, so those actions will fail until it is.",
                    false,
                )
            },
        ))
}

fn restriction_note(restriction: devcore::service::Restriction) -> impl IntoElement {
    let show_settings = restriction.settings;
    let show_fda_guide = svc::active_source() == Some(Source::LoginItem);
    view()
        .test_id("login-item-restriction")
        .flex_col()
        .gap(tokens::space(2.0))
        .px(tokens::space(4.0))
        .py(tokens::space(2.5))
        .border_b_1()
        .border_color(tokens::hairline)
        .bg(tokens::inset)
        .child(
            view()
                .flex_row()
                .items_start()
                .gap(tokens::space(2.0))
                .child(
                    icons::lock()
                        .size(px(14.0))
                        .flex_none()
                        .mt(px(1.0))
                        .text_color(move || theme().muted_foreground),
                )
                .child(
                    text(restriction.text)
                        .flex_1()
                        .min_w(px(0.0))
                        .text_size(px(tokens::TEXT_SMALL))
                        .text_color(move || theme().muted_foreground),
                ),
        )
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(tokens::space(2.0))
                .pl(px(22.0))
                .child(when(show_settings, || {
                    nsh_base::button()
                        .test_id("open-full-disk-access")
                        .h(px(24.0))
                        .px(tokens::space(2.5))
                        .items_center()
                        .justify_center()
                        .rounded(move || theme().radius)
                        .bg(tokens::control)
                        .border_1()
                        .border_color(tokens::hairline)
                        .hover(move |style| style.bg(tokens::row_hover()))
                        .on_press(svc::open_restriction_settings)
                        .child(
                            text("Open Full Disk Access…")
                                .text_size(px(tokens::TEXT_CAPTION))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(move || theme().foreground),
                        )
                }))
                .child(when(show_fda_guide, || {
                    nsh_base::button()
                        .test_id("login-items-fda-guide")
                        .h(px(24.0))
                        .px(tokens::space(2.5))
                        .items_center()
                        .justify_center()
                        .rounded(move || theme().radius)
                        .bg(tokens::control)
                        .border_1()
                        .border_color(tokens::hairline)
                        .hover(move |style| style.bg(tokens::row_hover()))
                        .on_press(crate::guide::open)
                        .child(
                            text("Full Disk Access Guide…")
                                .text_size(px(tokens::TEXT_CAPTION))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(move || theme().foreground),
                        )
                }))
                .child(
                    nsh_base::button()
                        .test_id("read-login-items-once")
                        .h(px(24.0))
                        .px(tokens::space(2.5))
                        .items_center()
                        .justify_center()
                        .rounded(move || theme().radius)
                        .bg(tokens::control)
                        .border_1()
                        .border_color(tokens::hairline)
                        .hover(move |style| style.bg(tokens::row_hover()))
                        .on_press(svc::read_restricted_once)
                        .child(
                            text("Read Once…")
                                .text_size(px(tokens::TEXT_CAPTION))
                                .text_color(move || theme().foreground),
                        ),
                ),
        )
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
                    svc::visible_items,
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
                                text("Clear filters")
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
/// takes root, and a hover-only quick action on unselected rows (when the
/// Inspector is already open for the selected row, actions live beside it
/// in the Inspector instead of duplicating `Stop` on the row).
fn row(item: ReadSignal<Item>) -> impl IntoElement {
    let name = item.with_untracked(|item| item.name.clone());
    let test_id = format!("item-{}", name);
    let action_id = format!("action-{}", name);
    view()
        .test_id(&test_id)
        .relative()
        .w_full()
        .h(px(34.0))
        .flex_row()
        .items_center()
        .gap(tokens::space(2.0))
        .px(tokens::space(3.0))
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
        .on_context_menu(move |e: &ContextMenuEvent| {
            let current = item.get_untracked();
            let items = row_context_menu_items(&current);
            if !items.is_empty() {
                drop(popup_menu_at(items, e.page_position));
            }
        })
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
            move || svc::active_scope() == ServiceScope::All,
            move || {
                let source = item.with_untracked(|item| item.source);
                source_badge(source)
            },
        ))
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
            text(move || {
                let id = item.with_untracked(|item| item.id.clone());
                if svc::is_busy(&id) {
                    "Working".to_string()
                } else {
                    item.get().state.label().to_string()
                }
            })
            .w(px(56.0))
            .flex_none()
            .text_right()
            .text_size(px(tokens::TEXT_SMALL))
            .text_color(move || theme().muted_foreground)
            .opacity(move || {
                let id = item.with_untracked(|item| item.id.clone());
                let action_overlay_visible = !svc::is_busy(&id)
                    && !svc::is_selected(&id)
                    && svc::hovered_item().as_deref() == Some(id.as_str())
                    && primary_action(&item.get()).is_some();
                if action_overlay_visible {
                    0.0_f32
                } else {
                    1.0_f32
                }
            })
            .truncate(),
        )
        .child(when(
            move || {
                let id = item.with_untracked(|item| item.id.clone());
                !svc::is_selected(&id)
                    && !svc::is_busy(&id)
                    && primary_action(&item.get()).is_some()
            },
            move || {
                let action_id = action_id.clone();
                view()
                    .absolute()
                    .right(tokens::space(2.0))
                    .top(px(5.0))
                    .flex_none()
                    .child(row_action(item, action_id))
            },
        ))
}

/// Subtle pill identifying which source owns an item when viewing `All Items`.
fn source_badge(source: Source) -> impl IntoElement {
    view()
        .test_id(match source {
            Source::Brew => "source-badge-brew",
            Source::LoginItem => "source-badge-login-item",
            Source::UserAgent => "source-badge-user-agent",
            Source::SystemAgent => "source-badge-system-agent",
            Source::Daemon => "source-badge-daemon",
        })
        .flex_none()
        .flex_row()
        .items_center()
        .h(px(18.0))
        .px(tokens::space(1.5))
        .rounded(px(99.0))
        .bg(tokens::inset)
        .border_1()
        .border_color(tokens::hairline)
        .child(
            text(source_badge_label(source))
                .text_size(px(tokens::TEXT_CAPTION))
                .font_weight(FontWeight::MEDIUM)
                .text_color(move || theme().muted_foreground),
        )
}

fn source_badge_label(source: Source) -> &'static str {
    match source {
        Source::Brew => "Homebrew",
        Source::LoginItem => "Login Item",
        Source::UserAgent => "User Agent",
        Source::SystemAgent => "System Agent",
        Source::Daemon => "Daemon",
    }
}

/// The main action of an unselected row, revealed on hover.
fn row_action(item: ReadSignal<Item>, action_id: String) -> impl IntoElement {
    nsh_base::button()
        .test_id(&action_id)
        .h(px(24.0))
        .px(tokens::space(2.0))
        .items_center()
        .justify_center()
        .rounded(move || theme().radius)
        .bg(tokens::control)
        .border_1()
        .border_color(tokens::hairline)
        .hover(move |style| style.bg(tokens::row_hover()))
        .focus_visible(|s| s.ring(px(2.0)).ring_color(move || theme().ring))
        .opacity(move || {
            let id = item.with_untracked(|item| item.id.clone());
            let shown = svc::hovered_item().as_deref() == Some(id.as_str());
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

/// Builds the native right-click context menu for a service list row.
fn row_context_menu_items(item: &Item) -> Vec<MenuItem> {
    let actions = svc::provider(item.source)
        .map(|provider| provider.actions(item))
        .unwrap_or_default();
    let busy = svc::is_busy(&item.id);
    let mut entries = Vec::new();

    for action in actions.iter().copied() {
        if matches!(action, Action::Enable | Action::Disable | Action::Remove) {
            continue;
        }
        let target = item.clone();
        entries.push(
            MenuItem::new(action.label())
                .disabled(busy)
                .on_select(move || svc::request_action(target.clone(), action)),
        );
    }

    if is_editable_plist(item) {
        if !entries.is_empty() {
            entries.push(MenuItem::separator());
        }
        let source = item.source;
        let path = item.path.clone();
        entries.push(
            MenuItem::new("Edit Plist…")
                .on_select(move || crate::editor::open_edit(source, path.clone())),
        );
    }

    if !item.path.as_os_str().is_empty() {
        if !entries.is_empty() {
            entries.push(MenuItem::separator());
        }
        let copy_path = item.path.display().to_string();
        entries.push(MenuItem::new("Copy Path").on_select(move || svc::copy(&copy_path)));
        let reveal = item.path.clone();
        entries.push(MenuItem::new("Reveal in Finder").on_select(move || {
            let path = reveal.clone();
            Owner::detached().run(move || {
                spawn(async move {
                    let _ = reveal_path(path).await;
                });
            });
        }));
    }

    if actions.contains(&Action::Remove) {
        if !entries.is_empty() {
            entries.push(MenuItem::separator());
        }
        let target = item.clone();
        entries.push(
            MenuItem::new("Remove…")
                .destructive(true)
                .disabled(busy)
                .on_select(move || svc::request_action(target.clone(), Action::Remove)),
        );
    }

    entries
}

/// Extracts an absolute `.app` bundle path from a dropped `file:` URI or raw
/// absolute path.
fn app_bundle_path_from_uri(uri: &str) -> Option<PathBuf> {
    let raw = if let Some(rest) = uri.strip_prefix("file://localhost/") {
        format!("/{rest}")
    } else if let Some(rest) = uri.strip_prefix("file:///") {
        format!("/{rest}")
    } else if uri.starts_with('/') {
        uri.to_string()
    } else {
        return None;
    };
    let decoded = percent_decode(&raw)?;
    let trimmed = decoded.trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let path = PathBuf::from(trimmed);
    let is_app = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("app"));
    (path.is_absolute() && is_app).then_some(path)
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16)?;
            let lo = (bytes[i + 2] as char).to_digit(16)?;
            out.push(((hi << 4) | lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dropped_app_bundle_uris() {
        assert_eq!(
            app_bundle_path_from_uri("file:///Applications/Raycast.app/"),
            Some(PathBuf::from("/Applications/Raycast.app"))
        );
        assert_eq!(
            app_bundle_path_from_uri("file://localhost/Applications/Visual%20Studio%20Code.app"),
            Some(PathBuf::from("/Applications/Visual Studio Code.app"))
        );
        assert_eq!(
            app_bundle_path_from_uri("/Applications/Devbase.APP/"),
            Some(PathBuf::from("/Applications/Devbase.APP"))
        );
        assert_eq!(
            app_bundle_path_from_uri("file:///Applications/README.txt"),
            None
        );
        assert_eq!(
            app_bundle_path_from_uri("https://example.com/Foo.app"),
            None
        );
        assert_eq!(app_bundle_path_from_uri("Applications/Foo.app"), None);
    }
}

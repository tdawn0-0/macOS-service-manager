//! Global non-blocking toast notifications backed by `nsh_base::ToastManager`
//! and `nsh_base::toast_stack()`.
//!
//! Call [`info`], [`success`], or [`error`] from any UI handler or async task
//! on the engine thread. State lives in a per-surface `global::<ToastHub>()` so
//! headless tests stay isolated from one another.

use std::{cell::RefCell, rc::Rc, time::Duration};

use nsh_base::{
    Anchor, ToastManager, ToastMotion, ToastOptions, ToastStackState, ToastTransitionStatus,
    button, theme, toast, toast_stack,
};
use nsh_core::{PointerEvents, prelude::*, time::now};

use crate::{components::icons, theme as tokens};

/// Maximum non-ending toasts visible in the stack at once.
const MAX_VISIBLE: usize = 3;

/// Default auto-dismiss duration for info and success toasts.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(4);

/// Slightly longer auto-dismiss duration for error toasts so messages can be read.
const ERROR_TIMEOUT: Duration = Duration::from_secs(6);

/// Lifecycle polling cadence while at least one toast is mounted.
const TICK_INTERVAL: Duration = Duration::from_millis(50);

/// Semantic emphasis of a toast card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Info,
    Success,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    tone: Tone,
    message: String,
}

struct ToastInner {
    next_id: u64,
    manager: ToastManager<u64, Entry>,
}

#[derive(Clone)]
struct ToastHub {
    inner: Rc<RefCell<ToastInner>>,
    revision: u64,
}

impl Default for ToastHub {
    fn default() -> Self {
        Self {
            inner: Rc::new(RefCell::new(ToastInner {
                next_id: 1,
                manager: ToastManager::new(ToastMotion::sonner()),
            })),
            revision: 0,
        }
    }
}

fn hub() -> Signal<ToastHub> {
    global::<ToastHub>()
}

fn bump(state: Signal<ToastHub>) {
    state.update(|hub| {
        hub.revision = hub.revision.wrapping_add(1);
    });
}

fn push(tone: Tone, message: impl Into<String>, timeout: Option<Duration>) {
    let message = message.into();
    if message.trim().is_empty() {
        return;
    }
    let Some(state) = try_global::<ToastHub>() else {
        return;
    };
    state.with_untracked(|hub| {
        let mut inner = hub.inner.borrow_mut();
        let id = inner.next_id;
        inner.next_id = inner.next_id.wrapping_add(1).max(1);
        inner.manager.push(
            id,
            Entry { tone, message },
            ToastOptions::timeout(timeout),
            now(),
        );
    });
    bump(state);
}

/// Shows an informational toast at the bottom-right corner.
pub fn info(message: impl Into<String>) {
    push(Tone::Info, message, Some(DEFAULT_TIMEOUT));
}

/// Shows a success toast at the bottom-right corner.
pub fn success(message: impl Into<String>) {
    push(Tone::Success, message, Some(DEFAULT_TIMEOUT));
}

/// Shows an error toast at the bottom-right corner.
pub fn error(message: impl Into<String>) {
    push(Tone::Error, message, Some(ERROR_TIMEOUT));
}

/// Dismisses a single toast by id, starting its exit transition.
pub fn dismiss(id: u64) {
    let Some(state) = try_global::<ToastHub>() else {
        return;
    };
    let changed = state.with_untracked(|hub| hub.inner.borrow_mut().manager.dismiss(&id, now()));
    if changed {
        bump(state);
    }
}

/// Dismisses every mounted toast.
pub fn dismiss_all() {
    let Some(state) = try_global::<ToastHub>() else {
        return;
    };
    let changed =
        state.with_untracked(|hub| !hub.inner.borrow_mut().manager.dismiss_all(now()).is_empty());
    if changed {
        bump(state);
    }
}

fn dismiss_newest() {
    let Some(state) = try_global::<ToastHub>() else {
        return;
    };
    let changed = state.with_untracked(|hub| {
        let mut inner = hub.inner.borrow_mut();
        let newest = inner
            .manager
            .iter()
            .rev()
            .find(|(_, _, status)| *status != ToastTransitionStatus::Ending)
            .map(|(id, _, _)| *id);
        if let Some(id) = newest {
            inner.manager.dismiss(&id, now())
        } else {
            false
        }
    });
    if changed {
        bump(state);
    }
}

fn has_toasts() -> bool {
    hub().with(|state| {
        let _ = state.revision;
        !state.inner.borrow().manager.is_empty()
    })
}

fn visible_toasts() -> Vec<(u64, Entry, ToastTransitionStatus)> {
    hub().with(|state| {
        let _ = state.revision;
        state
            .inner
            .borrow()
            .manager
            .visible(MAX_VISIBLE)
            .map(|(id, entry, status)| (*id, entry.clone(), status))
            .collect()
    })
}

fn ticker(stack_state: ToastStackState) -> impl IntoElement {
    set_interval(TICK_INTERVAL, move || {
        let paused = untracked(|| stack_state.is_expanded());
        let state = hub();
        let changed = state.with_untracked(|hub| {
            let mut inner = hub.inner.borrow_mut();
            !inner.manager.is_empty() && inner.manager.advance(now(), paused).changed
        });
        if changed {
            bump(state);
        }
    });
    view().w(px(0.0)).h(px(0.0)).flex_none()
}

fn tone_icon(tone: Tone) -> impl IntoElement {
    match tone {
        Tone::Success => icons::check()
            .size(px(14.0))
            .flex_none()
            .text_color(tokens::success)
            .into_element(),
        Tone::Error => icons::warn()
            .size(px(14.0))
            .flex_none()
            .text_color(move || theme().destructive)
            .into_element(),
        Tone::Info => view()
            .w(px(8.0))
            .h(px(8.0))
            .flex_none()
            .rounded_full()
            .bg(move || theme().primary)
            .into_element(),
    }
}

fn tone_border(tone: Tone) -> AdaptiveColor {
    match tone {
        Tone::Info => tokens::hairline(),
        Tone::Success => tokens::success().opacity(0.35),
        Tone::Error => theme().destructive.opacity(0.42),
    }
}

/// The bottom-right global toast stack, mounted once in [`crate::App`].
pub fn stack() -> impl IntoElement {
    let stack_state = ToastStackState::new();
    view()
        .test_id("toast-viewport")
        .absolute()
        .right(px(16.0))
        .bottom(px(16.0))
        .w(px(340.0))
        .pointer_events(PointerEvents::BoxNone)
        .child(when(has_toasts, move || ticker(stack_state)))
        .child(
            toast_stack()
                .test_id("toast-stack")
                .placement(Anchor::BottomRight)
                .motion(ToastMotion::sonner())
                .state(stack_state)
                .w_full()
                .on_cancel(dismiss_newest)
                .child(each(
                    visible_toasts,
                    |(id, _, _)| *id,
                    move |item, _| {
                        let initial = item.get_untracked();
                        let id = initial.0;
                        let tone = initial.1.tone;
                        toast()
                            .test_id("toast-item")
                            .transition_status(move || item.with(|(_, _, status)| *status))
                            .accessibility_label(move || {
                                Some(SharedString::from(
                                    item.with(|(_, entry, _)| entry.message.clone()),
                                ))
                            })
                            .on_dismiss_request(move || dismiss(id))
                            .w_full()
                            .flex_row()
                            .items_center()
                            .gap(tokens::space(2.5))
                            .px(tokens::space(3.0))
                            .py(tokens::space(2.5))
                            .rounded(tokens::panel_radius)
                            .bg(tokens::panel)
                            .border_1()
                            .border_color(move || tone_border(tone))
                            .shadow_lg()
                            .transition_all()
                            .duration(200.0)
                            .starting_class("opacity-0 translate-y-4")
                            .ending_class("opacity-0 scale-95")
                            .child(tone_icon(tone))
                            .child(
                                text(move || item.with(|(_, entry, _)| entry.message.clone()))
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .text_size(px(tokens::TEXT_SMALL))
                                    .text_color(move || theme().foreground),
                            )
                            .child(
                                button()
                                    .test_id("toast-dismiss")
                                    .accessibility_label("Dismiss notification")
                                    .size(px(20.0))
                                    .flex_none()
                                    .items_center()
                                    .justify_center()
                                    .rounded(move || theme().radius)
                                    .hover(|s| s.bg(tokens::row_hover()))
                                    .focus_visible(|s| {
                                        s.ring(px(2.0)).ring_color(move || theme().ring)
                                    })
                                    .on_press(move || dismiss(id))
                                    .child(
                                        icons::close()
                                            .size(px(12.0))
                                            .text_color(move || theme().muted_foreground),
                                    ),
                            )
                    },
                )),
        )
}

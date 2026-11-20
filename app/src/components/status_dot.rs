//! The state mark and its color, ported from `theme.go` and `view.go`.

use devcore::service::State;
use nsh_base::theme;
use nsh_core::prelude::*;

use crate::theme as tokens;

/// The color of an item's state: green runs, blue waits for its time, red
/// failed, and gray is at rest, which is nothing to worry about.
pub(crate) fn state_color(state: State) -> AdaptiveColor {
    match state {
        State::Running => tokens::success(),
        State::Scheduled => tokens::warning(),
        State::Failed | State::Invalid => theme().destructive,
        _ => theme().muted_foreground,
    }
}

/// Shows an item's state: hollow where launchd has not even loaded it, solid
/// at rest, and solid with a ring while it runs. The state is reactive, so a
/// row follows its item when an action changes it.
pub(crate) fn status_dot(state: impl IntoReactive<State> + 'static) -> impl IntoElement {
    let state = state.into_reactive();
    let opacity_state = state.clone();
    let fill_state = state.clone();
    let border_state = state;
    view()
        .size(px(8.0))
        .flex_none()
        .rounded(px(4.0))
        .border_2()
        .opacity(move || {
            if opacity_state.get() == State::None {
                0.0_f32
            } else {
                1.0_f32
            }
        })
        .bg(move || match fill_state.get() {
            State::Stopped | State::None => AdaptiveColor::TRANSPARENT,
            State::Loaded => state_color(fill_state.get()).opacity(0.65),
            _ => state_color(fill_state.get()),
        })
        .border_color(move || match border_state.get() {
            State::Stopped => state_color(border_state.get()).opacity(0.65),
            State::Running => state_color(border_state.get()).opacity(0.18),
            _ => AdaptiveColor::TRANSPARENT,
        })
}

//! The first-run guide, ported from `guide.go`.
//!
//! The Go app opened the guide in a window of its own, above everything, and
//! followed System Settings once it was opened. NSH keeps one window here, so
//! the guide is an overlay over the workspace: the same pages (welcome, the
//! way into System Settings, done), the same words, and the same one thing it
//! asks for — Full Disk Access, which macOS needs before the app may read the
//! list of login items.
//!
//! The Go welcome guide remembered that it had opened by writing a file named
//! `welcomed` in the app's user-data directory (`mygo.App.Path(mygo.PathUserData)`).
//! This port keeps that mechanism and the file's name: present means the guide
//! was shown and closed or skipped, absent means a first run.
//!
//! Tests install a [`Persistence`] and an [`Access`] of their own; nothing
//! here reads the real file or probes TCC while a test provider is installed.

use std::{
    path::PathBuf,
    sync::{
        LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use nsh_base::theme;
use nsh_core::{
    desktop::{
        TitleBarOptions, TitleBarStyle, WindowAttributes, WindowKind, WindowLevel, WindowPosition,
    },
    element::{
        ImageSrc, Svg,
        desktop::{title_bar, window},
        svg,
    },
    geometry::size,
    prelude::*,
};

use crate::{store::system, theme as tokens};

/// The page the guide shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Page {
    /// What the permission is for, and the two ways out.
    #[default]
    Welcome,
    /// System Settings is open, with the way to add the app to its list.
    Sidecar,
    /// macOS allowed it; the page closes in a moment.
    Done,
}

/// Remembers that the guide has been shown, as Go's `welcomed` file did.
pub trait Persistence: Send + Sync {
    fn completed(&self) -> Result<bool, String>;
    fn complete(&self) -> Result<(), String>;
}

/// Whether macOS still needs the permission the guide asks for. Go asked its
/// restricted login-item provider; this asks the same question of TCC.
pub trait Access: Send + Sync {
    fn needs_access(&self) -> bool;
}

/// The `welcomed` file in the app's Application Support directory.
struct WelcomedFile {
    path: Option<PathBuf>,
}

impl WelcomedFile {
    fn new() -> Self {
        let path = std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join("Library/Application Support/Devbase/welcomed"));
        WelcomedFile { path }
    }
}

impl Persistence for WelcomedFile {
    fn completed(&self) -> Result<bool, String> {
        Ok(self.path.as_ref().is_some_and(|path| path.exists()))
    }

    fn complete(&self) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::write(path, b"").map_err(|error| error.to_string())
    }
}

/// The permission probe: Full Disk Access lets the app read TCC's database.
struct SystemAccess;

impl Access for SystemAccess {
    fn needs_access(&self) -> bool {
        let Some(home) = std::env::var_os("HOME") else {
            return false;
        };
        let database = PathBuf::from(home).join("Library/Application Support/com.apple.TCC/TCC.db");
        std::fs::File::open(database).is_err()
    }
}

/// A test's persistence that was already completed.
struct CompletedPersistence;

impl Persistence for CompletedPersistence {
    fn completed(&self) -> Result<bool, String> {
        Ok(true)
    }

    fn complete(&self) -> Result<(), String> {
        Ok(())
    }
}

/// A test's access that needs nothing.
struct AccessGranted;

impl Access for AccessGranted {
    fn needs_access(&self) -> bool {
        false
    }
}

static FAKE_MODE: AtomicBool = AtomicBool::new(false);
static PERSISTENCE: LazyLock<Mutex<Option<std::sync::Arc<dyn Persistence>>>> =
    LazyLock::new(|| Mutex::new(None));
static ACCESS: LazyLock<Mutex<Option<std::sync::Arc<dyn Access>>>> =
    LazyLock::new(|| Mutex::new(None));

fn persistence_of() -> std::sync::Arc<dyn Persistence> {
    if let Some(provider) = PERSISTENCE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
    {
        return provider;
    }
    if FAKE_MODE.load(Ordering::Relaxed) {
        return std::sync::Arc::new(CompletedPersistence);
    }
    std::sync::Arc::new(WelcomedFile::new())
}

fn access_of() -> std::sync::Arc<dyn Access> {
    if let Some(provider) = ACCESS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
    {
        return provider;
    }
    if FAKE_MODE.load(Ordering::Relaxed) {
        return std::sync::Arc::new(AccessGranted);
    }
    std::sync::Arc::new(SystemAccess)
}

/// Makes the guide use test providers instead of the real file and TCC.
/// Call it before the surface starts, then [`install_persistence`] and
/// [`install_access`]. Tests only.
pub fn use_fake_provider() {
    FAKE_MODE.store(true, Ordering::Relaxed);
    *PERSISTENCE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    *ACCESS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

/// Gives the guide a persistence of the test's own. Tests only.
pub fn install_persistence(persistence: std::sync::Arc<dyn Persistence>) {
    FAKE_MODE.store(true, Ordering::Relaxed);
    *PERSISTENCE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(persistence);
}

/// Gives the guide an access probe of the test's own. Tests only.
pub fn install_access(access: std::sync::Arc<dyn Access>) {
    FAKE_MODE.store(true, Ordering::Relaxed);
    *ACCESS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(access);
}

#[derive(Debug, Clone)]
struct Guide {
    started: bool,
    open: bool,
    page: Page,
    /// System Settings was opened: the sidecar page is the one to show.
    settings_open: bool,
    /// Bumped when the guide closes or moves on, so an older poll is dropped.
    sequence: u64,
}

impl Default for Guide {
    fn default() -> Self {
        Guide {
            started: false,
            open: false,
            page: Page::Welcome,
            settings_open: false,
            sequence: 0,
        }
    }
}

fn guide() -> Signal<Guide> {
    global::<Guide>()
}

/// Runs guide work outside the view that asked for it.
fn task(future: impl Future<Output = ()> + 'static) {
    Owner::detached().run(move || {
        spawn(future);
    });
}

/// Runs a guide change later, outside the view that asked for it.
fn later(delay: Duration, f: impl FnOnce() + 'static) {
    Owner::detached().run(move || {
        set_timeout(delay, f);
    });
}

/// Opens the guide on a first run, once. App calls this at launch on a
/// desktop; the file is read on a blocking thread, never in render.
pub fn start() {
    let state = guide();
    if state.with(|state| state.started) {
        return;
    }
    state.update(|state| state.started = true);
    let persistence = persistence_of();
    task(async move {
        let completed = match blocking(move || persistence.completed()).await {
            Ok(Ok(completed)) => completed,
            _ => true,
        };
        if !completed {
            state.update(|state| {
                if !state.open && state.page == Page::Welcome {
                    state.open = true;
                }
            });
        }
    });
}

/// Opens the Full Disk Access guide on demand (from Login Items, the Help menu,
/// or Command Palette), resetting to the welcome page regardless of whether the
/// first-run `welcomed` marker already exists.
pub fn open() {
    guide().update(|state| {
        state.open = true;
        state.page = Page::Welcome;
        state.settings_open = false;
        state.sequence += 1;
    });
}

/// Whether the overlay shows.
pub fn visible() -> bool {
    guide().with(|state| state.open)
}

/// Closes the guide and remembers that it was shown, as Go's `closeGuide`.
pub fn close() {
    let was_open = guide().with(|state| state.open);
    guide().update(|state| {
        state.open = false;
        state.page = Page::Welcome;
        state.settings_open = false;
        state.sequence += 1;
    });
    if !was_open {
        return;
    }
    let persistence = persistence_of();
    task(async move {
        let _ = blocking(move || persistence.complete()).await;
    });
}

/// The welcome page's `Allow…`: open System Settings and wait for the drag.
fn allow() {
    guide().update(|state| {
        state.page = Page::Sidecar;
        state.settings_open = true;
        state.sequence += 1;
    });
    system::open_full_disk_access();
    check_access();
}

/// The sidecar's `Open Settings`.
fn open_settings() {
    guide().update(|state| {
        state.settings_open = true;
        state.sequence += 1;
    });
    system::open_full_disk_access();
    check_access();
}

/// Asks whether the permission is on, once a second while the sidecar shows,
/// as Go's `checkGuide` did. The answer moves the page to done and closes it.
fn check_access() {
    let state = guide();
    let sequence = state.with(|guide| guide.sequence);
    let access = access_of();
    task(async move {
        let needs = blocking(move || access.needs_access())
            .await
            .unwrap_or(true);
        let mut poll = false;
        let mut done = false;
        state.update(|guide| {
            if !guide.open || guide.page != Page::Sidecar || guide.sequence != sequence {
                return;
            }
            if needs {
                poll = true;
            } else {
                guide.page = Page::Done;
                done = true;
            }
        });
        if poll {
            later(Duration::from_secs(1), move || {
                if state.with(|guide| guide.open && guide.page == Page::Sidecar) {
                    check_access();
                }
            });
        } else if done {
            // Go's guideDoneAfter: the page says it worked, then closes.
            later(Duration::from_millis(1400), move || {
                if state.with(|guide| {
                    guide.open && guide.page == Page::Done && guide.sequence == sequence
                }) {
                    close();
                }
            });
        }
    });
}

/// Distinct Devbase stacked-toggle brand mark (tinted with `currentColor`).
const GUIDE_ICON: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="36" height="36" viewBox="0 0 36 36"><path fill="currentColor" fill-rule="evenodd" d="M9.5 3h17a6.5 6.5 0 0 1 0 13h-17a6.5 6.5 0 0 1 0-13Zm17 2.9a3.6 3.6 0 1 0 0 7.2 3.6 3.6 0 0 0 0-7.2Z"/><rect x="4.2" y="21.2" width="27.6" height="10.6" rx="5.3" fill="none" stroke="currentColor" stroke-width="2.4"/><circle cx="9.5" cy="26.5" r="2.7" fill="currentColor"/></svg>"#;

const DISK_ICON: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><line x1="22" x2="2" y1="12" y2="12"/><path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11Z"/><line x1="6" x2="6.01" y1="16" y2="16"/><line x1="10" x2="10.01" y1="16" y2="16"/></svg>"#;

const CHECK_ICON: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M20 6 9 17l-5-5"/></svg>"#;

fn icon(markup: &str) -> Svg {
    svg(ImageSrc::svg_markup(markup))
}

fn guide_button(
    label: &'static str,
    test_id: &'static str,
    primary: bool,
    disabled: bool,
    on_press: impl Fn() + 'static,
) -> impl IntoElement {
    nsh_base::button()
        .h(px(30.0))
        .px(px(16.0))
        .flex_none()
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
        .hover(move |style| {
            if primary {
                style.bg(move || theme().primary.opacity(0.86))
            } else {
                style.bg(tokens::row_hover())
            }
        })
        .focus_visible(|style| style.ring(px(2.0)).ring_color(move || theme().ring))
        .disabled(disabled)
        .opacity(if disabled { 0.5 } else { 1.0 })
        .text_color(move || {
            if primary {
                theme().primary_foreground
            } else {
                theme().foreground
            }
        })
        .accessibility_label(label)
        .test_id(test_id)
        .on_press(on_press)
        .child(text(label).text_size(px(tokens::TEXT_SMALL)))
}

/// The floating native `WindowKind::Panel` guide window used on desktop.
/// Stays above System Settings while non-resizable with a hidden title bar.
pub(crate) fn guide_window() -> impl IntoElement {
    let mut bar = TitleBarOptions::default();
    bar.style = TitleBarStyle::Hidden;
    bar.height = Some(px(40.0));

    let mut attrs = WindowAttributes::default();
    attrs.title_bar = Some(bar);
    attrs.resizable = Some(false);
    attrs.minimizable = Some(false);
    attrs.maximizable = Some(false);
    attrs.level = Some(WindowLevel::Floating);
    attrs.activate = Some(true);
    attrs.close_on_escape = Some(true);

    window()
        .id("guide")
        .kind(WindowKind::Panel)
        .title("Full Disk Access")
        .default_size(size(px(420.0), px(310.0)))
        .default_position(WindowPosition::Center)
        .attributes(attrs)
        .open(visible)
        .on_open_change(|change| {
            if !change.open {
                close();
            }
        })
        .content(window_content)
}

fn window_content() -> impl IntoElement {
    view()
        .test_id("guide-window")
        .size_full()
        .flex_col()
        .bg(tokens::panel)
        .text_color(move || theme().foreground)
        .child(
            view()
                .test_id("guide-header-drag")
                .w_full()
                .flex_none()
                .h(move || {
                    let top = title_bar().top;
                    if top.0 > 0.0 { top } else { px(20.0) }
                })
                .window_drag(true),
        )
        .child(
            view()
                .test_id("guide-card")
                .flex_1()
                .w_full()
                .flex_col()
                .items_center()
                .px(px(28.0))
                .pb(px(24.0))
                .child(dynamic(|| Some(page_body().into_element()))),
        )
}

/// The overlay the app root shows while [`visible`] off desktop (and what
/// headless guide tests mount directly). It fills the window, so a click
/// behind it cannot reach the workspace while the guide is open.
pub fn overlay() -> impl IntoElement {
    view()
        .test_id("guide-overlay")
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .size_full()
        .items_center()
        .justify_center()
        .bg(AdaptiveColor::rgb(0x000000, 0x000000).opacity(0.35))
        .child(dynamic(|| Some(card().into_element())))
}

fn page_body() -> impl IntoElement {
    let page = guide().with(|state| state.page);
    match page {
        Page::Welcome => welcome().into_element(),
        Page::Sidecar => sidecar().into_element(),
        Page::Done => done().into_element(),
    }
}

fn card() -> impl IntoElement {
    view()
        .test_id("guide-card")
        .w(px(420.0))
        .h(px(310.0))
        .flex_none()
        .flex_col()
        .items_center()
        .p(px(28.0))
        .rounded(tokens::panel_radius)
        .bg(tokens::panel)
        .border_1()
        .border_color(tokens::hairline)
        .shadow_xl()
        .child(page_body())
}

fn welcome() -> impl IntoElement {
    view()
        .flex_col()
        .flex_1()
        .w_full()
        .items_center()
        .child(
            icon(GUIDE_ICON)
                .size(px(64.0))
                .flex_none()
                .text_color(move || theme().primary),
        )
        .child(
            text("Welcome to Devbase")
                .text_size(px(19.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_center()
                .mt(px(12.0)),
        )
        .child(view().h(px(16.0)))
        .child(
            view()
                .w_full()
                .flex_row()
                .items_center()
                .gap(px(12.0))
                .p(px(12.0))
                .rounded(move || theme().radius)
                .bg(move || theme().foreground.opacity(0.045))
                .child(
                    view()
                        .size(px(38.0))
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .rounded(px(9.0))
                        .bg(move || theme().primary)
                        .child(
                            icon(DISK_ICON)
                                .size(px(20.0))
                                .text_color(move || theme().primary_foreground),
                        ),
                )
                .child(
                    view()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            text("Full Disk Access")
                                .text_size(px(tokens::TEXT_BODY))
                                .font_weight(FontWeight::SEMIBOLD),
                        )
                        .child(
                            text("Only to read the list of login items.")
                                .text_size(px(tokens::TEXT_SMALL))
                                .text_color(move || theme().muted_foreground),
                        ),
                )
                .child(guide_button("Allow…", "guide-allow", true, false, allow)),
        )
        .child(view().flex_1())
        .child(guide_button("Skip", "guide-skip", false, false, close))
}

fn sidecar() -> impl IntoElement {
    let settings_open = guide().with(|state| state.settings_open);
    view()
        .flex_col()
        .flex_1()
        .w_full()
        .items_center()
        .child(
            icon(GUIDE_ICON)
                .size(px(64.0))
                .flex_none()
                .text_color(move || theme().primary),
        )
        .child(
            text("Add this app with the + button")
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_center()
                .mt(px(16.0)),
        )
        .child(view().flex_1())
        .child(if settings_open {
            text("Waiting for access")
                .text_size(px(tokens::TEXT_SMALL))
                .text_color(move || theme().muted_foreground)
                .mb(px(12.0))
                .into_element()
        } else {
            view()
                .mb(px(12.0))
                .child(guide_button(
                    "Open Settings",
                    "guide-open-settings",
                    false,
                    false,
                    open_settings,
                ))
                .into_element()
        })
        .child(guide_button("Skip", "guide-skip", false, false, close))
}

fn done() -> impl IntoElement {
    view()
        .flex_col()
        .flex_1()
        .items_center()
        .justify_center()
        .pb(px(40.0))
        .child(
            view()
                .size(px(56.0))
                .items_center()
                .justify_center()
                .rounded(px(28.0))
                .bg(tokens::success)
                .child(
                    icon(CHECK_ICON)
                        .size(px(30.0))
                        .text_color(AdaptiveColor::rgb(0xffffff, 0xffffff)),
                ),
        )
        .child(
            text("Full Disk Access is on")
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD)
                .mt(px(14.0)),
        )
}

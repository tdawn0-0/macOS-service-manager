//! Shared inline Lucide SVG icon catalog for Devbase screens, the sidebar, and
//! the menu bar. Inline SVGs follow `currentColor` and ship with zero external
//! icon assets.
//!
//! Callers can render an [`Svg`] directly via named helpers (e.g. [`copy`],
//! [`package`]) or obtain static SVG markup / [`ImageSrc`] via [`markup`] and
//! [`image_src`] (for native menus and status items).

#![allow(dead_code)]

use devcore::service::{Action, Source};
use nsh_core::element::{ImageSrc, Svg, svg};

use crate::store::software::SoftwareKind;

macro_rules! svg_doc {
    ($body:expr) => {
        concat!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round">"#,
            $body,
            "</svg>"
        )
    };
}

/// Every icon in the shared Lucide catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Icon {
    Brew,
    LoginItem,
    UserAgent,
    SystemAgent,
    Daemon,
    PlayFilled,
    StopFilled,
    Dot,
    Package,
    Database,
    Redis,
    MongoDb,
    Shield,
    ShieldCheck,
    Cloud,
    Layers,
    Plus,
    Play,
    Stop,
    Terminal,
    Copy,
    Check,
    Folder,
    Refresh,
    RotateCcw,
    External,
    Trash,
    Edit,
    Download,
    Wrench,
    ArrowLeft,
    ArrowRight,
    ChevronRight,
    ChevronDown,
    Sliders,
    Settings,
    Activity,
    Power,
    Window,
    Search,
    Close,
    More,
    Lock,
    Warn,
    Globe,
    Key,
    Link,
    Plug,
    FileText,
}

/// Returns the complete static SVG document for `icon`.
pub(crate) fn markup(icon: Icon) -> &'static str {
    match icon {
        Icon::Brew => svg_doc!(
            r#"<path d="M17 11h1a3 3 0 0 1 0 6h-1"/><path d="M9 12v6"/><path d="M13 12v6"/><path d="M14 7.5c-1 0-1.44.5-3 .5s-2-.5-3-.5-1.72.5-2.5.5a2.5 2.5 0 0 1 0-5c.78 0 1.57.5 2.5.5S9.44 2 11 2s2 1.5 3 1.5 1.72-.5 2.5-.5a2.5 2.5 0 0 1 0 5c-.78 0-1.5-.5-2.5-.5Z"/><path d="M5 8v12a2 2 0 0 0 2 2h8a2 2 0 0 0 2-2V8"/>"#
        ),
        Icon::LoginItem => svg_doc!(
            r#"<path d="M15 3h4a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2h-4"/><polyline points="10 17 15 12 10 7"/><line x1="15" x2="3" y1="12" y2="12"/>"#
        ),
        Icon::UserAgent => svg_doc!(
            r#"<path d="M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2"/><circle cx="12" cy="7" r="4"/>"#
        ),
        Icon::SystemAgent => svg_doc!(
            r#"<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M22 21v-2a4 4 0 0 0-3-3.87"/><path d="M16 3.13a4 4 0 0 1 0 7.75"/>"#
        ),
        Icon::Daemon => svg_doc!(
            r#"<rect width="20" height="8" x="2" y="2" rx="2" ry="2"/><rect width="20" height="8" x="2" y="14" rx="2" ry="2"/><line x1="6" x2="6.01" y1="6" y2="6"/><line x1="6" x2="6.01" y1="18" y2="18"/>"#
        ),
        Icon::PlayFilled => {
            svg_doc!(r#"<polygon points="6 3 20 12 6 21 6 3" fill="currentColor"/>"#)
        }
        Icon::StopFilled => {
            svg_doc!(r#"<rect width="14" height="14" x="5" y="5" rx="2" fill="currentColor"/>"#)
        }
        Icon::Dot => svg_doc!(r#"<circle cx="12" cy="12" r="1"/>"#),
        Icon::Package => svg_doc!(
            r#"<path d="M11 21.73a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73z"/><path d="M12 22V12"/><polyline points="3.29 7 12 12 20.71 7"/><path d="m7.5 4.27 9 5.15"/>"#
        ),
        Icon::Database => svg_doc!(
            r#"<ellipse cx="12" cy="5" rx="9" ry="3"/><path d="M3 5v14a9 3 0 0 0 18 0V5"/><path d="M3 12a9 3 0 0 0 18 0"/>"#
        ),
        Icon::Redis => svg_doc!(
            r#"<path d="M4 7h16"/><path d="M4 12h16"/><path d="M4 17h16"/><circle cx="7" cy="7" r="1" fill="currentColor"/><circle cx="7" cy="12" r="1" fill="currentColor"/><circle cx="7" cy="17" r="1" fill="currentColor"/>"#
        ),
        Icon::MongoDb => svg_doc!(
            r#"<path d="M12 2c2.5 3 4 5.5 4 9a4 4 0 0 1-8 0c0-3.5 1.5-6 4-9Z"/><path d="M12 15v7"/>"#
        ),
        Icon::Shield => svg_doc!(
            r#"<path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"/>"#
        ),
        Icon::ShieldCheck => svg_doc!(
            r#"<path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"/><path d="m9 12 2 2 4-4"/>"#
        ),
        Icon::Cloud => {
            svg_doc!(r#"<path d="M17.5 19H9a7 7 0 1 1 6.71-9h1.79a4.5 4.5 0 1 1 0 9Z"/>"#)
        }
        Icon::Layers => svg_doc!(
            r#"<path d="M12.83 2.18a2 2 0 0 0-1.66 0L2.6 6.08a1 1 0 0 0 0 1.83l8.58 3.91a2 2 0 0 0 1.66 0l8.58-3.9a1 1 0 0 0 0-1.83z"/><path d="M2 12a1 1 0 0 0 .58.91l8.6 3.91a2 2 0 0 0 1.65 0l8.58-3.9A1 1 0 0 0 22 12"/><path d="M2 17a1 1 0 0 0 .58.91l8.6 3.91a2 2 0 0 0 1.65 0l8.58-3.9A1 1 0 0 0 22 17"/>"#
        ),
        Icon::Plus => svg_doc!(r#"<path d="M5 12h14"/><path d="M12 5v14"/>"#),
        Icon::Play => svg_doc!(r#"<polygon points="6 3 20 12 6 21 6 3"/>"#),
        Icon::Stop => svg_doc!(r#"<rect x="5" y="5" width="14" height="14" rx="2"/>"#),
        Icon::Terminal => svg_doc!(
            r#"<polyline points="4 17 10 11 4 5"/><line x1="12" x2="20" y1="19" y2="19"/>"#
        ),
        Icon::Copy => svg_doc!(
            r#"<rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>"#
        ),
        Icon::Check => svg_doc!(r#"<path d="M20 6 9 17l-5-5"/>"#),
        Icon::Folder => svg_doc!(
            r#"<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>"#
        ),
        Icon::Refresh => svg_doc!(
            r#"<path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/>"#
        ),
        Icon::RotateCcw => svg_doc!(
            r#"<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/>"#
        ),
        Icon::External => svg_doc!(
            r#"<path d="M15 3h6v6"/><path d="M10 14 21 3"/><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/>"#
        ),
        Icon::Trash => svg_doc!(
            r#"<path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/>"#
        ),
        Icon::Edit => svg_doc!(
            r#"<path d="M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z"/><path d="m15 5 4 4"/>"#
        ),
        Icon::Download => svg_doc!(
            r#"<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" x2="12" y1="15" y2="3"/>"#
        ),
        Icon::Wrench => svg_doc!(
            r#"<path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"/>"#
        ),
        Icon::ArrowLeft => svg_doc!(r#"<path d="m12 19-7-7 7-7"/><path d="M19 12H5"/>"#),
        Icon::ArrowRight => svg_doc!(r#"<path d="M5 12h14"/><path d="m12 5 7 7-7 7"/>"#),
        Icon::ChevronRight => svg_doc!(r#"<path d="m9 18 6-6-6-6"/>"#),
        Icon::ChevronDown => svg_doc!(r#"<path d="m6 9 6 6 6-6"/>"#),
        Icon::Sliders => svg_doc!(
            r#"<line x1="21" x2="14" y1="4" y2="4"/><line x1="10" x2="3" y1="4" y2="4"/><line x1="21" x2="12" y1="12" y2="12"/><line x1="8" x2="3" y1="12" y2="12"/><line x1="21" x2="16" y1="20" y2="20"/><line x1="12" x2="3" y1="20" y2="20"/><line x1="14" x2="14" y1="2" y2="6"/><line x1="8" x2="8" y1="10" y2="14"/><line x1="16" x2="16" y1="18" y2="22"/>"#
        ),
        Icon::Settings => svg_doc!(
            r#"<path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/><circle cx="12" cy="12" r="3"/>"#
        ),
        Icon::Activity => svg_doc!(
            r#"<path d="M22 12h-2.48a2 2 0 0 0-1.93 1.46l-2.35 8.36a.25.25 0 0 1-.48 0L9.24 2.18a.25.25 0 0 0-.48 0l-2.35 8.36A2 2 0 0 1 4.49 12H2"/>"#
        ),
        Icon::Power => svg_doc!(r#"<path d="M12 2v10"/><path d="M18.4 6.6a9 9 0 1 1-12.77.04"/>"#),
        Icon::Window => svg_doc!(
            r#"<rect x="2" y="4" width="20" height="16" rx="2"/><path d="M10 4v4"/><path d="M2 8h20"/><path d="M6 4v4"/>"#
        ),
        Icon::Search => svg_doc!(r#"<path d="m21 21-4.34-4.34"/><circle cx="11" cy="11" r="8"/>"#),
        Icon::Close => svg_doc!(r#"<path d="M18 6 6 18"/><path d="m6 6 12 12"/>"#),
        Icon::More => svg_doc!(
            r#"<circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/><circle cx="5" cy="12" r="1"/>"#
        ),
        Icon::Lock => svg_doc!(
            r#"<rect width="18" height="11" x="3" y="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/>"#
        ),
        Icon::Warn => svg_doc!(
            r#"<path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3"/><path d="M12 9v4"/><path d="M12 17h.01"/>"#
        ),
        Icon::Globe => svg_doc!(
            r#"<circle cx="12" cy="12" r="10"/><path d="M12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20"/><path d="M2 12h20"/>"#
        ),
        Icon::Key => svg_doc!(
            r#"<path d="m15.5 7.5 2.3 2.3a1 1 0 0 0 1.4 0l2.1-2.1a1 1 0 0 0 0-1.4L19 4"/><path d="m21 2-9.6 9.6"/><circle cx="7.5" cy="15.5" r="5.5"/>"#
        ),
        Icon::Link => svg_doc!(
            r#"<path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/>"#
        ),
        Icon::Plug => svg_doc!(
            r#"<path d="M12 22v-5"/><path d="M9 8V2"/><path d="M15 8V2"/><path d="M18 8v5a4 4 0 0 1-4 4h-4a4 4 0 0 1-4-4V8Z"/>"#
        ),
        Icon::FileText => svg_doc!(
            r#"<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/><path d="M10 9H8"/><path d="M16 13H8"/><path d="M16 17H8"/>"#
        ),
    }
}

/// Returns an [`ImageSrc`] for `icon` (suitable for native menus and tray items).
pub(crate) fn image_src(icon: Icon) -> ImageSrc {
    ImageSrc::svg_markup(markup(icon))
}

/// Renders `icon` as an inline [`Svg`] element.
pub(crate) fn render(icon: Icon) -> Svg {
    svg(image_src(icon))
}

/// The icon of a launchd service source.
pub(crate) fn source(source: Source) -> Svg {
    render(match source {
        Source::Brew => Icon::Brew,
        Source::LoginItem | Source::Autostart => Icon::LoginItem,
        Source::UserAgent | Source::SystemdUser => Icon::UserAgent,
        Source::SystemAgent => Icon::SystemAgent,
        Source::Daemon | Source::SystemdSystem => Icon::Daemon,
    })
}

/// The icon of a service action: play, stop, restart, or dot.
pub(crate) fn action(action: Action) -> Svg {
    render(match action {
        Action::Start | Action::Load => Icon::PlayFilled,
        Action::Stop => Icon::StopFilled,
        Action::Restart | Action::Unload => Icon::Refresh,
        _ => Icon::Dot,
    })
}

/// A runtime or database engine in the Software rail.
pub(crate) fn software(kind: SoftwareKind) -> Svg {
    render(match kind {
        SoftwareKind::Node => Icon::Terminal,
        _ => Icon::Database,
    })
}

/// A database engine icon (`redis`, `mongodb`, or SQL cylinder).
pub(crate) fn engine(kind: &str) -> Svg {
    render(match kind {
        "redis" => Icon::Redis,
        "mongodb" => Icon::MongoDb,
        _ => Icon::Database,
    })
}

pub(crate) fn package() -> Svg {
    render(Icon::Package)
}

pub(crate) fn database() -> Svg {
    render(Icon::Database)
}

pub(crate) fn shield() -> Svg {
    render(Icon::Shield)
}

pub(crate) fn shield_check() -> Svg {
    render(Icon::ShieldCheck)
}

pub(crate) fn cloud() -> Svg {
    render(Icon::Cloud)
}

pub(crate) fn layers() -> Svg {
    render(Icon::Layers)
}

pub(crate) fn plus() -> Svg {
    render(Icon::Plus)
}

pub(crate) fn play() -> Svg {
    render(Icon::Play)
}

pub(crate) fn stop() -> Svg {
    render(Icon::Stop)
}

pub(crate) fn terminal() -> Svg {
    render(Icon::Terminal)
}

pub(crate) fn copy() -> Svg {
    render(Icon::Copy)
}

pub(crate) fn check() -> Svg {
    render(Icon::Check)
}

pub(crate) fn folder() -> Svg {
    render(Icon::Folder)
}

pub(crate) fn refresh() -> Svg {
    render(Icon::Refresh)
}

pub(crate) fn rotate_ccw() -> Svg {
    render(Icon::RotateCcw)
}

pub(crate) fn external() -> Svg {
    render(Icon::External)
}

pub(crate) fn trash() -> Svg {
    render(Icon::Trash)
}

pub(crate) fn edit() -> Svg {
    render(Icon::Edit)
}

pub(crate) fn download() -> Svg {
    render(Icon::Download)
}

pub(crate) fn wrench() -> Svg {
    render(Icon::Wrench)
}

pub(crate) fn arrow_left() -> Svg {
    render(Icon::ArrowLeft)
}

pub(crate) fn arrow_right() -> Svg {
    render(Icon::ArrowRight)
}

pub(crate) fn chevron_right() -> Svg {
    render(Icon::ChevronRight)
}

pub(crate) fn chevron_down() -> Svg {
    render(Icon::ChevronDown)
}

pub(crate) fn sliders() -> Svg {
    render(Icon::Sliders)
}

pub(crate) fn settings() -> Svg {
    render(Icon::Settings)
}

pub(crate) fn activity() -> Svg {
    render(Icon::Activity)
}

pub(crate) fn power() -> Svg {
    render(Icon::Power)
}

pub(crate) fn window_icon() -> Svg {
    render(Icon::Window)
}

pub(crate) fn search() -> Svg {
    render(Icon::Search)
}

pub(crate) fn close() -> Svg {
    render(Icon::Close)
}

pub(crate) fn more() -> Svg {
    render(Icon::More)
}

pub(crate) fn lock() -> Svg {
    render(Icon::Lock)
}

pub(crate) fn warn() -> Svg {
    render(Icon::Warn)
}

pub(crate) fn globe() -> Svg {
    render(Icon::Globe)
}

pub(crate) fn key() -> Svg {
    render(Icon::Key)
}

pub(crate) fn link() -> Svg {
    render(Icon::Link)
}

pub(crate) fn plug() -> Svg {
    render(Icon::Plug)
}

pub(crate) fn file_text() -> Svg {
    render(Icon::FileText)
}

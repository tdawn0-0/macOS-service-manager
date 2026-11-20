//! The inspector's icons, the Lucide drawings the Go app used (`icons.go`).
//! They are inline SVG, so `currentColor` follows the text color the caller
//! sets and no asset has to ship beside the binary.

use nsh_core::element::{ImageSrc, Svg, svg};

use devcore::service::{Action, Source};

const OPEN: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round">"#;
const CLOSE: &str = "</svg>";

fn icon(body: &str) -> Svg {
    svg(ImageSrc::svg_markup(&format!("{OPEN}{body}{CLOSE}")))
}

/// The icon of a source, as the Go sidebar drew it.
pub(crate) fn source(source: Source) -> Svg {
    match source {
        Source::UserAgent => icon(
            r#"<path d="M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2"/><circle cx="12" cy="7" r="4"/>"#,
        ),
        Source::SystemAgent => icon(
            r#"<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M22 21v-2a4 4 0 0 0-3-3.87"/><path d="M16 3.13a4 4 0 0 1 0 7.75"/>"#,
        ),
        Source::Daemon => icon(
            r#"<rect width="20" height="8" x="2" y="2" rx="2" ry="2"/><rect width="20" height="8" x="2" y="14" rx="2" ry="2"/><line x1="6" x2="6.01" y1="6" y2="6"/><line x1="6" x2="6.01" y1="18" y2="18"/>"#,
        ),
        _ => icon(r#"<circle cx="12" cy="12" r="9"/>"#),
    }
}

/// The icon of an action: play, stop, restart, or the row's first word.
pub(crate) fn action(action: Action) -> Svg {
    match action {
        Action::Start | Action::Load => {
            icon(r#"<polygon points="6 3 20 12 6 21 6 3" fill="currentColor"/>"#)
        }
        Action::Stop => {
            icon(r#"<rect width="14" height="14" x="5" y="5" rx="2" fill="currentColor"/>"#)
        }
        Action::Restart | Action::Unload => icon(
            r#"<path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/>"#,
        ),
        _ => icon(r#"<circle cx="12" cy="12" r="1"/>"#),
    }
}

pub(crate) fn refresh() -> Svg {
    action(Action::Restart)
}

pub(crate) fn search() -> Svg {
    icon(r#"<path d="m21 21-4.34-4.34"/><circle cx="11" cy="11" r="8"/>"#)
}

pub(crate) fn close() -> Svg {
    icon(r#"<path d="M18 6 6 18"/><path d="m6 6 12 12"/>"#)
}

pub(crate) fn more() -> Svg {
    icon(
        r#"<circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/><circle cx="5" cy="12" r="1"/>"#,
    )
}

pub(crate) fn copy() -> Svg {
    icon(
        r#"<rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>"#,
    )
}

pub(crate) fn lock() -> Svg {
    icon(
        r#"<rect width="18" height="11" x="3" y="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/>"#,
    )
}

pub(crate) fn warn() -> Svg {
    icon(
        r#"<path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3"/><path d="M12 9v4"/><path d="M12 17h.01"/>"#,
    )
}

pub(crate) fn external() -> Svg {
    icon(
        r#"<path d="M15 3h6v6"/><path d="M10 14 21 3"/><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/>"#,
    )
}

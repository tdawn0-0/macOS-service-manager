//! The database screens' icons, drawn inline so `currentColor` follows the
//! text color and no asset ships beside the binary.

use nsh_core::element::{ImageSrc, Svg, svg};

const OPEN: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round">"#;
const CLOSE: &str = "</svg>";

fn icon(body: &str) -> Svg {
    svg(ImageSrc::svg_markup(&format!("{OPEN}{body}{CLOSE}")))
}

/// A database engine, as the Go rows drew it.
pub(crate) fn engine(kind: &str) -> Svg {
    match kind {
        "redis" => icon(
            r#"<path d="M4 7h16"/><path d="M4 12h16"/><path d="M4 17h16"/><circle cx="7" cy="7" r="1" fill="currentColor"/><circle cx="7" cy="12" r="1" fill="currentColor"/><circle cx="7" cy="17" r="1" fill="currentColor"/>"#,
        ),
        "mongodb" => icon(
            r#"<path d="M12 2c2.5 3 4 5.5 4 9a4 4 0 0 1-8 0c0-3.5 1.5-6 4-9Z"/><path d="M12 15v7"/>"#,
        ),
        _ => icon(
            r#"<ellipse cx="12" cy="5" rx="9" ry="3"/><path d="M3 5v14a9 3 0 0 0 18 0V5"/><path d="M3 12a9 3 0 0 0 18 0"/>"#,
        ),
    }
}

pub(crate) fn folder() -> Svg {
    icon(
        r#"<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>"#,
    )
}

pub(crate) fn rotate() -> Svg {
    icon(r#"<path d="M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/>"#)
}

pub(crate) fn check() -> Svg {
    icon(r#"<path d="M20 6 9 17l-5-5"/>"#)
}

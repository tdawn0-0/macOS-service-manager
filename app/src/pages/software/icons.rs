//! The software screen's icons, drawn inline so `currentColor` follows the
//! text color and no asset ships beside the binary.

use nsh_core::element::{ImageSrc, Svg, svg};

use crate::store::software::SoftwareKind;

const OPEN: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round">"#;
const CLOSE: &str = "</svg>";

fn icon(body: &str) -> Svg {
    svg(ImageSrc::svg_markup(&format!("{OPEN}{body}{CLOSE}")))
}

/// A runtime or engine in the rail.
pub(crate) fn software(kind: SoftwareKind) -> Svg {
    match kind {
        SoftwareKind::Node => {
            icon(r#"<polyline points="4 17 10 11 4 5"/><line x1="12" x2="20" y1="19" y2="19"/>"#)
        }
        _ => icon(
            r#"<ellipse cx="12" cy="5" rx="9" ry="3"/><path d="M3 5v14a9 3 0 0 0 18 0V5"/><path d="M3 12a9 3 0 0 0 18 0"/>"#,
        ),
    }
}

pub(crate) fn package() -> Svg {
    icon(
        r#"<path d="m7.5 4.27 9 5.15"/><path d="M21 8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16Z"/><path d="M3.3 7 12 12l8.7-5"/><path d="M12 22V12"/>"#,
    )
}

pub(crate) fn folder() -> Svg {
    icon(
        r#"<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>"#,
    )
}

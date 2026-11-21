//! The HTTPS proxy screens' icons, drawn inline so `currentColor` follows the
//! text color and no asset ships beside the binary.

use nsh_core::element::{ImageSrc, Svg, svg};

const OPEN: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round">"#;
const CLOSE: &str = "</svg>";

fn src(body: &str) -> ImageSrc {
    ImageSrc::svg_markup(&format!("{OPEN}{body}{CLOSE}"))
}

fn icon(body: &str) -> Svg {
    svg(src(body))
}

const SHIELD: &str = r#"<path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"/>"#;
const TRASH: &str = r#"<path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/>"#;
const COPY: &str = r#"<rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>"#;
const LINK: &str = r#"<path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/>"#;
const LOGS: &str = r#"<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/><path d="M10 9H8"/><path d="M16 13H8"/><path d="M16 17H8"/>"#;

pub(crate) fn shield() -> Svg {
    icon(SHIELD)
}

pub(crate) fn shield_src() -> ImageSrc {
    src(SHIELD)
}

pub(crate) fn globe() -> Svg {
    icon(
        r#"<circle cx="12" cy="12" r="10"/><path d="M12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20"/><path d="M2 12h20"/>"#,
    )
}

pub(crate) fn key() -> Svg {
    icon(
        r#"<path d="m15.5 7.5 2.3 2.3a1 1 0 0 0 1.4 0l2.1-2.1a1 1 0 0 0 0-1.4L19 4"/><path d="m21 2-9.6 9.6"/><circle cx="7.5" cy="15.5" r="5.5"/>"#,
    )
}

pub(crate) fn trash() -> Svg {
    icon(TRASH)
}

pub(crate) fn trash_src() -> ImageSrc {
    src(TRASH)
}

pub(crate) fn copy() -> Svg {
    icon(COPY)
}

pub(crate) fn copy_src() -> ImageSrc {
    src(COPY)
}

pub(crate) fn link() -> Svg {
    icon(LINK)
}

pub(crate) fn link_src() -> ImageSrc {
    src(LINK)
}

pub(crate) fn logs_src() -> ImageSrc {
    src(LOGS)
}

pub(crate) fn arrow() -> Svg {
    icon(r#"<path d="M5 12h14"/><path d="m12 5 7 7-7 7"/>"#)
}

pub(crate) fn plug() -> Svg {
    icon(
        r#"<path d="M12 22v-5"/><path d="M9 8V2"/><path d="M15 8V2"/><path d="M18 8v5a4 4 0 0 1-4 4h-4a4 4 0 0 1-4-4V8Z"/>"#,
    )
}

pub(crate) fn check() -> Svg {
    icon(r#"<path d="M20 6 9 17l-5-5"/>"#)
}

pub(crate) fn rotate() -> Svg {
    icon(r#"<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/>"#)
}

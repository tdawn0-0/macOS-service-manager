//! Facts about the app, shared by the window shell and the crate's `main`.

/// The display name (`nsh.json` `"name"`).
pub(crate) const NAME: &str = "Devbase";

/// The bundle identifier (`nsh.json` `"id"`).
pub(crate) const IDENTIFIER: &str = "xyz.jyeu.devbase";

/// The user-visible version; `nsh.json` `"version"` carries the same one.
pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The oldest macOS this app supports (`nsh.json` `macos.infoPlist`).
pub(crate) const MINIMUM_MACOS: &str = "26.0";

//! Facts about the app, shared by the window shell and the crate's `main`.

/// The display name (`nsh.json` `"name"`).
pub(crate) const NAME: &str = "Devbase";

/// The bundle identifier (`nsh.json` `"id"`).
pub(crate) const IDENTIFIER: &str = "xyz.jyeu.devbase";

/// The user-visible version; `nsh.json` `"version"` carries the same one.
pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The oldest macOS this app supports (`nsh.json` `macos.infoPlist`).
pub(crate) const MINIMUM_MACOS: &str = "26.0";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_matches_nsh_json_and_cli_version() {
        let manifest: serde_json::Value =
            serde_json::from_str(include_str!("../nsh.json")).expect("valid app/nsh.json");

        assert_eq!(manifest["version"].as_str(), Some(VERSION));
        assert_eq!(manifest["id"].as_str(), Some(IDENTIFIER));
        assert_eq!(manifest["name"].as_str(), Some(NAME));
        assert_eq!(
            manifest["macos"]["infoPlist"]["LSMinimumSystemVersion"].as_str(),
            Some(MINIMUM_MACOS)
        );
        assert_eq!(devcli::schema::CLI_VERSION, VERSION);
    }
}

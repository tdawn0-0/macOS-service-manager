//! The Go-compatible JSON writers and time formatting.
//!
//! The CLI writer reproduces `json.Encoder` with `SetIndent("", "  ")` and
//! `SetEscapeHTML(false)`: two-space indentation, a trailing newline, and
//! literal `<`, `>`, `&`. Go still escapes U+2028 and U+2029 in every mode,
//! which `serde_json` does not, so the bytes are post-processed.
//!
//! Map key ordering comes from `BTreeMap` (or ordered declarations); struct
//! field order is the Rust declaration order, which the CLI view types keep
//! identical to the Go structs.

use std::collections::BTreeMap;
use std::io::{self, Write};

use chrono::{DateTime, Local, SecondsFormat, Utc};
use serde::Serialize;

/// An ordered JSON object with sorted keys, the Go `map` equivalent.
pub type JsonMap = BTreeMap<String, serde_json::Value>;

/// How a `time.Time` is formatted: RFC3339Nano, fraction trimmed, `Z` in UTC.
pub fn go_rfc3339_nano(value: &DateTime<Utc>) -> String {
    let text = value.to_rfc3339_opts(SecondsFormat::Nanos, true);
    trim_fraction(text)
}

pub fn go_rfc3339_nano_local(value: &DateTime<Local>) -> String {
    let text = value.to_rfc3339_opts(SecondsFormat::Nanos, false);
    trim_fraction(text)
}

fn trim_fraction(text: String) -> String {
    let Some(dot) = text.find('.') else {
        return text;
    };
    let zone = text[dot..]
        .find(['Z', '+', '-'])
        .map(|offset| dot + offset)
        .unwrap_or(text.len());
    let fraction = &text[dot + 1..zone];
    let trimmed = fraction.trim_end_matches('0');
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..dot]);
    if !trimmed.is_empty() {
        out.push('.');
        out.push_str(trimmed);
    }
    out.push_str(&text[zone..]);
    out
}

/// Encode a value exactly like the CLI's Go encoder.
pub fn to_go_json(value: &impl Serialize) -> Result<String, serde_json::Error> {
    let pretty = serde_json::to_string_pretty(value)?;
    Ok(escape_go_line_separators(pretty))
}

/// Write a value with a trailing newline, as `json.Encoder.Encode` does.
pub fn write_json(w: &mut dyn Write, value: &impl Serialize) -> io::Result<()> {
    let text = to_go_json(value).map_err(io::Error::other)?;
    w.write_all(text.as_bytes())?;
    w.write_all(b"\n")?;
    w.flush()
}

/// The `{"error","exitCode"}` object written to stderr in `--json` mode.
#[derive(Serialize)]
struct ActionError<'a> {
    error: &'a str,
    #[serde(rename = "exitCode")]
    exit_code: i32,
}

pub fn write_action_error_json(w: &mut dyn Write, message: &str, code: i32) -> io::Result<()> {
    write_json(
        w,
        &ActionError {
            error: message,
            exit_code: code,
        },
    )
}

pub fn write_human_error(w: &mut dyn Write, message: &str) -> io::Result<()> {
    writeln!(w, "Error: {message}")
}

fn escape_go_line_separators(text: String) -> String {
    if !text.contains(['\u{2028}', '\u{2029}']) {
        return text;
    }
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            other => out.push(other),
        }
    }
    out
}

/// Base64 `[]byte` serde, standard alphabet with padding, matching Go.
pub mod base64_serde {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        STANDARD.decode(text).map_err(serde::de::Error::custom)
    }
}

/// `--json` error payload used by tests and callers.
pub fn action_error_json(message: &str, code: i32) -> String {
    let mut text = String::from("{\n  \"error\": ");
    text.push_str(&serde_json::to_string(message).expect("string serializes"));
    text.push_str(",\n  \"exitCode\": ");
    text.push_str(&code.to_string());
    text.push_str("\n}\n");
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_trims_fraction_like_go() {
        let dt = DateTime::parse_from_rfc3339("2026-11-20T12:34:56.120000000Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(go_rfc3339_nano(&dt), "2026-11-20T12:34:56.12Z");
        let dt = DateTime::parse_from_rfc3339("2026-11-20T12:34:56Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(go_rfc3339_nano(&dt), "2026-11-20T12:34:56Z");
    }

    #[test]
    fn escaping_matches_cli_mode() {
        let text = to_go_json(&"<a>&\u{2028}").unwrap();
        assert_eq!(text, "\"<a>&\\u2028\"");
    }
}

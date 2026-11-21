//! Reader for macOS's Background Task Management (`BackgroundItems-v*.btm`)
//! NSKeyedArchiver database files, ported from Go's `internal/loginitem/database.go`.

use std::io::{self, Cursor};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use plist::{Dictionary, Value};
use regex::Regex;
use url::Url;

use crate::launchd::ProcessRunner;

use super::{LoginItemError, Record, TYPE_APP, TYPE_LOGIN_ITEM};

/// The database files are named `BackgroundItems-v<version>.btm`, and from
/// version 18 `BackgroundItems-v<version>-<user>.btm` for each user's records.
static DATABASE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^BackgroundItems-v(\d+)(?:-(.+))?\.btm$").expect("valid regex"));

/// The values of a record's type, and the bit of its disposition that enables
/// it, as `sfltool dumpbtm` prints them: `"app (0x2)"`, `"[enabled, ...] (0xb)"`.
const RECORD_TYPE_APP: i64 = 0x2;
const RECORD_TYPE_LOGIN_ITEM: i64 = 0x4;
const DISPOSITION_ENABLED: i64 = 0x1;

/// Seconds between the Unix epoch (1970-01-01) and the Apple NSDate reference
/// epoch (2001-01-01).
pub(crate) const APPLE_EPOCH_OFFSET: i64 = 978_307_200;

/// Returns the identifier macOS knows the current user by (`GeneratedUID`).
pub(crate) fn generated_uid(runner: &dyn ProcessRunner) -> Result<String, LoginItemError> {
    let name = runner.run("id", &["-un"])?;
    let user_record = format!("/Users/{}", name.trim());
    let out = runner.run("dscl", &[".", "-read", &user_record, "GeneratedUID"])?;
    let fields: Vec<&str> = out.split_whitespace().collect();
    if fields.len() != 2 {
        return Err(LoginItemError::command(format!(
            "unexpected GeneratedUID: {out:?}"
        )));
    }
    Ok(fields[1].to_string())
}

/// Returns the newest database file in `dir` holding `user`'s records.
pub(crate) fn database_file(dir: &Path, user: &str) -> Result<PathBuf, LoginItemError> {
    let entries = std::fs::read_dir(dir).map_err(map_io_error)?;
    let mut best: Option<String> = None;
    let mut best_version: i64 = -1;
    let mut best_own = false;

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        let Some(caps) = DATABASE_RE.captures(name) else {
            continue;
        };
        let file_user = caps.get(2).map_or("", |m| m.as_str());
        if !file_user.is_empty() && file_user != user {
            continue;
        }
        let version: i64 = caps
            .get(1)
            .and_then(|m| m.as_str().parse().ok())
            .unwrap_or(0);
        let own = file_user == user;
        // A version's own file holds the records; its shared one, from
        // version 18, only says whose files there are.
        if version > best_version || (version == best_version && own && !best_own) {
            best = Some(name.to_string());
            best_version = version;
            best_own = own;
        }
    }

    match best {
        Some(name) => Ok(dir.join(name)),
        None => Err(LoginItemError::not_found(format!(
            "no database in {}",
            dir.display()
        ))),
    }
}

pub(crate) fn map_io_error(error: io::Error) -> LoginItemError {
    if error.kind() == io::ErrorKind::PermissionDenied {
        LoginItemError::permission_denied(error.to_string())
    } else if error.kind() == io::ErrorKind::NotFound {
        LoginItemError::not_found(error.to_string())
    } else {
        LoginItemError::io(error.to_string())
    }
}

/// Reads the records of an `NSKeyedArchiver` BTM database file, failing on
/// anything it does not recognize rather than silently listing less than there
/// is.
pub fn parse_database(data: &[u8], user: &str) -> Result<Vec<Record>, LoginItemError> {
    let root = Value::from_reader(Cursor::new(data)).map_err(|error| {
        LoginItemError::database_format(format!("the database is not a property list: {error}"))
    })?;
    let Some(root_dict) = root.as_dictionary() else {
        return Err(LoginItemError::database_format(
            "the database has an unknown format",
        ));
    };
    let objects = root_dict
        .get("$objects")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let Some(top) = root_dict.get("$top").and_then(Value::as_dictionary) else {
        return Err(LoginItemError::database_format(
            "the database has an unknown format",
        ));
    };
    let archive = Archive { objects };

    let items = if let Some(user_store) = top.get("userStore") {
        archive
            .dict(Some(user_store))
            .and_then(|store| store.get("records"))
    } else if let Some(store) = top.get("store") {
        let by_user = archive
            .dict(Some(store))
            .and_then(|s| archive.dict(s.get("itemsByUserIdentifier")));
        let keys = by_user
            .and_then(|d| d.get("NS.keys"))
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let values = by_user
            .and_then(|d| d.get("NS.objects"))
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let mut found = None;
        for (index, key) in keys.iter().enumerate() {
            if archive.str(Some(key)) == user && index < values.len() {
                found = Some(&values[index]);
            }
        }
        match found {
            Some(items) => Some(items),
            None => return Ok(Vec::new()), // the user has no records yet
        }
    } else {
        return Err(LoginItemError::database_format(
            "the database has an unknown format",
        ));
    };

    if archive.dict(items).is_none() {
        return Err(LoginItemError::database_format(
            "the database has an unknown format",
        ));
    }

    let mut records = Vec::new();
    for item in archive.list(items) {
        let Some(dict) = archive.dict(Some(item)) else {
            return Err(LoginItemError::database_format(
                "the database holds an unknown record: \"\"",
            ));
        };
        records.push(archive.record(dict)?);
    }
    Ok(records)
}

/// Reads objects from a keyed archive, in which each refers to others by index.
struct Archive<'a> {
    objects: &'a [Value],
}

impl<'a> Archive<'a> {
    /// Follows `UID` references to a value; the archive's `$null` sentinel
    /// resolves to `None`.
    fn at(&self, mut value: Option<&'a Value>) -> Option<&'a Value> {
        for _ in 0..16 {
            let Some(Value::Uid(uid)) = value else {
                break;
            };
            let index = usize::try_from(uid.get()).ok()?;
            value = self.objects.get(index);
        }
        if let Some(Value::String(text)) = value
            && text == "$null"
        {
            return None;
        }
        value
    }

    fn dict(&self, value: Option<&'a Value>) -> Option<&'a Dictionary> {
        self.at(value)?.as_dictionary()
    }

    fn str(&self, value: Option<&'a Value>) -> String {
        self.at(value)
            .and_then(Value::as_string)
            .unwrap_or("")
            .to_string()
    }

    /// Returns the class name of an archived object.
    fn class(&self, object: &'a Dictionary) -> String {
        self.dict(object.get("$class"))
            .map(|class_dict| self.str(class_dict.get("$classname")))
            .unwrap_or_default()
    }

    /// Returns the objects of an archived `NSArray` / `NSMutableArray`.
    fn list(&self, value: Option<&'a Value>) -> &'a [Value] {
        self.dict(value)
            .and_then(|d| d.get("NS.objects"))
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Reads an archived `ItemRecord`.
    fn record(&self, object: &'a Dictionary) -> Result<Record, LoginItemError> {
        let class = self.class(object);
        if class != "ItemRecord" {
            return Err(LoginItemError::database_format(format!(
                "the database holds an unknown record: {class:?}"
            )));
        }
        let Some(kind) = integer(self.at(object.get("type"))) else {
            return Err(LoginItemError::database_format(
                "the database has a record without a type",
            ));
        };
        let Some(disposition) = integer(self.at(object.get("disposition"))) else {
            return Err(LoginItemError::database_format(
                "the database has a record without a disposition",
            ));
        };

        let record_type = match kind {
            RECORD_TYPE_APP => TYPE_APP.to_string(),
            RECORD_TYPE_LOGIN_ITEM => TYPE_LOGIN_ITEM.to_string(),
            other => format!("0x{other:x}"),
        };

        let uuid = self
            .dict(object.get("uuid"))
            .and_then(|d| d.get("NS.uuidbytes"))
            .and_then(Value::as_data)
            .map(hex_encode)
            .unwrap_or_default();
        if uuid.is_empty() {
            return Err(LoginItemError::database_format(
                "the database has a record without an identifier",
            ));
        }

        let url = self.path(self.dict(object.get("url")), record_type == TYPE_LOGIN_ITEM);

        let last_use = self
            .dict(object.get("lastUseDate"))
            .and_then(|d| d.get("NS.time"))
            .and_then(Value::as_real)
            .map(|seconds| format_local_ymd_hm(APPLE_EPOCH_OFFSET + seconds as i64))
            .unwrap_or_default();

        Ok(Record {
            uuid,
            name: self.str(object.get("name")),
            developer: self.str(object.get("developerName")),
            team: self.str(object.get("teamIdentifier")),
            record_type,
            identifier: self.str(object.get("identifier")),
            url,
            bundle: self.str(object.get("bundleIdentifier")),
            parent: self.str(object.get("container")),
            last_use,
            enabled: (disposition & DISPOSITION_ENABLED) != 0,
        })
    }

    /// Returns the decoded path of an archived `NSURL`. An app's URL is a full
    /// `file://` URL; a helper's is relative to its parent app bundle, such as
    /// `"Contents/Library/LoginItems/My%20Helper.app"` on a base of `"file:///"`.
    fn path(&self, object: Option<&'a Dictionary>, relative: bool) -> String {
        let Some(object) = object else {
            return String::new();
        };
        let raw = self.str(object.get("NS.relative"));
        if raw.is_empty() {
            return String::new();
        }
        let decoded = match Url::parse(&raw) {
            Ok(parsed) => {
                if parsed.scheme() != "file" {
                    return raw;
                }
                match percent_decode(parsed.path()) {
                    Some(path) => path,
                    None => return raw,
                }
            }
            Err(url::ParseError::RelativeUrlWithoutBase) => {
                let path_part = raw
                    .split_once(['?', '#'])
                    .map_or(raw.as_str(), |(before, _)| before);
                match percent_decode(path_part) {
                    Some(path) => path,
                    None => return raw,
                }
            }
            Err(_) => return raw,
        };
        let mut path = decoded.strip_suffix('/').unwrap_or(&decoded);
        if relative {
            path = path.strip_prefix('/').unwrap_or(path);
        }
        path.to_string()
    }
}

fn integer(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    value.as_signed_integer().or_else(|| {
        value
            .as_unsigned_integer()
            .and_then(|u| i64::try_from(u).ok())
    })
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return None;
            }
            let hi = hex_digit(bytes[index + 1])?;
            let lo = hex_digit(bytes[index + 2])?;
            out.push((hi << 4) | lo);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Formats a Unix timestamp in local time as `"YYYY-MM-DD HH:MM"`, matching
/// Go's `time.Unix(unix_secs, 0).Format("2006-01-02 15:04")`.
#[cfg(unix)]
pub(crate) fn format_local_ymd_hm(unix_secs: i64) -> String {
    // SAFETY: `raw` is a valid time_t and `tm` is properly zero-initialized.
    unsafe {
        let raw = unix_secs as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&raw, &mut tm).is_null() {
            return String::new();
        }
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
        )
    }
}

#[cfg(not(unix))]
pub(crate) fn format_local_ymd_hm(_unix_secs: i64) -> String {
    String::new()
}

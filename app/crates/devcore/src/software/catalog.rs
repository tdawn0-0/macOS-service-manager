//! Catalog resolution, ported from the Go `catalog.go:18-163`.
//!
//! MySQL and MongoDB are exact pinned releases (URL + SHA-256) so a saved
//! package never floats. Node is resolved against the upstream
//! `SHASUMS256.txt` and must name an exact version; the caller resolves `22`,
//! `lts` or `latest` first. Every returned manifest is validated.

use std::collections::BTreeMap;

use super::{HttpTransport, SoftwareError};
use crate::model::Package;

struct ArchiveRelease {
    version: &'static str,
    url: &'static str,
    sha256: &'static str,
}

const MYSQL_RELEASES: &[ArchiveRelease] = &[
    ArchiveRelease {
        version: "8.4.13",
        url: "https://dev.mysql.com/get/Downloads/MySQL-8.4/mysql-8.4.13-macos15-arm64.tar.gz",
        sha256: "f324f8950e24d4147470bbc6a6ef0b90ef93b91d3fc74154b3d2ba085b44912a",
    },
    ArchiveRelease {
        version: "8.4.4",
        url: "https://cdn.mysql.com/archives/mysql-8.4/mysql-8.4.4-macos15-arm64.tar.gz",
        sha256: "a23a5beada1404a609ee8f2a7b14f2869444fe05b5731f94a3d2215d4f6c41b5",
    },
    ArchiveRelease {
        version: "8.0.45",
        url: "https://cdn.mysql.com/archives/mysql-8.0/mysql-8.0.45-macos15-arm64.tar.gz",
        sha256: "3b8bd89b839e663479775a99deb03ff6cde11c86a1c37c0aed7116a778e4a8bc",
    },
    ArchiveRelease {
        version: "8.0.41",
        url: "https://cdn.mysql.com/archives/mysql-8.0/mysql-8.0.41-macos15-arm64.tar.gz",
        sha256: "37ed31ff5013b6cc964b2ad071da941d5f9cb423c11c615996e216f6224c889f",
    },
];

const MONGODB_RELEASES: &[ArchiveRelease] = &[
    ArchiveRelease {
        version: "8.0.38",
        url: "https://fastdl.mongodb.org/osx/mongodb-macos-arm64-8.0.38.tgz",
        sha256: "5c372533187a7789b6b2f4d7856f0651557dfbed572dbe5f4c23f43578fc74df",
    },
    ArchiveRelease {
        version: "8.0.5",
        url: "https://fastdl.mongodb.org/osx/mongodb-macos-arm64-8.0.5.tgz",
        sha256: "71e2510f0455ff24e1961aa0507eb0660b300c84f6da9899052be409090fb990",
    },
    ArchiveRelease {
        version: "7.0.30",
        url: "https://fastdl.mongodb.org/osx/mongodb-macos-arm64-7.0.30.tgz",
        sha256: "6c6c6fcfc38e025d48e99c6f5892a9e6aa8012ccb08ce247787936423d634dd1",
    },
    ArchiveRelease {
        version: "7.0.16",
        url: "https://fastdl.mongodb.org/osx/mongodb-macos-arm64-7.0.16.tgz",
        sha256: "e01c5ce1ef8efbef4196797130564b8f121f6e29f20ccf70e025feba07e15739",
    },
];

const NODE_DIST: &str = "https://nodejs.org/dist/";

/// The UI's Node list; resolution still requires an exact version.
pub const NODE_VERSIONS: &[&str] = &["22.14.0", "20.19.0"];

/// The version list the UI offers for `name` (empty when unknown).
pub fn available_versions(name: &str) -> Vec<String> {
    match name {
        "node" => NODE_VERSIONS
            .iter()
            .map(|value| value.to_string())
            .collect(),
        "mysql" => release_versions(MYSQL_RELEASES),
        "postgresql" => vec!["17".into(), "18".into(), "16".into()],
        "redis" => vec!["8.2".into()],
        "mongodb" => release_versions(MONGODB_RELEASES),
        _ => Vec::new(),
    }
}

fn release_versions(releases: &[ArchiveRelease]) -> Vec<String> {
    releases
        .iter()
        .map(|release| release.version.to_string())
        .collect()
}

fn release_list(releases: &[ArchiveRelease]) -> String {
    release_versions(releases).join(", ")
}

/// Resolve a pinned archive release. Homebrew (PostgreSQL/Redis) and Node need
/// the software manager for their command/HTTP seams; Java belongs to the Java
/// catalog slice.
pub fn resolve(name: &str, version: &str) -> Result<Package, SoftwareError> {
    let version = version.strip_prefix('v').unwrap_or(version);
    let mut package = Package {
        name: name.to_string(),
        version: version.to_string(),
        arch: "arm64".into(),
        format: "tar.gz".into(),
        strip: 1,
        ..Package::default()
    };
    if !crate::model::valid_id(&package.name) || !crate::model::valid_id(&package.version) {
        return Err(SoftwareError::invalid("choose an exact package version"));
    }
    match name {
        "mysql" => {
            let release = find_release(MYSQL_RELEASES, version).ok_or_else(|| {
                SoftwareError::invalid(format!(
                    "supported MySQL versions: {}",
                    release_list(MYSQL_RELEASES)
                ))
            })?;
            package.url = release.url.into();
            package.sha256 = release.sha256.into();
            package.min_macos = "26.0".into();
            package.license = "GPL-2.0 and bundled component licenses".into();
            package.binaries = binaries([
                ("mysqld", "bin/mysqld"),
                ("mysql", "bin/mysql"),
                ("mysqldump", "bin/mysqldump"),
            ]);
            package.validate()?;
            Ok(package)
        }
        "mongodb" => {
            let release = find_release(MONGODB_RELEASES, version).ok_or_else(|| {
                SoftwareError::invalid(format!(
                    "supported MongoDB versions: {}",
                    release_list(MONGODB_RELEASES)
                ))
            })?;
            package.url = release.url.into();
            package.sha256 = release.sha256.into();
            package.min_macos = "26.0".into();
            package.license = "SSPL-1.0".into();
            package.binaries = binaries([("mongod", "bin/mongod"), ("mongos", "bin/mongos")]);
            package.validate()?;
            Ok(package)
        }
        "postgresql" | "redis" => Err(SoftwareError::invalid(
            "Homebrew packages must be resolved through the software manager",
        )),
        "node" => Err(SoftwareError::invalid(
            "Node.js versions must be resolved against the upstream checksum list",
        )),
        "java" => Err(SoftwareError::invalid(
            "Java versions are resolved by the Java catalog",
        )),
        _ => Err(SoftwareError::invalid(
            "use an explicit verified package manifest for this software",
        )),
    }
}

/// Resolve an exact Node.js version against its upstream `SHASUMS256.txt`.
pub fn resolve_node(http: &dyn HttpTransport, version: &str) -> Result<Package, SoftwareError> {
    let version = version.strip_prefix('v').unwrap_or(version);
    if !is_exact_runtime_version(version) {
        return Err(SoftwareError::invalid(
            "choose an exact Node.js version, for example 22.14.0; resolve 22, lts or latest first",
        ));
    }
    let mut package = Package {
        name: "node".into(),
        version: version.into(),
        arch: "arm64".into(),
        format: "tar.gz".into(),
        strip: 1,
        ..Package::default()
    };
    if !crate::model::valid_id(&package.name) || !crate::model::valid_id(&package.version) {
        return Err(SoftwareError::invalid("choose an exact package version"));
    }
    let filename = format!("node-v{version}-darwin-arm64.tar.gz");
    let base = format!("{NODE_DIST}v{version}/");
    let body = http.get(&format!("{base}SHASUMS256.txt"))?;
    if body.len() > 1 << 20 {
        return Err(SoftwareError::invalid("node catalog: response too large"));
    }
    let text = String::from_utf8_lossy(&body);
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() == 2 && fields[1] == filename {
            package.url = format!("{base}{filename}");
            package.sha256 = fields[0].to_string();
            package.binaries =
                binaries([("node", "bin/node"), ("npm", "bin/npm"), ("npx", "bin/npx")]);
            package.license = "MIT".into();
            package.validate()?;
            return Ok(package);
        }
    }
    Err(SoftwareError::invalid(
        "requested version is not available for this architecture",
    ))
}

fn find_release<'a>(releases: &'a [ArchiveRelease], version: &str) -> Option<&'a ArchiveRelease> {
    releases.iter().find(|release| release.version == version)
}

fn binaries<const N: usize>(entries: [(&str, &str); N]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(name, path)| ((*name).to_string(), (*path).to_string()))
        .collect()
}

/// The Go `exactRuntimeVersion` shape.
pub fn is_exact_runtime_version(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

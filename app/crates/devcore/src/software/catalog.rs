//! Catalog resolution, ported from the Go `catalog.go:18-163`.
//!
//! MySQL and MongoDB are exact pinned releases (URL + SHA-256) so a saved
//! package never floats. Node is resolved against the upstream
//! `SHASUMS256.txt` and must name an exact version; the caller resolves `22`,
//! `lts` or `latest` first. Every returned manifest is validated.

use std::collections::BTreeMap;

use super::{HttpTransport, SoftwareError};
use crate::model::Package;
use crate::platform::{HostArch, HostOs, HostTarget};

struct ArchiveRelease {
    version: &'static str,
    url: &'static str,
    sha256: &'static str,
    format: &'static str,
}

const MYSQL_RELEASES_MACOS_ARM64: &[ArchiveRelease] = &[
    ArchiveRelease {
        version: "8.4.13",
        url: "https://dev.mysql.com/get/Downloads/MySQL-8.4/mysql-8.4.13-macos15-arm64.tar.gz",
        sha256: "f324f8950e24d4147470bbc6a6ef0b90ef93b91d3fc74154b3d2ba085b44912a",
        format: "tar.gz",
    },
    ArchiveRelease {
        version: "8.4.4",
        url: "https://cdn.mysql.com/archives/mysql-8.4/mysql-8.4.4-macos15-arm64.tar.gz",
        sha256: "a23a5beada1404a609ee8f2a7b14f2869444fe05b5731f94a3d2215d4f6c41b5",
        format: "tar.gz",
    },
    ArchiveRelease {
        version: "8.0.45",
        url: "https://cdn.mysql.com/archives/mysql-8.0/mysql-8.0.45-macos15-arm64.tar.gz",
        sha256: "3b8bd89b839e663479775a99deb03ff6cde11c86a1c37c0aed7116a778e4a8bc",
        format: "tar.gz",
    },
    ArchiveRelease {
        version: "8.0.41",
        url: "https://cdn.mysql.com/archives/mysql-8.0/mysql-8.0.41-macos15-arm64.tar.gz",
        sha256: "37ed31ff5013b6cc964b2ad071da941d5f9cb423c11c615996e216f6224c889f",
        format: "tar.gz",
    },
];

const MYSQL_RELEASES_LINUX_AMD64: &[ArchiveRelease] = &[
    ArchiveRelease {
        version: "8.4.13",
        url: "https://dev.mysql.com/get/Downloads/MySQL-8.4/mysql-8.4.13-linux-glibc2.28-x86_64.tar.xz",
        sha256: "e32f82598aef1384d2bbb8e57a1c684f9b7734409ae8028b363fb913e56b8e92",
        format: "tar.xz",
    },
    ArchiveRelease {
        version: "8.0.45",
        url: "https://cdn.mysql.com/archives/mysql-8.0/mysql-8.0.45-linux-glibc2.28-x86_64.tar.xz",
        sha256: "c09137539ab42590c8682d498716e6b97a45d83d2188339fa2e980dd54b6e0af",
        format: "tar.xz",
    },
];

const MYSQL_RELEASES_LINUX_ARM64: &[ArchiveRelease] = &[
    ArchiveRelease {
        version: "8.4.13",
        url: "https://dev.mysql.com/get/Downloads/MySQL-8.4/mysql-8.4.13-linux-glibc2.28-aarch64.tar.xz",
        sha256: "6531bb50f64c986a2abdab6b647cbd553e8740b3e5d3867f5415e33568879fc9",
        format: "tar.xz",
    },
    ArchiveRelease {
        version: "8.0.45",
        url: "https://cdn.mysql.com/archives/mysql-8.0/mysql-8.0.45-linux-glibc2.28-aarch64.tar.xz",
        sha256: "2bbbdd107bc02bd6a135d5140b6a790de5ed4d4ff5389803600c26c79e208d2a",
        format: "tar.xz",
    },
];

const MONGODB_RELEASES_MACOS_ARM64: &[ArchiveRelease] = &[
    ArchiveRelease {
        version: "8.0.38",
        url: "https://fastdl.mongodb.org/osx/mongodb-macos-arm64-8.0.38.tgz",
        sha256: "5c372533187a7789b6b2f4d7856f0651557dfbed572dbe5f4c23f43578fc74df",
        format: "tar.gz",
    },
    ArchiveRelease {
        version: "8.0.5",
        url: "https://fastdl.mongodb.org/osx/mongodb-macos-arm64-8.0.5.tgz",
        sha256: "71e2510f0455ff24e1961aa0507eb0660b300c84f6da9899052be409090fb990",
        format: "tar.gz",
    },
    ArchiveRelease {
        version: "7.0.30",
        url: "https://fastdl.mongodb.org/osx/mongodb-macos-arm64-7.0.30.tgz",
        sha256: "6c6c6fcfc38e025d48e99c6f5892a9e6aa8012ccb08ce247787936423d634dd1",
        format: "tar.gz",
    },
    ArchiveRelease {
        version: "7.0.16",
        url: "https://fastdl.mongodb.org/osx/mongodb-macos-arm64-7.0.16.tgz",
        sha256: "e01c5ce1ef8efbef4196797130564b8f121f6e29f20ccf70e025feba07e15739",
        format: "tar.gz",
    },
];

const MONGODB_RELEASES_LINUX_AMD64: &[ArchiveRelease] = &[
    ArchiveRelease {
        version: "8.0.38",
        url: "https://fastdl.mongodb.org/linux/mongodb-linux-x86_64-ubuntu2404-8.0.38.tgz",
        sha256: "2885d4382bd47b96bfaf3edc7dec0ef44463e3e2a79479f584d20deb6182cda9",
        format: "tar.gz",
    },
    ArchiveRelease {
        version: "8.0.5",
        url: "https://fastdl.mongodb.org/linux/mongodb-linux-x86_64-ubuntu2404-8.0.5.tgz",
        sha256: "6edeea38310084fa50b2428015352c1fd4eae98b6defa82a42b68a814f5563cd",
        format: "tar.gz",
    },
    ArchiveRelease {
        version: "7.0.30",
        url: "https://fastdl.mongodb.org/linux/mongodb-linux-x86_64-ubuntu2204-7.0.30.tgz",
        sha256: "2a6eb856a481a8bca47601a1ad62d26a58adf1999f1a023595de42f94f900d2a",
        format: "tar.gz",
    },
    ArchiveRelease {
        version: "7.0.16",
        url: "https://fastdl.mongodb.org/linux/mongodb-linux-x86_64-ubuntu2204-7.0.16.tgz",
        sha256: "376c258ae9b104b88814214bc3214a2e0d1300d8192d24d8c46259f364866422",
        format: "tar.gz",
    },
];

const MONGODB_RELEASES_LINUX_ARM64: &[ArchiveRelease] = &[
    ArchiveRelease {
        version: "8.0.38",
        url: "https://fastdl.mongodb.org/linux/mongodb-linux-aarch64-ubuntu2404-8.0.38.tgz",
        sha256: "178f6a9ef70637626f44f9edda779848bb8c72000cd5f8f46c9eb5b2aa1d26d7",
        format: "tar.gz",
    },
    ArchiveRelease {
        version: "8.0.5",
        url: "https://fastdl.mongodb.org/linux/mongodb-linux-aarch64-ubuntu2404-8.0.5.tgz",
        sha256: "565cf494f07cebcefc8b327d80ef9ee5effb213c0c7d574c66a8dc97a44ed40c",
        format: "tar.gz",
    },
    ArchiveRelease {
        version: "7.0.30",
        url: "https://fastdl.mongodb.org/linux/mongodb-linux-aarch64-ubuntu2204-7.0.30.tgz",
        sha256: "f0fbf57ceeb0e5eb298efb089f4fb4758cc5a19ff5d8dec31f05d114aa15f72a",
        format: "tar.gz",
    },
    ArchiveRelease {
        version: "7.0.16",
        url: "https://fastdl.mongodb.org/linux/mongodb-linux-aarch64-ubuntu2204-7.0.16.tgz",
        sha256: "0259920a7349cb7b52aa22d54cd8cb6e6989120a612ee3b32972a7c7548a221a",
        format: "tar.gz",
    },
];

fn mysql_releases_for(target: HostTarget) -> &'static [ArchiveRelease] {
    match (target.os, target.arch) {
        (HostOs::MacOs, _) => MYSQL_RELEASES_MACOS_ARM64,
        (HostOs::Linux, HostArch::Amd64) => MYSQL_RELEASES_LINUX_AMD64,
        (HostOs::Linux, HostArch::Arm64) => MYSQL_RELEASES_LINUX_ARM64,
    }
}

fn mongodb_releases_for(target: HostTarget) -> &'static [ArchiveRelease] {
    match (target.os, target.arch) {
        (HostOs::MacOs, _) => MONGODB_RELEASES_MACOS_ARM64,
        (HostOs::Linux, HostArch::Amd64) => MONGODB_RELEASES_LINUX_AMD64,
        (HostOs::Linux, HostArch::Arm64) => MONGODB_RELEASES_LINUX_ARM64,
    }
}

const NODE_DIST: &str = "https://nodejs.org/dist/";

/// The UI's Node list; resolution still requires an exact version.
pub const NODE_VERSIONS: &[&str] = &["22.14.0", "20.19.0"];

/// The version list the UI offers for `name` on the compiled host target (empty when unknown).
pub fn available_versions(name: &str) -> Vec<String> {
    available_versions_for_target(name, HostTarget::current())
}

/// The version list the UI offers for `name` on an explicit [`HostTarget`].
pub fn available_versions_for_target(name: &str, target: HostTarget) -> Vec<String> {
    match name {
        "node" => NODE_VERSIONS
            .iter()
            .map(|value| value.to_string())
            .collect(),
        "mysql" => release_versions(mysql_releases_for(target)),
        "postgresql" => {
            if target.is_linux() {
                vec!["system".into(), "17".into(), "18".into(), "16".into()]
            } else {
                vec!["17".into(), "18".into(), "16".into()]
            }
        }
        "redis" => {
            if target.is_linux() {
                vec!["system".into(), "8.2".into()]
            } else {
                vec!["8.2".into()]
            }
        }
        "mongodb" => release_versions(mongodb_releases_for(target)),
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

/// Resolve a pinned archive release for the compiled host target. Homebrew /
/// system (PostgreSQL/Redis) and Node need the software manager for their
/// command/HTTP seams; Java belongs to the Java catalog slice.
pub fn resolve(name: &str, version: &str) -> Result<Package, SoftwareError> {
    resolve_for_target(name, version, HostTarget::current())
}

/// Resolve a pinned archive release for an explicit [`HostTarget`].
pub fn resolve_for_target(
    name: &str,
    version: &str,
    target: HostTarget,
) -> Result<Package, SoftwareError> {
    let version = version.strip_prefix('v').unwrap_or(version);
    let mut package = Package {
        name: name.to_string(),
        version: version.to_string(),
        arch: target.arch_name().into(),
        format: "tar.gz".into(),
        strip: 1,
        ..Package::default()
    };
    if !crate::model::valid_id(&package.name) || !crate::model::valid_id(&package.version) {
        return Err(SoftwareError::invalid("choose an exact package version"));
    }
    match name {
        "mysql" => {
            let releases = mysql_releases_for(target);
            let release = find_release(releases, version).ok_or_else(|| {
                SoftwareError::invalid(format!(
                    "supported MySQL versions: {}",
                    release_list(releases)
                ))
            })?;
            package.url = release.url.into();
            package.sha256 = release.sha256.into();
            package.format = release.format.into();
            if target.is_macos() {
                package.min_macos = "26.0".into();
            }
            package.license = "GPL-2.0 and bundled component licenses".into();
            package.binaries = binaries([
                ("mysqld", "bin/mysqld"),
                ("mysql", "bin/mysql"),
                ("mysqldump", "bin/mysqldump"),
            ]);
            package.validate_for_target(target)?;
            Ok(package)
        }
        "mongodb" => {
            let releases = mongodb_releases_for(target);
            let release = find_release(releases, version).ok_or_else(|| {
                SoftwareError::invalid(format!(
                    "supported MongoDB versions: {}",
                    release_list(releases)
                ))
            })?;
            package.url = release.url.into();
            package.sha256 = release.sha256.into();
            package.format = release.format.into();
            if target.is_macos() {
                package.min_macos = "26.0".into();
            }
            package.license = "SSPL-1.0".into();
            package.binaries = binaries([("mongod", "bin/mongod"), ("mongos", "bin/mongos")]);
            package.validate_for_target(target)?;
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

/// Resolve an exact Node.js version against its upstream `SHASUMS256.txt` for
/// the compiled host target.
pub fn resolve_node(http: &dyn HttpTransport, version: &str) -> Result<Package, SoftwareError> {
    resolve_node_for_target(http, version, HostTarget::current())
}

/// Resolve an exact Node.js version against its upstream `SHASUMS256.txt` for
/// an explicit [`HostTarget`].
pub fn resolve_node_for_target(
    http: &dyn HttpTransport,
    version: &str,
    target: HostTarget,
) -> Result<Package, SoftwareError> {
    let version = version.strip_prefix('v').unwrap_or(version);
    if !is_exact_runtime_version(version) {
        return Err(SoftwareError::invalid(
            "choose an exact Node.js version, for example 22.14.0; resolve 22, lts or latest first",
        ));
    }
    let mut package = Package {
        name: "node".into(),
        version: version.into(),
        arch: target.arch_name().into(),
        format: "tar.gz".into(),
        strip: 1,
        ..Package::default()
    };
    if !crate::model::valid_id(&package.name) || !crate::model::valid_id(&package.version) {
        return Err(SoftwareError::invalid("choose an exact package version"));
    }
    let platform = target.node_archive_platform();
    let candidates: Vec<(String, &'static str)> = if target.is_linux() {
        vec![
            (format!("node-v{version}-{platform}.tar.xz"), "tar.xz"),
            (format!("node-v{version}-{platform}.tar.gz"), "tar.gz"),
        ]
    } else {
        vec![(format!("node-v{version}-{platform}.tar.gz"), "tar.gz")]
    };
    let base = format!("{NODE_DIST}v{version}/");
    let body = http.get(&format!("{base}SHASUMS256.txt"))?;
    if body.len() > 1 << 20 {
        return Err(SoftwareError::invalid("node catalog: response too large"));
    }
    let text = String::from_utf8_lossy(&body);
    for (filename, format) in &candidates {
        for line in text.lines() {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() == 2 && fields[1] == filename.as_str() {
                package.url = format!("{base}{filename}");
                package.sha256 = fields[0].to_string();
                package.format = (*format).to_string();
                package.binaries =
                    binaries([("node", "bin/node"), ("npm", "bin/npm"), ("npx", "bin/npx")]);
                package.license = "MIT".into();
                package.validate_for_target(target)?;
                return Ok(package);
            }
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

#[cfg(test)]
mod linux_catalog_tests {
    use std::path::Path;

    use super::*;

    struct StubHttp(Vec<u8>);
    impl HttpTransport for StubHttp {
        fn download(&self, _: &str, _: &Path) -> Result<u64, SoftwareError> {
            unimplemented!()
        }
        fn get(&self, _: &str) -> Result<Vec<u8>, SoftwareError> {
            Ok(self.0.clone())
        }
        fn size(&self, _: &str) -> Result<Option<i64>, SoftwareError> {
            Ok(None)
        }
    }

    #[test]
    fn linux_catalog_resolves_mysql_and_mongodb_for_x86_64_and_aarch64() {
        for (target, mysql_arch, mongo_arch, model_arch) in [
            (HostTarget::LINUX_AMD64, "x86_64", "x86_64", "amd64"),
            (HostTarget::LINUX_ARM64, "aarch64", "aarch64", "arm64"),
        ] {
            let mysql = resolve_for_target("mysql", "8.4.13", target).unwrap();
            assert_eq!(mysql.arch, model_arch);
            assert_eq!(mysql.format, "tar.xz");
            assert!(mysql.min_macos.is_empty());
            assert!(
                mysql
                    .url
                    .contains(&format!("linux-glibc2.28-{mysql_arch}.tar.xz"))
            );
            assert!(mysql.validate_for_target(target).is_ok());

            let mysql80 = resolve_for_target("mysql", "8.0.45", target).unwrap();
            assert_eq!(mysql80.format, "tar.xz");
            assert!(mysql80.validate_for_target(target).is_ok());

            for ver in ["8.0.38", "8.0.5", "7.0.30", "7.0.16"] {
                let mongo = resolve_for_target("mongodb", ver, target).unwrap();
                assert_eq!(mongo.arch, model_arch);
                assert_eq!(mongo.format, "tar.gz");
                assert!(mongo.min_macos.is_empty());
                assert!(mongo.url.contains(&format!("mongodb-linux-{mongo_arch}-")));
                assert!(mongo.validate_for_target(target).is_ok());
            }
        }
    }

    #[test]
    fn linux_node_catalog_prefers_tar_xz_and_falls_back_to_tar_gz() {
        let sha_xz = "1".repeat(64);
        let sha_gz = "2".repeat(64);
        let both = format!(
            "{sha_gz}  node-v22.14.0-linux-x64.tar.gz\n{sha_xz}  node-v22.14.0-linux-x64.tar.xz\n"
        );
        let pkg = resolve_node_for_target(
            &StubHttp(both.into_bytes()),
            "22.14.0",
            HostTarget::LINUX_AMD64,
        )
        .unwrap();
        assert_eq!(pkg.arch, "amd64");
        assert_eq!(pkg.format, "tar.xz");
        assert_eq!(pkg.sha256, sha_xz);
        assert!(pkg.url.ends_with("node-v22.14.0-linux-x64.tar.xz"));

        let gz_only = format!("{sha_gz}  node-v22.14.0-linux-arm64.tar.gz\n");
        let pkg_arm = resolve_node_for_target(
            &StubHttp(gz_only.into_bytes()),
            "22.14.0",
            HostTarget::LINUX_ARM64,
        )
        .unwrap();
        assert_eq!(pkg_arm.arch, "arm64");
        assert_eq!(pkg_arm.format, "tar.gz");
        assert_eq!(pkg_arm.sha256, sha_gz);
        assert!(pkg_arm.url.ends_with("node-v22.14.0-linux-arm64.tar.gz"));
    }
}

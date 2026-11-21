//! The JetBrains JDK catalog and Java runtime defaults, ported from
//! `java_catalog.go`, `runtime_defaults.go` and `runtime_fish.go`.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::{Shell, ShellError, fish_quote, shell_quote};
use crate::model::Package;
use crate::platform::{HostArch, HostTarget};
use crate::software::HttpTransport;
use crate::state::Tx;

/// The JetBrains JDK feed. IDEA uses the `.xz` representation; the same JSON
/// is served uncompressed, so no extra decompressor is linked.
pub const JAVA_CATALOG_URL: &str = "https://download.jetbrains.com/jdk/feed/v1/jdks.json";

/// A parsed catalog, kept as a named type for callers that cache it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JavaCatalog {
    pub packages: Vec<Package>,
}

#[derive(Debug, Deserialize)]
struct Feed {
    #[serde(default)]
    jdks: Option<Vec<Product>>,
}

#[derive(Debug, Deserialize)]
struct Product {
    #[serde(default)]
    vendor: String,
    #[serde(default)]
    product: String,
    #[serde(default)]
    flavour: String,
    #[serde(default, rename = "jdk_version")]
    version: String,
    #[serde(default, rename = "jdk_vendor_version")]
    vendor_version: String,
    #[serde(default)]
    filter: serde_json::Value,
    #[serde(default)]
    listed: serde_json::Value,
    #[serde(default)]
    preview: serde_json::Value,
    #[serde(default)]
    packages: Vec<FeedPackage>,
}

#[derive(Debug, Deserialize)]
struct FeedPackage {
    #[serde(default, rename = "os")]
    os_name: String,
    #[serde(default)]
    arch: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    sha256: String,
    #[serde(default, rename = "package_type")]
    package_type: String,
    #[serde(default, rename = "package_root_prefix")]
    root: String,
    #[serde(default, rename = "package_to_java_home_prefix")]
    home: String,
    #[serde(default, rename = "install_folder_name")]
    folder: String,
    #[serde(default)]
    filter: serde_json::Value,
}

/// Evaluate a feed predicate for `expected_arch`. Unknown predicates fail closed.
fn feed_predicate(raw: &serde_json::Value, absent: bool, expected_arch: &str) -> bool {
    match raw {
        serde_json::Value::Null => absent,
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::Object(object) => {
            let kind = object
                .get("type")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            match kind {
                "const" => object
                    .get("value")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false),
                "supports_arch" => match object.get("arch").and_then(|value| value.as_str()) {
                    None | Some("") => true,
                    Some(arch) => arch == expected_arch,
                },
                "build_number_range" => true,
                "and" => {
                    let Some(items) = object.get("items").and_then(|value| value.as_array()) else {
                        return false;
                    };
                    !items.is_empty()
                        && items
                            .iter()
                            .all(|item| feed_predicate(item, false, expected_arch))
                }
                "or" => object
                    .get("items")
                    .and_then(|value| value.as_array())
                    .is_some_and(|items| {
                        items
                            .iter()
                            .any(|item| feed_predicate(item, false, expected_arch))
                    }),
                "not" => {
                    let Some(item) = object.get("item") else {
                        return false;
                    };
                    let known = match item {
                        serde_json::Value::Bool(_) => true,
                        serde_json::Value::Object(inner) => matches!(
                            inner.get("type").and_then(|value| value.as_str()),
                            Some("const") | Some("supports_arch") | Some("build_number_range")
                        ),
                        _ => false,
                    };
                    known && !feed_predicate(item, false, expected_arch)
                }
                _ => false,
            }
        }
        _ => false,
    }
}

/// Parse and filter the JetBrains feed into installable Java manifests for the
/// compiled host target.
pub fn java_catalog(http: &dyn HttpTransport) -> Result<Vec<Package>, ShellError> {
    java_catalog_for_target(http, HostTarget::current())
}

/// Parse and filter the JetBrains feed into installable Java manifests for an
/// explicit [`HostTarget`].
pub fn java_catalog_for_target(
    http: &dyn HttpTransport,
    target: HostTarget,
) -> Result<Vec<Package>, ShellError> {
    let body = http.get(JAVA_CATALOG_URL).map_err(ShellError::Software)?;
    let feed: Feed = serde_json::from_slice(&body)
        .map_err(|error| ShellError::invalid(format!("JetBrains JDK catalog: {error}")))?;
    let Some(products) = feed.jdks else {
        return Err(ShellError::invalid(
            "JetBrains JDK catalog is missing its version list",
        ));
    };
    let want_os = target.jetbrains_os();
    let want_arch = target.jetbrains_arch();
    let mut found: Vec<Package> = Vec::new();
    for product in products {
        if product.vendor.is_empty()
            || !feed_predicate(&product.filter, true, want_arch)
            || !feed_predicate(&product.listed, true, want_arch)
            || feed_predicate(&product.preview, false, want_arch)
        {
            continue;
        }
        for item in product.packages {
            if !item.os_name.eq_ignore_ascii_case(want_os)
                || item.arch != want_arch
                || !feed_predicate(&item.filter, true, want_arch)
            {
                continue;
            }
            let format = match item.package_type.as_str() {
                "targz" => "tar.gz",
                "zip" => "zip",
                _ => continue,
            };
            let strip = if item.root.is_empty() || item.root == "." {
                0
            } else {
                if !crate::fs::safe_relative(&item.root) {
                    continue;
                }
                let cleaned = crate::fs::clean_path(Path::new(&item.root.replace('\\', "/")));
                cleaned
                    .to_string_lossy()
                    .split('/')
                    .filter(|part| !part.is_empty())
                    .count() as i64
            };
            let version = if item.version.is_empty() {
                product.version.clone()
            } else {
                item.version.clone()
            };
            let mut label = format!("{} {}", product.vendor, product.product)
                .trim()
                .to_string();
            if !product.flavour.is_empty() {
                label.push_str(&format!(" ({})", product.flavour));
            }
            label.push_str(&format!(" · {version}"));
            if !product.vendor_version.is_empty() {
                label.push_str(&format!(" · {}", product.vendor_version));
            }
            let key = item.folder.replace('+', "_");
            let home = if item.home.is_empty() {
                PathBuf::new()
            } else {
                PathBuf::from(&item.home)
            };
            let package = Package {
                java_install_name: item.folder.clone(),
                java_product: label,
                name: "java".into(),
                version: key.clone(),
                arch: target.arch_name().into(),
                url: item.url,
                sha256: item.sha256.to_lowercase(),
                format: format.into(),
                strip,
                binaries: [
                    (
                        "java".to_string(),
                        home.join("bin/java").to_string_lossy().into_owned(),
                    ),
                    (
                        "javac".to_string(),
                        home.join("bin/javac").to_string_lossy().into_owned(),
                    ),
                ]
                .into_iter()
                .collect(),
                license: "See the distribution's license and terms in the upstream archive".into(),
                java_folder: String::new(),
                formula: String::new(),
                min_macos: String::new(),
            };
            if package.validate_for_target(target).is_err() {
                continue;
            }
            if let Some(previous) = found.iter().find(|existing| existing.version == key) {
                if previous.sha256 != package.sha256 || previous.url != package.url {
                    return Err(ShellError::invalid(format!(
                        "ambiguous JDK catalog entry {key}; refresh before installing"
                    )));
                }
                continue;
            }
            found.push(package);
        }
    }
    if found.is_empty() {
        return Err(ShellError::invalid(if target.is_macos() {
            "JetBrains catalog contains no supported macOS arm64 JDK archives".to_string()
        } else {
            format!(
                "JetBrains catalog contains no supported {} {} JDK archives",
                want_os, want_arch
            )
        }));
    }
    Ok(found)
}

/// Resolve a catalog key (`install folder with +→_`) to its manifest.
pub fn resolve_java(http: &dyn HttpTransport, key: &str) -> Result<Package, ShellError> {
    let rows = java_catalog(http)?;
    rows.into_iter()
        .find(|package| package.version == key)
        .ok_or_else(|| {
            ShellError::invalid(
                "selected JDK distribution/version is unavailable; refresh the JetBrains catalog",
            )
        })
}

fn valid_additional_runtime(name: &str) -> bool {
    name == "java"
}

/// One runtime script file snapshot, for rollback.
struct ScriptSnapshot {
    path: PathBuf,
    data: Option<Vec<u8>>,
}

fn snapshot_scripts(shell: &Shell, name: &str) -> Result<Vec<ScriptSnapshot>, ShellError> {
    let base = shell
        .store()
        .root()
        .join("runtimes")
        .join(name)
        .join("init");
    let mut snapshots = Vec::new();
    for suffix in ["sh", "fish"] {
        let path = base.with_extension(suffix);
        let data = match std::fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        snapshots.push(ScriptSnapshot { path, data });
    }
    Ok(snapshots)
}

fn restore_scripts(snapshots: Vec<ScriptSnapshot>) -> Result<(), crate::state::StateError> {
    for snapshot in snapshots.into_iter().rev() {
        match snapshot.data {
            Some(data) => crate::fs::atomic_write(&snapshot.path, &data, 0o600)
                .map_err(crate::state::StateError::Io)?,
            None => match std::fs::remove_file(&snapshot.path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            },
        }
    }
    Ok(())
}

fn write_runtime_scripts(
    shell: &Shell,
    name: &str,
    scripts: [String; 2],
) -> Result<(), ShellError> {
    let base = shell
        .store()
        .root()
        .join("runtimes")
        .join(name)
        .join("init");
    for (suffix, script) in ["sh", "fish"].into_iter().zip(scripts) {
        crate::fs::atomic_write(&base.with_extension(suffix), script.as_bytes(), 0o600)?;
    }
    Ok(())
}

fn remove_runtime_scripts(shell: &Shell, name: &str) -> Result<(), ShellError> {
    let base = shell
        .store()
        .root()
        .join("runtimes")
        .join(name)
        .join("init");
    for suffix in ["sh", "fish"] {
        match std::fs::remove_file(base.with_extension(suffix)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

impl Shell {
    /// Select an installed Java version and write its shell scripts inside the
    /// state transaction.
    pub fn set_runtime_default(&self, name: &str, id: &str) -> Result<(), ShellError> {
        if !valid_additional_runtime(name) {
            return Err(ShellError::invalid("unsupported runtime"));
        }
        self.store().with_state_transaction(|state| {
            let package = state.packages.get(id).cloned().ok_or_else(|| {
                crate::state::StateError::invalid("choose an installed runtime version")
            })?;
            if package.name != name {
                return Err(crate::state::StateError::invalid(
                    "choose an installed runtime version",
                ));
            }
            let binary = self
                .software()
                .binary(state, id, name)
                .map_err(|error| crate::state::StateError::invalid(error.to_string()))?;
            self.save_runtime_default(state, name, id, &binary)
        })?;
        Ok(())
    }

    fn save_runtime_default(
        &self,
        state: &mut crate::model::State,
        name: &str,
        id: &str,
        binary: &Path,
    ) -> Result<Tx<()>, crate::state::StateError> {
        let snapshots = snapshot_scripts(self, name).map_err(software_to_state)?;
        let scripts = [
            runtime_init_script(name, binary, &self.store().java_directory()),
            runtime_fish_script(name, binary, &self.store().java_directory()),
        ];
        write_runtime_scripts(self, name, scripts).map_err(software_to_state)?;
        state
            .runtime_defaults
            .insert(name.to_string(), id.to_string());
        Ok(Tx::new(()).with_undo(move || restore_scripts(snapshots)))
    }

    /// Enable or disable the Java shell integration. Enabling refreshes the
    /// default's scripts under the same transaction as the block edits.
    pub fn configure_runtime_shell(&self, name: &str, enable: bool) -> Result<(), ShellError> {
        if !valid_additional_runtime(name) {
            return Err(ShellError::invalid("unsupported runtime"));
        }
        if enable {
            self.store().with_state_transaction(|state| {
                let id = state
                    .runtime_defaults
                    .get(name)
                    .cloned()
                    .unwrap_or_default();
                if id.is_empty() {
                    return Err(crate::state::StateError::invalid(
                        "select a default version first",
                    ));
                }
                let package = state.packages.get(&id).cloned().ok_or_else(|| {
                    crate::state::StateError::invalid("select a default version first")
                })?;
                if package.name != name {
                    return Err(crate::state::StateError::invalid(
                        "select a default version first",
                    ));
                }
                let binary = self
                    .software()
                    .binary(state, &id, name)
                    .map_err(|error| crate::state::StateError::invalid(error.to_string()))?;
                self.save_runtime_default(state, name, &id, &binary)
            })?;
        }
        let (begin, end) = super::runtime_block(name);
        self.configure_runtime_shell_files(name, &begin, &end, enable)?;
        if !enable {
            remove_runtime_scripts(self, name)?;
        }
        Ok(())
    }
}

fn software_to_state(error: ShellError) -> crate::state::StateError {
    crate::state::StateError::invalid(error.to_string())
}

fn runtime_init_script(name: &str, binary: &Path, jvm_dir: &Path) -> String {
    runtime_init_script_for_target(name, binary, jvm_dir, HostTarget::current())
}

fn runtime_init_script_for_target(
    name: &str,
    binary: &Path,
    jvm_dir: &Path,
    target: HostTarget,
) -> String {
    if name != "java" {
        return runtime_path_script(&binary.parent().unwrap_or(Path::new("")).to_string_lossy());
    }
    let default_bin = binary.parent().unwrap_or(Path::new(""));
    let default_home = default_bin.parent().unwrap_or(Path::new(""));
    let (arch_reject, arch_match) = match target.arch {
        HostArch::Arm64 => (
            r#"if [ -n "$os_arch" ] && [ "$os_arch" != "aarch64" ]; then
        continue
      fi"#,
            r#"[ "$os_arch" = "aarch64" ]"#,
        ),
        HostArch::Amd64 => (
            r#"case "$os_arch" in ""|amd64|x86_64) ;; *) continue ;; esac"#,
            r#"{ [ "$os_arch" = "amd64" ] || [ "$os_arch" = "x86_64" ]; }"#,
        ),
    };
    let template = r#"# Written by Devbase. Changes are overwritten.
MSM_JAVA_DEFAULT_HOME=__DEFAULT_HOME__
MSM_JAVA_DEFAULT_BIN=__DEFAULT_BIN__
MSM_JAVA_JVM_DIR=__JVM_DIR__

__msm_java_apply() {
  local dir spec line val home bin cand cand_home java_ver os_arch implementor
  local sdk_ver sdk_vendor sdk_distro distro_spec base
  local match1 match2 match2_prefix match3 match4 resolved rest item clean have old_bin
  [ -n "${ZSH_VERSION-}" ] && setopt local_options null_glob
  dir="${PWD:-$(pwd)}"
  spec=""
  while [ -n "$dir" ]; do
    if [ -r "$dir/.java-version" ]; then
      while IFS= read -r line || [ -n "$line" ]; do
        line="${line%%#*}"
        line="${line#"${line%%[![:space:]]*}"}"
        line="${line%"${line##*[![:space:]]}"}"
        if [ -n "$line" ]; then
          spec="$line"
          break
        fi
      done < "$dir/.java-version"
    fi
    if [ -z "$spec" ] && [ -r "$dir/.sdkmanrc" ]; then
      while IFS= read -r line || [ -n "$line" ]; do
        line="${line%%#*}"
        line="${line#"${line%%[![:space:]]*}"}"
        line="${line%"${line##*[![:space:]]}"}"
        case "$line" in
          java=*) val="${line#java=}" ;;
          java[[:space:]]*=*) val="${line#*=}" ;;
          *) continue ;;
        esac
        val="${val#"${val%%[![:space:]]*}"}"
        val="${val%"${val##*[![:space:]]}"}"
        case "$val" in
          \"*\") val="${val#\"}"; val="${val%\"}" ;;
          \'*\') val="${val#\'}"; val="${val%\'}" ;;
        esac
        if [ -n "$val" ]; then
          spec="$val"
          break
        fi
      done < "$dir/.sdkmanrc"
    fi
    if [ -n "$spec" ] || [ "$dir" = "/" ]; then
      break
    fi
    dir="${dir%/*}"
    [ -n "$dir" ] || dir="/"
  done

  home=""
  bin=""
  if [ -n "$spec" ] && [ -n "$MSM_JAVA_JVM_DIR" ] && [ -d "$MSM_JAVA_JVM_DIR" ]; then
    sdk_ver=""
    sdk_vendor=""
    sdk_distro=""
    distro_spec=""
    case "$spec" in
      [0-9]*-[A-Za-z]*)
        sdk_ver="${spec%%-*}"
        sdk_vendor="${spec#*-}"
        case "$sdk_vendor" in
          [Tt][Ee][Mm]|[Tt][Ee][Mm][Uu][Rr][Ii][Nn]|[Ee][Cc][Ll][Ii][Pp][Ss][Ee]|[Aa][Dd][Oo][Pp][Tt][Ii][Uu][Mm]) sdk_distro="temurin" ;;
          [Aa][Mm][Zz][Nn]|[Cc][Oo][Rr][Rr][Ee][Tt][Tt][Oo]|[Aa][Mm][Aa][Zz][Oo][Nn]) sdk_distro="corretto" ;;
          [Zz][Uu][Ll][Uu]|[Aa][Zz][Uu][Ll]) sdk_distro="zulu" ;;
          [Ss][Ee][Mm]|[Ss][Ee][Mm][Ee][Rr][Uu]|[Ii][Bb][Mm]) sdk_distro="semeru" ;;
          [Gg][Rr][Aa][Aa][Ll]|[Gg][Rr][Aa][Aa][Ll][Cc][Ee]|[Gg][Rr][Aa][Aa][Ll][Vv][Mm]) sdk_distro="graalvm" ;;
          [Ll][Ii][Bb][Rr][Cc][Aa]|[Ll][Ii][Bb][Ee][Rr][Ii][Cc][Aa]|[Bb][Ee][Ll][Ll][Ss][Oo][Ff][Tt]) sdk_distro="liberica" ;;
          [Ss][Aa][Pp][Mm][Cc][Hh][Nn]|[Ss][Aa][Pp][Mm][Aa][Cc][Hh][Ii][Nn][Ee]) sdk_distro="sapmachine" ;;
          [Mm][Ss]|[Mm][Ii][Cc][Rr][Oo][Ss][Oo][Ff][Tt]) sdk_distro="ms" ;;
          [Oo][Rr][Aa][Cc][Ll][Ee]) sdk_distro="oracle" ;;
          [Oo][Pp][Ee][Nn]|[Oo][Pp][Ee][Nn][Jj][Dd][Kk]) sdk_distro="openjdk" ;;
          [Jj][Bb][Rr]|[Jj][Ee][Tt][Bb][Rr][Aa][Ii][Nn][Ss]) sdk_distro="jbr" ;;
          *) sdk_distro="$sdk_vendor" ;;
        esac
        distro_spec="$sdk_distro-$sdk_ver"
        ;;
    esac

    match1=""
    match2=""
    match2_prefix=""
    match3=""
    match4=""
    for cand in "$MSM_JAVA_JVM_DIR"/*; do
      [ -d "$cand" ] || continue
      base="${cand##*/}"
      case "$base" in .* ) continue ;; esac
      if [ -x "$cand/Contents/Home/bin/java" ]; then
        cand_home="$cand/Contents/Home"
      elif [ -x "$cand/bin/java" ]; then
        cand_home="$cand"
      else
        continue
      fi
      java_ver=""
      os_arch=""
      implementor=""
      if [ -r "$cand_home/release" ]; then
        while IFS= read -r line || [ -n "$line" ]; do
          line="${line%"${line##*[![:space:]]}"}"
          case "$line" in
            JAVA_VERSION=*)
              val="${line#JAVA_VERSION=}"
              case "$val" in \"*\") val="${val#\"}"; val="${val%\"}" ;; esac
              java_ver="$val"
              ;;
            OS_ARCH=*)
              val="${line#OS_ARCH=}"
              case "$val" in \"*\") val="${val#\"}"; val="${val%\"}" ;; esac
              os_arch="$val"
              ;;
            IMPLEMENTOR=*)
              val="${line#IMPLEMENTOR=}"
              case "$val" in \"*\") val="${val#\"}"; val="${val%\"}" ;; esac
              implementor="$val"
              ;;
          esac
        done < "$cand_home/release"
      fi
      __ARCH_REJECT__

      if [ "$base" = "$spec" ]; then
        match1="$cand_home"
        break
      fi
      if [ -z "$match2" ]; then
        case "$base" in
          "$spec".jdk) match2="$cand_home" ;;
          *)
            if [ -n "$distro_spec" ]; then
              case "$base" in
                "$distro_spec"|"$distro_spec".jdk) match2="$cand_home" ;;
              esac
            fi
            ;;
        esac
      fi
      if [ -z "$match2_prefix" ]; then
        case "$base" in
          "$spec".*|"$spec"-*|"$spec"_*|"$spec"+*) match2_prefix="$cand_home" ;;
          *)
            if [ -n "$distro_spec" ]; then
              case "$base" in
                "$distro_spec".*|"$distro_spec"-*|"$distro_spec"_*|"$distro_spec"+*) match2_prefix="$cand_home" ;;
              esac
            fi
            ;;
        esac
      fi
      if __ARCH_MATCH__ && [ -n "$java_ver" ]; then
        if [ -z "$match3" ] && [ "$java_ver" = "$spec" ]; then
          match3="$cand_home"
        elif [ -z "$sdk_ver" ] && [ -z "$match4" ]; then
          case "$java_ver" in
            "$spec".*|"$spec"+*|"$spec"_*|"$spec"-*) match4="$cand_home" ;;
          esac
        elif [ -n "$sdk_ver" ]; then
          case "$sdk_distro:$implementor" in
            temurin:*[Ee][Cc][Ll][Ii][Pp][Ss][Ee]*|temurin:*[Aa][Dd][Oo][Pp][Tt][Ii][Uu][Mm]*|temurin:*[Tt][Ee][Mm][Uu][Rr][Ii][Nn]*|\
            corretto:*[Aa][Mm][Aa][Zz][Oo][Nn]*|corretto:*[Cc][Oo][Rr][Rr][Ee][Tt][Tt][Oo]*|\
            zulu:*[Aa][Zz][Uu][Ll]*|zulu:*[Zz][Uu][Ll][Uu]*|\
            semeru:*[Ii][Bb][Mm]*|semeru:*[Ss][Ee][Mm][Ee][Rr][Uu]*|\
            graalvm:*[Gg][Rr][Aa][Aa][Ll]*|\
            liberica:*[Bb][Ee][Ll][Ll][Ss][Oo][Ff][Tt]*|liberica:*[Ll][Ii][Bb][Ee][Rr][Ii][Cc][Aa]*|\
            sapmachine:*[Ss][Aa][Pp]*|\
            ms:*[Mm][Ii][Cc][Rr][Oo][Ss][Oo][Ff][Tt]*|\
            oracle:*[Oo][Rr][Aa][Cc][Ll][Ee]*|\
            openjdk:*[Oo][Pp][Ee][Nn][Jj][Dd][Kk]*|openjdk:*[Hh][Oo][Mm][Ee][Bb][Rr][Ee][Ww]*|\
            jbr:*[Jj][Ee][Tt][Bb][Rr][Aa][Ii][Nn][Ss]*)
              if [ -z "$match3" ] && [ "$java_ver" = "$sdk_ver" ]; then
                match3="$cand_home"
              elif [ -z "$match4" ]; then
                case "$java_ver" in
                  "$sdk_ver".*|"$sdk_ver"+*|"$sdk_ver"_*|"$sdk_ver"-*) match4="$cand_home" ;;
                esac
              fi
              ;;
          esac
        fi
      fi
    done

    for resolved in "$match1" "$match2" "$match2_prefix" "$match3" "$match4"; do
      if [ -n "$resolved" ]; then
        home="$resolved"
        bin="$resolved/bin"
        break
      fi
    done
  fi

  if [ -z "$home" ]; then
    home="$MSM_JAVA_DEFAULT_HOME"
    bin="$MSM_JAVA_DEFAULT_BIN"
  fi

  old_bin="${__MSM_JAVA_BIN-}"
  rest="$PATH"
  clean=""
  have=""
  while :; do
    item="${rest%%:*}"
    if [ "$item" = "$bin" ] || { [ -n "$old_bin" ] && [ "$item" = "$old_bin" ]; }; then
      :
    else
      clean="$clean${have:+:}$item"
      have=1
    fi
    case "$rest" in *:*) rest="${rest#*:}" ;; *) break ;; esac
  done
  export PATH="$bin${have:+:$clean}"
  export JAVA_HOME="$home"
  export __MSM_JAVA_BIN="$bin"
  __MSM_JAVA_PWD="$PWD"
}

if [ -n "${ZSH_VERSION-}" ]; then
  autoload -Uz add-zsh-hook 2>/dev/null && add-zsh-hook chpwd __msm_java_apply
elif [ -n "${BASH_VERSION-}" ]; then
  declare -F java >/dev/null || java() { __msm_java_apply; command java "$@"; }
  declare -F javac >/dev/null || javac() { __msm_java_apply; command javac "$@"; }
  case $- in
    *i*)
      __msm_java_prompt() {
        [ "$PWD" = "${__MSM_JAVA_PWD-}" ] && return 0
        __msm_java_apply
      }
      case ";${PROMPT_COMMAND-};" in
        *";__msm_java_prompt;"*) ;;
        *) PROMPT_COMMAND="__msm_java_prompt${PROMPT_COMMAND:+;$PROMPT_COMMAND}" ;;
      esac
      ;;
  esac
fi
__msm_java_apply
"#;
    template
        .replace(
            "__DEFAULT_HOME__",
            &shell_quote(&default_home.to_string_lossy()),
        )
        .replace(
            "__DEFAULT_BIN__",
            &shell_quote(&default_bin.to_string_lossy()),
        )
        .replace("__JVM_DIR__", &shell_quote(&jvm_dir.to_string_lossy()))
        .replace("__ARCH_REJECT__", arch_reject)
        .replace("__ARCH_MATCH__", arch_match)
}

fn runtime_path_script(bin: &str) -> String {
    let template = r#"__msm_runtime_path() {
  local bin rest item result
  bin=__BIN__
  rest="$PATH"
  result="$bin"
  while :; do
    item="${rest%%:*}"
    [ "$item" = "$bin" ] || result="$result:$item"
    case "$rest" in *:*) rest="${rest#*:}" ;; *) break ;; esac
  done
  export PATH="$result"
}
__msm_runtime_path
unset -f __msm_runtime_path
"#;
    template.replace("__BIN__", &shell_quote(bin))
}

fn runtime_fish_script(name: &str, binary: &Path, jvm_dir: &Path) -> String {
    runtime_fish_script_for_target(name, binary, jvm_dir, HostTarget::current())
}

fn runtime_fish_script_for_target(
    name: &str,
    binary: &Path,
    jvm_dir: &Path,
    target: HostTarget,
) -> String {
    if name != "java" {
        let dir = binary.parent().unwrap_or(Path::new(""));
        return format!(
            "# Written by Devbase. Only the current process environment changes.\n\
             set -l msm_bin {bin}\n\
             set -l msm_path \"$msm_bin\"\n\
             for entry in $PATH\n  if test \"$entry\" != \"$msm_bin\"\n    set -a msm_path \"$entry\"\n  end\nend\n\
             set -gx PATH $msm_path\n",
            bin = fish_quote(&dir.to_string_lossy())
        );
    }
    let default_bin = binary.parent().unwrap_or(Path::new(""));
    let default_home = default_bin.parent().unwrap_or(Path::new(""));
    let (fish_arch_reject, fish_arch_match) = match target.arch {
        HostArch::Arm64 => (
            r#"if test -n "$os_arch"; and test "$os_arch" != "aarch64""#,
            r#"test "$os_arch" = "aarch64""#,
        ),
        HostArch::Amd64 => (
            r#"if test -n "$os_arch"; and test "$os_arch" != "amd64"; and test "$os_arch" != "x86_64""#,
            r#"begin; test "$os_arch" = "amd64"; or test "$os_arch" = "x86_64"; end"#,
        ),
    };
    let template = r#"# Written by Devbase. Changes are overwritten.
set -g MSM_JAVA_DEFAULT_HOME __DEFAULT_HOME__
set -g MSM_JAVA_DEFAULT_BIN __DEFAULT_BIN__
set -g MSM_JAVA_JVM_DIR __JVM_DIR__

functions -e __msm_java_apply
function __msm_java_apply --on-variable PWD
  set -l dir "$PWD"
  if test -z "$dir"
    set dir (pwd)
  end
  set -l spec ""
  while test -n "$dir"
    if test -r "$dir/.java-version"
      while read -l line
        set line (string replace -r '#.*' '' -- "$line" | string trim)
        if test -n "$line"
          set spec "$line"
          break
        end
      end < "$dir/.java-version"
    end
    if test -z "$spec"; and test -r "$dir/.sdkmanrc"
      while read -l line
        set line (string replace -r '#.*' '' -- "$line" | string trim)
        if string match -rq '^java\s*=' -- "$line"
          set -l val (string replace -r '^java\s*=\s*' '' -- "$line" | string trim)
          set val (string trim -c '"' -- "$val")
          set val (string trim -c "'" -- "$val")
          if test -n "$val"
            set spec "$val"
            break
          end
        end
      end < "$dir/.sdkmanrc"
    end
    if test -n "$spec"; or test "$dir" = "/"
      break
    end
    set dir (string replace -r '/[^/]*$' '' -- "$dir")
    if test -z "$dir"
      set dir "/"
    end
  end

  set -l home ""
  set -l bin ""
  if test -n "$spec"; and test -n "$MSM_JAVA_JVM_DIR"; and test -d "$MSM_JAVA_JVM_DIR"
    set -l sdk_ver ""
    set -l sdk_vendor ""
    set -l sdk_distro ""
    set -l distro_spec ""
    if string match -rq '^[0-9].*-[A-Za-z]' -- "$spec"
      set sdk_ver (string replace -r '-.*$' '' -- "$spec")
      set sdk_vendor (string lower -- (string replace -r '^[^-]*-' '' -- "$spec"))
      switch "$sdk_vendor"
        case tem temurin eclipse adoptium
          set sdk_distro temurin
        case amzn corretto amazon
          set sdk_distro corretto
        case zulu azul
          set sdk_distro zulu
        case sem semeru ibm
          set sdk_distro semeru
        case graal graalce graalvm
          set sdk_distro graalvm
        case librca liberica bellsoft
          set sdk_distro liberica
        case sapmchn sapmachine
          set sdk_distro sapmachine
        case ms microsoft
          set sdk_distro ms
        case oracle
          set sdk_distro oracle
        case open openjdk
          set sdk_distro openjdk
        case jbr jetbrains
          set sdk_distro jbr
        case '*'
          set sdk_distro "$sdk_vendor"
      end
      set distro_spec "$sdk_distro-$sdk_ver"
    end

    set -l match1 ""
    set -l match2 ""
    set -l match2_prefix ""
    set -l match3 ""
    set -l match4 ""
    for base in (command ls -1 -- "$MSM_JAVA_JVM_DIR" 2>/dev/null)
      set -l cand "$MSM_JAVA_JVM_DIR/$base"
      test -d "$cand"; or continue
      set -l cand_home ""
      if test -x "$cand/Contents/Home/bin/java"
        set cand_home "$cand/Contents/Home"
      else if test -x "$cand/bin/java"
        set cand_home "$cand"
      else
        continue
      end
      set -l java_ver ""
      set -l os_arch ""
      set -l implementor ""
      if test -r "$cand_home/release"
        while read -l line
          set line (string trim -- "$line")
          if string match -q 'JAVA_VERSION=*' -- "$line"
            set java_ver (string replace 'JAVA_VERSION=' '' -- "$line" | string trim -c '"')
          else if string match -q 'OS_ARCH=*' -- "$line"
            set os_arch (string replace 'OS_ARCH=' '' -- "$line" | string trim -c '"')
          else if string match -q 'IMPLEMENTOR=*' -- "$line"
            set implementor (string replace 'IMPLEMENTOR=' '' -- "$line" | string trim -c '"')
          end
        end < "$cand_home/release"
      end
      __FISH_ARCH_REJECT__
        continue
      end

      if test "$base" = "$spec"
        set match1 "$cand_home"
        break
      end
      if test -z "$match2"
        if test "$base" = "$spec.jdk"
          set match2 "$cand_home"
        else if test -n "$distro_spec"; and begin; test "$base" = "$distro_spec"; or test "$base" = "$distro_spec.jdk"; end
          set match2 "$cand_home"
        end
      end
      if test -z "$match2_prefix"
        if string match -q -- "$spec.*" "$base"; or string match -q -- "$spec-*" "$base"; or string match -q -- "$spec""_*" "$base"; or string match -q -- "$spec+*" "$base"
          set match2_prefix "$cand_home"
        else if test -n "$distro_spec"
          if string match -q -- "$distro_spec.*" "$base"; or string match -q -- "$distro_spec-*" "$base"; or string match -q -- "$distro_spec""_*" "$base"; or string match -q -- "$distro_spec+*" "$base"
            set match2_prefix "$cand_home"
          end
        end
      end
      if __FISH_ARCH_MATCH__; and test -n "$java_ver"
        if test -z "$match3"; and test "$java_ver" = "$spec"
          set match3 "$cand_home"
        else if test -z "$sdk_ver"; and test -z "$match4"
          if string match -q -- "$spec.*" "$java_ver"; or string match -q -- "$spec+*" "$java_ver"; or string match -q -- "$spec""_*" "$java_ver"; or string match -q -- "$spec-*" "$java_ver"
            set match4 "$cand_home"
          end
        else if test -n "$sdk_ver"
          set -l imp_lower (string lower -- "$implementor")
          set -l vendor_ok 0
          switch "$sdk_distro"
            case temurin
              string match -rq 'eclipse|adoptium|temurin|adoptopenjdk' -- "$imp_lower"; and set vendor_ok 1
            case corretto
              string match -rq 'amazon|corretto' -- "$imp_lower"; and set vendor_ok 1
            case zulu
              string match -rq 'azul|zulu' -- "$imp_lower"; and set vendor_ok 1
            case semeru
              string match -rq 'ibm|semeru' -- "$imp_lower"; and set vendor_ok 1
            case graalvm
              string match -rq 'graal' -- "$imp_lower"; and set vendor_ok 1
            case liberica
              string match -rq 'bellsoft|liberica' -- "$imp_lower"; and set vendor_ok 1
            case sapmachine
              string match -rq 'sap' -- "$imp_lower"; and set vendor_ok 1
            case ms
              string match -rq 'microsoft' -- "$imp_lower"; and set vendor_ok 1
            case oracle
              string match -rq 'oracle' -- "$imp_lower"; and set vendor_ok 1
            case openjdk
              string match -rq 'openjdk|homebrew' -- "$imp_lower"; and set vendor_ok 1
            case jbr
              string match -rq 'jetbrains' -- "$imp_lower"; and set vendor_ok 1
          end
          if test "$vendor_ok" -eq 1
            if test -z "$match3"; and test "$java_ver" = "$sdk_ver"
              set match3 "$cand_home"
            else if test -z "$match4"
              if string match -q -- "$sdk_ver.*" "$java_ver"; or string match -q -- "$sdk_ver+*" "$java_ver"; or string match -q -- "$sdk_ver""_*" "$java_ver"; or string match -q -- "$sdk_ver-*" "$java_ver"
                set match4 "$cand_home"
              end
            end
          end
        end
      end
    end

    for resolved in "$match1" "$match2" "$match2_prefix" "$match3" "$match4"
      if test -n "$resolved"
        set home "$resolved"
        set bin "$resolved/bin"
        break
      end
    end
  end

  if test -z "$home"
    set home "$MSM_JAVA_DEFAULT_HOME"
    set bin "$MSM_JAVA_DEFAULT_BIN"
  end

  set -l remaining
  for entry in $PATH
    if test "$entry" != "$bin"; and begin; not set -q __MSM_JAVA_BIN; or test "$entry" != "$__MSM_JAVA_BIN"; end
      set -a remaining "$entry"
    end
  end
  set -gx PATH "$bin" $remaining
  set -gx JAVA_HOME "$home"
  set -gx __MSM_JAVA_BIN "$bin"
end
__msm_java_apply
"#;
    template
        .replace(
            "__DEFAULT_HOME__",
            &fish_quote(&default_home.to_string_lossy()),
        )
        .replace(
            "__DEFAULT_BIN__",
            &fish_quote(&default_bin.to_string_lossy()),
        )
        .replace("__JVM_DIR__", &fish_quote(&jvm_dir.to_string_lossy()))
        .replace("__FISH_ARCH_REJECT__", fish_arch_reject)
        .replace("__FISH_ARCH_MATCH__", fish_arch_match)
}

#[cfg(test)]
mod linux_java_tests {
    use super::*;
    use crate::software::SoftwareError;

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
    fn java_catalog_filters_linux_x86_64_and_aarch64_entries() {
        let sha = "a".repeat(64);
        let feed = format!(
            r#"{{
              "jdks": [
                {{
                  "vendor": "Eclipse Temurin",
                  "product": "JDK",
                  "jdk_version": "21.0.5",
                  "packages": [
                    {{
                      "os": "linux",
                      "arch": "x86_64",
                      "url": "https://example.com/temurin-21-linux-x64.tar.gz",
                      "sha256": "{sha}",
                      "package_type": "targz",
                      "package_root_prefix": "jdk-21.0.5+11",
                      "package_to_java_home_prefix": "",
                      "install_folder_name": "temurin-21.0.5+11"
                    }},
                    {{
                      "os": "linux",
                      "arch": "aarch64",
                      "url": "https://example.com/temurin-21-linux-arm64.tar.gz",
                      "sha256": "{sha}",
                      "package_type": "targz",
                      "package_root_prefix": "jdk-21.0.5+11",
                      "package_to_java_home_prefix": "",
                      "install_folder_name": "temurin-21.0.5+11"
                    }}
                  ]
                }}
              ]
            }}"#
        );
        let http = StubHttp(feed.into_bytes());

        let x64 = java_catalog_for_target(&http, HostTarget::LINUX_AMD64).unwrap();
        assert_eq!(x64.len(), 1);
        assert_eq!(x64[0].arch, "amd64");
        assert!(x64[0].url.contains("linux-x64"));
        assert_eq!(x64[0].binaries["java"], "bin/java");

        let arm = java_catalog_for_target(&http, HostTarget::LINUX_ARM64).unwrap();
        assert_eq!(arm.len(), 1);
        assert_eq!(arm[0].arch, "arm64");
        assert!(arm[0].url.contains("linux-arm64"));
    }

    #[test]
    fn java_scripts_handle_amd64_and_aarch64_os_arch() {
        let bin = Path::new("/home/dev/.local/share/devbase/jdks/temurin-21/bin/java");
        let dir = Path::new("/home/dev/.local/share/devbase/jdks");

        let sh_x64 = runtime_init_script_for_target("java", bin, dir, HostTarget::LINUX_AMD64);
        assert!(sh_x64.contains("amd64|x86_64"));
        assert!(!sh_x64.contains("!= \"aarch64\""));

        let fish_x64 = runtime_fish_script_for_target("java", bin, dir, HostTarget::LINUX_AMD64);
        assert!(fish_x64.contains("test \"$os_arch\" = \"amd64\""));
        assert!(fish_x64.contains("test \"$os_arch\" = \"x86_64\""));

        let sh_mac = runtime_init_script_for_target("java", bin, dir, HostTarget::MACOS_ARM64);
        assert!(sh_mac.contains("!= \"aarch64\""));
    }
}

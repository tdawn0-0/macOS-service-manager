//! Node.js release resolution, version selection and shell setup, ported from
//! `node_releases.go` and `node_manage.go`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::{Shell, ShellError, fish_quote, shell_quote};
use crate::model::{Package, State};
use crate::software::SoftwareError;
use crate::state::Store;

/// One nodejs.org release row.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeRelease {
    pub version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub date: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub lts: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub npm: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub v8: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub security: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NodeIndexCache {
    fetched_at: u64,
    releases: Vec<NodeRelease>,
}

/// The cached release index path (`cache/node-index.json`).
pub fn node_index_path(store: &Store) -> PathBuf {
    store.cache_directory().join("node-index.json")
}

const NODE_INDEX_TTL_SECONDS: u64 = 3600;

/// Fetch nodejs.org's index and keep the releases that run on Apple Silicon,
/// newest first.
pub fn fetch_node_releases(
    http: &dyn crate::software::HttpTransport,
    base: &str,
) -> Result<Vec<NodeRelease>, ShellError> {
    #[derive(Deserialize)]
    struct Row {
        #[serde(default)]
        version: String,
        #[serde(default)]
        date: String,
        #[serde(default)]
        files: Vec<String>,
        #[serde(default)]
        npm: String,
        #[serde(default)]
        v8: String,
        #[serde(default)]
        lts: serde_json::Value,
        #[serde(default)]
        security: bool,
    }
    let url = format!("{}/index.json", base.trim_end_matches('/'));
    let body = http.get(&url).map_err(|error| match error {
        SoftwareError::Fetch(message) => {
            ShellError::invalid(format!("nodejs.org release index: {message}"))
        }
        other => ShellError::Software(other),
    })?;
    let rows: Vec<Row> = serde_json::from_slice(&body)
        .map_err(|error| ShellError::invalid(format!("nodejs.org release index: {error}")))?;
    let mut out = Vec::new();
    for row in rows {
        let version = row.version.trim_start_matches('v').to_string();
        if parse_semver(&version).is_none() {
            continue;
        }
        if !row.files.iter().any(|file| file == "osx-arm64-tar") {
            continue;
        }
        let lts = match row.lts {
            serde_json::Value::String(name) => name,
            _ => String::new(),
        };
        out.push(NodeRelease {
            version,
            date: row.date,
            lts,
            npm: row.npm,
            v8: row.v8,
            security: row.security,
        });
    }
    out.sort_by(|a, b| version_order(&a.version, &b.version));
    if out.is_empty() {
        return Err(ShellError::invalid(
            "nodejs.org lists no release for Apple Silicon",
        ));
    }
    Ok(out)
}

impl Shell {
    /// Read the cached release index, if any.
    pub fn cached_node_releases(&self) -> Vec<NodeRelease> {
        let Ok(bytes) = std::fs::read(node_index_path(self.store())) else {
            return Vec::new();
        };
        serde_json::from_slice::<NodeIndexCache>(&bytes)
            .map(|cache| cache.releases)
            .unwrap_or_default()
    }

    /// Read nodejs.org's releases. The index is kept for an hour; `refresh`
    /// forces a fetch. When the network fails but a cache exists, the last
    /// copy is returned with `stale = true`.
    pub fn node_releases(
        &self,
        cancelled: &dyn Fn() -> bool,
        refresh: bool,
    ) -> Result<(Vec<NodeRelease>, bool), ShellError> {
        if cancelled() {
            return Err(ShellError::State(crate::state::StateError::Cancelled));
        }
        let path = node_index_path(self.store());
        let cached = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<NodeIndexCache>(&bytes).ok())
            .filter(|cache| !cache.releases.is_empty());
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        if let Some(cache) = &cached
            && !refresh
            && now.saturating_sub(cache.fetched_at) < NODE_INDEX_TTL_SECONDS
        {
            return Ok((cache.releases.clone(), false));
        }
        match fetch_node_releases(self.http(), self.node_dist_base()) {
            Ok(fresh) => {
                let cache = NodeIndexCache {
                    fetched_at: now,
                    releases: fresh.clone(),
                };
                if let Ok(bytes) = serde_json::to_vec(&cache)
                    && let Err(error) = crate::fs::atomic_write(&path, &bytes, 0o600)
                {
                    self.store()
                        .emit(&format!("Release index could not be cached: {error}"));
                }
                Ok((fresh, false))
            }
            Err(error) => match cached {
                Some(cache) => Ok((cache.releases, true)),
                None => Err(error),
            },
        }
    }
}

/// `major.minor.patch`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Semver {
    pub major: i64,
    pub minor: i64,
    pub patch: i64,
}

impl Semver {
    fn less(self, other: Semver) -> bool {
        (self.major, self.minor, self.patch) < (other.major, other.minor, other.patch)
    }
}

/// Parse an exact version, tolerating a leading `v`.
pub fn parse_semver(value: &str) -> Option<Semver> {
    let value = value.trim().trim_start_matches('v');
    let parts: Vec<&str> = value.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let mut numbers = [0i64; 3];
    for (index, part) in parts.iter().enumerate() {
        let number = part.parse::<i64>().ok()?;
        if number < 0 {
            return None;
        }
        numbers[index] = number;
    }
    Some(Semver {
        major: numbers[0],
        minor: numbers[1],
        patch: numbers[2],
    })
}

fn version_greater(a: &str, b: &str) -> bool {
    let (Some(a), Some(b)) = (parse_semver(a), parse_semver(b)) else {
        return false;
    };
    b.less(a)
}

/// Newest first, with a deterministic fallback for malformed versions.
fn version_order(a: &str, b: &str) -> std::cmp::Ordering {
    match (parse_semver(a), parse_semver(b)) {
        (Some(a), Some(b)) => b.cmp(&a),
        _ => b.cmp(a),
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NodeBound {
    lo: Semver,
    hi: Semver,
    has_hi: bool,
}

impl NodeBound {
    fn contains(self, value: Semver) -> bool {
        !value.less(self.lo) && (!self.has_hi || value.less(self.hi))
    }

    fn intersect(self, other: NodeBound) -> NodeBound {
        let mut out = self;
        if self.lo.less(other.lo) {
            out.lo = other.lo;
        }
        if other.has_hi && (!self.has_hi || other.hi.less(self.hi)) {
            out.hi = other.hi;
            out.has_hi = true;
        }
        out
    }
}

fn parse_partial(value: &str) -> Option<[i64; 3]> {
    let value = value.trim().trim_start_matches('v');
    let parts: Vec<&str> = value.split('.').collect();
    if value.is_empty() || parts.len() > 3 {
        return None;
    }
    let mut numbers = [-1i64; 3];
    let mut wild = false;
    for (index, part) in parts.iter().enumerate() {
        if matches!(*part, "x" | "X" | "*") {
            wild = true;
            continue;
        }
        if wild {
            return None;
        }
        let number = part.parse::<i64>().ok()?;
        if number < 0 {
            return None;
        }
        numbers[index] = number;
    }
    Some(numbers)
}

fn floor(numbers: [i64; 3]) -> Semver {
    Semver {
        major: numbers[0].max(0),
        minor: numbers[1].max(0),
        patch: numbers[2].max(0),
    }
}

fn parse_comparator(token: &str) -> Result<NodeBound, ShellError> {
    let mut op = "";
    let mut rest = token;
    for candidate in [">=", "<=", ">", "<", "=", "^", "~"] {
        if let Some(stripped) = token.strip_prefix(candidate) {
            op = candidate;
            rest = stripped.trim();
            break;
        }
    }
    let Some(numbers) = parse_partial(rest) else {
        return Err(ShellError::invalid(format!(
            "{rest:?} is not a Node.js version or range"
        )));
    };
    let lo = floor(numbers);
    let next = || {
        if numbers[0] < 0 {
            Semver::default()
        } else if numbers[1] < 0 {
            Semver {
                major: numbers[0] + 1,
                minor: 0,
                patch: 0,
            }
        } else if numbers[2] < 0 {
            Semver {
                major: numbers[0],
                minor: numbers[1] + 1,
                patch: 0,
            }
        } else {
            Semver {
                major: numbers[0],
                minor: numbers[1],
                patch: numbers[2] + 1,
            }
        }
    };
    match op {
        "" | "=" => {
            if numbers[0] < 0 {
                Ok(NodeBound::default())
            } else {
                Ok(NodeBound {
                    lo,
                    hi: next(),
                    has_hi: true,
                })
            }
        }
        "^" => {
            if numbers[0] < 0 {
                Ok(NodeBound::default())
            } else if numbers[0] > 0 || numbers[1] < 0 {
                Ok(NodeBound {
                    lo,
                    hi: Semver {
                        major: numbers[0] + 1,
                        minor: 0,
                        patch: 0,
                    },
                    has_hi: true,
                })
            } else if numbers[1] > 0 || numbers[2] < 0 {
                Ok(NodeBound {
                    lo,
                    hi: Semver {
                        major: 0,
                        minor: numbers[1] + 1,
                        patch: 0,
                    },
                    has_hi: true,
                })
            } else {
                Ok(NodeBound {
                    lo,
                    hi: Semver {
                        major: 0,
                        minor: 0,
                        patch: numbers[2] + 1,
                    },
                    has_hi: true,
                })
            }
        }
        "~" => {
            if numbers[0] < 0 {
                Ok(NodeBound::default())
            } else if numbers[1] < 0 {
                Ok(NodeBound {
                    lo,
                    hi: Semver {
                        major: numbers[0] + 1,
                        minor: 0,
                        patch: 0,
                    },
                    has_hi: true,
                })
            } else {
                Ok(NodeBound {
                    lo,
                    hi: Semver {
                        major: numbers[0],
                        minor: numbers[1] + 1,
                        patch: 0,
                    },
                    has_hi: true,
                })
            }
        }
        ">=" => Ok(NodeBound {
            lo,
            ..Default::default()
        }),
        ">" => Ok(NodeBound {
            lo: next(),
            ..Default::default()
        }),
        "<" => Ok(NodeBound {
            hi: lo,
            has_hi: true,
            ..Default::default()
        }),
        "<=" => Ok(NodeBound {
            hi: next(),
            has_hi: true,
            ..Default::default()
        }),
        _ => Err(ShellError::invalid(format!(
            "unsupported comparator {op:?}"
        ))),
    }
}

/// Parse an nvm/fnm/package.json range into a union of half-open bounds.
pub fn parse_range(spec: &str) -> Result<Vec<NodeBound>, ShellError> {
    let mut out = Vec::new();
    for set in spec.split("||") {
        let mut fields: Vec<String> = set.split_whitespace().map(str::to_string).collect();
        if fields.is_empty() {
            out.push(NodeBound::default());
            continue;
        }
        let mut tokens = Vec::new();
        let mut index = 0;
        while index < fields.len() {
            if index + 2 < fields.len() && fields[index + 1] == "-" {
                tokens.push(format!(">={}", fields[index]));
                tokens.push(format!("<={}", fields[index + 2]));
                index += 3;
                continue;
            }
            if matches!(
                fields[index].as_str(),
                ">=" | ">" | "<" | "<=" | "^" | "~" | "="
            ) && index + 1 < fields.len()
            {
                fields[index + 1] = format!("{}{}", fields[index], fields[index + 1]);
                index += 1;
                continue;
            }
            tokens.push(fields[index].clone());
            index += 1;
        }
        let mut bound = NodeBound::default();
        for token in tokens {
            bound = bound.intersect(parse_comparator(&token)?);
        }
        out.push(bound);
    }
    Ok(out)
}

type NodeMatcher = Box<dyn Fn(&NodeRelease) -> bool>;

fn node_matcher(spec: &str) -> Result<NodeMatcher, ShellError> {
    let lower = spec.trim().to_lowercase();
    match lower.as_str() {
        "" => Err(ShellError::invalid(
            "name a Node.js version, for example 22, 22.14.0, lts or latest",
        )),
        "latest" | "node" | "current" | "stable" => Ok(Box::new(|_| true)),
        "lts" | "lts/*" | "lts/latest" => Ok(Box::new(|release| !release.lts.is_empty())),
        _ if lower.starts_with("lts/") => {
            let name = lower.trim_start_matches("lts/").to_string();
            Ok(Box::new(move |release| release.lts.to_lowercase() == name))
        }
        _ => {
            let bounds = parse_range(&lower)?;
            Ok(Box::new(move |release| {
                let Some(version) = parse_semver(&release.version) else {
                    return false;
                };
                bounds.iter().any(|bound| bound.contains(version))
            }))
        }
    }
}

/// Pick the newest candidate the spec names.
pub fn pick_node(
    spec: &str,
    candidates: &[NodeRelease],
) -> Result<Option<NodeRelease>, ShellError> {
    let matches = node_matcher(spec)?;
    let mut best: Option<NodeRelease> = None;
    for release in candidates {
        if matches(release)
            && best
                .as_ref()
                .is_none_or(|current| version_greater(&release.version, &current.version))
        {
            best = Some(release.clone());
        }
    }
    Ok(best)
}

/// Resolve `22`, `lts/iron` or `latest` against nodejs.org.
pub fn resolve_node_spec(
    shell: &Shell,
    cancelled: &dyn Fn() -> bool,
    spec: &str,
) -> Result<NodeRelease, ShellError> {
    let _ = node_matcher(spec)?;
    let (releases, _) = shell.node_releases(cancelled, false)?;
    if let Some(release) = pick_node(spec, &releases)? {
        return Ok(release);
    }
    let (fresh, _) = shell.node_releases(cancelled, true)?;
    if let Some(release) = pick_node(spec, &fresh)? {
        return Ok(release);
    }
    Err(ShellError::invalid(format!(
        "nodejs.org has no Node.js release for {:?} on Apple Silicon",
        spec.trim()
    )))
}

/// One Node.js version installed in the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledNode {
    pub id: String,
    pub version: String,
    pub lts: String,
    pub default: bool,
    pub package: Package,
}

/// Installed managed Node versions, newest first.
pub fn installed_nodes(state: &State, index: &[NodeRelease]) -> Vec<InstalledNode> {
    let lts: BTreeMap<String, String> = index
        .iter()
        .map(|release| (release.version.clone(), release.lts.clone()))
        .collect();
    let mut out: Vec<InstalledNode> = state
        .packages
        .iter()
        .filter(|(_, package)| {
            package.name == "node"
                && package.formula.is_empty()
                && (package.arch == "arm64" || package.arch == "universal")
        })
        .map(|(id, package)| InstalledNode {
            id: id.clone(),
            version: package.version.clone(),
            lts: lts.get(&package.version).cloned().unwrap_or_default(),
            default: *id == state.node_default,
            package: package.clone(),
        })
        .collect();
    out.sort_by(|a, b| version_order(&a.version, &b.version));
    out
}

/// The version a directory runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeSelection {
    pub id: String,
    pub version: String,
    pub dir: PathBuf,
    pub prefix: PathBuf,
    pub spec: String,
    pub source: String,
    pub system: bool,
}

impl NodeSelection {
    pub fn bin(&self) -> PathBuf {
        self.dir.join("bin")
    }

    /// What goes ahead of `PATH`: global packages, then Node.js itself.
    pub fn path(&self) -> String {
        if self.system {
            return String::new();
        }
        format!(
            "{}:{}",
            self.prefix.join("bin").display(),
            self.bin().display()
        )
    }
}

/// What a directory, or the nearest one above it, asks for.
pub fn node_version_file(dir: &Path) -> (String, String) {
    let mut dir = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    loop {
        for name in [".node-version", ".nvmrc"] {
            let path = dir.join(name);
            if let Ok(bytes) = std::fs::read(&path) {
                for line in String::from_utf8_lossy(&bytes).lines() {
                    let line = line.split('#').next().unwrap_or_default().trim();
                    if !line.is_empty() {
                        return (line.to_string(), path.to_string_lossy().into_owned());
                    }
                }
            }
        }
        let package = dir.join("package.json");
        if let Ok(bytes) = std::fs::read(&package)
            && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes)
            && let Some(node) = value
                .get("engines")
                .and_then(|engines| engines.get("node"))
                .and_then(|node| node.as_str())
            && !node.trim().is_empty()
        {
            return (
                node.trim().to_string(),
                package.to_string_lossy().into_owned(),
            );
        }
        let Some(parent) = dir.parent() else {
            return (String::new(), String::new());
        };
        if parent == dir {
            return (String::new(), String::new());
        }
        dir = parent.to_path_buf();
    }
}

/// A public alias matching the Go exported helper.
pub type NodeVersionFile = (String, String);

impl Shell {
    fn selection_of(&self, node: &InstalledNode, spec: &str, source: &str) -> NodeSelection {
        NodeSelection {
            id: node.id.clone(),
            version: node.version.clone(),
            dir: self.store().package_directory(&node.package),
            prefix: self.node_global_prefix(&node.version),
            spec: spec.to_string(),
            source: source.to_string(),
            system: false,
        }
    }

    /// Choose the version for a directory. `override` beats the directory's
    /// files, which beat the default.
    pub fn select_node(
        &self,
        dir: &Path,
        override_spec: Option<&str>,
    ) -> Result<NodeSelection, ShellError> {
        let state = self.store().read()?;
        let installed = installed_nodes(&state, &self.cached_node_releases());
        let (mut spec, mut source) = (
            override_spec.unwrap_or_default().trim().to_string(),
            "the requested version".to_string(),
        );
        if spec.is_empty() {
            (spec, source) = node_version_file(dir);
        }
        if spec.is_empty() {
            for node in &installed {
                if node.id == state.node_default {
                    return Ok(self.selection_of(node, "", "default"));
                }
            }
            return Err(ShellError::Missing(
                "no default Node.js version. Install one in Devbase: Software, Node.js".to_string(),
            ));
        }
        if spec.eq_ignore_ascii_case("system") {
            return Ok(NodeSelection {
                spec,
                source,
                system: true,
                ..NodeSelection::default()
            });
        }
        let candidates: Vec<NodeRelease> = installed
            .iter()
            .map(|node| NodeRelease {
                version: node.version.clone(),
                lts: node.lts.clone(),
                ..NodeRelease::default()
            })
            .collect();
        let Some(best) = pick_node(&spec, &candidates)
            .map_err(|error| ShellError::invalid(format!("{source}: {error}")))?
        else {
            return Err(ShellError::Missing(format!(
                "Node.js {spec}, asked for by {source}, is not installed. Install it in Devbase: Software, Node.js"
            )));
        };
        for node in &installed {
            if node.version == best.version {
                return Ok(self.selection_of(node, &spec, &source));
            }
        }
        Err(ShellError::Missing(format!(
            "Node.js {spec}, asked for by {source}, is not installed. Install it in Devbase: Software, Node.js"
        )))
    }

    /// Make `id` the default outside a directory that asks for another.
    pub fn set_node_default(&self, id: &str) -> Result<(), ShellError> {
        self.store().with_state(|state| {
            let package = state.packages.get(id).ok_or_else(|| {
                crate::state::StateError::invalid("that Node.js version is not installed")
            })?;
            if package.name != "node" {
                return Err(crate::state::StateError::invalid(
                    "that Node.js version is not installed",
                ));
            }
            state.node_default = id.to_string();
            Ok(())
        })?;
        let state = self.store().read()?;
        self.link_node_default(&state)?;
        Ok(())
    }

    pub(crate) fn link_node_default(&self, state: &State) -> Result<(), ShellError> {
        let link = self.node_default_link();
        crate::fs::create_private_dir_all(&self.node_home())?;
        match std::fs::remove_file(&link) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let Some(package) = state.packages.get(&state.node_default) else {
            return Ok(());
        };
        std::os::unix::fs::symlink(self.store().package_directory(package), link)?;
        Ok(())
    }

    /// Make the first installed Node version the default, like fnm.
    pub fn default_first_node(&self, package: &Package) -> Result<(), ShellError> {
        if package.name != "node" || !package.formula.is_empty() {
            return Ok(());
        }
        let linked = self.store().with_state(|state| {
            if !state.packages.contains_key(&package.id()) || !state.node_default.is_empty() {
                return Ok(None);
            }
            state.node_default = package.id();
            Ok(Some(state.clone()))
        })?;
        if let Some(state) = linked {
            self.link_node_default(&state)?;
        }
        Ok(())
    }
}

fn node_init_script(shell: &Shell) -> String {
    let template = r#"# Written by Devbase. Changes are overwritten.
MSM_NODE_RESOLVE=__RESOLVE__
export MSM_NODE_HOME=__HOME__

# Puts the Node.js version for the current directory first in PATH: the one
# .node-version, .nvmrc or engines.node of package.json names, else the default.
__msm_node_apply() {
  local out frag prefix rest item clean have old_global old_bin new_global new_bin
  out="$("$MSM_NODE_RESOLVE" node path "$PWD")" || out=""
  frag="${out%%$'\n'*}"
  prefix=""
  case "$out" in *$'\n'*) prefix="${out#*$'\n'}" ;; esac
  old_global="${__MSM_NODE_PATH%%:*}"
  old_bin="${__MSM_NODE_PATH#*:}"
  new_global="${frag%%:*}"
  new_bin="${frag#*:}"
  rest="$PATH"
  clean=""
  have=""
  while :; do
    item="${rest%%:*}"
    if { [ -n "$old_global" ] && [ "$item" = "$old_global" ]; } ||
       { [ -n "$old_bin" ] && [ "$item" = "$old_bin" ]; } ||
       { [ -n "$new_global" ] && [ "$item" = "$new_global" ]; } ||
       { [ -n "$new_bin" ] && [ "$item" = "$new_bin" ]; }; then
      :
    else
      clean="$clean${have:+:}$item"
      have=1
    fi
    case "$rest" in *:*) rest="${rest#*:}" ;; *) break ;; esac
  done
  export PATH="$clean"
  if [ -n "$frag" ]; then
    export PATH="$frag${have:+:$clean}"
    export npm_config_prefix="$prefix"
  else
    unset npm_config_prefix
  fi
  export __MSM_NODE_PATH="$frag"
}

if [ -n "$ZSH_VERSION" ]; then
  autoload -Uz add-zsh-hook 2>/dev/null && add-zsh-hook chpwd __msm_node_apply
elif [ -n "$BASH_VERSION" ]; then
  declare -F node >/dev/null || node() { __msm_node_apply; command node "$@"; }
  declare -F npm >/dev/null || npm() { __msm_node_apply; command npm "$@"; }
  declare -F npx >/dev/null || npx() { __msm_node_apply; command npx "$@"; }
  declare -F corepack >/dev/null || corepack() { __msm_node_apply; command corepack "$@"; }
  if [[ $- == *i* ]]; then
  __msm_node_prompt() {
    [ "$PWD" = "$__MSM_NODE_PWD" ] && return 0
    __MSM_NODE_PWD="$PWD"
    __msm_node_apply
  }
  case ";$PROMPT_COMMAND;" in *";__msm_node_prompt;"*) ;; *) PROMPT_COMMAND="__msm_node_prompt${PROMPT_COMMAND:+;$PROMPT_COMMAND}" ;; esac
  fi
fi
__msm_node_apply
"#;
    template
        .replace(
            "__RESOLVE__",
            &shell_quote(&shell.node_wrapper_path().to_string_lossy()),
        )
        .replace(
            "__HOME__",
            &shell_quote(&shell.node_home().to_string_lossy()),
        )
}

fn node_fish_script(shell: &Shell) -> String {
    let template = r#"# Written by Devbase. Changes are overwritten.
set -g MSM_NODE_RESOLVE __RESOLVE__
set -gx MSM_NODE_HOME __HOME__
functions -e __msm_node_apply
function __msm_node_apply --on-variable PWD
  set -l selection (command "$MSM_NODE_RESOLVE" node path "$PWD")
  set -l previous (string split : -- (string join : -- $__MSM_NODE_PATH))
  set -l remaining
  for entry in $PATH
    if not contains -- "$entry" $previous
      set -a remaining "$entry"
    end
  end
  set -l selected
  if test (count $selection) -ge 2
    set selected (string split : -- "$selection[1]")
    set -gx npm_config_prefix "$selection[2]"
  else
    set -e npm_config_prefix
  end
  set -gx PATH $selected $remaining
  set -gx __MSM_NODE_PATH $selected
end
__msm_node_apply
"#;
    template
        .replace(
            "__RESOLVE__",
            &fish_quote(&shell.node_wrapper_path().to_string_lossy()),
        )
        .replace(
            "__HOME__",
            &fish_quote(&shell.node_home().to_string_lossy()),
        )
}

impl Shell {
    /// Whether every supported shell has current Node blocks and scripts.
    pub fn node_shell_enabled(&self) -> bool {
        if !self.runtime_shell_enabled("node") {
            return false;
        }
        if std::fs::read(self.node_init_path()).ok().as_ref()
            != Some(&node_init_script(self).into_bytes())
        {
            return false;
        }
        if self.has_shell("fish") {
            let fish = self.node_home().join("init.fish");
            if std::fs::read(&fish).ok().as_ref() != Some(&node_fish_script(self).into_bytes()) {
                return false;
            }
        }
        true
    }

    /// Whether `name`'s marker blocks exist in every configured startup file
    /// plus `bash-env.sh`.
    pub fn runtime_shell_enabled(&self, name: &str) -> bool {
        let (begin, end) = if name == "node" {
            (
                super::NODE_BLOCK_BEGIN.to_string(),
                super::NODE_BLOCK_END.to_string(),
            )
        } else {
            super::runtime_block(name)
        };
        let Ok(mut files) = self.shell_startup_files(true) else {
            return false;
        };
        if files.is_empty() {
            return false;
        }
        files.push(super::ShellStartupFile::new(self.bash_env_path()));
        for file in files {
            let Ok(bytes) = std::fs::read(&file.path) else {
                return false;
            };
            if !contains(&bytes, begin.as_bytes()) || !contains(&bytes, end.as_bytes()) {
                return false;
            }
        }
        true
    }

    /// Write the Node command wrapper and setup scripts without touching a
    /// dotfile.
    pub fn write_node_shell(&self) -> Result<(), ShellError> {
        let exe = self.helper_executable();
        crate::fs::create_private_dir_all(&self.node_home())?;
        let mut wrapper = format!(
            "#!/bin/sh\nif [ -x {} ]; then exec {} --dev \"$@\"; fi\n",
            shell_quote(&exe.to_string_lossy()),
            shell_quote(&exe.to_string_lossy()),
        );
        if self.store().uses_user_storage() {
            let fallback = "/Applications/Devbase.app/Contents/MacOS/Devbase";
            wrapper.push_str(&format!(
                "if [ -x {} ]; then exec {} --dev \"$@\"; fi\n",
                shell_quote(fallback),
                shell_quote(fallback)
            ));
        }
        wrapper.push_str("exit 1\n");
        crate::fs::atomic_write(&self.node_wrapper_path(), wrapper.as_bytes(), 0o700)?;
        crate::fs::atomic_write(
            &self.node_init_path(),
            node_init_script(self).as_bytes(),
            0o600,
        )?;
        crate::fs::atomic_write(
            &self.node_home().join("init.fish"),
            node_fish_script(self).as_bytes(),
            0o600,
        )?;
        Ok(())
    }

    /// Update existing Node shell files without creating them when Node shell
    /// integration has never been written.
    pub fn refresh_node_shell_if_present(&self) -> Result<(), ShellError> {
        match std::fs::metadata(self.node_wrapper_path()) {
            Ok(_) => self.write_node_shell(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// Install Node blocks for zsh, Bash and fish.
    pub fn enable_node_shell(&self) -> Result<String, ShellError> {
        self.write_node_shell()?;
        self.configure_runtime_shell_files(
            "node",
            super::NODE_BLOCK_BEGIN,
            super::NODE_BLOCK_END,
            true,
        )
    }

    /// Remove only this runtime's marked blocks in all modes.
    pub fn disable_node_shell(&self) -> Result<String, ShellError> {
        self.configure_runtime_shell_files(
            "node",
            super::NODE_BLOCK_BEGIN,
            super::NODE_BLOCK_END,
            false,
        )
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

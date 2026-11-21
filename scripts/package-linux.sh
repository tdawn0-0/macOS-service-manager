#!/usr/bin/env bash
# Builds the single-binary Linux Devbase tarball bundle.
#
#   scripts/package-linux.sh                 run tests, build release binary, create tarball
#   scripts/package-linux.sh --skip-tests    skip cargo test before building
#   scripts/package-linux.sh --out <dir>     output directory (default: build/linux-<arch>)
#
# Environment:
#   DEVBASE_VERSION  release version override (leading "v" stripped); defaults
#                    to app/nsh.json's "version".
set -euo pipefail
cd "$(dirname "$0")/.."
repo=$PWD

die() {
	echo "error: $*" >&2
	exit 1
}

run_tests=1
out_override=""
while [ $# -gt 0 ]; do
	case "$1" in
	--skip-tests)
		run_tests=0
		shift
		;;
	--out)
		[ $# -ge 2 ] || die "--out requires a directory argument"
		out_override=$2
		shift 2
		;;
	-h | --help)
		sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'
		exit 0
		;;
	*) die "unknown argument: $1 (see --help)" ;;
	esac
done

[ "$(uname)" = Linux ] || die "Linux packaging requires a Linux host"

raw_arch=$(uname -m)
case "$raw_arch" in
x86_64 | amd64) arch="x86_64" ;;
aarch64 | arm64) arch="aarch64" ;;
*) die "unsupported Linux architecture: $raw_arch" ;;
esac

if [ -n "${DEVBASE_VERSION:-}" ]; then
	version=${DEVBASE_VERSION#v}
else
	version=$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["version"])' app/nsh.json)
fi
[ -n "$version" ] || die "version is empty (check DEVBASE_VERSION or app/nsh.json)"
if ! [[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]]; then
	die "invalid version: $version (expected semver such as 1.2.3 or 1.2.3-rc.1)"
fi

if [ "$run_tests" = 1 ]; then
	echo "==> Testing"
	(cd app && env -u DEVBASE_VERSION cargo test --locked)
	for crate in devcore devproxy devcli; do
		(cd "app/crates/$crate" && env -u DEVBASE_VERSION cargo test --locked)
	done
fi

echo "==> Checking bundled license notices"
for dir in app app/crates/devcore app/crates/devproxy app/crates/devcli; do
	(cd "$dir" && cargo fetch --locked)
done
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_*.py'
python3 scripts/collect-third-party-notices.py --check

out=${out_override:-"build/linux-${arch}"}
bundle_name="Devbase-${version}-linux-${arch}"
stage="${out}/${bundle_name}"
archive="${out}/${bundle_name}.tar.gz"
checksum="${archive}.sha256"

rm -rf "$stage" "$archive" "$checksum"
mkdir -p "$stage/resources" "$stage/nsh/assets"

echo "==> Building single-binary Linux executable (version=$version, arch=$arch)"
(cd app && DEVBASE_VERSION="$version" cargo build --release --locked --bin nsh-app-devbase)
target_dir=${CARGO_TARGET_DIR:-app/target}
install -m 0755 "${target_dir}/release/nsh-app-devbase" "$stage/devbase"

echo "==> Writing updater metadata"
python3 - "$repo/app/nsh.json" "$stage/nsh" "$version" <<'PY'
import json
import pathlib
import sys

manifest = json.loads(pathlib.Path(sys.argv[1]).read_text())
nsh_dir = pathlib.Path(sys.argv[2])
version = sys.argv[3]

app_meta = {
    "id": manifest["id"],
    "name": manifest["name"],
    "version": version,
}
(nsh_dir / "app.json").write_text(json.dumps(app_meta, indent=2) + "\n")

updater = manifest.get("updater")
if isinstance(updater, dict):
    updater_cfg = {
        "id": manifest["id"],
        "version": version,
    }
    for key in ("github", "url", "channels", "publicKey", "packages"):
        if key in updater:
            updater_cfg[key] = updater[key]
    if "build" in manifest:
        updater_cfg["build"] = manifest["build"]
    assets_dir = nsh_dir / "assets"
    assets_dir.mkdir(parents=True, exist_ok=True)
    (assets_dir / "nsh-updater.json").write_text(
        json.dumps(updater_cfg, indent=2) + "\n"
    )
PY

echo "==> Writing Freedesktop desktop entry"
cat >"$stage/xyz.jyeu.devbase.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Version=1.0
Name=Devbase
GenericName=Local Development Workspace
Comment=Manage local databases, HTTPS reverse proxy, runtimes, and systemd services
Exec=devbase %U
TryExec=devbase
Icon=xyz.jyeu.devbase
Terminal=false
Categories=Development;System;Utility;
StartupNotify=true
StartupWMClass=xyz.jyeu.devbase
EOF
chmod 0644 "$stage/xyz.jyeu.devbase.desktop"

echo "==> Bundling icon and license notices"
cp "$repo/resources/icon.png" "$stage/resources/icon.png"
cp "$repo/LICENSE" "$stage/LICENSE"
cp "$repo/NOTICE" "$stage/NOTICE"
rm -rf "$stage/licenses"
cp -R "$repo/licenses" "$stage/licenses"

echo "==> Creating Linux archive ($archive)"
tar -C "$out" -czf "$archive" "$bundle_name"

sha=$(sha256sum "$archive" | cut -d' ' -f1)
printf '%s  %s\n' "$sha" "$(basename "$archive")" >"$checksum"
chmod 0644 "$checksum"

echo
echo "Stage:          $stage"
echo "Executable:     $stage/devbase ($(du -h "$stage/devbase" | cut -f1))"
echo "Archive:        $archive ($(du -h "$archive" | cut -f1))"
echo "Checksum file:  $checksum"
echo "Archive SHA256: $sha"

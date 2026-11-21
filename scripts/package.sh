#!/usr/bin/env bash
# Builds the single-binary Apple Silicon Devbase.app bundle, signs it, and checks it.
#
#   scripts/package.sh                 signed with Developer ID + disk image (arm64)
#   scripts/package.sh --ad-hoc        ad-hoc signed local .app bundle (no Developer ID or DMG)
#   scripts/package.sh --skip-tests    without running the tests first
#
# Environment:
#   SIGNING_IDENTITY  the identity to sign with. By default the one
#                     "Developer ID Application" certificate in the keychain.
#   NOTARY_PROFILE    a profile of `xcrun notarytool store-credentials`. With it
#                     the disk image is notarized and stapled, so that it opens
#                     without a warning on other Macs. Without it, it is only
#                     signed, and the script says so.
#   NOTARY_KEYCHAIN   the keychain that holds NOTARY_PROFILE, when that is not
#                     the login keychain.
#   NOTARY_KEY        instead of a profile, the path of an App Store Connect API
#   NOTARY_KEY_ID     key and its ID, and for a team key its issuer ID, as a
#   NOTARY_ISSUER     machine with no profile uses.
#   REQUIRE_NOTARIZE  when set, stop unless the disk image is notarized.
#   NSH_UPDATER_SECRET_KEY
#                     base64 Ed25519 secret key matching app/nsh.json's
#                     updater.publicKey; when set, generates signed
#                     latest.json + latest.json.sig beside the update archive.
#   NSH_UPDATER_SECRET_KEY_FILE
#                     path to an `nsh keygen` .secret file (alternative to
#                     NSH_UPDATER_SECRET_KEY).
set -euo pipefail
cd "$(dirname "$0")/.."
repo=$PWD

die() {
	echo "error: $*" >&2
	exit 1
}

platform=darwin/arm64
run_tests=1
ad_hoc=0
while [ $# -gt 0 ]; do
	case "$1" in
	--skip-tests)
		run_tests=0
		shift
		;;
	--ad-hoc)
		ad_hoc=1
		shift
		;;
	-h | --help)
		sed -n '2,28p' "$0" | sed 's/^# \{0,1\}//'
		exit 0
		;;
	*) die "unknown argument: $1 (see --help)" ;;
	esac
done

[ "$(uname)" = Darwin ] || die "signing and disk images need macOS"

notary=()
identity=""
if [ "$ad_hoc" = 0 ]; then
	if [ -n "${NOTARY_PROFILE:-}" ]; then
		notary=(--keychain-profile "$NOTARY_PROFILE")
		if [ -n "${NOTARY_KEYCHAIN:-}" ]; then
			notary+=(--keychain "$NOTARY_KEYCHAIN")
		fi
	elif [ -n "${NOTARY_KEY:-}" ]; then
		[ -f "$NOTARY_KEY" ] || die "NOTARY_KEY is not a file: $NOTARY_KEY"
		[ -n "${NOTARY_KEY_ID:-}" ] || die "NOTARY_KEY needs NOTARY_KEY_ID"
		notary=(--key "$NOTARY_KEY" --key-id "$NOTARY_KEY_ID")
		if [ -n "${NOTARY_ISSUER:-}" ]; then
			notary+=(--issuer "$NOTARY_ISSUER")
		fi
	fi
	if [ -n "${REQUIRE_NOTARIZE:-}" ] && [ "${#notary[@]}" -eq 0 ]; then
		die "notarizing was required, and neither NOTARY_PROFILE nor NOTARY_KEY is set"
	fi

	identity=${SIGNING_IDENTITY:-}
	if [ -z "$identity" ]; then
		identities=$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application: .*\)"$/\1/p')
		count=$(printf '%s' "$identities" | grep -c . || true)
		case "$count" in
		0) die "no Developer ID Application certificate in the keychain (pass --ad-hoc for an ad-hoc local bundle)" ;;
		1) identity=$identities ;;
		*) die "several Developer ID certificates; choose one with SIGNING_IDENTITY:
$identities" ;;
		esac
	fi
	security find-identity -v -p codesigning | grep -qF "$identity" ||
		die "no valid signing identity in the keychain matches: $identity"
	echo "==> Signing with: $identity"
else
	echo "==> Ad-hoc local packaging (no Developer ID certificate or DMG)"
fi

if [ "$run_tests" = 1 ]; then
	echo "==> Testing"
	(cd app && cargo test)
	for crate in app/crates/*; do
		(cd "$crate" && cargo test)
	done
fi

echo "==> Checking bundled license notices"
for dir in app app/crates/devcore app/crates/devproxy app/crates/devcli; do
	(cd "$dir" && cargo fetch --locked)
done
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_*.py'
python3 scripts/collect-third-party-notices.py --check

out="build/${platform/\//-}"
rm -rf "$out"
mkdir -p "$out"
version=$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["version"])' app/nsh.json)
[ -n "$version" ] || die "app/nsh.json is missing version"

echo "==> Scaffolding $platform bundle with nsh package"
(cd app && nsh package --target macos --arch arm64 --out "$repo/$out")
# Remove the preliminary archive written before replacing the executable and signing.
rm -f "$out"/*.tar.gz

app=$(find "$out" -maxdepth 1 -name '*.app' -print -quit)
[ -n "$app" ] || die "no app in $out"

entitlements=$(mktemp)
updater_key_tmp=""
cleanup() {
	rm -f "$entitlements"
	if [ -n "$updater_key_tmp" ]; then
		rm -f "$updater_key_tmp"
	fi
}
trap cleanup EXIT

archive="$out/Devbase-${version}-macos-aarch64.tar.gz"
manifest_signed=0
build_update_archive() {
	echo "==> Creating macOS update archive ($archive)"
	rm -f "$archive" "$out/latest.json" "$out/latest.json.sig"
	COPYFILE_DISABLE=1 tar -C "$out" -czf "$archive" "$(basename "$app")"

	local key_file=""
	if [ -n "${NSH_UPDATER_SECRET_KEY_FILE:-}" ]; then
		[ -f "$NSH_UPDATER_SECRET_KEY_FILE" ] || die "NSH_UPDATER_SECRET_KEY_FILE is not a file: $NSH_UPDATER_SECRET_KEY_FILE"
		key_file="$NSH_UPDATER_SECRET_KEY_FILE"
	elif [ -n "${NSH_UPDATER_SECRET_KEY:-}" ]; then
		updater_key_tmp=$(
			umask 077
			mktemp
		)
		printf '%s\n' "$NSH_UPDATER_SECRET_KEY" >"$updater_key_tmp"
		key_file="$updater_key_tmp"
	fi

	if [ -n "$key_file" ]; then
		echo "==> Generating signed update manifest (latest.json + latest.json.sig)"
		nsh update-manifest --entry app --key "$key_file" --out "$out" "$archive"
		if [ -n "$updater_key_tmp" ]; then
			rm -f "$updater_key_tmp"
			updater_key_tmp=""
		fi
		[ -s "$out/latest.json" ] || die "nsh update-manifest did not write $out/latest.json"
		[ -s "$out/latest.json.sig" ] || die "nsh update-manifest did not write $out/latest.json.sig"
		manifest_signed=1
	fi
}

# Preserve entitlements generated by `nsh package` before replacing the executable.
has_entitlements=0
if codesign -d --entitlements :- "$app" >"$entitlements" 2>/dev/null && grep -q '<plist' "$entitlements"; then
	has_entitlements=1
fi

# Replace nsh's generated runner with the trimmed single-binary executable from
# `app/src/main.rs` so `Devbase.app/Contents/MacOS/Devbase` dispatches `--dev`,
# the public `devbase` CLI, `--background`, and the macOS desktop GUI.
min_macos=$(python3 -c 'import plistlib, sys; print(plistlib.loads(open(sys.argv[1], "rb").read())["LSMinimumSystemVersion"])' "$app/Contents/Info.plist")
echo "==> Building single-binary macOS executable (version=$version, MACOSX_DEPLOYMENT_TARGET=$min_macos)"
(cd app && DEVBASE_VERSION="$version" MACOSX_DEPLOYMENT_TARGET="$min_macos" cargo build --release --locked --bin nsh-app-devbase)
install -m 0755 "app/target/release/nsh-app-devbase" "$app/Contents/MacOS/Devbase"

echo "==> Bundling license notices into $app/Contents/Resources"
cp "$repo/LICENSE" "$app/Contents/Resources/LICENSE"
cp "$repo/NOTICE" "$app/Contents/Resources/NOTICE"
rm -rf "$app/Contents/Resources/licenses"
cp -R "$repo/licenses" "$app/Contents/Resources/licenses"

echo "==> Signing the app"
if [ "$ad_hoc" = 1 ]; then
	if [ "$has_entitlements" = 1 ]; then
		codesign --force --deep --entitlements "$entitlements" --sign - "$app"
	else
		codesign --force --deep --sign - "$app"
	fi
else
	codesign --force --deep --options runtime --timestamp --sign "$identity" "$app"
	if [ "$has_entitlements" = 1 ]; then
		codesign --force --options runtime --timestamp --entitlements "$entitlements" --sign "$identity" "$app"
	fi
fi

echo "==> Checking the app"
codesign --verify --deep --strict "$app" || die "the app's signature is not valid"
python3 scripts/verify-macos-requirements.py --release-bundle "$app"

if [ "$ad_hoc" = 1 ]; then
	build_update_archive
	echo
	echo "App:            $app"
	echo "Executable:     $app/Contents/MacOS/Devbase ($(du -h "$app/Contents/MacOS/Devbase" | cut -f1))"
	echo "Update archive: $archive ($(du -h "$archive" | cut -f1))"
	if [ "$manifest_signed" = 1 ]; then
		echo "Manifest:       $out/latest.json + $out/latest.json.sig"
	fi
	exit 0
fi

dmg="$out/Devbase.dmg"
rm -f "$dmg"
hdiutil create -volname "Devbase" -srcfolder "$app" -ov -format UDZO "$dmg" >/dev/null
codesign --force --timestamp --sign "$identity" "$dmg"
codesign --verify --strict "$dmg" || die "the disk image's signature is not valid"
details=$(codesign -dvv "$app" 2>&1)
printf '%s\n' "$details" | grep -q "^Authority=$identity\$" || die "the app is not signed by $identity"
printf '%s\n' "$details" | grep -q 'flags=.*runtime' || die "the app lacks the hardened runtime, which notarizing requires"
printf '%s\n' "$details" | grep -q '^Timestamp=' || die "the app lacks a secure timestamp, which notarizing requires"
hdiutil verify -quiet "$dmg" || die "the disk image is damaged"

notarized=0
if [ "${#notary[@]}" -gt 0 ]; then
	echo "==> Notarizing (this uploads the disk image to Apple)"
	log=$(mktemp)
	if ! xcrun notarytool submit "$dmg" "${notary[@]}" --wait --timeout 30m >"$log" 2>&1; then
		cat "$log" >&2
		id=$(sed -n 's/^  id: //p' "$log")
		id=${id%%$'\n'*}
		if [ -n "$id" ]; then
			echo "==> Apple's notarization log" >&2
			xcrun notarytool log "$id" "${notary[@]}" >&2 || true
		fi
		rm -f "$log"
		die "Apple did not notarize the disk image"
	fi
	cat "$log"
	rm -f "$log"
	xcrun stapler staple "$dmg"
	xcrun stapler validate "$dmg"
	spctl --assess --type open --context context:primary-signature --verbose "$dmg"
	# Also staple the .app directly so the .tar.gz update archive carries its
	# notarization ticket offline when extracted by nsh-updater.
	xcrun stapler staple "$app"
	xcrun stapler validate "$app"
	codesign --verify --deep --strict "$app" || die "the stapled app's signature is not valid"
	notarized=1
fi

build_update_archive

echo
echo "App:            $app"
echo "Disk image:     $dmg ($(du -h "$dmg" | cut -f1))"
echo "DMG SHA-256:    $(shasum -a 256 "$dmg" | cut -d' ' -f1)"
echo "Update archive: $archive ($(du -h "$archive" | cut -f1))"
echo "Archive SHA256: $(shasum -a 256 "$archive" | cut -d' ' -f1)"
if [ "$manifest_signed" = 1 ]; then
	echo "Manifest:       $out/latest.json + $out/latest.json.sig"
fi
if [ "$notarized" = 1 ]; then
	echo "Notarized:      yes"
else
	echo "Notarized:      no. It runs here, but other Macs warn before opening it. To notarize:"
	echo "  xcrun notarytool store-credentials msm --apple-id <you> --team-id <team>"
	echo "  NOTARY_PROFILE=msm scripts/package.sh"
fi

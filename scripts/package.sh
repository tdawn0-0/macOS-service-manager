#!/usr/bin/env bash
# Builds the app signed with a Developer ID, in a disk image, and checks them.
#
#   scripts/package.sh                 a universal app, for Apple silicon and Intel
#   scripts/package.sh --arch arm64    for one architecture (arm64 or amd64)
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
set -euo pipefail
cd "$(dirname "$0")/.."

die() {
	echo "error: $*" >&2
	exit 1
}

platform=darwin/universal
run_tests=1
while [ $# -gt 0 ]; do
	case "$1" in
	--arch)
		[ $# -ge 2 ] || die "--arch needs arm64 or amd64"
		case "$2" in
		arm64 | amd64) platform="darwin/$2" ;;
		*) die "--arch needs arm64 or amd64, not $2" ;;
		esac
		shift 2
		;;
	--skip-tests)
		run_tests=0
		shift
		;;
	-h | --help)
		sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'
		exit 0
		;;
	*) die "unknown argument: $1 (see --help)" ;;
	esac
done

[ "$(uname)" = Darwin ] || die "signing and disk images need macOS"

notary=()
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
	0) die "no Developer ID Application certificate in the keychain" ;;
	1) identity=$identities ;;
	*) die "several Developer ID certificates; choose one with SIGNING_IDENTITY:
$identities" ;;
	esac
fi
security find-identity -v -p codesigning | grep -qF "$identity" ||
	die "no valid signing identity in the keychain matches: $identity"
echo "==> Signing with: $identity"

if [ "$run_tests" = 1 ]; then
	echo "==> Testing"
	go test ./...
fi

out="build/${platform/\//-}"
rm -rf "$out"
echo "==> Building $platform"
go tool mygo build -platform "$platform" -sign "$identity" -skip-notarize

app=$(find "$out" -maxdepth 1 -name '*.app' -print -quit)
dmg=$(find "$out" -maxdepth 1 -name '*.dmg' -print -quit)
[ -n "$app" ] || die "no app in $out"
[ -n "$dmg" ] || die "no disk image in $out"

echo "==> Checking the signature"
codesign --verify --deep --strict "$app" || die "the app's signature is not valid"
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
	notarized=1
fi

echo
echo "App:        $app"
echo "Disk image: $dmg ($(du -h "$dmg" | cut -f1))"
echo "SHA-256:    $(shasum -a 256 "$dmg" | cut -d' ' -f1)"
if [ "$notarized" = 1 ]; then
	echo "Notarized:  yes"
else
	echo "Notarized:  no. It runs here, but other Macs warn before opening it. To notarize:"
	echo "  xcrun notarytool store-credentials msm --apple-id <you> --team-id <team>"
	echo "  NOTARY_PROFILE=msm scripts/package.sh"
fi

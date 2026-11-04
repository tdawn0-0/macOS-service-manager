#!/usr/bin/env bash
# Gives GitHub what it needs to sign and notarize releases, after checking that
# it works. It keeps them in an environment named "release", which only release
# tags can use, and prints no secret.
#
#   scripts/setup-release-secrets.sh --p12 developer-id.p12 \
#       --apple-id <you> --team-id <team> [--check-only]
#
# --p12       the Developer ID Application certificate with its private key,
#             exported from Keychain Access. It asks for that password.
# --apple-id  the Apple ID of the Developer Program
# --team-id   its Team ID, ten letters or digits
# --check-only  check them with Apple, and send GitHub nothing
#
# It also asks for an app-specific password, the same one a notarytool
# keychain profile stores. Delete the .p12 once this is done: GitHub has it.
set -euo pipefail
cd "$(dirname "$0")/.."

die() {
	echo "error: $*" >&2
	exit 1
}

p12=''
apple_id=''
team_id=''
check_only=0
while [ $# -gt 0 ]; do
	case "$1" in
	--p12 | --apple-id | --team-id)
		[ $# -ge 2 ] || die "$1 needs a value"
		case "$1" in
		--p12) p12=$2 ;;
		--apple-id) apple_id=$2 ;;
		--team-id) team_id=$2 ;;
		esac
		shift 2
		;;
	--check-only)
		check_only=1
		shift
		;;
	-h | --help)
		sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'
		exit 0
		;;
	*) die "unknown argument: $1 (see --help)" ;;
	esac
done
[ -n "$p12" ] && [ -n "$apple_id" ] && [ -n "$team_id" ] || die "--p12, --apple-id and --team-id are needed (see --help)"
[ -f "$p12" ] || die "no file $p12"
[[ $team_id =~ ^[A-Z0-9]{10}$ ]] || die "--team-id must be 10 letters or digits"

if [ -z "${P12_PASSWORD+set}" ]; then
	read -rsp "Password of $p12: " P12_PASSWORD
	echo
fi
export P12_PASSWORD
if [ -z "${APPLE_APP_PASSWORD+set}" ]; then
	read -rsp "App-specific password for $apple_id: " APPLE_APP_PASSWORD
	echo
fi
[ -n "$APPLE_APP_PASSWORD" ] || die "the app-specific password is empty"

echo "==> Checking the certificate"
# macOS' own openssl reads the older ciphers Keychain Access exports with.
certificate=$(/usr/bin/openssl pkcs12 -in "$p12" -passin env:P12_PASSWORD -nokeys -clcerts 2>/dev/null) ||
	die "cannot read $p12: is the password right?"
/usr/bin/openssl pkcs12 -in "$p12" -passin env:P12_PASSWORD -nocerts -nodes 2>/dev/null | grep -q "PRIVATE KEY" ||
	die "$p12 has no private key: export the certificate from My Certificates in Keychain Access"
subject=$(printf '%s\n' "$certificate" | /usr/bin/openssl x509 -noout -subject)
case "$subject" in
*"Developer ID Application"*) ;;
*) die "$p12 is not a Developer ID Application certificate: $subject" ;;
esac
printf '%s\n' "$certificate" | /usr/bin/openssl x509 -noout -checkend 0 >/dev/null || die "the certificate has expired"
printf '%s\n' "$certificate" | /usr/bin/openssl x509 -noout -subject -enddate | sed 's/^/    /'

echo "==> Checking the Apple ID with Apple"
xcrun notarytool history --apple-id "$apple_id" --password "$APPLE_APP_PASSWORD" --team-id "$team_id" >/dev/null ||
	die "Apple refuses this Apple ID for notarizing"
echo "    Apple accepts it"

if [ "$check_only" = 1 ]; then
	echo "==> All good. --check-only: GitHub was sent nothing."
	exit 0
fi

command -v gh >/dev/null || die "gh is needed"
echo "==> Making the environment \"release\", for release tags only"
gh api -X PUT 'repos/{owner}/{repo}/environments/release' --silent --input - <<'EOF'
{"deployment_branch_policy": {"protected_branches": false, "custom_branch_policies": true}}
EOF
# GitHub matches a policy against GITHUB_REF. For a tag that is refs/tags/<name>,
# and * does not match a slash, so the prefix has to be in the pattern.
# Without it the job never starts, because no tag is allowed.
custom=$(gh api 'repos/{owner}/{repo}/environments/release' --jq '.deployment_branch_policy.custom_branch_policies')
[ "$custom" = true ] || die "the release environment does not limit which tags can use it"
policies=$(gh api 'repos/{owner}/{repo}/environments/release/deployment-branch-policies' --jq '.branch_policies // [] | .[].name')
for pattern in 'refs/tags/v[0-9]*' 'refs/tags/msm-v[0-9]*'; do
	printf '%s\n' "$policies" | grep -qxF "$pattern" ||
		gh api -X POST 'repos/{owner}/{repo}/environments/release/deployment-branch-policies' --silent -f name="$pattern" -f type=tag
	echo "    $pattern"
done

echo "==> Setting the secrets"
base64 -i "$p12" | gh secret set MACOS_CERTIFICATE_P12 --env release
printf '%s' "$P12_PASSWORD" | gh secret set MACOS_CERTIFICATE_PASSWORD --env release
printf '%s' "$apple_id" | gh secret set APPLE_ID --env release
printf '%s' "$APPLE_APP_PASSWORD" | gh secret set APPLE_APP_PASSWORD --env release
printf '%s' "$team_id" | gh secret set APPLE_TEAM_ID --env release

echo "==> Done. Delete $p12, then release with:"
echo "    git tag v1.2.3 && git push origin master v1.2.3"

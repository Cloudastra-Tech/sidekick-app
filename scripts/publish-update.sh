#!/usr/bin/env bash
# Uploads one signed build to the Sidekick backend so installed apps on that platform offer it.
# Used by the release workflow (.github/workflows/release.yml) for Windows and macOS, and by
# scripts/release-mac.sh.
#
#   scripts/publish-update.sh <installer-or-update-file> <target> [<target>...]
#
# The signature is read from "<file>.sig" (Tauri writes it when signing). Targets are what Tauri
# calls a platform: windows-x86_64, darwin-aarch64, darwin-x86_64.
#
# Environment:
#   SIDEKICK_BACKEND_URL      e.g. https://sidekick.example.com
#   SIDEKICK_RELEASE_EMAIL    an admin account of the workspace
#   SIDEKICK_RELEASE_PASSWORD asked for when unset
#   SIDEKICK_RELEASE_NOTES    shown to users with the update (optional)
#   SIDEKICK_BACKEND_ORIGIN_IP  the server's own IP (optional). Connects to it directly instead of
#                             through Cloudflare, which refuses CI runners; the certificate is
#                             still checked against the hostname.

set -euo pipefail

if [ $# -lt 2 ]; then
  sed -n '2,16p' "$0"
  exit 1
fi
file="$1"
shift
: "${SIDEKICK_BACKEND_URL:?Set SIDEKICK_BACKEND_URL}"
: "${SIDEKICK_RELEASE_EMAIL:?Set SIDEKICK_RELEASE_EMAIL}"
backend="${SIDEKICK_BACKEND_URL%/}"
direct=()
if [ -n "${SIDEKICK_BACKEND_ORIGIN_IP:-}" ]; then
  host="${backend#*://}"
  host="${host%%/*}"
  direct=(--resolve "$host:443:$SIDEKICK_BACKEND_ORIGIN_IP")
fi

[ -f "$file" ] || { echo "No such file: $file" >&2; exit 1; }
[ -f "$file.sig" ] || { echo "No signature beside it: $file.sig" >&2; exit 1; }

repo="$(cd "$(dirname "$0")/.." && pwd)"
version="$(node -p "require(process.argv[1]).version" "$repo/client/src-tauri/tauri.conf.json")"

if [ -z "${SIDEKICK_RELEASE_PASSWORD:-}" ]; then
  read -r -s -p "Password for $SIDEKICK_RELEASE_EMAIL: " SIDEKICK_RELEASE_PASSWORD
  echo
  export SIDEKICK_RELEASE_PASSWORD
fi
login="$(node -e 'process.stdout.write(JSON.stringify({
  email: process.env.SIDEKICK_RELEASE_EMAIL, password: process.env.SIDEKICK_RELEASE_PASSWORD }))')"
token="$(curl -fsS ${direct[@]+"${direct[@]}"} -X POST "$backend/auth/login" -H "Content-Type: application/json" \
  --data "$login" | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",
  ()=>process.stdout.write(JSON.parse(s).access_token))')"

for target in "$@"; do
  curl -fsS ${direct[@]+"${direct[@]}"} -X POST "$backend/updates" -H "Authorization: Bearer $token" \
    -F "version=$version" -F "notes=${SIDEKICK_RELEASE_NOTES:-}" \
    -F "signature=$(cat "$file.sig")" -F "target=$target" -F "installer=@$file" >/dev/null
  echo "Published $version for $target"
done

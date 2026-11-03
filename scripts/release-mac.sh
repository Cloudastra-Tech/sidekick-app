#!/usr/bin/env bash
# Builds the macOS release (one universal app for Intel and Apple Silicon) and publishes it to the
# Sidekick backend, so every installed Mac copy offers the update the next time it checks.
#
# Normally the release workflow runs this when a version tag is pushed (see CLAUDE.md "How we
# ship"); running it by hand is the fallback.
#
#   scripts/release-mac.sh                 # build, sign and publish (needs SIDEKICK_BACKEND_URL,
#                                          # SIDEKICK_RELEASE_EMAIL; see scripts/publish-update.sh)
#   scripts/release-mac.sh --skip-upload   # build and sign only
#   scripts/release-mac.sh --setup-only    # create the signing certificate, then stop
#
# Keys, kept in ~/.sidekick-release (or $SIDEKICK_RELEASE_DIR), never in git:
#   sidekick.key + password.txt   The update signing key, the same one the Windows release uses.
#                                 Installed apps refuse any update not signed with it.
#   mac-signing.p12 + mac-signing-password.txt
#                                 A self-made code-signing certificate (made by --setup-only or the
#                                 first run). It gives every build the same identity, so macOS keeps
#                                 the mic and system-audio permissions across updates. It is not an
#                                 Apple Developer ID: a fresh install still needs right-click > Open.
# Back them up. Losing the update key means installed apps can never update again; losing the
# certificate means every Mac asks for its permissions once more after the next update.

set -euo pipefail

SKIP_UPLOAD=0
SETUP_ONLY=0
while [ $# -gt 0 ]; do
  case "$1" in
    --skip-upload) SKIP_UPLOAD=1; shift ;;
    --setup-only) SETUP_ONLY=1; shift ;;
    -h|--help) sed -n '2,23p' "$0"; exit 0 ;;
    *) echo "Unknown option: $1 (see --help)" >&2; exit 1 ;;
  esac
done

REPO="$(cd "$(dirname "$0")/.." && pwd)"
KEY_DIR="${SIDEKICK_RELEASE_DIR:-$HOME/.sidekick-release}"
CERT_NAME="Sidekick Release"
VERSION="$(node -p "require(process.argv[1]).version" "$REPO/client/src-tauri/tauri.conf.json")"
mkdir -p "$KEY_DIR"

# ---- Code-signing certificate (made once) ----------------------------------------------------
if [ ! -f "$KEY_DIR/mac-signing.p12" ]; then
  if [ -n "${CI:-}" ]; then
    # A new certificate would make every Mac ask for its permissions again.
    echo "No signing certificate in $KEY_DIR; refusing to make a new one in CI" >&2
    exit 1
  fi
  echo "Creating the macOS signing certificate (first run only)"
  work="$(mktemp -d)"
  cat > "$work/cert.cnf" <<EOF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $CERT_NAME
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
EOF
  openssl req -x509 -newkey rsa:2048 -nodes -days 7300 -config "$work/cert.cnf" \
    -keyout "$work/key.pem" -out "$work/cert.pem" 2>/dev/null
  openssl rand -hex 24 > "$KEY_DIR/mac-signing-password.txt"
  openssl pkcs12 -export -inkey "$work/key.pem" -in "$work/cert.pem" \
    -out "$KEY_DIR/mac-signing.p12" -passout "file:$KEY_DIR/mac-signing-password.txt"
  chmod 600 "$KEY_DIR/mac-signing.p12" "$KEY_DIR/mac-signing-password.txt"
  rm -rf "$work"
  echo "Created $KEY_DIR/mac-signing.p12. Back it up with its password file."
fi

if [ "$SETUP_ONLY" -eq 1 ]; then
  exit 0
fi

echo "Releasing Sidekick $VERSION for macOS"
for file in sidekick.key password.txt; do
  if [ ! -f "$KEY_DIR/$file" ]; then
    echo "Missing $KEY_DIR/$file. Copy sidekick.key and password.txt from the machine that" >&2
    echo "makes Windows releases (%USERPROFILE%\\.sidekick-release); a new key would not be" >&2
    echo "accepted by the apps already installed." >&2
    exit 1
  fi
done

# A keychain of our own for this run, deleted on exit. codesign only finds an identity in a
# keychain on the search list (on GitHub's runners --keychain alone isn't enough), so it's added
# for the run and the previous list is put back afterwards.
work="$(mktemp -d)"
keychain="$work/signing.keychain-db"
keychain_password="$(openssl rand -hex 16)"
previous_keychains=()
while IFS= read -r line; do
  line="${line#"${line%%[![:space:]]*}"}"
  line="${line%\"}"
  previous_keychains+=("${line#\"}")
done < <(security list-keychains -d user)
cleanup() {
  security list-keychains -d user -s "${previous_keychains[@]}" 2>/dev/null || true
  security delete-keychain "$keychain" 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT
security create-keychain -p "$keychain_password" "$keychain"
security list-keychains -d user -s "$keychain" "${previous_keychains[@]}"
security set-keychain-settings "$keychain"   # no auto-lock: the build takes minutes
security unlock-keychain -p "$keychain_password" "$keychain"
security import "$KEY_DIR/mac-signing.p12" -k "$keychain" \
  -P "$(cat "$KEY_DIR/mac-signing-password.txt")" -T /usr/bin/codesign >/dev/null
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$keychain_password" \
  "$keychain" >/dev/null
identity="$(security find-identity -p codesigning "$keychain" | awk -v n="\"$CERT_NAME\"" \
  '$0 ~ n {print $2; exit}')"
if [ -z "$identity" ]; then
  echo "The signing certificate in $KEY_DIR/mac-signing.p12 couldn't be loaded" >&2
  exit 1
fi

# ---- Build, sign, package ------------------------------------------------------------------
cd "$REPO/client"
# Tauri only signs with certificates macOS fully trusts, so it builds ad hoc and we sign after.
npx tauri build --target universal-apple-darwin --bundles app \
  --config '{"bundle":{"createUpdaterArtifacts":false,"macOS":{"signingIdentity":"-"}}}'
bundle="$REPO/client/src-tauri/target/universal-apple-darwin/release/bundle/macos"
app="$bundle/Sidekick.app"

security unlock-keychain -p "$keychain_password" "$keychain"
codesign --force --options runtime --timestamp=none \
  --entitlements "$REPO/client/src-tauri/Entitlements.plist" \
  --keychain "$keychain" -s "$identity" "$app"
codesign --verify --strict "$app"

out="$REPO/dist-mac/$VERSION"
mkdir -p "$out"
update="$out/Sidekick_${VERSION}_universal.app.tar.gz"
# The updater unpacks this over the installed app: the archive holds Sidekick.app at its root.
COPYFILE_DISABLE=1 tar -czf "$update" -C "$bundle" Sidekick.app
# Strip a Windows byte-order mark and line endings: password.txt often comes from Notepad.
TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$(LC_ALL=C sed $'1s/^\xEF\xBB\xBF//' "$KEY_DIR/password.txt" | tr -d '\r\n')" \
  npx tauri signer sign -f "$KEY_DIR/sidekick.key" "$update" >/dev/null
[ -f "$update.sig" ] || { echo "Signing the update failed" >&2; exit 1; }

# A disk image for fresh installs.
stage="$work/dmg"
mkdir -p "$stage"
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
hdiutil create -quiet -volname Sidekick -srcfolder "$stage" -ov -format UDZO \
  "$out/Sidekick_${VERSION}_universal.dmg"
# Leave no extra copies of the app around: macOS lists every one it finds.
rm -rf "$bundle"
echo "Built $out"

if [ "$SKIP_UPLOAD" -eq 1 ]; then
  exit 0
fi

# ---- Publish -------------------------------------------------------------------------------
# One universal build serves both kinds of Mac; the updater looks itself up by architecture.
"$REPO/scripts/publish-update.sh" "$update" darwin-aarch64 darwin-x86_64
echo "Installed Macs will offer the update the next time they check (within 15 minutes)."

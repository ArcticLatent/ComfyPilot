#!/usr/bin/env bash
# Writes and signs a release's update manifests, next to its files in a
# folder (the release workflow's publish job):
#
#   packaging/release-manifests.sh <dir> <version> <owner/repo> [notes.md]
#
#   SHA256SUMS           every file's checksum
#   update.json          Windows: the installer (ComfyPilot-Setup-<v>.exe),
#                        with the release notes the app shows
#   linux-release.json   Linux: the .deb, .rpm and Arch packages and the AppImage
#
# Both manifests are signed with ARCTIC_UPDATE_SIGNING_KEY (tools/
# manifest-signer) and checked against the public key built into the app,
# so a wrong key stops the release instead of shipping updates nobody can
# install.
set -euo pipefail

DIR="${1:?usage: $0 <dir> <version> <owner/repo>}"
VERSION="${2:?version missing}"
REPOSITORY="${3:?repository missing}"
NOTES_FILE="${4:-}"
TAG="v$VERSION"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASE_URL="https://github.com/$REPOSITORY/releases/download/$TAG"
INSTALLER="ComfyPilot-Setup-$VERSION.exe"

[[ -n "${ARCTIC_UPDATE_SIGNING_KEY:-}" ]] || {
  echo "ARCTIC_UPDATE_SIGNING_KEY is not set" >&2
  exit 1
}
[[ -f "$DIR/$INSTALLER" ]] || {
  echo "missing $DIR/$INSTALLER" >&2
  exit 1
}

sha() { sha256sum "$1" | awk '{print $1}'; }
signer() {
  cargo run --quiet --release --locked --manifest-path "$ROOT/tools/manifest-signer/Cargo.toml" -- "$@"
}

mapfile -t packages < <(find "$DIR" -maxdepth 1 -type f \( -name '*.deb' -o -name '*.rpm' -o -name '*.pkg.tar.zst' -o -name '*.AppImage' \) -printf '%f\n' | sort)
((${#packages[@]} == 4)) || {
  echo "expected a .deb, an .rpm, an Arch package and an AppImage in $DIR, found: ${packages[*]:-none}" >&2
  exit 1
}

notes="Release $VERSION"
[[ -n "$NOTES_FILE" ]] && notes="$(cat "$NOTES_FILE")"

# Windows.
jq -n --arg version "$VERSION" --arg url "$BASE_URL/$INSTALLER" \
  --arg sha "$(sha "$DIR/$INSTALLER")" --arg notes "$notes" \
  '{version: $version, download_url: $url, sha256: $sha, notes: $notes}' >"$DIR/update.json"

# Linux.
for name in "${packages[@]}"; do
  jq -n --arg name "$name" --arg sha "$(sha "$DIR/$name")" --arg url "$BASE_URL/$name" \
    '{name: $name, sha256: $sha, download_url: $url}'
done | jq -s --arg version "$VERSION" --arg tag "$TAG" --arg repo "$REPOSITORY" \
  '{version: $version, tag: $tag, repository: $repo, assets: .}' >"$DIR/linux-release.json"

signer sign --format update --manifest "$DIR/update.json"
signer sign --format linux-release --manifest "$DIR/linux-release.json"
signer verify --format update --manifest "$DIR/update.json"
signer verify --format linux-release --manifest "$DIR/linux-release.json"

(cd "$DIR" && sha256sum "$INSTALLER" "${packages[@]}" update.json linux-release.json >SHA256SUMS)
echo "Manifests for $TAG:"
cat "$DIR/SHA256SUMS"

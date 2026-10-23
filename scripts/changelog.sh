#!/usr/bin/env bash
# Reads CHANGELOG.md, whose sections start with "## <version>" (newest first):
#
#   scripts/changelog.sh version            the newest section's version
#   scripts/changelog.sh notes [version]    that section's text (default: the newest)
set -euo pipefail

CHANGELOG="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/CHANGELOG.md"
[[ -f "$CHANGELOG" ]] || { echo "missing $CHANGELOG" >&2; exit 1; }

# "## 1.0.0 - 2026-10-24" or "## [v1.0.0]" -> 1.0.0
versions() {
  sed -n 's/^## \[\{0,1\}v\{0,1\}\([0-9][0-9A-Za-z.+-]*\).*/\1/p' "$CHANGELOG"
}

case "${1:-}" in
  version)
    v="$(versions | head -1)"
    [[ -n "$v" ]] || { echo "no '## <version>' section in CHANGELOG.md" >&2; exit 1; }
    echo "$v"
    ;;
  notes)
    v="${2:-$(versions | head -1)}"
    versions | grep -qxF "$v" || { echo "no section for $v in CHANGELOG.md" >&2; exit 1; }
    awk -v v="$v" '
      /^## / { if (found) exit; line = $0; sub(/^## \[?v?/, "", line); split(line, w, /[] ]/); found = (w[1] == v); next }
      found { print }
    ' "$CHANGELOG" | sed -e '/./,$!d' | sed -e ':a' -e '/^\n*$/{$d;N;ba' -e '}'
    ;;
  *)
    echo "usage: $0 version | notes [version]" >&2
    exit 1
    ;;
esac

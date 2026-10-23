#!/usr/bin/env bash
# Releases the version at the top of CHANGELOG.md, in one command:
#
#   scripts/release.sh            release it
#   scripts/release.sh --dry-run  only show what would happen
#
# 1. Checks: on main, in sync with GitHub, no uncommitted changes other than
#    CHANGELOG.md, and no tag for that version yet.
# 2. Sets the version in Cargo.toml / app/Cargo.toml / Cargo.lock and
#    commits "Release <version>" (with CHANGELOG.md).
# 3. Tags v<version> and pushes main and the tag to GitHub.
# 4. The Release workflow (.github/workflows/release.yml) builds the .deb,
#    .rpm, Arch package and Windows installer, signs the update manifests and
#    publishes the GitHub release with that CHANGELOG.md section as notes.
#    This script follows the run and prints the release's address.
#
# Needs git, cargo and the GitHub CLI (gh, logged in). REMOTE overrides the
# remote to push to (default: the one pointing at ArcticLatent/ComfyPilot).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
REPO="ArcticLatent/ComfyPilot"
DRY_RUN=0
[[ "${1:-}" == "--dry-run" ]] && DRY_RUN=1

fail() {
  echo "release: $*" >&2
  exit 1
}
step() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
# git with GitHub credentials from gh (whatever git's own credential setup is).
ghgit() { git -c credential.helper= -c credential.helper='!gh auth git-credential' "$@"; }

for cmd in git cargo gh; do
  command -v "$cmd" >/dev/null 2>&1 || fail "$cmd is not installed"
done
gh auth status >/dev/null 2>&1 || fail "gh is not logged in (run: gh auth login)"

REMOTE="${REMOTE:-$(git remote -v | awk -v r="$REPO" '$2 ~ r"(\\.git)?$" && $3 == "(push)" {print $1; exit}')}"
[[ -n "$REMOTE" ]] || fail "no git remote points at $REPO (set REMOTE=<name>)"

VERSION="$(scripts/changelog.sh version)"
TAG="v$VERSION"
NOTES="$(scripts/changelog.sh notes "$VERSION")"
[[ -n "$NOTES" ]] || fail "the $VERSION section of CHANGELOG.md is empty"

step "Checking"
branch="$(git symbolic-ref --short HEAD)"
[[ "$branch" == main ]] || fail "on branch '$branch'; releases are made from main"
# CHANGELOG.md, and the version files a failed earlier run may have left.
dirty="$(git status --porcelain | grep -vE ' (CHANGELOG\.md|Cargo\.toml|app/Cargo\.toml|Cargo\.lock)$' || true)"
[[ -z "$dirty" ]] || fail "uncommitted changes (only CHANGELOG.md may be):"$'\n'"$dirty"
ghgit fetch --quiet --tags "$REMOTE" main
[[ "$(git rev-parse HEAD)" == "$(git rev-parse "$REMOTE/main")" ]] ||
  fail "main isn't the same as $REMOTE/main (pull or push first)"
if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
  fail "$TAG already exists; add a new section at the top of CHANGELOG.md"
fi
echo "Releasing ComfyPilot $VERSION to $REPO ($REMOTE), with these notes:"
echo
sed 's/^/    /' <<<"$NOTES"

if ((DRY_RUN)); then
  echo
  echo "Dry run: nothing changed."
  exit 0
fi
echo
read -r -p "Release $VERSION? [y/N] " answer
[[ "$answer" == [yY]* ]] || fail "cancelled"

step "Setting version $VERSION"
for manifest in Cargo.toml app/Cargo.toml; do
  # The [package] version: the first `version = ` line.
  sed -i "0,/^version = \".*\"/s//version = \"$VERSION\"/" "$manifest"
done
cargo update --workspace --offline --quiet 2>/dev/null || cargo update --workspace --quiet
git add Cargo.toml app/Cargo.toml Cargo.lock CHANGELOG.md
committed=0
if git diff --cached --quiet; then
  echo "(already at $VERSION)"
else
  git commit --quiet -m "Release $VERSION"
  committed=1
fi
git tag -a "$TAG" -m "ComfyPilot $VERSION"

step "Pushing main and $TAG"
if ! ghgit push --atomic "$REMOTE" main "$TAG"; then
  git tag -d "$TAG" >/dev/null
  # Back to before this run (the changes stay), so it can simply run again.
  ((committed)) && git reset --quiet --mixed HEAD~1
  fail "push failed; undid the local $TAG tag and commit, so run this again once it's fixed"
fi

step "Building and publishing on GitHub"
run_id=""
for _ in $(seq 1 30); do
  run_id="$(gh run list --repo "$REPO" --workflow release.yml --branch "$TAG" --limit 1 \
    --json databaseId --jq '.[0].databaseId // empty' 2>/dev/null || true)"
  [[ -n "$run_id" ]] && break
  sleep 2
done
[[ -n "$run_id" ]] || fail "the Release workflow didn't start; see https://github.com/$REPO/actions"
echo "https://github.com/$REPO/actions/runs/$run_id"
gh run watch "$run_id" --repo "$REPO" --exit-status --interval 15 ||
  fail "the Release workflow failed: https://github.com/$REPO/actions/runs/$run_id"

step "Released"
gh release view "$TAG" --repo "$REPO" --json url,assets \
  --jq '"\(.url)\n" + ([.assets[] | "  \(.name)"] | join("\n"))'
